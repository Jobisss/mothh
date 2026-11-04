//! App desktop do mothh.
//!
//! Duas partes rodam ao mesmo tempo:
//! - a interface Slint, na thread principal (exigência do sistema de janelas);
//! - a rede, numa thread com runtime Tokio (`net.rs`).
//!
//! Elas conversam por mensagens: a interface manda `Command`s para a rede, e a
//! rede atualiza a interface com `slint::Weak::upgrade_in_event_loop`.
//!
//! O som (gravar e tocar áudio) tem uma terceira thread, em `audio.rs`.

// No Windows, o app de release abre sem a janela do terminal; o que ele
// escreveria lá vai para um arquivo (`log_file.rs`).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod audio;
mod call;
mod clipboard;
mod devices;
mod dither;
mod e2ee;
mod fonts;
mod levels;
mod listening;
mod log_file;
mod modules;
mod net;
mod noise;
mod overlay;
mod photos;
mod profiles;
mod screen;
mod session;
mod sframe;
mod share_audio;
mod sound;
mod system_audio;
mod title_bar;
mod turntable;
mod turntable_mesh;
mod update;
mod video;
mod volume;

// `Modules` sozinho é o global do Slint; o registro é `modules::Modules`.
use modules::Module;
use protocol::{AttachmentId, ConversationId};
use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use tokio::sync::mpsc;

slint::include_modules!();

/// Anexo do rascunho. Imagens coladas são temporárias e somem ao cancelar
/// ou concluir o upload; arquivos escolhidos pelo usuário nunca são apagados.
pub struct PendingFile {
    path: std::path::PathBuf,
    temporary: bool,
}

/// Mostra o anexo pendente acima do campo: o nome (ou quantos são) e a
/// miniatura da última imagem, se houver.
fn show_pending(ui: &AppWindow, pending: &[PendingFile]) {
    let name = match pending {
        [] => String::new(),
        [one] => one
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
        many => format!("{} anexos", many.len()),
    };
    let preview = pending
        .iter()
        .rev()
        .filter(|file| {
            let ext = file
                .path
                .extension()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();
            matches!(
                ext.as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
            )
        })
        .find_map(|file| slint::Image::load_from_path(&file.path).ok())
        .unwrap_or_default();
    ui.set_pending_preview(preview);
    ui.set_pending_name(name.into());
}

impl Drop for PendingFile {
    fn drop(&mut self) {
        if self.temporary {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// O que a interface pede para a rede fazer.
pub enum Command {
    ListeningSample(Option<protocol::ListeningActivity>),
    Login {
        username: String,
        password: String,
    },
    Register {
        username: String,
        email: String,
        password: String,
    },
    /// "Esqueci a senha": pede o código para esse e-mail.
    RequestReset(String),
    /// Troca a senha com o código recebido.
    ResetPassword {
        email: String,
        code: String,
        password: String,
    },
    /// Define ou troca o e-mail da conta (no perfil).
    SetEmail(String),
    Logout,
    Open(String),
    Send(String),
    /// Troca o texto de uma mensagem minha (id da mensagem e texto novo).
    EditMessage {
        id: String,
        text: String,
    },
    /// Apaga uma mensagem minha, para todo mundo.
    DeleteMessage(String),
    /// Overlay: abre a conversa da chamada na janela principal (as mensagens
    /// que o overlay mostra vêm dela).
    OpenCallChat,
    /// Mensagem rápida do overlay, para a conversa da chamada.
    SendCall(String),
    /// A pessoa rolou até o topo da conversa.
    LoadOlder,
    /// Abrir a conversa direta com esse usuário.
    OpenDirect(String),
    /// Cria um grupo com este nome.
    CreateGroup(String),
    /// Abre (`true`) ou fecha a coluna de membros do grupo aberto.
    ShowMembers(bool),
    /// Convida para o grupo aberto, pelo usuário.
    Invite(String),
    /// Remove do grupo aberto a pessoa com este id.
    Remove(String),
    /// Sai do grupo aberto.
    Leave,
    /// Um áudio gravado (Ogg Opus) para a conversa aberta. Vem da thread de som.
    SendAudio(Vec<u8>),
    /// Tocar o áudio com este id de anexo (baixa antes, se precisar).
    PlayAudio(String),
    /// Os três abaixo a própria rede manda para si mesma, quando termina um
    /// envio ou download que rodou em paralelo.
    AudioUploaded {
        conversation_id: ConversationId,
        attachment_id: AttachmentId,
        ogg: Vec<u8>,
    },
    AudioDownloaded {
        attachment_id: AttachmentId,
        ogg: Vec<u8>,
    },
    AudioFailed(String),
    SendFile(PendingFile, String),
    ViewImage(AttachmentId, String),
    FileUploaded {
        conversation_id: ConversationId,
        attachment_id: AttachmentId,
        caption: String,
    },
    SaveFile {
        attachment_id: AttachmentId,
        path: std::path::PathBuf,
    },
    ImageReady {
        attachment_id: AttachmentId,
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    },
    ImageFailed(AttachmentId),
    /// Entrar na chamada do grupo aberto.
    JoinCall,
    LeaveCall,
    /// Liga ou desliga o microfone na chamada.
    ToggleMute,
    /// Liga ou desliga a câmera na chamada.
    ToggleCamera,
    /// A sua foto de perfil nova, já no formato do servidor (PNG quadrado).
    SetPhoto(Vec<u8>),
    /// Mostrar o perfil de alguém no mini cartão ou na página completa
    /// (id e o nome que a tela já sabe, para mostrar enquanto o resto chega).
    ShowProfile {
        user_id: String,
        name: String,
        place: profiles::Place,
    },
    /// Fechou o cartão ou a página.
    HideProfile(profiles::Place),
    /// Salvar os textos e vitrines da sua página.
    SaveProfile(protocol::ProfileText),
    /// Uma imagem escolhida para a sua página (ainda do jeito que veio do disco).
    SetProfileImage {
        kind: protocol::ProfileImage,
        bytes: Vec<u8>,
    },
    RemoveProfileImage(protocol::ProfileImage),
    /// Terminou de enviar ou tirar uma imagem (vem da tarefa HTTP).
    ProfileImageDone(Result<String, String>),
    /// Página de perfil: trocar o nome e a senha.
    Rename(String),
    ChangePassword {
        current: String,
        new: String,
    },
    /// Os três abaixo vêm da tarefa da chamada (`call.rs`).
    CallJoined(ConversationId),
    /// Quem está na sala e quem está falando agora.
    CallPeople {
        conversation_id: ConversationId,
        people: Vec<call::CallPerson>,
    },
    CallEnded {
        conversation_id: ConversationId,
        /// Uma frase para a tela, quando a chamada caiu ou não abriu.
        reason: Option<String>,
    },
    /// A câmera não abriu ou parou no meio; vem de `call.rs`/`video.rs`.
    CameraFailed(String),
    /// Abrir a lista de telas e janelas para compartilhar (F2-06).
    OpenSharePicker,
    CloseSharePicker,
    /// A lista chegou, de `screen.rs`.
    ShareSources(Vec<screen::ShareSource>),
    /// A foto de uma opção da lista, de `screen.rs`.
    SharePreview(screen::Preview),
    /// Compartilhar a tela ou janela com este id da lista, com ou sem som.
    StartShare {
        source_id: String,
        audio: bool,
    },
    StopShare,
    /// A captura da tela não abriu ou parou; vem de `call.rs`/`screen.rs`.
    ShareFailed(String),
    /// A pessoa trocou microfone, saída ou câmera nas configurações.
    DevicesChanged,
    /// Algo da chamada não deu certo, mas ela continua (o som da tela não
    /// abriu, o microfone escolhido recusou): só avisa no rodapé.
    CallWarning(String),
}

fn main() -> Result<(), slint::PlatformError> {
    #[cfg(all(windows, debug_assertions))]
    if std::env::var_os("MOTHH_SPOTIFY_PROBE").is_some() {
        println!("{:?}", listening::probe());
        return Ok(());
    }
    let profile = std::env::var("MOTHH_PERFIL").unwrap_or_else(|_| "padrao".into());
    log_file::redirect(&profile);
    let url = std::env::var("MOTHH_SERVIDOR").unwrap_or_else(|_| {
        option_env!("MOTHH_DEFAULT_SERVIDOR")
            .unwrap_or("ws://127.0.0.1:3000/ws")
            .into()
    });

    // Os módulos são lidos antes da janela existir: o renderizador é
    // escolhido uma vez, na criação do backend, e não muda mais.
    let modules = modules::Modules::for_profile(&profile);
    select_renderer(&modules)?;

    let ui = AppWindow::new()?;
    let _turntable = turntable::connect(&ui);
    if profile != "padrao" {
        ui.set_profile(profile.clone().into());
    }
    title_bar::connect(&ui);

    let (commands, command_rx) = mpsc::unbounded_channel::<Command>();
    listening::start(commands.clone(), modules.clone());
    ui.on_open_spotify(move |title, artist| listening::open(&title, &artist));
    show_modules(&ui, &modules);
    let ui_handle = ui.as_weak();
    let registry = modules.clone();
    ui.on_toggle_module(move |id, on| {
        let Some(module) = Module::from_id(&id) else {
            return;
        };
        registry.set(module, on);
        if let Some(ui) = ui_handle.upgrade() {
            show_modules(&ui, &registry);
        }
    });

    // Cada callback ganha a sua cópia do `Sender` (`move` leva ela para dentro).
    let sender = commands.clone();
    ui.on_login(move |username, password| {
        let _ = sender.send(Command::Login {
            username: username.into(),
            password: password.into(),
        });
    });
    let sender = commands.clone();
    ui.on_register(move |username, email, password| {
        let _ = sender.send(Command::Register {
            username: username.into(),
            email: email.into(),
            password: password.into(),
        });
    });
    let sender = commands.clone();
    ui.on_request_reset(move |email| {
        let _ = sender.send(Command::RequestReset(email.into()));
    });
    let sender = commands.clone();
    ui.on_reset_password(move |email, code, password| {
        let _ = sender.send(Command::ResetPassword {
            email: email.into(),
            code: code.into(),
            password: password.into(),
        });
    });
    let sender = commands.clone();
    ui.on_set_email(move |email| {
        let _ = sender.send(Command::SetEmail(email.trim().to_string()));
    });
    let sender = commands.clone();
    ui.on_logout(move || {
        let _ = sender.send(Command::Logout);
    });
    let sender = commands.clone();
    ui.on_open_conversation(move |id| {
        let _ = sender.send(Command::Open(id.to_string()));
    });
    let sender = commands.clone();
    ui.on_open_direct(move |username| {
        let _ = sender.send(Command::OpenDirect(username.to_string()));
    });
    let sender = commands.clone();
    ui.on_load_older(move || {
        let _ = sender.send(Command::LoadOlder);
    });
    let sender = commands.clone();
    ui.on_create_group(move |name| {
        let _ = sender.send(Command::CreateGroup(name.into()));
    });
    let sender = commands.clone();
    ui.on_show_members(move |open| {
        let _ = sender.send(Command::ShowMembers(open));
    });
    let sender = commands.clone();
    ui.on_invite(move |username| {
        let _ = sender.send(Command::Invite(username.into()));
    });
    let sender = commands.clone();
    ui.on_remove_member(move |id| {
        let _ = sender.send(Command::Remove(id.into()));
    });
    let sender = commands.clone();
    ui.on_leave_group(move || {
        let _ = sender.send(Command::Leave);
    });
    let sender = commands.clone();
    ui.on_play_audio(move |id| {
        let _ = sender.send(Command::PlayAudio(id.into()));
    });
    let sender = commands.clone();
    ui.on_join_call(move || {
        let _ = sender.send(Command::JoinCall);
    });
    let sender = commands.clone();
    ui.on_leave_call(move || {
        let _ = sender.send(Command::LeaveCall);
    });
    let sender = commands.clone();
    ui.on_toggle_mute(move || {
        let _ = sender.send(Command::ToggleMute);
    });
    let sender = commands.clone();
    ui.on_toggle_camera(move || {
        let _ = sender.send(Command::ToggleCamera);
    });
    overlay::connect(&ui, commands.clone());
    let sender = commands.clone();
    ui.on_open_share_picker(move || {
        let _ = sender.send(Command::OpenSharePicker);
    });
    let sender = commands.clone();
    ui.on_close_share_picker(move || {
        let _ = sender.send(Command::CloseSharePicker);
    });
    let sender = commands.clone();
    ui.on_start_share(move |id, audio| {
        let _ = sender.send(Command::StartShare {
            source_id: id.into(),
            audio,
        });
    });
    let sender = commands.clone();
    ui.on_stop_share(move || {
        let _ = sender.send(Command::StopShare);
    });
    // `video.rs` troca só a linha de cada tile, então a lista precisa ser
    // um `VecModel` desde o começo.
    ui.set_video_tiles(ModelRc::new(VecModel::<VideoTile>::default()));

    // Volume de cada pessoa: grava, avisa a chamada e já mostra na tela,
    // sem esperar a rede, para o slider andar junto com o mouse.
    volume::init(&profile);
    let ui_handle = ui.as_weak();
    ui.on_set_volume(move |id, screen, percent| {
        let kind = if screen {
            volume::Kind::Screen
        } else {
            volume::Kind::Voice
        };
        let volumes = volume::shared();
        volumes.set(&id, kind, percent);
        let percent = volumes.get(&id, kind);
        let Some(ui) = ui_handle.upgrade() else {
            return;
        };
        let people = ui.get_call_people();
        for (i, mut row) in people.iter().enumerate() {
            if row.id == id {
                if screen {
                    row.screen = percent;
                } else {
                    row.voice = percent;
                }
                people.set_row_data(i, row);
            }
        }
    });
    // Assistir a tela de alguém e qual vídeo está grande: vão direto para a
    // chamada, que assina ou larga as faixas (`call::Viewing`).
    ui.on_watch_screen(|person, on| call::viewing().watch_screen(&person, on));
    ui.on_focus_changed(|big, full| call::viewing().set_focus(&big, &full));
    let ui_handle = ui.as_weak();
    ui.on_set_window_fullscreen(move |on| {
        if let Some(ui) = ui_handle.upgrade() {
            ui.window().set_fullscreen(on);
        }
    });
    let pending = Arc::new(Mutex::new(Vec::<PendingFile>::new()));
    let sender = commands.clone();
    ui.on_edit_chat_message(move |id, text| {
        let _ = sender.send(Command::EditMessage {
            id: id.into(),
            text: text.into(),
        });
    });
    let sender = commands.clone();
    ui.on_delete_chat_message(move |id| {
        let _ = sender.send(Command::DeleteMessage(id.into()));
    });
    let sender = commands.clone();
    let slot = pending.clone();
    let weak = ui.as_weak();
    ui.on_send_message(move |text| {
        let files = std::mem::take(&mut *slot.lock().unwrap());
        if files.is_empty() {
            if !text.trim().is_empty() {
                let _ = sender.send(Command::Send(text.to_string()));
            }
        } else {
            for (index, file) in files.into_iter().enumerate() {
                let caption = if index == 0 {
                    text.to_string()
                } else {
                    String::new()
                };
                let _ = sender.send(Command::SendFile(file, caption));
            }
        }
        if let Some(ui) = weak.upgrade() {
            show_pending(&ui, &[]);
        }
    });
    let slot = pending.clone();
    let weak = ui.as_weak();
    ui.on_cancel_attachment(move || {
        slot.lock().unwrap().clear();
        if let Some(ui) = weak.upgrade() {
            show_pending(&ui, &[]);
        }
    });
    // Fontes (V6-05): aplica as escolhidas e troca na hora pelo seletor.
    let fonts = fonts::Fonts::for_profile(&profile);
    show_fonts(&ui, &fonts);
    let ui_handle = ui.as_weak();
    let registry = fonts.clone();
    ui.on_choose_font(move |kind, index| {
        let Some(kind) = fonts::Kind::from_id(&kind) else {
            return;
        };
        registry.set(kind, index.max(0) as usize);
        if let Some(ui) = ui_handle.upgrade() {
            show_fonts(&ui, &registry);
        }
    });

    let devices = devices::Devices::for_profile(&profile);
    // Redução de ruído do microfone na chamada (`noise.rs`).
    let noise_level = devices.get().noise_level;
    noise::set_level(noise_level);
    ui.set_noise_level(i32::from(noise_level));
    let registry = devices.clone();
    let ui_handle = ui.as_weak();
    ui.on_set_noise_level(move |level| {
        let level = level.clamp(0, 100) as u8;
        registry.set_noise_level(level);
        noise::set_level(level);
        if let Some(ui) = ui_handle.upgrade() {
            ui.set_noise_level(i32::from(level));
        }
    });
    ui.set_share_audio_supported(share_audio::supported());
    // A lista de dispositivos é procurada de novo cada vez que a tela de
    // configurações abre (alguém pode ter ligado um fone).
    let available = Arc::new(Mutex::new(devices::Available::default()));
    let ui_handle = ui.as_weak();
    let (registry, found) = (devices.clone(), available.clone());
    ui.on_refresh_devices(move || {
        let (ui_handle, registry, found) = (ui_handle.clone(), registry.clone(), found.clone());
        std::thread::spawn(move || {
            let list = devices::available();
            *found.lock().unwrap() = list.clone();
            let _ = ui_handle.upgrade_in_event_loop(move |ui| show_devices(&ui, &list, &registry));
        });
    });
    let ui_handle = ui.as_weak();
    let (registry, found, sender) = (devices.clone(), available.clone(), commands.clone());
    ui.on_choose_device(move |kind, index| {
        let Some(kind) = devices::Kind::from_id(&kind) else {
            return;
        };
        let list = found.lock().unwrap().clone();
        let options = match kind {
            devices::Kind::Mic => &list.mics,
            devices::Kind::Speaker => &list.speakers,
            devices::Kind::Camera => &list.cameras,
        };
        // 0 é o padrão do sistema.
        let choice = usize::try_from(index)
            .ok()
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| options.get(i).cloned());
        registry.set(kind, choice);
        if let Some(ui) = ui_handle.upgrade() {
            show_devices(&ui, &list, &registry);
        }
        let _ = sender.send(Command::DevicesChanged);
        // Trocou a câmera no meio do teste: testa a nova.
        if kind == devices::Kind::Camera
            && let Some(ui) = ui_handle.upgrade()
            && ui.get_camera_testing()
        {
            ui.invoke_test_camera(true);
        }
    });

    // Teste da câmera das configurações. Só um por vez; ligar de novo
    // troca pelo da câmera escolhida agora.
    // `round` conta os cliques: um teste que demorou a abrir e já foi
    // desligado ou trocado não fica com a câmera.
    let camera_test = Arc::new(Mutex::new(None::<video::CameraCapture>));
    let round = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let ui_handle = ui.as_weak();
    let registry = devices.clone();
    ui.on_test_camera(move |on| {
        let Some(ui) = ui_handle.upgrade() else {
            return;
        };
        let mine = round.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        // Largar espera a thread da câmera soltar a webcam, o que pode levar
        // um quadro inteiro; fora da thread da tela, para ela não travar.
        if let Some(old) = camera_test.lock().unwrap().take() {
            std::thread::spawn(move || drop(old));
        }
        ui.set_camera_test_frame(slint::Image::default());
        if !on {
            ui.set_camera_testing(false);
            ui.set_camera_test_status(SharedString::default());
            return;
        }
        if ui.get_call_camera() {
            ui.set_camera_testing(false);
            ui.set_camera_test_status("a câmera já está ligada na chamada".into());
            return;
        }
        ui.set_camera_testing(true);
        let chosen = registry.get().camera;
        let (ui_weak, slot, round) = (ui_handle.clone(), camera_test.clone(), round.clone());
        // Achar a câmera pergunta ao sistema e demora; fora da tela também.
        std::thread::spawn(move || {
            let index = devices::camera_index(&chosen);
            let mut slot = slot.lock().unwrap();
            if round.load(std::sync::atomic::Ordering::SeqCst) == mine {
                *slot = Some(video::start_test(index, ui_weak));
            }
        });
    });

    let audio = audio::spawn(ui.as_weak(), commands.clone(), devices.clone());
    let handle = audio.clone();
    ui.on_start_recording(move || handle.start_recording());
    let handle = audio.clone();
    ui.on_stop_recording(move || handle.stop_recording());
    let handle = audio.clone();
    ui.on_cancel_recording(move || handle.cancel_recording());
    let handle = audio.clone();
    ui.on_stop_audio(move || handle.stop_playback());

    let slot = pending.clone();
    let weak = ui.as_weak();
    ui.on_pick_file(move || {
        let slot = slot.clone();
        let weak = weak.clone();
        std::thread::spawn(move || {
            if let Some(path) = rfd::FileDialog::new().pick_file() {
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    let mut pending = slot.lock().unwrap();
                    pending.push(PendingFile {
                        path,
                        temporary: false,
                    });
                    show_pending(&ui, &pending);
                });
            }
        });
    });
    let slot = pending.clone();
    let weak = ui.as_weak();
    ui.on_paste_clipboard(move || {
        let Some(paste) = clipboard::read() else {
            return false;
        };
        let files = match paste {
            clipboard::Paste::Files(files) => files
                .into_iter()
                .map(|path| PendingFile {
                    path,
                    temporary: false,
                })
                .collect::<Vec<_>>(),
            clipboard::Paste::Image(image) => {
                let Some(path) = clipboard::save_image(image) else {
                    return false;
                };
                vec![PendingFile {
                    path,
                    temporary: true,
                }]
            }
        };
        let mut pending = slot.lock().unwrap();
        pending.extend(files);
        if let Some(ui) = weak.upgrade() {
            show_pending(&ui, &pending);
        }
        true
    });
    let sender = commands.clone();
    ui.on_save_file(move |id, name| {
        let sender = sender.clone();
        std::thread::spawn(move || {
            if let Some(path) = rfd::FileDialog::new()
                .set_file_name(name.as_str())
                .save_file()
                && let Ok(id) = id.parse().map(AttachmentId)
            {
                let _ = sender.send(Command::SaveFile {
                    attachment_id: id,
                    path,
                });
            }
        });
    });

    let sender = commands.clone();
    ui.on_open_image(move |id, name| {
        if let Ok(id) = id.parse() {
            let _ = sender.send(Command::ViewImage(AttachmentId(id), name.into()));
        }
    });
    let sender = commands.clone();
    ui.on_rename_user(move |name| {
        let _ = sender.send(Command::Rename(name.trim().to_string()));
    });
    let sender = commands.clone();
    ui.on_change_password(move |current, new| {
        let _ = sender.send(Command::ChangePassword {
            current: current.into(),
            new: new.into(),
        });
    });
    // Perfis: mini cartão, página completa e o editor da sua página.
    let sender = commands.clone();
    let weak = ui.as_weak();
    ui.on_request_profile(move |id, name, page| {
        let place = if page {
            profiles::Place::Page
        } else {
            profiles::Place::Card
        };
        if let Some(ui) = weak.upgrade() {
            let me = id == ui.get_my_id();
            // A foto pequena das listas aparece na hora; a grande vem depois.
            let small = if me {
                ui.get_profile_photo()
            } else {
                photos::image(&id).unwrap_or_default()
            };
            let current = if page { ui.get_page() } else { ui.get_card() };
            let placeholder = (current.id != id).then(|| ProfileViewData {
                id: id.clone(),
                is_me: me,
                name: name.clone(),
                initial: net::initial_of(&name).into(),
                loading: ui.get_profiles_supported(),
                ..Default::default()
            });
            if page {
                ui.set_page_small_photo(small);
                if let Some(view) = placeholder {
                    ui.set_page(view);
                    ui.set_page_photo(slint::Image::default());
                    ui.set_page_background(slint::Image::default());
                    ui.set_page_art(slint::Image::default());
                    ui.set_page_art_side(slint::Image::default());
                }
            } else {
                ui.set_card_small_photo(small);
                if let Some(view) = placeholder {
                    ui.set_card(view);
                    ui.set_card_photo(slint::Image::default());
                    ui.set_card_background(slint::Image::default());
                }
            }
        }
        let _ = sender.send(Command::ShowProfile {
            user_id: id.into(),
            name: name.into(),
            place,
        });
    });
    let sender = commands.clone();
    ui.on_profile_hidden(move |page| {
        let place = if page {
            profiles::Place::Page
        } else {
            profiles::Place::Card
        };
        let _ = sender.send(Command::HideProfile(place));
    });
    let sender = commands.clone();
    let weak = ui.as_weak();
    ui.on_save_profile(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let text = profiles::text_from_editor(
            &ui.get_edit_real_name(),
            &ui.get_edit_location(),
            &ui.get_edit_about(),
            &ui.get_edit_accent(),
            ui.get_edit_show_art(),
            ui.get_edit_show_about(),
        );
        let _ = sender.send(Command::SaveProfile(text));
    });
    let sender = commands.clone();
    let weak = ui.as_weak();
    ui.on_pick_profile_image(move |kind| {
        let Some(kind) = protocol::ProfileImage::from_slug(&kind) else {
            return;
        };
        let sender = sender.clone();
        let weak = weak.clone();
        std::thread::spawn(move || {
            let Some(path) = rfd::FileDialog::new()
                .add_filter("Imagem", &["png", "jpg", "jpeg", "gif", "webp"])
                .pick_file()
            else {
                return;
            };
            let message = match std::fs::read(&path) {
                Ok(bytes) => {
                    let _ = sender.send(Command::SetProfileImage { kind, bytes });
                    ("enviando a imagem...", false)
                }
                Err(_) => ("não deu para abrir o arquivo", true),
            };
            let _ = weak.upgrade_in_event_loop(move |ui| {
                ui.set_edit_message(message.0.into());
                ui.set_edit_error(message.1);
            });
        });
    });
    let sender = commands.clone();
    ui.on_remove_profile_image(move |kind| {
        if let Some(kind) = protocol::ProfileImage::from_slug(&kind) {
            let _ = sender.send(Command::RemoveProfileImage(kind));
        }
    });
    let _profile_animation = profiles::start_animation(&ui);

    let photo_path = session::profile_file(&profile, "photo.png");
    if let Some(path) = &photo_path
        && let Ok(image) = slint::Image::load_from_path(path)
    {
        ui.set_profile_photo(image);
    }
    let weak = ui.as_weak();
    let sender = commands.clone();
    ui.on_pick_profile_photo(move || {
        let weak = weak.clone();
        let destination = photo_path.clone();
        let sender = sender.clone();
        std::thread::spawn(move || {
            let Some(path) = rfd::FileDialog::new()
                .add_filter("Imagem", &["png", "jpg", "jpeg", "webp", "gif"])
                .pick_file()
            else {
                return;
            };
            // A foto grande da página (pode ser GIF) vai junto, se o
            // servidor guarda perfis; a pequena das listas é o 1º quadro.
            if let Ok(bytes) = std::fs::read(&path) {
                let _ = sender.send(Command::SetProfileImage {
                    kind: protocol::ProfileImage::Avatar,
                    bytes,
                });
            }
            let result = (|| -> Result<_, Box<dyn std::error::Error>> {
                let opened = image::open(path)?;
                // A versão pequena e quadrada vai para o servidor (os outros
                // veem na chamada); a grande fica no perfil, para esta tela.
                if let Ok(png) = photos::encode(&opened) {
                    let _ = sender.send(Command::SetPhoto(png));
                }
                let image = opened.thumbnail(512, 512).to_rgba8();
                if let Some(path) = destination {
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    image.save(path)?;
                }
                Ok(image)
            })();
            if let Ok(image) = result {
                let (w, h) = image.dimensions();
                let bytes = image.into_raw();
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    ui.set_profile_photo(slint::Image::from_rgba8(
                        slint::SharedPixelBuffer::clone_from_slice(&bytes, w, h),
                    ));
                });
            }
        });
    });

    let _dither = start_dither(&ui);
    let _levels = start_voice_levels(&ui);

    // Versão nova nas Releases do GitHub: baixa sozinha e avisa no rodapé.
    if let Some(version) = update::VERSION {
        ui.set_app_version(version.into());
    }
    let updater = update::Updater::start(ui.as_weak());
    // Sem a busca (versão de desenvolvimento): a mariposa da abertura só diz isso e sai.
    if !updater.is_active() {
        update::show_phase(&ui.as_weak(), &update::Phase::Dev);
    }
    let handle = updater.clone();
    let weak = ui.as_weak();
    ui.on_check_updates(move || handle.check_now(&weak));
    let handle = updater.clone();
    ui.on_splash_skip(move || handle.skip());
    let handle = updater.clone();
    ui.on_splash_restart(move || {
        if handle.install(true) {
            let _ = slint::quit_event_loop();
        }
    });
    let handle = updater.clone();
    ui.on_apply_update(move || {
        // Instalador rodando: fecha para ele trocar o .exe e abrir de novo.
        if handle.install(true) {
            let _ = slint::quit_event_loop();
        }
    });
    ui.on_open_update_page(|url| update::open_page(&url));

    let saved = session::SavedSession::for_profile(&profile);
    let ui_handle = ui.as_weak();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().expect("não deu para iniciar o Tokio");
        runtime.block_on(net::run(
            url, saved, ui_handle, commands, command_rx, audio, devices,
        ));
    });

    ui.run()?;
    // Fechou com uma atualização baixada: instala agora, sem reabrir.
    updater.install_on_exit();
    Ok(())
}

/// Com o módulo "desenhar pela placa de vídeo" desligado, o Slint usa o
/// renderizador por software em vez do OpenGL. Num PC com driver ruim o
/// OpenGL desenhava tudo deslocado para baixo (os cliques continuavam no
/// lugar certo). `SLINT_BACKEND` no ambiente continua mandando, para testes.
fn select_renderer(modules: &modules::Modules) -> Result<(), slint::PlatformError> {
    if modules.is_on(Module::Gpu) || std::env::var_os("SLINT_BACKEND").is_some() {
        return Ok(());
    }
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .select()
}

/// As listas de fontes das configurações e o global `Theme`, que todo
/// texto da interface lê: trocar aqui redesenha o app inteiro.
fn show_fonts(ui: &AppWindow, fonts: &fonts::Fonts) {
    let labels = |kind: fonts::Kind| {
        let rows: Vec<SharedString> = kind.faces().iter().map(|f| f.label.into()).collect();
        ModelRc::new(VecModel::from(rows))
    };
    ui.set_font_pixel_options(labels(fonts::Kind::Pixel));
    ui.set_font_sans_options(labels(fonts::Kind::Sans));
    ui.set_font_pixel_choice(fonts.choice(fonts::Kind::Pixel) as i32);
    ui.set_font_sans_choice(fonts.choice(fonts::Kind::Sans) as i32);
    let theme = ui.global::<Theme>();
    theme.set_pixel(fonts.family(fonts::Kind::Pixel).into());
    theme.set_sans(fonts.family(fonts::Kind::Sans).into());
    theme.set_pixel_scale(fonts.scale(fonts::Kind::Pixel));
    theme.set_sans_scale(fonts.scale(fonts::Kind::Sans));
}

/// Copia o registro para a tela: a lista das configurações e o global
/// `Modules`, que o resto da interface lê para mostrar ou esconder cada coisa.
fn show_modules(ui: &AppWindow, modules: &modules::Modules) {
    let rows: Vec<ModuleRow> = Module::ALL
        .into_iter()
        .map(|module| ModuleRow {
            id: module.id().into(),
            name: module.name().into(),
            description: module.description().into(),
            on: modules.is_on(module),
        })
        .collect();
    ui.set_modules(ModelRc::new(VecModel::from(rows)));

    let global = ui.global::<Modules>();
    global.set_dither(modules.is_on(Module::Dither));
    global.set_received_sound(modules.is_on(Module::ReceivedSound));
    global.set_call_sound(modules.is_on(Module::CallSound));
    global.set_music(modules.is_on(Module::Music));
}

/// As listas de microfone, saída e câmera das configurações, com o
/// escolhido marcado. A primeira opção de cada uma é o padrão do sistema.
fn show_devices(ui: &AppWindow, list: &devices::Available, registry: &devices::Devices) {
    let prefs = registry.get();
    let options = |found: &[devices::Choice]| {
        let names: Vec<SharedString> = std::iter::once("padrão do sistema".into())
            .chain(found.iter().map(|c| c.name.as_str().into()))
            .collect();
        ModelRc::new(VecModel::from(names))
    };
    let at = |found: &[devices::Choice], chosen| devices::position(found, chosen) as i32;
    ui.set_mic_options(options(&list.mics));
    ui.set_mic_choice(at(&list.mics, &prefs.mic));
    ui.set_speaker_options(options(&list.speakers));
    ui.set_speaker_choice(at(&list.speakers, &prefs.speaker));
    ui.set_camera_options(options(&list.cameras));
    ui.set_camera_choice(at(&list.cameras, &prefs.camera));
}

/// Largura da coluna de conversas no `app.slint`.
const SIDEBAR_WIDTH: f32 = 320.0;

/// Troca o quadro do dither algumas vezes por segundo. O timer precisa viver
/// enquanto a janela estiver aberta, por isso volta para o `main` guardar.
/// Borda da foto de quem fala: lê o nível da voz de cada um (`levels.rs`)
/// umas 30 vezes por segundo e mexe só no campo `level` das linhas da
/// chamada que mudaram. Sem ninguém na chamada, não faz nada.
fn start_voice_levels(ui: &AppWindow) -> slint::Timer {
    let ui_handle = ui.as_weak();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(33),
        move || {
            let Some(ui) = ui_handle.upgrade() else {
                return;
            };
            let model = ui.get_call_people();
            let Some(rows) = model.as_any().downcast_ref::<VecModel<CallPersonRow>>() else {
                return;
            };
            if rows.row_count() == 0 {
                return;
            }
            let now = levels::snapshot();
            for i in 0..rows.row_count() {
                let Some(mut row) = rows.row_data(i) else {
                    continue;
                };
                let key = if row.is_me {
                    levels::ME
                } else {
                    row.id.as_str()
                };
                let level = levels::smooth(row.level, now.get(key).copied().unwrap_or(0.0));
                // Abaixo de 1% vira zero, senão a borda nunca some de vez.
                let level = if level < 0.01 { 0.0 } else { level };
                if (level - row.level).abs() > 0.004 {
                    row.level = level;
                    rows.set_row_data(i, row);
                }
            }
        },
    );
    timer
}

fn start_dither(ui: &AppWindow) -> slint::Timer {
    let started = std::time::Instant::now();
    let ui_handle = ui.as_weak();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(1000 / dither::FPS),
        move || {
            let Some(ui) = ui_handle.upgrade() else {
                return;
            };
            // Desligado ou fora da tela: não gasta nada.
            if !ui.get_logged_in() || !ui.global::<Modules>().get_dither() {
                return;
            }
            let window = ui.window();
            let height = window.size().to_logical(window.scale_factor()).height;
            let cols = (SIDEBAR_WIDTH / dither::CELL).ceil() as u32;
            let rows = (height / dither::CELL).ceil().max(1.0) as u32;
            let frame = dither::frame(cols, rows, started.elapsed().as_secs_f32());
            ui.set_dither_frame(slint::Image::from_rgba8(frame));
        },
    );
    timer
}

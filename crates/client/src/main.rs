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
mod devices;
mod dither;
mod e2ee;
mod log_file;
mod modules;
mod net;
mod screen;
mod session;
mod sframe;
mod share_audio;
mod sound;
mod title_bar;
mod video;
mod volume;

// `Modules` sozinho é o global do Slint; o registro é `modules::Modules`.
use modules::Module;
use protocol::{AttachmentId, ConversationId};
use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use tokio::sync::mpsc;

slint::include_modules!();

/// O que a interface pede para a rede fazer.
pub enum Command {
    Login {
        username: String,
        password: String,
    },
    Register {
        username: String,
        password: String,
    },
    Logout,
    Open(String),
    Send(String),
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
    SendFile(std::path::PathBuf),
    FileUploaded {
        conversation_id: ConversationId,
        attachment_id: AttachmentId,
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
    let profile = std::env::var("MOTHH_PERFIL").unwrap_or_else(|_| "padrao".into());
    log_file::redirect(&profile);
    let url = std::env::var("MOTHH_SERVIDOR").unwrap_or_else(|_| {
        option_env!("MOTHH_DEFAULT_SERVIDOR")
            .unwrap_or("ws://127.0.0.1:3000/ws")
            .into()
    });

    let ui = AppWindow::new()?;
    if profile != "padrao" {
        ui.set_profile(profile.clone().into());
    }
    title_bar::connect(&ui);

    let modules = modules::Modules::for_profile(&profile);
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

    let (commands, command_rx) = mpsc::unbounded_channel::<Command>();

    // Cada callback ganha a sua cópia do `Sender` (`move` leva ela para dentro).
    let sender = commands.clone();
    ui.on_login(move |username, password| {
        let _ = sender.send(Command::Login {
            username: username.into(),
            password: password.into(),
        });
    });
    let sender = commands.clone();
    ui.on_register(move |username, password| {
        let _ = sender.send(Command::Register {
            username: username.into(),
            password: password.into(),
        });
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
    let sender = commands.clone();
    ui.on_send_message(move |text| {
        let _ = sender.send(Command::Send(text.to_string()));
    });

    let devices = devices::Devices::for_profile(&profile);
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

    let sender = commands.clone();
    ui.on_pick_file(move || {
        let sender = sender.clone();
        std::thread::spawn(move || {
            if let Some(path) = rfd::FileDialog::new().pick_file() {
                let _ = sender.send(Command::SendFile(path));
            }
        });
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

    let _dither = start_dither(&ui);

    let saved = session::SavedSession::for_profile(&profile);
    let ui_handle = ui.as_weak();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().expect("não deu para iniciar o Tokio");
        runtime.block_on(net::run(
            url, saved, ui_handle, commands, command_rx, audio, devices,
        ));
    });

    ui.run()
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

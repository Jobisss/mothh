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

mod audio;
mod call;
mod dither;
mod modules;
mod net;
mod session;
mod sound;
mod video;

// `Modules` sozinho é o global do Slint; o registro é `modules::Modules`.
use modules::Module;
use protocol::{AttachmentId, ConversationId};
use slint::{ComponentHandle, ModelRc, VecModel};
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
}

fn main() -> Result<(), slint::PlatformError> {
    let profile = std::env::var("MOTHH_PERFIL").unwrap_or_else(|_| "padrao".into());
    let url = std::env::var("MOTHH_SERVIDOR").unwrap_or_else(|_| {
        option_env!("MOTHH_DEFAULT_SERVIDOR")
            .unwrap_or("ws://127.0.0.1:3000/ws")
            .into()
    });

    let ui = AppWindow::new()?;
    if profile != "padrao" {
        ui.set_profile(profile.clone().into());
    }

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
    // `video.rs` troca só a linha de cada tile, então a lista precisa ser
    // um `VecModel` desde o começo.
    ui.set_video_tiles(ModelRc::new(VecModel::<VideoTile>::default()));
    let sender = commands.clone();
    ui.on_send_message(move |text| {
        let _ = sender.send(Command::Send(text.to_string()));
    });

    let audio = audio::spawn(ui.as_weak(), commands.clone());
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
        runtime.block_on(net::run(url, saved, ui_handle, commands, command_rx, audio));
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

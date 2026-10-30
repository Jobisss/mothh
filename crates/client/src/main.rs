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
mod net;
mod session;

use protocol::{AttachmentId, ConversationId};
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
}

fn main() -> Result<(), slint::PlatformError> {
    let profile = std::env::var("MOTHH_PERFIL").unwrap_or_else(|_| "padrao".into());
    let url = std::env::var("MOTHH_SERVIDOR").unwrap_or_else(|_| "ws://127.0.0.1:3000/ws".into());

    let ui = AppWindow::new()?;
    if profile != "padrao" {
        ui.set_profile(profile.clone().into());
    }

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

    let saved = session::SavedSession::for_profile(&profile);
    let ui_handle = ui.as_weak();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().expect("não deu para iniciar o Tokio");
        runtime.block_on(net::run(url, saved, ui_handle, commands, command_rx, audio));
    });

    ui.run()
}

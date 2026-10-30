//! App desktop do mothh.
//!
//! Duas partes rodam ao mesmo tempo:
//! - a interface Slint, na thread principal (exigência do sistema de janelas);
//! - a rede, numa thread com runtime Tokio (`net.rs`).
//!
//! Elas conversam por mensagens: a interface manda `Command`s para a rede, e a
//! rede atualiza a interface com `slint::Weak::upgrade_in_event_loop`.

mod modules;
mod net;
mod session;

// `Modules` sozinho é o global do Slint; o registro é `modules::Modules`.
use modules::Module;
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
}

fn main() -> Result<(), slint::PlatformError> {
    let profile = std::env::var("MOTHH_PERFIL").unwrap_or_else(|_| "padrao".into());
    let url = std::env::var("MOTHH_SERVIDOR").unwrap_or_else(|_| "ws://127.0.0.1:3000/ws".into());

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
    ui.on_send_message(move |text| {
        let _ = commands.send(Command::Send(text.to_string()));
    });

    let saved = session::SavedSession::for_profile(&profile);
    let ui_handle = ui.as_weak();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().expect("não deu para iniciar o Tokio");
        runtime.block_on(net::run(url, saved, ui_handle, command_rx));
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
    global.set_sent_sound(modules.is_on(Module::SentSound));
    global.set_received_sound(modules.is_on(Module::ReceivedSound));
}

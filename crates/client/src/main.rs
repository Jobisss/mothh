//! App desktop do mothh.
//!
//! Duas partes rodam ao mesmo tempo:
//! - a interface Slint, na thread principal (exigência do sistema de janelas);
//! - a rede, numa thread com runtime Tokio (`net.rs`).
//!
//! Elas conversam por mensagens: a interface manda `Command`s para a rede, e a
//! rede atualiza a interface com `slint::Weak::upgrade_in_event_loop`.

mod net;

use tokio::sync::mpsc;

slint::include_modules!();

/// O que a interface pede para a rede fazer.
pub enum Command {
    Open(String),
    Send(String),
}

fn main() -> Result<(), slint::PlatformError> {
    let name = std::env::var("MOTHH_NOME").unwrap_or_else(|_| "visitante".into());
    let url = std::env::var("MOTHH_SERVIDOR").unwrap_or_else(|_| "ws://127.0.0.1:3000/ws".into());

    let ui = AppWindow::new()?;
    ui.set_user_name(name.clone().into());

    let (commands, command_rx) = mpsc::unbounded_channel::<Command>();

    {
        let commands = commands.clone();
        ui.on_open_conversation(move |id| {
            let _ = commands.send(Command::Open(id.to_string()));
        });
    }
    ui.on_send_message(move |text| {
        let _ = commands.send(Command::Send(text.to_string()));
    });

    let ui_handle = ui.as_weak();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().expect("não deu para iniciar o Tokio");
        runtime.block_on(net::run(url, name, ui_handle, command_rx));
    });

    ui.run()
}

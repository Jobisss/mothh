//! Conexão com o servidor e o estado local da conversa.
//!
//! Esta tarefa é a única dona do estado (lista de conversas, conversa aberta,
//! mensagens). A interface só recebe cópias prontas para desenhar.

use std::rc::Rc;

use futures_util::{SinkExt, StreamExt};
use protocol::{
    ChatMessage, ClientMessage, ConversationId, ConversationKind, ConversationSummary,
    ServerMessage,
};
use slint::{ModelRc, SharedString, VecModel};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::{AppWindow, Command, ConversationRow, MessageRow, token};

#[derive(Default)]
struct LocalState {
    conversations: Vec<ConversationSummary>,
    open: Option<ConversationId>,
    messages: Vec<ChatMessage>,
    /// Entrando com o token salvo: se o servidor recusar, o token é esquecido.
    resuming: bool,
}

pub async fn run(
    url: String,
    profile: String,
    ui: slint::Weak<AppWindow>,
    mut commands: UnboundedReceiver<Command>,
) {
    let (socket, _) = match connect_async(&url).await {
        Ok(ok) => ok,
        Err(err) => {
            set_status(&ui, format!("sem conexão com {url}: {err}"));
            return;
        }
    };
    let (mut sink, mut stream) = socket.split();

    let mut state = LocalState::default();
    set_status(&ui, "conectado".into());

    // Tem um login salvo? Entra direto, sem mostrar a tela de senha.
    if let Some(token) = token::load(&profile) {
        state.resuming = true;
        set_auth(&ui, true, String::new());
        if send(&mut sink, &ClientMessage::Resume { token })
            .await
            .is_err()
        {
            set_status(&ui, "a conexão caiu".into());
            return;
        }
    }

    loop {
        tokio::select! {
            frame = stream.next() => {
                let Some(Ok(Message::Text(text))) = frame else {
                    if frame.is_none() || matches!(frame, Some(Err(_))) {
                        set_status(&ui, "a conexão caiu".into());
                        return;
                    }
                    continue;
                };
                match serde_json::from_str::<ServerMessage>(&text) {
                    Ok(msg) => apply(&mut state, msg, &profile, &ui),
                    Err(_) => set_status(&ui, "o servidor mandou algo que não entendi".into()),
                }
            }
            command = commands.recv() => {
                let Some(command) = command else { return };
                let outgoing = match command {
                    Command::Login { username, password } => {
                        set_auth(&ui, true, String::new());
                        Some(ClientMessage::Login { username, password })
                    }
                    Command::Register { username, password } => {
                        set_auth(&ui, true, String::new());
                        Some(ClientMessage::Register { username, password })
                    }
                    Command::Logout => {
                        token::forget(&profile);
                        state = LocalState::default();
                        render(&state, &ui);
                        let _ = ui.upgrade_in_event_loop(|ui| {
                            ui.set_logged_in(false);
                            ui.set_user_name("".into());
                        });
                        Some(ClientMessage::Logout)
                    }
                    Command::Open(id) => parse_id(&id).map(|conversation_id| {
                        state.open = Some(conversation_id);
                        state.messages.clear();
                        render(&state, &ui);
                        ClientMessage::OpenConversation { conversation_id }
                    }),
                    Command::Send(text) => state.open.filter(|_| !text.trim().is_empty()).map(
                        |conversation_id| ClientMessage::SendMessage { conversation_id, text },
                    ),
                };
                if let Some(msg) = outgoing
                    && send(&mut sink, &msg).await.is_err()
                {
                    set_status(&ui, "a conexão caiu".into());
                    return;
                }
            }
        }
    }
}

/// Aplica uma mensagem do servidor ao estado local e redesenha.
fn apply(state: &mut LocalState, msg: ServerMessage, profile: &str, ui: &slint::Weak<AppWindow>) {
    match msg {
        ServerMessage::Welcome {
            username,
            token,
            conversations,
            ..
        } => {
            token::save(profile, &token);
            state.resuming = false;
            state.conversations = conversations;
            set_status(ui, "online".into());
            set_auth(ui, false, String::new());
            let _ = ui.upgrade_in_event_loop(move |ui| {
                ui.set_user_name(username.into());
                ui.set_logged_in(true);
            });
        }
        ServerMessage::AuthError { reason } => {
            if state.resuming {
                token::forget(profile);
                state.resuming = false;
            }
            set_auth(ui, false, reason);
        }
        ServerMessage::History {
            conversation_id,
            messages,
        } => {
            if state.open == Some(conversation_id) {
                state.messages = messages;
            }
        }
        ServerMessage::NewMessage { message } => {
            if state.open == Some(message.conversation_id) {
                state.messages.push(message.clone());
            }
            if let Some(conv) = state
                .conversations
                .iter_mut()
                .find(|c| c.id == message.conversation_id)
            {
                conv.last_message = Some(message);
            }
            // A conversa com mensagem mais nova sobe para o topo.
            state
                .conversations
                .sort_by_key(|c| std::cmp::Reverse(c.last_message.as_ref().map(|m| m.sent_at_ms)));
        }
        ServerMessage::Error { reason } => set_status(ui, reason),
    }
    render(state, ui);
}

/// Converte o estado em linhas que o Slint sabe desenhar.
fn render(state: &LocalState, ui: &slint::Weak<AppWindow>) {
    let conversations: Vec<ConversationRow> = state
        .conversations
        .iter()
        .map(|c| ConversationRow {
            id: c.id.0.to_string().into(),
            name: c.name.clone().into(),
            initial: c
                .name
                .chars()
                .next()
                .map(|ch| ch.to_uppercase().to_string())
                .unwrap_or_default()
                .into(),
            preview: c
                .last_message
                .as_ref()
                .map(|m| format!("{}: {}", m.author_name, m.text))
                .unwrap_or_else(|| "nenhuma mensagem ainda".into())
                .into(),
            time: c
                .last_message
                .as_ref()
                .map(|m| clock(m.sent_at_ms))
                .unwrap_or_default()
                .into(),
            is_group: c.kind == ConversationKind::Group,
            selected: state.open == Some(c.id),
        })
        .collect();

    let messages: Vec<MessageRow> = state
        .messages
        .iter()
        .map(|m| MessageRow {
            author: m.author_name.clone().into(),
            time: clock(m.sent_at_ms).into(),
            text: m.text.clone().into(),
        })
        .collect();

    let title: SharedString = state
        .open
        .and_then(|id| state.conversations.iter().find(|c| c.id == id))
        .map(|c| c.name.clone())
        .unwrap_or_default()
        .into();

    // `VecModel` não pode cruzar threads, então montamos ele já dentro da
    // thread da interface.
    let _ = ui.upgrade_in_event_loop(move |ui| {
        ui.set_conversations(ModelRc::from(Rc::new(VecModel::from(conversations))));
        ui.set_messages(ModelRc::from(Rc::new(VecModel::from(messages))));
        ui.set_open_title(title);
    });
}

/// Atualiza a tela de entrada: se está esperando o servidor e qual o erro.
fn set_auth(ui: &slint::Weak<AppWindow>, busy: bool, error: String) {
    let _ = ui.upgrade_in_event_loop(move |ui| {
        ui.set_auth_busy(busy);
        ui.set_auth_error(error.into());
    });
}

fn set_status(ui: &slint::Weak<AppWindow>, status: String) {
    let _ = ui.upgrade_in_event_loop(move |ui| ui.set_status(status.into()));
}

fn parse_id(id: &str) -> Option<ConversationId> {
    id.parse().ok().map(ConversationId)
}

/// "21:06" no fuso horário da máquina.
fn clock(ms: u64) -> String {
    use chrono::{Local, TimeZone};
    Local
        .timestamp_millis_opt(ms as i64)
        .single()
        .map(|t| t.format("%H:%M").to_string())
        .unwrap_or_default()
}

type Sink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;

/// O erro vem numa `Box` porque o tipo de erro do tungstenite é grande, e
/// assim o `Result` fica pequeno para ser passado de volta.
async fn send(
    sink: &mut Sink,
    msg: &ClientMessage,
) -> Result<(), Box<tokio_tungstenite::tungstenite::Error>> {
    let json = serde_json::to_string(msg).expect("ClientMessage sempre vira JSON");
    sink.send(Message::Text(json.into()))
        .await
        .map_err(Box::new)
}

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

use crate::{AppWindow, Command, ConversationRow, MessageRow};

#[derive(Default)]
struct LocalState {
    conversations: Vec<ConversationSummary>,
    open: Option<ConversationId>,
    messages: Vec<ChatMessage>,
}

pub async fn run(
    url: String,
    name: String,
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

    if send(&mut sink, &ClientMessage::Hello { name })
        .await
        .is_err()
    {
        set_status(&ui, "a conexão caiu".into());
        return;
    }

    let mut state = LocalState::default();

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
                    Ok(msg) => apply(&mut state, msg, &ui),
                    Err(_) => set_status(&ui, "o servidor mandou algo que não entendi".into()),
                }
            }
            command = commands.recv() => {
                let Some(command) = command else { return };
                let outgoing = match command {
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
fn apply(state: &mut LocalState, msg: ServerMessage, ui: &slint::Weak<AppWindow>) {
    match msg {
        ServerMessage::Welcome { conversations, .. } => {
            state.conversations = conversations;
            set_status(ui, "online".into());
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

/// O erro do tungstenite é grande; na caixa (`Box`) ele ocupa só um ponteiro.
async fn send(
    sink: &mut Sink,
    msg: &ClientMessage,
) -> Result<(), Box<tokio_tungstenite::tungstenite::Error>> {
    let json = serde_json::to_string(msg).expect("ClientMessage sempre vira JSON");
    sink.send(Message::Text(json.into()))
        .await
        .map_err(Box::new)
}

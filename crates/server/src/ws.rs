//! Uma conexão WebSocket por cliente.

use axum::extract::ws::{Message, Utf8Bytes, WebSocket};
use futures_util::{SinkExt, StreamExt};
use protocol::{ClientMessage, ServerMessage, UserId};
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

use crate::state::AppState;

const MAX_NAME_CHARS: usize = 32;

pub async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();

    // 1. A primeira mensagem tem que ser `Hello` com o nome da pessoa.
    let name = match receiver.next().await {
        Some(Ok(Message::Text(text))) => match serde_json::from_str::<ClientMessage>(&text) {
            Ok(ClientMessage::Hello { name }) => {
                name.trim().chars().take(MAX_NAME_CHARS).collect::<String>()
            }
            _ => String::new(),
        },
        _ => return,
    };
    if name.is_empty() {
        let _ = send(
            &mut sender,
            &ServerMessage::Error {
                reason: "Comece a conexão com um Hello e um nome.".into(),
            },
        )
        .await;
        return;
    }

    let user_id = UserId(Uuid::new_v4());
    tracing::info!(%name, "entrou");

    // Assina os eventos antes do Welcome, para não perder mensagens no meio.
    let mut events = state.subscribe();
    let welcome = ServerMessage::Welcome {
        user_id,
        conversations: state.summaries().await,
    };
    if send(&mut sender, &welcome).await.is_err() {
        return;
    }

    // 2. A partir daqui, duas coisas acontecem ao mesmo tempo:
    //    mensagens que chegam do cliente e eventos que chegam de outras conexões.
    //    `tokio::select!` espera pela primeira que ficar pronta.
    loop {
        tokio::select! {
            incoming = receiver.next() => {
                let Some(Ok(frame)) = incoming else { break };
                let Message::Text(text) = frame else { continue };
                let reply = match serde_json::from_str::<ClientMessage>(&text) {
                    Ok(msg) => handle_client_message(msg, &state, user_id, &name).await,
                    Err(_) => Some(ServerMessage::Error { reason: "Mensagem inválida.".into() }),
                };
                if let Some(reply) = reply
                    && send(&mut sender, &reply).await.is_err()
                {
                    break;
                }
            }
            event = events.recv() => {
                match event {
                    Ok(event) => if send(&mut sender, &event).await.is_err() { break },
                    // Conexão lenta demais perdeu eventos: segue em frente.
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                }
            }
        }
    }

    tracing::info!(%name, "saiu");
}

/// Responde a uma mensagem do cliente. `None` quando não há resposta direta
/// (uma mensagem enviada volta para todos pelo canal de eventos).
async fn handle_client_message(
    msg: ClientMessage,
    state: &AppState,
    user_id: UserId,
    name: &str,
) -> Option<ServerMessage> {
    match msg {
        ClientMessage::Hello { .. } => Some(ServerMessage::Error {
            reason: "Você já está conectado.".into(),
        }),
        ClientMessage::OpenConversation { conversation_id } => {
            Some(match state.history(conversation_id).await {
                Some(messages) => ServerMessage::History {
                    conversation_id,
                    messages,
                },
                None => ServerMessage::Error {
                    reason: "Essa conversa não existe.".into(),
                },
            })
        }
        ClientMessage::SendMessage {
            conversation_id,
            text,
        } => match state.post(conversation_id, user_id, name, &text).await {
            Ok(_) => None,
            Err(reason) => Some(ServerMessage::Error { reason }),
        },
    }
}

async fn send(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    msg: &ServerMessage,
) -> Result<(), axum::Error> {
    let json = serde_json::to_string(msg).expect("ServerMessage sempre vira JSON");
    sender.send(Message::Text(Utf8Bytes::from(json))).await
}

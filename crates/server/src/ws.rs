//! Uma conexão WebSocket por cliente.

use axum::extract::ws::{Message, Utf8Bytes, WebSocket};
use futures_util::{SinkExt, StreamExt};
use protocol::{ClientMessage, ServerMessage, UserId};
use tokio::sync::broadcast::error::RecvError;

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

    // Assina os eventos antes do Welcome, para não perder mensagens no meio.
    let mut events = state.subscribe();
    let welcome = match state.user_by_name(&name).await {
        Ok(user_id) => state
            .summaries(user_id)
            .await
            .map(|conversations| (user_id, conversations)),
        Err(reason) => Err(reason),
    };
    let user_id = match welcome {
        Ok((user_id, conversations)) => {
            let welcome = ServerMessage::Welcome {
                user_id,
                conversations,
            };
            if send(&mut sender, &welcome).await.is_err() {
                return;
            }
            user_id
        }
        Err(reason) => {
            let _ = send(&mut sender, &ServerMessage::Error { reason }).await;
            return;
        }
    };
    tracing::info!(%name, "entrou");

    // 2. A partir daqui, duas coisas acontecem ao mesmo tempo:
    //    mensagens que chegam do cliente e eventos que chegam de outras conexões.
    //    `tokio::select!` espera pela primeira que ficar pronta.
    loop {
        tokio::select! {
            incoming = receiver.next() => {
                let Some(Ok(frame)) = incoming else { break };
                let Message::Text(text) = frame else { continue };
                let reply = match serde_json::from_str::<ClientMessage>(&text) {
                    Ok(msg) => handle_client_message(msg, &state, user_id).await,
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
                    // Só repassa o que é de uma conversa da qual a pessoa participa.
                    Ok(event) if event.recipients.contains(&user_id) => {
                        if send(&mut sender, &event.message).await.is_err() { break }
                    }
                    Ok(_) => continue,
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
) -> Option<ServerMessage> {
    match msg {
        ClientMessage::Hello { .. } => Some(ServerMessage::Error {
            reason: "Você já está conectado.".into(),
        }),
        ClientMessage::OpenConversation { conversation_id } => {
            Some(match state.history(user_id, conversation_id).await {
                Ok(Some(messages)) => ServerMessage::History {
                    conversation_id,
                    messages,
                },
                Ok(None) => ServerMessage::Error {
                    reason: "Essa conversa não existe.".into(),
                },
                Err(reason) => ServerMessage::Error { reason },
            })
        }
        ClientMessage::SendMessage {
            conversation_id,
            text,
        } => match state.post(conversation_id, user_id, &text).await {
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

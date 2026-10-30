//! Uma conexão WebSocket por cliente.
//!
//! A conexão passa por duas fases, e pode voltar da segunda para a primeira:
//! 1. sem conta: só aceita cadastro, login ou `Resume`;
//! 2. com conta: conversas e mensagens, até o `Logout` ou a conexão cair.

use axum::extract::ws::{Message, Utf8Bytes, WebSocket};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use protocol::{ClientMessage, ServerMessage};
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;

use crate::auth::Session;
use crate::state::{AppState, DB_ERROR, Event};

type Sender = SplitSink<WebSocket, Message>;
type Receiver = SplitStream<WebSocket>;

/// Como a fase com conta terminou.
enum ChatEnd {
    LoggedOut,
    Closed,
}

pub async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    loop {
        let Some((session, events)) = authenticate(&mut sender, &mut receiver, &state).await else {
            return;
        };
        tracing::info!(username = %session.username, "entrou");
        let end = chat(&mut sender, &mut receiver, &state, &session, events).await;
        tracing::info!(username = %session.username, "saiu");
        if let ChatEnd::Closed = end {
            return;
        }
    }
}

/// Fase 1: espera um cadastro, login ou `Resume` que dê certo e manda o
/// `Welcome`. `None` quando a conexão cai antes.
async fn authenticate(
    sender: &mut Sender,
    receiver: &mut Receiver,
    state: &AppState,
) -> Option<(Session, broadcast::Receiver<Event>)> {
    loop {
        let Message::Text(text) = receiver.next().await?.ok()? else {
            continue;
        };
        let attempt = match serde_json::from_str::<ClientMessage>(&text) {
            Ok(ClientMessage::Register { username, password }) => {
                state.register(&username, &password).await
            }
            Ok(ClientMessage::Login { username, password }) => {
                state.login(&username, &password).await
            }
            Ok(ClientMessage::Resume { token }) => state.resume(&token).await,
            Ok(_) => Err("Entre na sua conta primeiro.".into()),
            Err(_) => Err("Mensagem inválida.".into()),
        };
        let session = match attempt {
            Ok(session) => session,
            Err(reason) => {
                send(sender, &ServerMessage::AuthError { reason })
                    .await
                    .ok()?;
                continue;
            }
        };

        // Assina os eventos antes do Welcome, para não perder mensagens no meio.
        let events = state.subscribe();
        let conversations = match state.summaries(session.user_id).await {
            Ok(conversations) => conversations,
            Err(e) => {
                tracing::error!(error = %e, "falha ao listar conversas");
                let reason = DB_ERROR.into();
                send(sender, &ServerMessage::AuthError { reason })
                    .await
                    .ok()?;
                continue;
            }
        };
        let welcome = ServerMessage::Welcome {
            user_id: session.user_id,
            username: session.username.clone(),
            token: session.token.clone(),
            conversations,
        };
        send(sender, &welcome).await.ok()?;
        return Some((session, events));
    }
}

/// Fase 2: duas coisas acontecem ao mesmo tempo, mensagens que chegam do
/// cliente e eventos que chegam de outras conexões. `tokio::select!` espera
/// pela primeira que ficar pronta.
async fn chat(
    sender: &mut Sender,
    receiver: &mut Receiver,
    state: &AppState,
    session: &Session,
    mut events: broadcast::Receiver<Event>,
) -> ChatEnd {
    loop {
        tokio::select! {
            incoming = receiver.next() => {
                let Some(Ok(frame)) = incoming else { return ChatEnd::Closed };
                let Message::Text(text) = frame else { continue };
                let reply = match serde_json::from_str::<ClientMessage>(&text) {
                    Ok(ClientMessage::Logout) => {
                        // Mesmo se o banco falhar, esta conexão sai da conta.
                        let _ = state.logout(session).await;
                        return ChatEnd::LoggedOut;
                    }
                    Ok(msg) => handle_client_message(msg, state, session).await,
                    Err(_) => Some(ServerMessage::Error { reason: "Mensagem inválida.".into() }),
                };
                if let Some(reply) = reply
                    && send(sender, &reply).await.is_err()
                {
                    return ChatEnd::Closed;
                }
            }
            event = events.recv() => {
                match event {
                    // Só repassa avisos de conversas de que a pessoa participa.
                    Ok(event) if event.audience.contains(&session.user_id) => {
                        if send(sender, &event.message).await.is_err() {
                            return ChatEnd::Closed;
                        }
                    }
                    Ok(_) => continue,
                    // Conexão lenta demais perdeu eventos: segue em frente.
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => return ChatEnd::Closed,
                }
            }
        }
    }
}

/// Responde a uma mensagem do cliente. `None` quando não há resposta direta
/// (uma mensagem enviada volta para todos pelo canal de eventos).
async fn handle_client_message(
    msg: ClientMessage,
    state: &AppState,
    session: &Session,
) -> Option<ServerMessage> {
    match msg {
        ClientMessage::Register { .. }
        | ClientMessage::Login { .. }
        | ClientMessage::Resume { .. } => Some(ServerMessage::Error {
            reason: "Você já está conectado. Saia da conta antes.".into(),
        }),
        // Tratado em `chat`, porque muda de fase.
        ClientMessage::Logout => None,
        ClientMessage::OpenConversation { conversation_id } => Some(
            match state.history(conversation_id, session.user_id).await {
                Ok(Some(messages)) => ServerMessage::History {
                    conversation_id,
                    messages,
                },
                Ok(None) => ServerMessage::Error {
                    reason: "Essa conversa não existe.".into(),
                },
                Err(e) => {
                    tracing::error!(error = %e, "falha ao ler histórico");
                    ServerMessage::Error {
                        reason: DB_ERROR.into(),
                    }
                }
            },
        ),
        ClientMessage::SendMessage {
            conversation_id,
            text,
        } => match state
            .post(conversation_id, session.user_id, &session.username, &text)
            .await
        {
            Ok(_) => None,
            Err(reason) => Some(ServerMessage::Error { reason }),
        },
    }
}

async fn send(sender: &mut Sender, msg: &ServerMessage) -> Result<(), axum::Error> {
    let json = serde_json::to_string(msg).expect("ServerMessage sempre vira JSON");
    sender.send(Message::Text(Utf8Bytes::from(json))).await
}

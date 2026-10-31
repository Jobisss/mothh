//! Uma conexão WebSocket por cliente.
//!
//! Cada conexão passa por duas fases, que podem se repetir:
//! 1. login: espera `Register`, `Login` ou `Resume`;
//! 2. conversa: mensagens até a pessoa sair (`Logout`) ou a conexão fechar.

use axum::extract::ws::{Message, Utf8Bytes, WebSocket};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use protocol::{ClientMessage, ConversationId, ServerMessage};
use tokio::sync::broadcast::error::RecvError;

use crate::auth::Session;
use crate::state::AppState;

type Sender = SplitSink<WebSocket, Message>;
type Receiver = SplitStream<WebSocket>;

/// Como terminou a fase de conversa.
enum Ended {
    LoggedOut,
    Closed,
}

pub async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();

    loop {
        let Some(session) = authenticate(&mut sender, &mut receiver, &state).await else {
            return;
        };
        tracing::info!(username = %session.username, "entrou");
        // A chamada em que esta conexão está; ao sair ou cair, sai dela também.
        let mut call = None;
        state.online.enter(session.user_id);
        let ended = chat(&mut sender, &mut receiver, &state, &session, &mut call).await;
        state.online.leave(session.user_id);
        // Se esta conexão era quem mudava os grupos, outro membro assume.
        let _ = state.announce_groups_of(session.user_id).await;
        if let Some(id) = call {
            state.leave_call(session.user_id, id).await;
        }
        tracing::info!(username = %session.username, "saiu");
        if let Ended::Closed = ended {
            return;
        }
    }
}

/// Fase 1. `None` quando a conexão fecha antes de alguém entrar.
async fn authenticate(
    sender: &mut Sender,
    receiver: &mut Receiver,
    state: &AppState,
) -> Option<Session> {
    loop {
        let text = next_text(receiver).await?;
        let result = match serde_json::from_str::<ClientMessage>(&text) {
            Ok(ClientMessage::Register { username, password }) => {
                state.register(&username, &password).await
            }
            Ok(ClientMessage::Login { username, password }) => {
                state.login(&username, &password).await
            }
            Ok(ClientMessage::Resume { token }) => state.resume(&token).await,
            Ok(_) => Err("Entre ou crie uma conta primeiro.".into()),
            Err(_) => Err("Mensagem inválida.".into()),
        };
        match result {
            Ok(session) => return Some(session),
            Err(reason) => send(sender, &ServerMessage::AuthError { reason })
                .await
                .ok()?,
        }
    }
}

/// Fase 2: manda o `Welcome` e repassa mensagens até a pessoa sair.
async fn chat(
    sender: &mut Sender,
    receiver: &mut Receiver,
    state: &AppState,
    session: &Session,
    call: &mut Option<ConversationId>,
) -> Ended {
    // Assina os eventos antes do Welcome, para não perder mensagens no meio.
    let mut events = state.subscribe();
    let (welcome, calls) = match state.summaries(session.user_id).await {
        Ok(conversations) => {
            let calls = state.call_presences(conversations.iter().map(|c| c.id));
            let welcome = ServerMessage::Welcome {
                user_id: session.user_id,
                username: session.username.clone(),
                token: session.token.clone(),
                conversations,
            };
            (welcome, calls)
        }
        Err(reason) => (ServerMessage::Error { reason }, Vec::new()),
    };
    if send(sender, &welcome).await.is_err() {
        return Ended::Closed;
    }
    for presence in &calls {
        if send(sender, presence).await.is_err() {
            return Ended::Closed;
        }
    }
    // Criptografia (F2-08): quantos pacotes de chaves faltam repor, os
    // convites para grupos MLS que chegaram com a pessoa fora, e o estado de
    // cada grupo (vai pelo canal de eventos, para todos os membros).
    let mut mls = Vec::new();
    match state.key_package_stock(session.user_id).await {
        Ok(stock) => mls.push(stock),
        Err(reason) => mls.push(ServerMessage::Error { reason }),
    }
    match state.pending_welcomes(session.user_id).await {
        Ok(welcomes) => mls.extend(welcomes),
        Err(reason) => mls.push(ServerMessage::Error { reason }),
    }
    for message in &mls {
        if send(sender, message).await.is_err() {
            return Ended::Closed;
        }
    }
    if let Err(reason) = state.announce_groups_of(session.user_id).await
        && send(sender, &ServerMessage::Error { reason })
            .await
            .is_err()
    {
        return Ended::Closed;
    }

    // Duas coisas acontecem ao mesmo tempo: mensagens que chegam do cliente e
    // eventos que chegam de outras conexões. `tokio::select!` espera pela
    // primeira que ficar pronta.
    loop {
        tokio::select! {
            incoming = next_text(receiver) => {
                let Some(text) = incoming else { return Ended::Closed };
                let reply = match serde_json::from_str::<ClientMessage>(&text) {
                    Ok(ClientMessage::Logout) => {
                        // Mesmo se o banco falhar, esta conexão sai da conta.
                        let _ = state.logout(&session.token).await;
                        return Ended::LoggedOut;
                    }
                    Ok(msg) => handle_client_message(msg, state, session, call).await,
                    Err(_) => Some(ServerMessage::Error { reason: "Mensagem inválida.".into() }),
                };
                if let Some(reply) = reply
                    && send(sender, &reply).await.is_err()
                {
                    return Ended::Closed;
                }
            }
            event = events.recv() => {
                match event {
                    // Só repassa o que é de uma conversa da qual a pessoa participa.
                    Ok(event) if event.recipients.contains(&session.user_id) => {
                        if send(sender, &event.message).await.is_err() {
                            return Ended::Closed;
                        }
                    }
                    Ok(_) => continue,
                    // Conexão lenta demais perdeu eventos: segue em frente.
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => return Ended::Closed,
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
    call: &mut Option<ConversationId>,
) -> Option<ServerMessage> {
    match msg {
        ClientMessage::Register { .. }
        | ClientMessage::Login { .. }
        | ClientMessage::Resume { .. } => Some(ServerMessage::Error {
            reason: "Você já entrou. Saia antes de trocar de conta.".into(),
        }),
        // Tratado em `chat`, porque muda de fase.
        ClientMessage::Logout => None,
        ClientMessage::OpenConversation { conversation_id } => Some(
            match state.history(session.user_id, conversation_id).await {
                Ok(Some(page)) => ServerMessage::History {
                    conversation_id,
                    messages: page.messages,
                    has_more: page.has_more,
                },
                Ok(None) => not_found(),
                Err(reason) => ServerMessage::Error { reason },
            },
        ),
        ClientMessage::LoadOlder {
            conversation_id,
            before,
        } => Some(
            match state.older(session.user_id, conversation_id, before).await {
                Ok(Some(page)) => ServerMessage::Older {
                    conversation_id,
                    messages: page.messages,
                    has_more: page.has_more,
                },
                Ok(None) => not_found(),
                Err(reason) => ServerMessage::Error { reason },
            },
        ),
        ClientMessage::SendMessage {
            conversation_id,
            sealed,
        } => match state.post(conversation_id, session.user_id, &sealed).await {
            Ok(_) => None,
            Err(reason) => Some(ServerMessage::Error { reason }),
        },
        ClientMessage::SendAudio {
            conversation_id,
            attachment_id,
        } => match state
            .post_audio(conversation_id, session.user_id, attachment_id)
            .await
        {
            Ok(_) => None,
            Err(reason) => Some(ServerMessage::Error { reason }),
        },
        ClientMessage::SendFile {
            conversation_id,
            attachment_id,
        } => match state
            .post_file(conversation_id, session.user_id, attachment_id)
            .await
        {
            Ok(_) => None,
            Err(reason) => Some(ServerMessage::Error { reason }),
        },
        ClientMessage::OpenDirect { username } => {
            Some(match state.open_direct(session.user_id, &username).await {
                Ok(conversation) => ServerMessage::DirectOpened { conversation },
                Err(reason) => ServerMessage::Error { reason },
            })
        }
        ClientMessage::CreateGroup { name } => {
            Some(match state.create_group(session.user_id, &name).await {
                Ok(conversation) => ServerMessage::GroupCreated { conversation },
                Err(reason) => ServerMessage::Error { reason },
            })
        }
        // As três abaixo respondem pelo canal de eventos (`Members`,
        // `ConversationAdded`, `ConversationRemoved`); aqui só volta erro.
        ClientMessage::InviteToGroup {
            conversation_id,
            username,
        } => state
            .invite(session.user_id, conversation_id, &username)
            .await
            .err()
            .map(|reason| ServerMessage::Error { reason }),
        ClientMessage::LeaveGroup { conversation_id } => state
            .leave(session.user_id, conversation_id)
            .await
            .err()
            .map(|reason| ServerMessage::Error { reason }),
        ClientMessage::RemoveFromGroup {
            conversation_id,
            user_id,
        } => state
            .remove(session.user_id, conversation_id, user_id)
            .await
            .err()
            .map(|reason| ServerMessage::Error { reason }),
        ClientMessage::ListMembers { conversation_id } => Some(
            match state.members(session.user_id, conversation_id).await {
                Ok(Some(members)) => members,
                Ok(None) => not_found(),
                Err(reason) => ServerMessage::Error { reason },
            },
        ),
        ClientMessage::UploadKeyPackages { packages, replace } => Some(
            state
                .upload_key_packages(session.user_id, &packages, replace)
                .await
                .unwrap_or_else(|reason| ServerMessage::Error { reason }),
        ),
        ClientMessage::ClaimKeyPackages {
            conversation_id,
            user_ids,
        } => Some(
            state
                .claim_key_packages(session.user_id, conversation_id, &user_ids)
                .await
                .unwrap_or_else(|reason| ServerMessage::Error { reason }),
        ),
        ClientMessage::Commit {
            conversation_id,
            epoch,
            commit,
            welcome,
            added,
            removed,
        } => Some(
            state
                .commit(
                    session.user_id,
                    conversation_id,
                    epoch,
                    commit,
                    welcome,
                    added,
                    removed,
                )
                .await
                .unwrap_or_else(|reason| ServerMessage::Error { reason }),
        ),
        ClientMessage::FetchCommits {
            conversation_id,
            from,
        } => Some(
            state
                .fetch_commits(session.user_id, conversation_id, from)
                .await
                .unwrap_or_else(|reason| ServerMessage::Error { reason }),
        ),
        ClientMessage::RejoinGroup { conversation_id } => state
            .rejoin_group(session.user_id, conversation_id)
            .await
            .err()
            .map(|reason| ServerMessage::Error { reason }),
        ClientMessage::JoinCall { conversation_id } => {
            let reply = state
                .join_call(session.user_id, &session.username, conversation_id)
                .await
                .unwrap_or_else(|reason| ServerMessage::Error { reason });
            // Pedir de novo a mesma chamada (o app reconectou) não conta duas
            // vezes; pedir outra tira da anterior.
            if matches!(reply, ServerMessage::CallToken { .. }) && *call != Some(conversation_id) {
                if let Some(previous) = call.replace(conversation_id) {
                    state.leave_call(session.user_id, previous).await;
                }
                state.enter_call(session.user_id, conversation_id).await;
            }
            Some(reply)
        }
        ClientMessage::LeaveCall { conversation_id } => {
            if *call == Some(conversation_id) {
                *call = None;
                state.leave_call(session.user_id, conversation_id).await;
            }
            None
        }
    }
}

fn not_found() -> ServerMessage {
    ServerMessage::Error {
        reason: "Essa conversa não existe.".into(),
    }
}

/// O próximo texto do cliente. `None` quando a conexão fechou.
async fn next_text(receiver: &mut Receiver) -> Option<Utf8Bytes> {
    loop {
        match receiver.next().await? {
            Ok(Message::Text(text)) => return Some(text),
            Ok(Message::Close(_)) | Err(_) => return None,
            // Ping, pong e binário: ignora.
            Ok(_) => continue,
        }
    }
}

async fn send(sender: &mut Sender, msg: &ServerMessage) -> Result<(), axum::Error> {
    let json = serde_json::to_string(msg).expect("ServerMessage sempre vira JSON");
    sender.send(Message::Text(Utf8Bytes::from(json))).await
}

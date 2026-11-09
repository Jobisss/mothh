//! Uma conexão WebSocket por cliente.
//!
//! Cada conexão passa por duas fases, que podem se repetir:
//! 1. login: espera `Register`, `Login` ou `Resume`;
//! 2. conversa: mensagens até a pessoa sair (`Logout`) ou a conexão fechar.

use std::time::Duration;

use axum::extract::ws::{Message, Utf8Bytes, WebSocket};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use protocol::{ClientMessage, ConversationId, ServerMessage};
use tokio::sync::broadcast::error::RecvError;
use tokio::time::{Instant, sleep_until};

/// Conexão sem nenhum quadro (o app manda ping a cada 15 s) por este tempo é
/// dada como caída. Sem isto, um PC que dormiu segue "online" até o TCP
/// desistir, e pode ser escolhido para mudar o grupo MLS sem estar lá.
const IDLE_LIMIT: Duration = Duration::from_secs(90);

use crate::auth::{SESSION_EXPIRED, Session};
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
        let Some(mut session) = authenticate(&mut sender, &mut receiver, &state).await else {
            return;
        };
        tracing::info!(username = %session.username, "entrou");
        // A chamada em que esta conexão está; ao sair ou cair, sai dela também.
        let mut call = None;
        state.online.enter(session.user_id);
        let ended = chat(&mut sender, &mut receiver, &state, &mut session, &mut call).await;
        state.online.leave(session.user_id);
        if !state.online.contains(session.user_id) {
            let _ = state.set_listening_activity(session.user_id, None).await;
        }
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
            Ok(ClientMessage::Resume { token }) => match state.resume(&token).await {
                Ok(Some(session)) => Ok(session),
                Ok(None) => Err(SESSION_EXPIRED.into()),
                // O banco falhou: fecha a conexão em vez de dizer que a
                // sessão expirou. O app reconecta e tenta de novo com o mesmo
                // token, sem esquecer o login.
                Err(reason) => {
                    tracing::warn!(%reason, "não deu para continuar a sessão; fechando");
                    return None;
                }
            },
            Ok(ClientMessage::Register {
                username,
                password,
                email,
            }) => state.register(&username, &password, email.as_deref()).await,
            Ok(ClientMessage::Login { username, password }) => {
                state.login(&username, &password).await
            }
            // Recuperar a senha acontece antes de entrar; a resposta não é
            // uma sessão, então sai daqui mesmo e continua esperando.
            Ok(ClientMessage::RequestPasswordReset { email }) => {
                let reply = match state.request_password_reset(&email).await {
                    Ok(()) => ServerMessage::ResetRequested,
                    Err(reason) => ServerMessage::AuthError { reason },
                };
                send(sender, &reply).await.ok()?;
                continue;
            }
            Ok(ClientMessage::ResetPassword {
                email,
                code,
                password,
            }) => {
                let reply = match state.reset_password(&email, &code, &password).await {
                    Ok(()) => ServerMessage::PasswordReset,
                    Err(reason) => ServerMessage::AuthError { reason },
                };
                send(sender, &reply).await.ok()?;
                continue;
            }
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
    session: &mut Session,
    call: &mut Option<ConversationId>,
) -> Ended {
    // Assina os eventos antes do Welcome, para não perder mensagens no meio.
    let mut events = state.subscribe();
    let (welcome, calls) = match state.summaries(session.user_id).await {
        Ok(conversations) => {
            let ids: Vec<ConversationId> = conversations.iter().map(|c| c.id).collect();
            let calls = state.call_presences(ids).await;
            // Se nem a versão da foto saiu do banco, o resto também não sairia bem.
            let photo_version = state.photo_version(session.user_id).await.unwrap_or(0);
            let email = state.email_of(session.user_id).await.unwrap_or(None);
            let welcome = ServerMessage::Welcome {
                user_id: session.user_id,
                username: session.username.clone(),
                token: session.token.clone(),
                conversations,
                photo_version: Some(photo_version),
                profiles: true,
                listening_activity: true,
                diagnostics: true,
                email,
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
        Err(reason) => {
            tracing::warn!(username = %session.username, %reason, "pacotes de chaves");
            mls.push(ServerMessage::Error { reason });
        }
    }
    match state.pending_welcomes(session.user_id).await {
        Ok(welcomes) => {
            if !welcomes.is_empty() {
                tracing::info!(
                    username = %session.username,
                    convites = welcomes.len(),
                    "mls: convites para grupos que chegaram com a pessoa fora"
                );
            }
            mls.extend(welcomes);
        }
        Err(reason) => {
            tracing::warn!(username = %session.username, %reason, "convites de grupo");
            mls.push(ServerMessage::Error { reason });
        }
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
    let mut deadline = Instant::now() + IDLE_LIMIT;
    let mut diagnostics = Diagnostics::default();
    loop {
        tokio::select! {
            incoming = receiver.next() => {
                deadline = Instant::now() + IDLE_LIMIT;
                let text = match incoming {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(Message::Close(_)) | Err(_)) | None => return Ended::Closed,
                    // Ping, pong e binário: só contam como sinal de vida.
                    Some(Ok(_)) => continue,
                };
                let reply = match serde_json::from_str::<ClientMessage>(&text) {
                    Ok(ClientMessage::Logout) => {
                        // Mesmo se o banco falhar, esta conexão sai da conta.
                        let _ = state.logout(&session.token).await;
                        return Ended::LoggedOut;
                    }
                    Ok(ClientMessage::Diagnostic { text }) => {
                        diagnostics.log(&session.username, &text);
                        None
                    }
                    Ok(msg) => {
                        let request = Peek::of(&text);
                        let reply = handle_client_message(msg, state, session, call).await;
                        request.log(&session.username, reply.as_ref());
                        reply
                    }
                    Err(err) => {
                        tracing::warn!(
                            username = %session.username,
                            pedido = %Peek::of(&text).kind,
                            err = %err.to_string().chars().take(120).collect::<String>(),
                            "mensagem que o servidor não entende (app mais novo que o servidor?)"
                        );
                        Some(ServerMessage::Error { reason: "Mensagem inválida.".into() })
                    }
                };
                if let Some(reply) = reply
                    && send(sender, &reply).await.is_err()
                {
                    return Ended::Closed;
                }
            }
            _ = sleep_until(deadline) => {
                tracing::info!(username = %session.username, "conexão calada: dada como caída");
                return Ended::Closed;
            }
            event = events.recv() => {
                match event {
                    // Só repassa o que é de uma conversa da qual a pessoa participa.
                    Ok(event) if event.recipients.contains(&session.user_id) => {
                        // Trocou de nome noutro aparelho: esta conexão usa o novo
                        // (é o nome que vai no token da chamada).
                        if let ServerMessage::UserRenamed { user_id, username } = &event.message
                            && *user_id == session.user_id
                        {
                            session.username.clone_from(username);
                        }
                        if send(sender, &event.message).await.is_err() {
                            return Ended::Closed;
                        }
                    }
                    Ok(_) => continue,
                    // Conexão lenta demais perdeu eventos (pode ter sido uma
                    // mensagem ou um commit do grupo MLS). Seguir em frente
                    // deixaria o app sem saber; fechando, ele reconecta, recebe
                    // o `Welcome` e pede de novo o histórico e os commits.
                    Err(RecvError::Lagged(lost)) => {
                        tracing::warn!(
                            username = %session.username,
                            perdidos = lost,
                            "conexão lenta perdeu eventos; fechando para o app sincronizar"
                        );
                        return Ended::Closed;
                    }
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
    session: &mut Session,
    call: &mut Option<ConversationId>,
) -> Option<ServerMessage> {
    match msg {
        ClientMessage::Register { .. }
        | ClientMessage::Login { .. }
        | ClientMessage::Resume { .. }
        | ClientMessage::RequestPasswordReset { .. }
        | ClientMessage::ResetPassword { .. } => Some(ServerMessage::Error {
            reason: "Você já entrou. Saia antes de trocar de conta.".into(),
        }),
        ClientMessage::SetEmail { email } => {
            Some(match state.set_email(session.user_id, &email).await {
                Ok(email) => ServerMessage::EmailChanged { email },
                Err(reason) => ServerMessage::Error { reason },
            })
        }
        // Tratados em `chat`: o primeiro muda de fase, o segundo só vai
        // para o log.
        ClientMessage::Logout | ClientMessage::Diagnostic { .. } => None,
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
        // O aviso (`MessageEdited`/`MessageDeleted`) volta pelo canal de
        // eventos, para todos os membros; aqui só o erro.
        ClientMessage::EditMessage {
            conversation_id,
            message_id,
            sealed,
        } => match state
            .edit_message(conversation_id, session.user_id, message_id, &sealed)
            .await
        {
            Ok(_) => None,
            Err(reason) => Some(ServerMessage::Error { reason }),
        },
        ClientMessage::DeleteMessage {
            conversation_id,
            message_id,
        } => match state
            .delete_message(conversation_id, session.user_id, message_id)
            .await
        {
            Ok(()) => None,
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
        ClientMessage::ChangePassword { current, new } => {
            Some(match state.change_password(session, &current, &new).await {
                Ok(()) => ServerMessage::PasswordChanged,
                Err(reason) => ServerMessage::Error { reason },
            })
        }
        // O aviso (`UserRenamed`) volta pelo canal de eventos, para todos.
        ClientMessage::Rename { username } => {
            match state.rename(session.user_id, &username).await {
                Ok(username) => {
                    session.username = username;
                    None
                }
                Err(reason) => Some(ServerMessage::Error { reason }),
            }
        }
        ClientMessage::SetPhoto { png } => state
            .set_photo(session.user_id, &png)
            .await
            .err()
            .map(|reason| ServerMessage::Error { reason }),
        ClientMessage::GetPhoto { user_id } => Some(
            state
                .photo(session.user_id, user_id)
                .await
                .unwrap_or_else(|reason| ServerMessage::Error { reason }),
        ),
        ClientMessage::GetProfile { user_id } => Some(
            state
                .profile(session.user_id, user_id)
                .await
                .unwrap_or_else(|reason| ServerMessage::Error { reason }),
        ),
        ClientMessage::SetListeningActivity { activity } => state
            .set_listening_activity(session.user_id, activity)
            .await
            .err()
            .map(|reason| ServerMessage::Error { reason }),
        ClientMessage::SaveProfile { text } => state
            .save_profile(session.user_id, &text)
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
        // O `GroupState` volta pelo canal de eventos, para todos.
        ClientMessage::NudgeGroup { conversation_id } => state
            .nudge_group(session.user_id, conversation_id)
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

/// O tipo e os ids de um pedido do app, lidos do JSON cru só para o log
/// (sem o conteúdo, que pode ser texto cifrado grande).
#[derive(serde::Deserialize, Default)]
struct Peek {
    #[serde(rename = "type", default)]
    kind: String,
    conversation_id: Option<ConversationId>,
    epoch: Option<u64>,
    from: Option<u64>,
}

impl Peek {
    fn of(text: &str) -> Self {
        serde_json::from_str(text).unwrap_or_default()
    }

    /// Erro devolvido ao app vira `WARN`; a troca de chaves (MLS) e a entrada
    /// e saída das chamadas viram `INFO`, com a época, para dar para seguir
    /// um grupo que travou ou uma pessoa que ficou com a chave velha.
    fn log(&self, username: &str, reply: Option<&ServerMessage>) {
        let conversa = self
            .conversation_id
            .map(|c| c.0.to_string())
            .unwrap_or_default();
        if let Some(ServerMessage::Error { reason }) = reply {
            tracing::warn!(
                %username,
                pedido = %self.kind,
                %conversa,
                motivo = %reason,
                "erro devolvido ao app"
            );
            return;
        }
        let watched = matches!(
            self.kind.as_str(),
            "commit"
                | "fetch_commits"
                | "rejoin_group"
                | "claim_key_packages"
                | "join_call"
                | "leave_call"
        );
        if !watched {
            return;
        }
        let resposta = match reply {
            None => "nenhuma".to_string(),
            Some(ServerMessage::CommitAccepted { epoch, .. }) => format!("aceito, época {epoch}"),
            Some(ServerMessage::CommitRejected { epoch, .. }) => {
                format!("recusado, grupo já na época {epoch}")
            }
            Some(ServerMessage::Commits { commits, .. }) => match (commits.first(), commits.last())
            {
                (Some(a), Some(b)) => format!(
                    "{} commits, épocas {}..={}",
                    commits.len(),
                    a.epoch,
                    b.epoch
                ),
                _ => "nenhum commit novo".to_string(),
            },
            Some(ServerMessage::KeyPackages { packages, .. }) => {
                format!("{} pacotes", packages.len())
            }
            Some(ServerMessage::CallToken { .. }) => "token da chamada".to_string(),
            Some(_) => "outra".to_string(),
        };
        let area = if self.kind.ends_with("_call") {
            "chamada"
        } else {
            "mls"
        };
        tracing::info!(
            %username,
            pedido = %self.kind,
            %conversa,
            epoca = ?self.epoch,
            desde = ?self.from,
            %resposta,
            "{area}"
        );
    }
}

/// As linhas de diagnóstico que o app manda (`ClientMessage::Diagnostic`),
/// com um teto por conexão para um app com defeito não encher o log.
#[derive(Default)]
struct Diagnostics {
    window: Option<Instant>,
    count: u32,
}

impl Diagnostics {
    const PER_MINUTE: u32 = 30;
    const MAX_CHARS: usize = 400;

    fn log(&mut self, username: &str, text: &str) {
        let now = Instant::now();
        if self
            .window
            .is_none_or(|start| now.duration_since(start) >= Duration::from_secs(60))
        {
            self.window = Some(now);
            self.count = 0;
        }
        self.count += 1;
        if self.count > Self::PER_MINUTE {
            return;
        }
        let text: String = text
            .chars()
            .filter(|c| !c.is_control())
            .take(Self::MAX_CHARS)
            .collect();
        if text.contains("erro") || text.contains("falh") || text.contains("não ") {
            tracing::warn!(target: "app", %username, "{text}");
        } else {
            tracing::info!(target: "app", %username, "{text}");
        }
    }
}

fn not_found() -> ServerMessage {
    ServerMessage::Error {
        reason: "Essa conversa não existe.".into(),
    }
}

/// O próximo texto do cliente. `None` quando a conexão fechou ou ficou
/// `IDLE_LIMIT` sem mandar nada: na tela de login o app também manda ping, então
/// só cai aqui uma conexão abandonada, que de outro jeito ficaria aberta para
/// sempre ocupando o servidor.
async fn next_text(receiver: &mut Receiver) -> Option<Utf8Bytes> {
    loop {
        let frame = tokio::time::timeout(IDLE_LIMIT, receiver.next())
            .await
            .ok()??;
        match frame {
            Ok(Message::Text(text)) => return Some(text),
            Ok(Message::Close(_)) | Err(_) => return None,
            // Ping, pong e binário: só contam como sinal de vida.
            Ok(_) => continue,
        }
    }
}

async fn send(sender: &mut Sender, msg: &ServerMessage) -> Result<(), axum::Error> {
    let json = serde_json::to_string(msg).expect("ServerMessage sempre vira JSON");
    sender.send(Message::Text(Utf8Bytes::from(json))).await
}

//! Conexão com o servidor e o estado local da conversa.
//!
//! Esta tarefa é a única dona do estado (lista de conversas, conversa aberta,
//! mensagens). A interface só recebe cópias prontas para desenhar.

use std::rc::Rc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use protocol::{
    ChatMessage, ClientMessage, ConversationId, ConversationKind, ConversationSummary, GROUP_ROOM,
    Member, ServerMessage, UserId,
};
use slint::{ModelRc, SharedString, VecModel};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::{Instant, interval, sleep_until};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::session::SavedSession;
use crate::sound::{self, Sound};
use crate::{AppWindow, Command, ConversationRow, MemberRow, MessageRow};

/// De quanto em quanto tempo mandamos um ping para o servidor.
const PING_EVERY: Duration = Duration::from_secs(15);
/// Sem ouvir nada do servidor por esse tempo, a conexão é dada como morta.
/// Pega o caso em que a rede some e o sistema não avisa que o TCP fechou.
const SILENCE_LIMIT: Duration = Duration::from_secs(45);

#[derive(Default)]
struct LocalState {
    conversations: Vec<ConversationSummary>,
    open: Option<ConversationId>,
    messages: Vec<ChatMessage>,
    /// O servidor tem mensagens mais antigas que a primeira de `messages`.
    has_older: bool,
    /// Já pedimos as anteriores e estamos esperando; evita pedir duas vezes.
    loading_older: bool,
    /// As últimas mudanças entraram no topo da lista: a tela deve manter o
    /// ponto de leitura em vez de pular. Volta a `false` depois de desenhar.
    prepended: bool,
    /// Token da sessão atual, guardado também em memória: se o disco falhar,
    /// a reconexão continua entrando sozinha.
    token: Option<String>,
    /// Já recebemos um `Welcome` e a tela de conversas está aberta.
    logged_in: bool,
    /// Mandamos um `Resume` com o token salvo e esperamos a resposta.
    resuming: bool,
    /// Pedimos uma conversa direta e esperamos a resposta: um erro agora é
    /// sobre a busca, e aparece embaixo do campo.
    opening_direct: bool,
    /// Quem está entrado, para saber se é o dono do grupo aberto.
    me: Option<UserId>,
    /// A coluna de membros do grupo aberto está na tela.
    members_open: bool,
    /// Membros e dono do grupo aberto, do último `Members`.
    members: Vec<Member>,
    owner: Option<UserId>,
    /// Pedimos algo de grupo e esperamos: um `Error` agora é a resposta dele.
    group_pending: bool,
}

/// Por que uma conexão terminou.
enum Ended {
    /// O servidor caiu ou a rede sumiu: tenta de novo.
    Lost,
    /// A janela fechou: acabou o app.
    Quit,
}

/// Espera antes da tentativa número `attempt` (começando em 0): 1s, 2s, 4s,
/// 8s, 16s e depois sempre 30s.
fn backoff(attempt: u32) -> Duration {
    let secs = 1u64 << attempt.min(5);
    Duration::from_secs(secs.min(30))
}

pub async fn run(
    url: String,
    saved: SavedSession,
    ui: slint::Weak<AppWindow>,
    mut commands: UnboundedReceiver<Command>,
) {
    let mut state = LocalState {
        token: saved.load(),
        ..LocalState::default()
    };
    let mut attempt = 0;

    loop {
        if let Ok((socket, _)) = connect_async(&url).await {
            attempt = 0;
            match connection(socket, &mut state, &saved, &ui, &mut commands).await {
                Ended::Quit => return,
                Ended::Lost => {}
            }
        }

        let delay = backoff(attempt);
        attempt = attempt.saturating_add(1);
        show_offline(&state, &ui, delay);
        if !wait_offline(delay, &mut state, &saved, &ui, &mut commands).await {
            return;
        }
    }
}

/// Uma conexão do começo ao fim. O estado local sobrevive entre conexões,
/// por isso a conversa aberta continua na tela enquanto o app reconecta.
async fn connection(
    socket: Socket,
    state: &mut LocalState,
    saved: &SavedSession,
    ui: &slint::Weak<AppWindow>,
    commands: &mut UnboundedReceiver<Command>,
) -> Ended {
    let (mut sink, mut stream) = socket.split();

    // Com um token, tenta entrar direto; senão fica na tela de login.
    if let Some(token) = state.token.clone() {
        state.resuming = true;
        if send(&mut sink, &ClientMessage::Resume { token })
            .await
            .is_err()
        {
            return Ended::Lost;
        }
    } else {
        set_auth_error(ui, String::new());
        show_login(ui);
    }

    let mut ping = interval(PING_EVERY);
    let mut heard_at = Instant::now();

    loop {
        tokio::select! {
            frame = stream.next() => {
                let text = match frame {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(Message::Close(_)) | Err(_)) | None => return Ended::Lost,
                    // Ping, pong, binário: só conta como sinal de vida.
                    Some(Ok(_)) => {
                        heard_at = Instant::now();
                        continue;
                    }
                };
                heard_at = Instant::now();
                let reply = match serde_json::from_str::<ServerMessage>(&text) {
                    Ok(msg) => apply(state, msg, saved, ui),
                    Err(_) => {
                        set_status(ui, "o servidor mandou algo que não entendi".into());
                        None
                    }
                };
                if let Some(reply) = reply
                    && send(&mut sink, &reply).await.is_err()
                {
                    return Ended::Lost;
                }
            }
            command = commands.recv() => {
                let Some(command) = command else { return Ended::Quit };
                let outgoing = match command {
                    Command::Login { username, password } => {
                        Some(ClientMessage::Login { username, password })
                    }
                    Command::Register { username, password } => {
                        Some(ClientMessage::Register { username, password })
                    }
                    Command::Logout => {
                        forget_session(state, saved, ui);
                        Some(ClientMessage::Logout)
                    }
                    Command::Open(id) => open(state, &id, ui)
                        .map(|conversation_id| ClientMessage::OpenConversation { conversation_id }),
                    Command::LoadOlder => {
                        let msg = load_older(state);
                        // Mostra o "carregando..." no topo.
                        render(state, ui);
                        msg
                    }
                    Command::Send(text) => state.open.filter(|_| !text.trim().is_empty()).map(
                        |conversation_id| ClientMessage::SendMessage { conversation_id, text },
                    ),
                    Command::OpenDirect(username) => {
                        let username = username.trim().to_string();
                        (!username.is_empty()).then(|| {
                            state.opening_direct = true;
                            set_direct_error(ui, String::new());
                            ClientMessage::OpenDirect { username }
                        })
                    }
                    command => group_command(state, command, ui),
                };
                if let Some(msg) = outgoing
                    && send(&mut sink, &msg).await.is_err()
                {
                    return Ended::Lost;
                }
            }
            _ = ping.tick() => {
                if heard_at.elapsed() > SILENCE_LIMIT
                    || sink.send(Message::Ping(Default::default())).await.is_err()
                {
                    return Ended::Lost;
                }
            }
        }
    }
}

/// Espera `delay` sem conexão, ainda atendendo a interface. `false` quando a
/// janela fechou.
async fn wait_offline(
    delay: Duration,
    state: &mut LocalState,
    saved: &SavedSession,
    ui: &slint::Weak<AppWindow>,
    commands: &mut UnboundedReceiver<Command>,
) -> bool {
    let until = Instant::now() + delay;
    loop {
        tokio::select! {
            _ = sleep_until(until) => return true,
            command = commands.recv() => {
                let Some(command) = command else { return false };
                match command {
                    // Os botões ficam desligados sem conexão; isto é só garantia.
                    Command::Login { .. } | Command::Register { .. } => {
                        set_auth_error(ui, "sem conexão com o servidor".into());
                    }
                    // O servidor só fica sabendo quando a sessão expirar, mas
                    // o token some deste computador na hora.
                    Command::Logout => {
                        forget_session(state, saved, ui);
                        show_offline(state, ui, until - Instant::now());
                    }
                    // Abre já com o que temos; o histórico vem ao reconectar.
                    Command::Open(id) => {
                        open(state, &id, ui);
                    }
                    // Pede de novo quando voltar, ao chegar no topo outra vez.
                    Command::LoadOlder => {}
                    Command::Send(_) => {
                        set_status(ui, "sem conexão: a mensagem não foi enviada".into());
                    }
                    Command::OpenDirect(_) => {
                        set_direct_error(ui, "sem conexão com o servidor".into());
                    }
                    // Abrir e fechar a coluna funciona sem rede.
                    Command::ShowMembers(open) => {
                        state.members_open = open;
                        render(state, ui);
                    }
                    Command::CreateGroup(_)
                    | Command::Invite(_)
                    | Command::Remove(_)
                    | Command::Leave => {
                        set_group_error(ui, "sem conexão com o servidor".into());
                    }
                }
            }
        }
    }
}

/// Muda a conversa aberta. Devolve o id, se ele for válido.
fn open(state: &mut LocalState, id: &str, ui: &slint::Weak<AppWindow>) -> Option<ConversationId> {
    let conversation_id = parse_id(id)?;
    switch_to(state, Some(conversation_id));
    render(state, ui);
    Some(conversation_id)
}

/// Troca a conversa aberta e esquece o que era da anterior.
fn switch_to(state: &mut LocalState, conversation: Option<ConversationId>) {
    if state.open != conversation {
        state.members_open = false;
        state.members.clear();
        state.owner = None;
    }
    state.open = conversation;
    state.messages.clear();
    state.has_older = false;
    state.loading_older = false;
}

/// Um grupo criado por alguém: dá para convidar, remover e sair.
fn is_managed_group(state: &LocalState, id: ConversationId) -> bool {
    id != GROUP_ROOM
        && state
            .conversations
            .iter()
            .any(|c| c.id == id && c.kind == ConversationKind::Group)
}

/// Os comandos de grupo, com a conexão aberta. Devolve o pedido ao servidor.
fn group_command(
    state: &mut LocalState,
    command: Command,
    ui: &slint::Weak<AppWindow>,
) -> Option<ClientMessage> {
    let group = state.open.filter(|&id| is_managed_group(state, id));
    let msg =
        match command {
            Command::CreateGroup(name) => Some(ClientMessage::CreateGroup { name }),
            Command::ShowMembers(open) => {
                state.members_open = open && group.is_some();
                render(state, ui);
                // Ao abrir, pede a lista; depois ela chega sozinha a cada mudança.
                return group
                    .filter(|_| open)
                    .map(|conversation_id| ClientMessage::ListMembers { conversation_id });
            }
            Command::Invite(username) => {
                group
                    .filter(|_| !username.trim().is_empty())
                    .map(|conversation_id| ClientMessage::InviteToGroup {
                        conversation_id,
                        username,
                    })
            }
            Command::Remove(id) => group.zip(id.parse().ok()).map(|(conversation_id, id)| {
                ClientMessage::RemoveFromGroup {
                    conversation_id,
                    user_id: UserId(id),
                }
            }),
            Command::Leave => {
                group.map(|conversation_id| ClientMessage::LeaveGroup { conversation_id })
            }
            // Os outros comandos são tratados em `connection`.
            _ => None,
        };
    state.group_pending = msg.is_some();
    msg
}

/// Sai da conta neste computador e volta para a tela de login.
fn forget_session(state: &mut LocalState, saved: &SavedSession, ui: &slint::Weak<AppWindow>) {
    saved.delete();
    *state = LocalState::default();
    render(state, ui);
    set_direct_error(ui, String::new());
    show_login(ui);
}

/// Aplica uma mensagem do servidor ao estado local e redesenha. Pode devolver
/// algo para mandar de volta ao servidor.
fn apply(
    state: &mut LocalState,
    msg: ServerMessage,
    saved: &SavedSession,
    ui: &slint::Weak<AppWindow>,
) -> Option<ClientMessage> {
    let mut reply = None;
    match msg {
        ServerMessage::Welcome {
            user_id,
            username,
            token,
            conversations,
        } => {
            state.me = Some(user_id);
            state.group_pending = false;
            state.resuming = false;
            state.logged_in = true;
            saved.save(&token);
            state.token = Some(token);
            state.conversations = conversations;
            // Voltando de uma queda, pede de novo o histórico da conversa que
            // estava aberta: pode ter chegado mensagem enquanto estávamos fora.
            // As mensagens antigas ficam na tela até o `History` chegar.
            state.loading_older = false;
            reply = state
                .open
                .map(|conversation_id| ClientMessage::OpenConversation { conversation_id });
            let _ = ui.upgrade_in_event_loop(move |ui| {
                ui.set_user_name(username.into());
                ui.set_auth_error(SharedString::new());
                ui.set_status("online".into());
                ui.set_connecting(false);
                ui.set_logged_in(true);
            });
        }
        ServerMessage::AuthError { reason } => {
            if state.resuming {
                // O token salvo não vale mais: esquece ele e pede a senha.
                saved.delete();
                *state = LocalState::default();
            }
            set_auth_error(ui, reason);
            show_login(ui);
        }
        ServerMessage::History {
            conversation_id,
            messages,
            has_more,
        } => {
            if state.open == Some(conversation_id) {
                state.messages = messages;
                state.has_older = has_more;
                state.loading_older = false;
            }
        }
        ServerMessage::Older {
            conversation_id,
            messages,
            has_more,
        } => {
            if state.open == Some(conversation_id) {
                state.messages.splice(0..0, messages);
                state.has_older = has_more;
                state.loading_older = false;
                state.prepended = true;
            }
        }
        ServerMessage::NewMessage { message } => {
            // O servidor manda de volta a minha própria mensagem: é a
            // confirmação de que ela foi, e toca o som de enviada.
            let kind = if state.me == Some(message.author_id) {
                Sound::Sent
            } else {
                Sound::Received
            };
            sound::play_if_on(ui, kind);
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
            sort_conversations(state);
        }
        ServerMessage::DirectOpened { conversation } => {
            state.opening_direct = false;
            let id = conversation.id;
            add_conversation(state, conversation);
            set_direct_error(ui, String::new());
            let _ = ui.upgrade_in_event_loop(|ui| ui.set_direct_search(SharedString::new()));
            if state.open != Some(id) {
                switch_to(state, Some(id));
                reply = Some(ClientMessage::OpenConversation {
                    conversation_id: id,
                });
            }
        }
        ServerMessage::ConversationAdded { conversation } => {
            add_conversation(state, conversation);
        }
        ServerMessage::GroupCreated { conversation } => {
            state.group_pending = false;
            let id = conversation.id;
            add_conversation(state, conversation);
            // O grupo novo já abre, com a coluna de membros para convidar.
            switch_to(state, Some(id));
            state.members_open = true;
            reply = Some(ClientMessage::ListMembers {
                conversation_id: id,
            });
            let _ = ui.upgrade_in_event_loop(|ui| {
                ui.set_creating_group(false);
                ui.set_group_error(SharedString::new());
            });
        }
        ServerMessage::ConversationRemoved { conversation_id } => {
            let name = state
                .conversations
                .iter()
                .find(|c| c.id == conversation_id)
                .map(|c| c.name.clone())
                .unwrap_or_default();
            state.conversations.retain(|c| c.id != conversation_id);
            if state.open == Some(conversation_id) {
                switch_to(state, None);
            }
            if std::mem::take(&mut state.group_pending) {
                set_status(ui, format!("você saiu de {name}"));
            } else {
                set_status(ui, format!("você foi removido de {name}"));
            }
        }
        ServerMessage::Members {
            conversation_id,
            owner_id,
            members,
        } => {
            if state.open == Some(conversation_id) {
                state.owner = owner_id;
                state.members = members;
                state.group_pending = false;
                set_group_error(ui, String::new());
            }
        }
        ServerMessage::Error { reason } => {
            // Se o erro foi a resposta a um `LoadOlder`, deixa pedir de novo.
            state.loading_older = false;
            if std::mem::take(&mut state.opening_direct) {
                set_direct_error(ui, reason);
            } else if std::mem::take(&mut state.group_pending) {
                set_group_error(ui, reason);
            } else {
                set_status(ui, reason);
            }
        }
    }
    render(state, ui);
    reply
}

/// Põe a conversa na lista, ou atualiza a que já estava lá.
fn add_conversation(state: &mut LocalState, conversation: ConversationSummary) {
    match state
        .conversations
        .iter_mut()
        .find(|c| c.id == conversation.id)
    {
        Some(existing) => *existing = conversation,
        None => state.conversations.push(conversation),
    }
    sort_conversations(state);
}

/// A conversa com mensagem mais nova sobe para o topo.
fn sort_conversations(state: &mut LocalState) {
    state
        .conversations
        .sort_by_key(|c| std::cmp::Reverse(c.last_message.as_ref().map(|m| m.sent_at_ms)));
}

/// O pedido das mensagens anteriores à mais antiga que temos, se fizer sentido.
fn load_older(state: &mut LocalState) -> Option<ClientMessage> {
    if !state.has_older || state.loading_older {
        return None;
    }
    let conversation_id = state.open?;
    let before = state.messages.first()?.id;
    state.loading_older = true;
    Some(ClientMessage::LoadOlder {
        conversation_id,
        before,
    })
}

/// Converte o estado em linhas que o Slint sabe desenhar.
fn render(state: &mut LocalState, ui: &slint::Weak<AppWindow>) {
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
        .zip(continues_flags(&state.messages))
        .map(|(m, continues)| MessageRow {
            author: m.author_name.clone().into(),
            time: clock(m.sent_at_ms).into(),
            text: m.text.clone().into(),
            continues,
        })
        .collect();

    let title: SharedString = state
        .open
        .and_then(|id| state.conversations.iter().find(|c| c.id == id))
        .map(|c| c.name.clone())
        .unwrap_or_default()
        .into();
    let i_own = state.me.is_some() && state.owner == state.me;
    let members: Vec<MemberRow> = state
        .members
        .iter()
        .map(|m| MemberRow {
            id: m.user_id.0.to_string().into(),
            name: m.username.clone().into(),
            is_owner: Some(m.user_id) == state.owner,
            can_remove: i_own && Some(m.user_id) != state.me,
        })
        .collect();
    let open_is_group = state.open.is_some_and(|id| is_managed_group(state, id));
    let members_open = state.members_open && open_is_group;
    let has_older = state.has_older;
    let loading_older = state.loading_older;
    let prepended = std::mem::take(&mut state.prepended);

    // `VecModel` não pode cruzar threads, então montamos ele já dentro da
    // thread da interface.
    let _ = ui.upgrade_in_event_loop(move |ui| {
        ui.set_conversations(ModelRc::from(Rc::new(VecModel::from(conversations))));
        // Antes de trocar as mensagens, para a tela já saber como rolar.
        if prepended {
            ui.set_keep_from_bottom(true);
        }
        ui.set_messages(ModelRc::from(Rc::new(VecModel::from(messages))));
        ui.set_has_older(has_older);
        ui.set_loading_older(loading_older);
        ui.set_open_title(title);
        ui.set_members(ModelRc::from(Rc::new(VecModel::from(members))));
        ui.set_open_is_group(open_is_group);
        ui.set_members_open(members_open);
    });
}

/// Mensagens mais próximas que isto, da mesma pessoa, ficam no mesmo bloco.
const GROUP_GAP_MS: u64 = 5 * 60 * 1000;

/// Para cada mensagem, diz se ela continua o bloco da anterior (mesma pessoa,
/// menos de 5 minutos depois). A primeira de cada bloco mostra nome e horário.
fn continues_flags(messages: &[ChatMessage]) -> Vec<bool> {
    let mut flags = Vec::with_capacity(messages.len());
    let mut prev: Option<&ChatMessage> = None;
    for m in messages {
        flags.push(prev.is_some_and(|p| {
            p.author_id == m.author_id
                && m.sent_at_ms >= p.sent_at_ms
                && m.sent_at_ms - p.sent_at_ms < GROUP_GAP_MS
        }));
        prev = Some(m);
    }
    flags
}

fn set_group_error(ui: &slint::Weak<AppWindow>, reason: String) {
    let _ = ui.upgrade_in_event_loop(move |ui| ui.set_group_error(reason.into()));
}

fn set_status(ui: &slint::Weak<AppWindow>, status: String) {
    let _ = ui.upgrade_in_event_loop(move |ui| ui.set_status(status.into()));
}

fn set_direct_error(ui: &slint::Weak<AppWindow>, reason: String) {
    let _ = ui.upgrade_in_event_loop(move |ui| ui.set_direct_error(reason.into()));
}

fn set_auth_error(ui: &slint::Weak<AppWindow>, reason: String) {
    let _ = ui.upgrade_in_event_loop(move |ui| ui.set_auth_error(reason.into()));
}

/// Volta para a tela de login, já pronta para digitar.
fn show_login(ui: &slint::Weak<AppWindow>) {
    let _ = ui.upgrade_in_event_loop(|ui| {
        ui.set_logged_in(false);
        ui.set_connecting(false);
    });
}

/// Mostra que estamos sem servidor. Com a conversa aberta, só o rodapé muda;
/// na tela de login, os botões ficam desligados até voltar.
fn show_offline(state: &LocalState, ui: &slint::Weak<AppWindow>, retry_in: Duration) {
    if state.logged_in {
        set_status(ui, "reconectando...".into());
    } else {
        let secs = retry_in.as_secs_f32().ceil() as u64;
        set_auth_error(
            ui,
            format!("sem conexão com o servidor; tentando de novo em {secs}s"),
        );
        let _ = ui.upgrade_in_event_loop(|ui| ui.set_connecting(true));
    }
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

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type Sink = futures_util::stream::SplitSink<Socket, Message>;

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

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{MessageId, UserId};
    use uuid::Uuid;

    #[test]
    fn backoff_doubles_up_to_thirty_seconds() {
        let secs: Vec<u64> = (0..9).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16, 30, 30, 30, 30]);
        assert_eq!(backoff(u32::MAX), Duration::from_secs(30));
    }

    fn msg(author: &str, minute: u64, second: u64) -> ChatMessage {
        ChatMessage {
            id: MessageId(0),
            conversation_id: ConversationId(Uuid::nil()),
            author_id: UserId(Uuid::from_u128(if author == "jobiss" { 1 } else { 2 })),
            author_name: author.into(),
            text: String::new(),
            sent_at_ms: (minute * 60 + second) * 1000,
        }
    }

    #[test]
    fn groups_same_author_within_five_minutes() {
        let messages = [
            msg("jobiss", 0, 0),
            msg("jobiss", 1, 0),
            // 4min59s depois da anterior: ainda junto.
            msg("jobiss", 5, 59),
            // Exatamente 5 minutos: bloco novo.
            msg("jobiss", 10, 59),
            // Outra pessoa: bloco novo, mesmo logo depois.
            msg("nina", 11, 0),
            msg("nina", 11, 30),
            msg("jobiss", 12, 0),
        ];
        assert_eq!(
            continues_flags(&messages),
            [false, true, true, false, false, true, false]
        );
    }

    #[test]
    fn empty_list_has_no_flags() {
        assert!(continues_flags(&[]).is_empty());
    }
}

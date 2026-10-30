//! Conexão com o servidor e o estado local da conversa.
//!
//! Esta tarefa é a única dona do estado (lista de conversas, conversa aberta,
//! mensagens). A interface só recebe cópias prontas para desenhar.

use std::rc::Rc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use protocol::{
    ChatMessage, ClientMessage, ConversationId, ConversationKind, ConversationSummary,
    ServerMessage,
};
use slint::{ModelRc, SharedString, VecModel};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::{Instant, interval, sleep_until};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::session::SavedSession;
use crate::{AppWindow, Command, ConversationRow, MessageRow};

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
                }
            }
        }
    }
}

/// Muda a conversa aberta. Devolve o id, se ele for válido.
fn open(state: &mut LocalState, id: &str, ui: &slint::Weak<AppWindow>) -> Option<ConversationId> {
    let conversation_id = parse_id(id)?;
    state.open = Some(conversation_id);
    state.messages.clear();
    state.has_older = false;
    state.loading_older = false;
    render(state, ui);
    Some(conversation_id)
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
            username,
            token,
            conversations,
            ..
        } => {
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
                state.open = Some(id);
                state.messages.clear();
                state.has_older = false;
                state.loading_older = false;
                reply = Some(ClientMessage::OpenConversation {
                    conversation_id: id,
                });
            }
        }
        ServerMessage::ConversationAdded { conversation } => {
            add_conversation(state, conversation);
        }
        ServerMessage::Error { reason } => {
            // Se o erro foi a resposta a um `LoadOlder`, deixa pedir de novo.
            state.loading_older = false;
            if std::mem::take(&mut state.opening_direct) {
                set_direct_error(ui, reason);
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
    });
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

    #[test]
    fn backoff_doubles_up_to_thirty_seconds() {
        let secs: Vec<u64> = (0..9).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16, 30, 30, 30, 30]);
        assert_eq!(backoff(u32::MAX), Duration::from_secs(30));
    }
}

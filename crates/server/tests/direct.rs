//! Conversas diretas (F1-09), de ponta a ponta pelo WebSocket.

use futures_util::{SinkExt, StreamExt};
use protocol::{
    ChatMessage, ClientMessage, ConversationId, ConversationKind, ConversationSummary, MessageId,
    Sealed, ServerMessage,
};
use server::{AppState, app, state::GROUP_ROOM};
use sqlx::PgPool;
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const PASSWORD: &str = "senha-boa-123";

async fn start_server(pool: PgPool) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app(AppState::new(pool)))
            .await
            .unwrap();
    });
    format!("ws://{addr}/ws")
}

async fn send(socket: &mut Socket, msg: &ClientMessage) {
    let json = serde_json::to_string(msg).unwrap();
    socket.send(Message::Text(json.into())).await.unwrap();
}

fn is_mls(msg: &ServerMessage) -> bool {
    matches!(
        msg,
        ServerMessage::KeyPackageStock { .. }
            | ServerMessage::GroupState { .. }
            | ServerMessage::GroupWelcome { .. }
            | ServerMessage::Commit { .. }
    )
}

/// O servidor guarda e devolve os bytes como vieram; aqui eles são o texto.
fn plain(message: &ChatMessage) -> String {
    String::from_utf8(message.sealed.clone().unwrap_or_default().0).unwrap()
}

async fn recv(socket: &mut Socket) -> ServerMessage {
    loop {
        match socket.next().await.unwrap().unwrap() {
            Message::Text(text) => {
                let msg: ServerMessage = serde_json::from_str(&text).unwrap();
                // A troca de chaves (F2-08) tem os testes dela em `mls.rs`.
                if !is_mls(&msg) {
                    return msg;
                }
            }
            _ => continue,
        }
    }
}

async fn ask(socket: &mut Socket, msg: &ClientMessage) -> ServerMessage {
    send(socket, msg).await;
    recv(socket).await
}

/// Cria a conta (ou entra nela) e devolve o `Welcome`.
async fn enter(url: &str, first: ClientMessage) -> (Socket, Vec<ConversationSummary>) {
    let (mut socket, _) = connect_async(url).await.unwrap();
    let ServerMessage::Welcome { conversations, .. } = ask(&mut socket, &first).await else {
        panic!("esperava Welcome");
    };
    (socket, conversations)
}

async fn join(url: &str, username: &str) -> Socket {
    let register = ClientMessage::Register {
        username: username.into(),
        password: PASSWORD.into(),
    };
    enter(url, register).await.0
}

async fn open_direct(socket: &mut Socket, username: &str) -> ServerMessage {
    let msg = ClientMessage::OpenDirect {
        username: username.into(),
    };
    ask(socket, &msg).await
}

async fn opened(socket: &mut Socket, username: &str) -> ConversationSummary {
    match open_direct(socket, username).await {
        ServerMessage::DirectOpened { conversation } => conversation,
        other => panic!("esperava DirectOpened, veio {other:?}"),
    }
}

async fn say(socket: &mut Socket, conversation_id: ConversationId, text: &str) {
    let msg = ClientMessage::SendMessage {
        conversation_id,
        sealed: Sealed(text.as_bytes().to_vec()),
    };
    send(socket, &msg).await;
}

fn is_error(msg: &ServerMessage) -> bool {
    matches!(msg, ServerMessage::Error { .. })
}

#[sqlx::test]
async fn jobiss_and_nina_talk_and_a_third_person_cannot_read(pool: PgPool) {
    let url = start_server(pool).await;
    let mut jobiss = join(&url, "jobiss").await;
    let mut nina = join(&url, "nina").await;
    let mut lia = join(&url, "lia").await;

    // Maiúsculas não importam; o nome na lista é o da outra pessoa.
    let direct = opened(&mut jobiss, "Nina").await;
    assert_eq!(direct.name, "nina");
    assert_eq!(direct.kind, ConversationKind::Direct);
    assert_eq!(direct.last_message, None);

    // A conversa aparece ao vivo na lista da Nina, com o nome do Jobiss.
    let ServerMessage::ConversationAdded { conversation } = recv(&mut nina).await else {
        panic!("Nina deveria receber a conversa nova");
    };
    assert_eq!(conversation.id, direct.id);
    assert_eq!(conversation.name, "jobiss");

    say(&mut jobiss, direct.id, "só entre nós").await;
    for socket in [&mut jobiss, &mut nina] {
        let ServerMessage::NewMessage { message } = recv(socket).await else {
            panic!("os dois deveriam receber a mensagem");
        };
        assert_eq!(message.conversation_id, direct.id);
        assert_eq!(plain(&message), "só entre nós");
    }

    // A Lia não recebeu nada: a próxima coisa que chega para ela é a própria
    // mensagem na sala do grupo.
    say(&mut lia, GROUP_ROOM, "oi gente").await;
    let ServerMessage::NewMessage { message } = recv(&mut lia).await else {
        panic!("esperava a própria mensagem");
    };
    assert_eq!(plain(&message), "oi gente");

    // E não consegue abrir, ler as antigas nem escrever na conversa dos dois.
    let open = ClientMessage::OpenConversation {
        conversation_id: direct.id,
    };
    assert!(is_error(&ask(&mut lia, &open).await));
    let older = ClientMessage::LoadOlder {
        conversation_id: direct.id,
        before: MessageId(i64::MAX),
    };
    assert!(is_error(&ask(&mut lia, &older).await));
    let intrude = ClientMessage::SendMessage {
        conversation_id: direct.id,
        sealed: Sealed(b"posso entrar?".to_vec()),
    };
    assert!(is_error(&ask(&mut lia, &intrude).await));

    // Nem aparece na lista dela.
    let login = ClientMessage::Login {
        username: "lia".into(),
        password: PASSWORD.into(),
    };
    let (_, conversations) = enter(&url, login).await;
    assert_eq!(conversations.len(), 1);
    assert_eq!(conversations[0].id, GROUP_ROOM);

    // Já a Nina, entrando de novo, vê a conversa com o nome do Jobiss.
    let login = ClientMessage::Login {
        username: "nina".into(),
        password: PASSWORD.into(),
    };
    let (_, conversations) = enter(&url, login).await;
    let direct_row = conversations.iter().find(|c| c.id == direct.id).unwrap();
    assert_eq!(direct_row.name, "jobiss");
    assert_eq!(
        plain(direct_row.last_message.as_ref().unwrap()),
        "só entre nós"
    );
}

#[sqlx::test]
async fn there_is_only_one_direct_conversation_per_pair(pool: PgPool) {
    let url = start_server(pool.clone()).await;
    let mut jobiss = join(&url, "jobiss").await;
    let mut nina = join(&url, "nina").await;

    let first = opened(&mut jobiss, "nina").await;
    let ServerMessage::ConversationAdded { .. } = recv(&mut nina).await else {
        panic!("Nina deveria receber a conversa nova");
    };
    let again = opened(&mut jobiss, "nina").await;
    let from_nina = opened(&mut nina, "jobiss").await;
    assert_eq!(again.id, first.id);
    assert_eq!(from_nina.id, first.id);
    assert_eq!(from_nina.name, "jobiss");

    let (count,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM conversations WHERE kind = 'direct'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
}

#[sqlx::test]
async fn opening_at_the_same_time_still_makes_one_conversation(pool: PgPool) {
    let state = AppState::new(pool.clone());
    let jobiss = state.register("jobiss", PASSWORD).await.unwrap();
    let nina = state.register("nina", PASSWORD).await.unwrap();

    let (a, b) = tokio::join!(
        state.open_direct(jobiss.user_id, "nina"),
        state.open_direct(nina.user_id, "jobiss"),
    );
    assert_eq!(a.unwrap().id, b.unwrap().id);

    let (count,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM conversations WHERE kind = 'direct'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
}

#[sqlx::test]
async fn unknown_user_and_yourself_are_refused(pool: PgPool) {
    let url = start_server(pool).await;
    let mut jobiss = join(&url, "jobiss").await;

    assert!(is_error(&open_direct(&mut jobiss, "ninguem").await));
    assert!(is_error(&open_direct(&mut jobiss, "não vale").await));
    assert!(is_error(&open_direct(&mut jobiss, "jobiss").await));
}

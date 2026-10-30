//! Testes de ponta a ponta: sobem o servidor e conectam clientes de verdade.
//!
//! `#[sqlx::test]` cria um banco novo e vazio para cada teste, aplica as
//! migrações e apaga tudo no final. Precisa do PostgreSQL ligado
//! (`docker compose up -d`) e de `DATABASE_URL` (vem de `.cargo/config.toml`).

use futures_util::{SinkExt, StreamExt};
use protocol::{ClientMessage, ConversationId, ServerMessage};
use server::{AppState, app, state::GROUP_ROOM};
use sqlx::PgPool;
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

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

async fn recv(socket: &mut Socket) -> ServerMessage {
    loop {
        match socket.next().await.unwrap().unwrap() {
            Message::Text(text) => return serde_json::from_str(&text).unwrap(),
            _ => continue,
        }
    }
}

async fn join(url: &str, name: &str) -> (Socket, ServerMessage) {
    let (mut socket, _) = connect_async(url).await.unwrap();
    send(&mut socket, &ClientMessage::Hello { name: name.into() }).await;
    let welcome = recv(&mut socket).await;
    (socket, welcome)
}

async fn say(socket: &mut Socket, conversation_id: ConversationId, text: &str) {
    send(
        socket,
        &ClientMessage::SendMessage {
            conversation_id,
            text: text.into(),
        },
    )
    .await;
}

async fn history(socket: &mut Socket, conversation_id: ConversationId) -> Vec<String> {
    send(socket, &ClientMessage::OpenConversation { conversation_id }).await;
    let ServerMessage::History { messages, .. } = recv(socket).await else {
        panic!("esperava History");
    };
    messages.into_iter().map(|m| m.text).collect()
}

#[sqlx::test]
async fn message_from_one_client_reaches_the_other(pool: PgPool) {
    let url = start_server(pool).await;
    let (mut lia, welcome) = join(&url, "Lia").await;
    let (mut jobiss, _) = join(&url, "Jobiss").await;

    let ServerMessage::Welcome { conversations, .. } = welcome else {
        panic!("esperava Welcome, veio {welcome:?}");
    };
    assert_eq!(conversations.len(), 1);
    assert_eq!(conversations[0].id, GROUP_ROOM);
    assert_eq!(conversations[0].name, "Sala do Grupo");

    say(&mut jobiss, GROUP_ROOM, "  bora abrir uma caixa depois  ").await;

    let ServerMessage::NewMessage { message } = recv(&mut lia).await else {
        panic!("Lia deveria receber a mensagem");
    };
    assert_eq!(message.author_name, "Jobiss");
    assert_eq!(message.text, "bora abrir uma caixa depois");

    // Quem entra depois vê a mensagem no histórico.
    let (mut nina, _) = join(&url, "Nina").await;
    assert_eq!(
        history(&mut nina, GROUP_ROOM).await,
        ["bora abrir uma caixa depois"]
    );
}

#[sqlx::test]
async fn messages_survive_a_server_restart(pool: PgPool) {
    // Dois servidores com o mesmo banco fazem o papel de "antes" e "depois".
    let url = start_server(pool.clone()).await;
    let (mut lia, _) = join(&url, "Lia").await;
    say(&mut lia, GROUP_ROOM, "isso fica guardado").await;
    let ServerMessage::NewMessage { .. } = recv(&mut lia).await else {
        panic!("esperava a própria mensagem de volta");
    };

    let url = start_server(pool).await;
    let (mut lia, welcome) = join(&url, "lia").await;
    let ServerMessage::Welcome { conversations, .. } = welcome else {
        panic!()
    };
    let last = conversations[0].last_message.as_ref().unwrap();
    assert_eq!(last.text, "isso fica guardado");
    // "lia" e "Lia" são a mesma conta.
    assert_eq!(last.author_name, "Lia");
    assert_eq!(history(&mut lia, GROUP_ROOM).await, ["isso fica guardado"]);
}

#[sqlx::test]
async fn history_keeps_only_the_latest_messages(pool: PgPool) {
    let url = start_server(pool.clone()).await;
    let (mut lia, _) = join(&url, "Lia").await;

    // Mais rápido que mandar 105 mensagens pelo WebSocket.
    sqlx::query(
        "INSERT INTO messages (conversation_id, author_id, text)
         SELECT $1, (SELECT id FROM users WHERE username = 'Lia'), 'msg ' || n
         FROM generate_series(1, 105) AS n",
    )
    .bind(GROUP_ROOM.0)
    .execute(&pool)
    .await
    .unwrap();

    let texts = history(&mut lia, GROUP_ROOM).await;
    assert_eq!(texts.len(), server::state::HISTORY_LIMIT as usize);
    assert_eq!(texts.first().unwrap(), "msg 6");
    assert_eq!(texts.last().unwrap(), "msg 105");
}

#[sqlx::test]
async fn only_members_receive_a_message(pool: PgPool) {
    let url = start_server(pool.clone()).await;
    let (mut lia, _) = join(&url, "Lia").await;
    let (mut jobiss, _) = join(&url, "Jobiss").await;

    // Uma conversa só com a Lia.
    let (private,): (uuid::Uuid,) = sqlx::query_as(
        "WITH c AS (INSERT INTO conversations (name, kind) VALUES ('Só a Lia', 'group') RETURNING id)
         INSERT INTO conversation_members (conversation_id, user_id)
         SELECT c.id, u.id FROM c, users u WHERE u.username = 'Lia'
         RETURNING conversation_id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let private = ConversationId(private);

    say(&mut lia, private, "segredo").await;
    say(&mut lia, GROUP_ROOM, "oi grupo").await;

    // O Jobiss recebe só a do grupo.
    let ServerMessage::NewMessage { message } = recv(&mut jobiss).await else {
        panic!("esperava NewMessage");
    };
    assert_eq!(message.text, "oi grupo");

    // E nem consegue abrir nem escrever na conversa da Lia.
    send(
        &mut jobiss,
        &ClientMessage::OpenConversation {
            conversation_id: private,
        },
    )
    .await;
    assert!(matches!(
        recv(&mut jobiss).await,
        ServerMessage::Error { .. }
    ));
    say(&mut jobiss, private, "entrei?").await;
    assert!(matches!(
        recv(&mut jobiss).await,
        ServerMessage::Error { .. }
    ));
}

#[sqlx::test]
async fn empty_message_is_rejected(pool: PgPool) {
    let url = start_server(pool).await;
    let (mut socket, _) = join(&url, "Lia").await;

    say(&mut socket, GROUP_ROOM, "   ").await;
    assert!(matches!(
        recv(&mut socket).await,
        ServerMessage::Error { .. }
    ));
}

#[sqlx::test]
async fn connection_without_hello_gets_an_error(pool: PgPool) {
    let url = start_server(pool).await;
    let (mut socket, _) = connect_async(&url).await.unwrap();
    send(
        &mut socket,
        &ClientMessage::OpenConversation {
            conversation_id: GROUP_ROOM,
        },
    )
    .await;
    assert!(matches!(
        recv(&mut socket).await,
        ServerMessage::Error { .. }
    ));
}

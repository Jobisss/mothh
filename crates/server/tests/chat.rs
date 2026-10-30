//! Teste de ponta a ponta: sobe o servidor e conecta dois clientes de verdade.
//!
//! Cada `#[sqlx::test]` ganha um banco novo e vazio, já com as migrations.
//! Precisa de um Postgres rodando e de `DATABASE_URL` apontando para ele.

use futures_util::{SinkExt, StreamExt};
use protocol::{ClientMessage, ServerMessage};
use server::{AppState, app};
use sqlx::PgPool;
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn start_server(db: PgPool) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app(AppState::from_pool(db)))
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

#[sqlx::test(migrations = "./migrations")]
async fn message_from_one_client_reaches_the_other(db: PgPool) {
    let url = start_server(db).await;
    let (mut lia, welcome) = join(&url, "Lia").await;
    let (mut jobiss, _) = join(&url, "Jobiss").await;

    let ServerMessage::Welcome { conversations, .. } = welcome else {
        panic!("esperava Welcome, veio {welcome:?}");
    };
    let group = conversations[0].id;

    send(
        &mut jobiss,
        &ClientMessage::SendMessage {
            conversation_id: group,
            text: "  bora abrir uma caixa depois  ".into(),
        },
    )
    .await;

    let ServerMessage::NewMessage { message } = recv(&mut lia).await else {
        panic!("Lia deveria receber a mensagem");
    };
    assert_eq!(message.author_name, "Jobiss");
    assert_eq!(message.text, "bora abrir uma caixa depois");

    // Quem entra depois vê a mensagem no histórico.
    let (mut nina, _) = join(&url, "Nina").await;
    send(
        &mut nina,
        &ClientMessage::OpenConversation {
            conversation_id: group,
        },
    )
    .await;
    let ServerMessage::History { messages, .. } = recv(&mut nina).await else {
        panic!("esperava History");
    };
    assert_eq!(messages.len(), 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn empty_message_is_rejected(db: PgPool) {
    let url = start_server(db).await;
    let (mut socket, welcome) = join(&url, "Lia").await;
    let ServerMessage::Welcome { conversations, .. } = welcome else {
        panic!()
    };

    send(
        &mut socket,
        &ClientMessage::SendMessage {
            conversation_id: conversations[0].id,
            text: "   ".into(),
        },
    )
    .await;
    assert!(matches!(
        recv(&mut socket).await,
        ServerMessage::Error { .. }
    ));
}

#[sqlx::test(migrations = "./migrations")]
async fn connection_without_hello_gets_an_error(db: PgPool) {
    let url = start_server(db).await;
    let (mut socket, _) = connect_async(&url).await.unwrap();
    send(
        &mut socket,
        &ClientMessage::OpenConversation {
            conversation_id: protocol::ConversationId(uuid_v4()),
        },
    )
    .await;
    assert!(matches!(
        recv(&mut socket).await,
        ServerMessage::Error { .. }
    ));
}

#[sqlx::test(migrations = "./migrations")]
async fn history_survives_a_server_restart(db: PgPool) {
    let url = start_server(db.clone()).await;
    let (mut jobiss, welcome) = join(&url, "Jobiss").await;
    let ServerMessage::Welcome { conversations, .. } = welcome else {
        panic!()
    };
    let group = conversations[0].id;
    send(
        &mut jobiss,
        &ClientMessage::SendMessage {
            conversation_id: group,
            text: "isso fica guardado".into(),
        },
    )
    .await;
    let ServerMessage::NewMessage { .. } = recv(&mut jobiss).await else {
        panic!("esperava a própria mensagem de volta");
    };

    // Um servidor novo, com o mesmo banco: a mensagem continua lá.
    let url = start_server(db).await;
    let (_, welcome) = join(&url, "Jobiss").await;
    let ServerMessage::Welcome { conversations, .. } = welcome else {
        panic!()
    };
    let last = conversations[0].last_message.as_ref().unwrap();
    assert_eq!(last.text, "isso fica guardado");
}

#[sqlx::test(migrations = "./migrations")]
async fn only_members_receive_the_message(db: PgPool) {
    let url = start_server(db.clone()).await;
    let (mut lia, welcome) = join(&url, "Lia").await;
    let ServerMessage::Welcome {
        user_id: lia_id, ..
    } = welcome
    else {
        panic!()
    };
    let (mut jobiss, _) = join(&url, "Jobiss").await;

    // Uma conversa só da Lia, criada direto no banco.
    let secret = uuid_v4();
    sqlx::query("INSERT INTO conversations (id, name, kind) VALUES ($1, 'Diário', 'group')")
        .bind(secret)
        .execute(&db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO conversation_members (conversation_id, user_id) VALUES ($1, $2)")
        .bind(secret)
        .bind(lia_id.0)
        .execute(&db)
        .await
        .unwrap();

    // Jobiss não participa: não pode escrever nem ler.
    let secret = protocol::ConversationId(secret);
    send(
        &mut jobiss,
        &ClientMessage::SendMessage {
            conversation_id: secret,
            text: "posso entrar?".into(),
        },
    )
    .await;
    assert!(matches!(
        recv(&mut jobiss).await,
        ServerMessage::Error { .. }
    ));

    // A Lia escreve; ela recebe, e a próxima coisa que o Jobiss recebe é a
    // resposta ao pedido de histórico, não a mensagem dela.
    send(
        &mut lia,
        &ClientMessage::SendMessage {
            conversation_id: secret,
            text: "só pra mim".into(),
        },
    )
    .await;
    let ServerMessage::NewMessage { message } = recv(&mut lia).await else {
        panic!("Lia deveria receber a própria mensagem");
    };
    assert_eq!(message.text, "só pra mim");
    send(
        &mut jobiss,
        &ClientMessage::OpenConversation {
            conversation_id: secret,
        },
    )
    .await;
    assert!(matches!(
        recv(&mut jobiss).await,
        ServerMessage::Error { .. }
    ));
}

fn uuid_v4() -> uuid::Uuid {
    uuid::Uuid::new_v4()
}

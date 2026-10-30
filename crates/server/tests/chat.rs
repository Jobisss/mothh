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

const PASSWORD: &str = "senha-de-teste";

/// Conecta e entra na conta, criando ela se ainda não existir.
async fn join(url: &str, name: &str) -> (Socket, ServerMessage) {
    let (mut socket, _) = connect_async(url).await.unwrap();
    let register = ClientMessage::Register {
        username: name.into(),
        password: PASSWORD.into(),
    };
    send(&mut socket, &register).await;
    let mut welcome = recv(&mut socket).await;
    if let ServerMessage::AuthError { .. } = welcome {
        let login = ClientMessage::Login {
            username: name.into(),
            password: PASSWORD.into(),
        };
        send(&mut socket, &login).await;
        welcome = recv(&mut socket).await;
    }
    assert!(
        matches!(welcome, ServerMessage::Welcome { .. }),
        "esperava Welcome, veio {welcome:?}"
    );
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
    // O nome de usuário é guardado em minúsculas.
    assert_eq!(message.author_name, "jobiss");
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
async fn chatting_without_an_account_is_refused(db: PgPool) {
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
        ServerMessage::AuthError { .. }
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

#[sqlx::test(migrations = "./migrations")]
async fn username_is_unique_ignoring_case(db: PgPool) {
    let url = start_server(db).await;
    join(&url, "Nina").await;
    let (mut socket, _) = connect_async(&url).await.unwrap();
    let register = ClientMessage::Register {
        username: "NINA".into(),
        password: "outra-senha-qualquer".into(),
    };
    send(&mut socket, &register).await;
    assert!(matches!(
        recv(&mut socket).await,
        ServerMessage::AuthError { .. }
    ));
}

#[sqlx::test(migrations = "./migrations")]
async fn wrong_password_is_refused(db: PgPool) {
    let url = start_server(db).await;
    join(&url, "nina").await;
    let (mut socket, _) = connect_async(&url).await.unwrap();
    let login = ClientMessage::Login {
        username: "nina".into(),
        password: "nao-e-essa".into(),
    };
    send(&mut socket, &login).await;
    assert!(matches!(
        recv(&mut socket).await,
        ServerMessage::AuthError { .. }
    ));
}

#[sqlx::test(migrations = "./migrations")]
async fn token_resumes_until_logout(db: PgPool) {
    let url = start_server(db.clone()).await;
    let (mut first, welcome) = join(&url, "nina").await;
    let ServerMessage::Welcome { token, user_id, .. } = welcome else {
        panic!()
    };

    // O banco guarda só o hash do token, nunca o token.
    let stored: Vec<u8> = sqlx::query_scalar("SELECT token_hash FROM sessions")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_ne!(stored, token.as_bytes());

    // Outra conexão entra com o token, sem senha.
    let (mut second, _) = connect_async(&url).await.unwrap();
    send(
        &mut second,
        &ClientMessage::Resume {
            token: token.clone(),
        },
    )
    .await;
    let ServerMessage::Welcome { user_id: again, .. } = recv(&mut second).await else {
        panic!("o token deveria valer");
    };
    assert_eq!(again, user_id);

    // Depois do logout, a conexão volta para a fase sem conta...
    send(&mut first, &ClientMessage::Logout).await;
    send(
        &mut first,
        &ClientMessage::OpenConversation {
            conversation_id: protocol::ConversationId(uuid_v4()),
        },
    )
    .await;
    assert!(matches!(
        recv(&mut first).await,
        ServerMessage::AuthError { .. }
    ));

    // ...e o token deixa de valer.
    let (mut third, _) = connect_async(&url).await.unwrap();
    send(&mut third, &ClientMessage::Resume { token }).await;
    assert!(matches!(
        recv(&mut third).await,
        ServerMessage::AuthError { .. }
    ));
}

fn uuid_v4() -> uuid::Uuid {
    uuid::Uuid::new_v4()
}

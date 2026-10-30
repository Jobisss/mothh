//! Testes de ponta a ponta: sobem o servidor e conectam clientes de verdade.
//!
//! `#[sqlx::test]` cria um banco novo e vazio para cada teste, aplica as
//! migrações e apaga tudo no final. Precisa do PostgreSQL ligado
//! (`docker compose up -d`) e de `DATABASE_URL` (vem de `.cargo/config.toml`).

use futures_util::{SinkExt, StreamExt};
use protocol::{ClientMessage, ConversationId, MessageId, OLDER_PAGE, ServerMessage};
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

async fn recv(socket: &mut Socket) -> ServerMessage {
    loop {
        match socket.next().await.unwrap().unwrap() {
            Message::Text(text) => return serde_json::from_str(&text).unwrap(),
            _ => continue,
        }
    }
}

/// Abre uma conexão, manda a primeira mensagem e devolve a resposta.
async fn connect(url: &str, first: ClientMessage) -> (Socket, ServerMessage) {
    let (mut socket, _) = connect_async(url).await.unwrap();
    send(&mut socket, &first).await;
    let reply = recv(&mut socket).await;
    (socket, reply)
}

fn register(username: &str) -> ClientMessage {
    ClientMessage::Register {
        username: username.into(),
        password: PASSWORD.into(),
    }
}

fn login(username: &str, password: &str) -> ClientMessage {
    ClientMessage::Login {
        username: username.into(),
        password: password.into(),
    }
}

/// Cria a conta e confere que veio um `Welcome`.
async fn join(url: &str, username: &str) -> (Socket, ServerMessage) {
    let (socket, welcome) = connect(url, register(username)).await;
    assert!(
        matches!(welcome, ServerMessage::Welcome { .. }),
        "esperava Welcome, veio {welcome:?}"
    );
    (socket, welcome)
}

fn token_of(welcome: &ServerMessage) -> String {
    let ServerMessage::Welcome { token, .. } = welcome else {
        panic!("esperava Welcome, veio {welcome:?}");
    };
    token.clone()
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
    let (mut lia, welcome) = join(&url, "lia").await;
    let (mut jobiss, _) = join(&url, "jobiss").await;

    let ServerMessage::Welcome { conversations, .. } = welcome else {
        unreachable!()
    };
    assert_eq!(conversations.len(), 1);
    assert_eq!(conversations[0].id, GROUP_ROOM);
    assert_eq!(conversations[0].name, "Sala do Grupo");

    say(&mut jobiss, GROUP_ROOM, "  bora abrir uma caixa depois  ").await;

    let ServerMessage::NewMessage { message } = recv(&mut lia).await else {
        panic!("Lia deveria receber a mensagem");
    };
    assert_eq!(message.author_name, "jobiss");
    assert_eq!(message.text, "bora abrir uma caixa depois");

    // Quem entra depois vê a mensagem no histórico.
    let (mut nina, _) = join(&url, "nina").await;
    assert_eq!(
        history(&mut nina, GROUP_ROOM).await,
        ["bora abrir uma caixa depois"]
    );
}

#[sqlx::test]
async fn messages_survive_a_server_restart(pool: PgPool) {
    // Dois servidores com o mesmo banco fazem o papel de "antes" e "depois".
    let url = start_server(pool.clone()).await;
    let (mut lia, _) = join(&url, "lia").await;
    say(&mut lia, GROUP_ROOM, "isso fica guardado").await;
    let ServerMessage::NewMessage { .. } = recv(&mut lia).await else {
        panic!("esperava a própria mensagem de volta");
    };

    let url = start_server(pool).await;
    let (mut lia, welcome) = connect(&url, login("lia", PASSWORD)).await;
    let ServerMessage::Welcome { conversations, .. } = welcome else {
        panic!("esperava Welcome, veio {welcome:?}");
    };
    let last = conversations[0].last_message.as_ref().unwrap();
    assert_eq!(last.text, "isso fica guardado");
    assert_eq!(history(&mut lia, GROUP_ROOM).await, ["isso fica guardado"]);
}

#[sqlx::test]
async fn history_keeps_only_the_latest_messages(pool: PgPool) {
    let url = start_server(pool.clone()).await;
    let (mut lia, _) = join(&url, "lia").await;

    // Mais rápido que mandar 105 mensagens pelo WebSocket.
    sqlx::query(
        "INSERT INTO messages (conversation_id, author_id, text)
         SELECT $1, (SELECT id FROM users WHERE username = 'lia'), 'msg ' || n
         FROM generate_series(1, 105) AS n",
    )
    .bind(GROUP_ROOM.0)
    .execute(&pool)
    .await
    .unwrap();

    let texts = history(&mut lia, GROUP_ROOM).await;
    assert_eq!(texts.len(), server::state::HISTORY_LIMIT);
    assert_eq!(texts.first().unwrap(), "msg 6");
    assert_eq!(texts.last().unwrap(), "msg 105");
}

/// Mensagens "msg 1" a "msg n" da Lia na Sala do Grupo, direto no banco
/// (mais rápido que mandar uma por uma pelo WebSocket).
async fn seed_messages(pool: &PgPool, n: i32) {
    sqlx::query(
        "INSERT INTO messages (conversation_id, author_id, text)
         SELECT $1, (SELECT id FROM users WHERE username = 'lia'), 'msg ' || n
         FROM generate_series(1, $2) AS n",
    )
    .bind(GROUP_ROOM.0)
    .bind(n)
    .execute(pool)
    .await
    .unwrap();
}

async fn older(
    socket: &mut Socket,
    conversation_id: ConversationId,
    before: MessageId,
) -> ServerMessage {
    send(
        socket,
        &ClientMessage::LoadOlder {
            conversation_id,
            before,
        },
    )
    .await;
    recv(socket).await
}

#[sqlx::test]
async fn scrolling_up_reads_the_whole_conversation(pool: PgPool) {
    let url = start_server(pool.clone()).await;
    let (mut lia, _) = join(&url, "lia").await;
    seed_messages(&pool, 230).await;

    send(
        &mut lia,
        &ClientMessage::OpenConversation {
            conversation_id: GROUP_ROOM,
        },
    )
    .await;
    let ServerMessage::History {
        messages, has_more, ..
    } = recv(&mut lia).await
    else {
        panic!("esperava History");
    };
    assert!(has_more);
    let mut all = messages;

    // Pede as anteriores à mais antiga que tem, até o servidor dizer que acabou.
    loop {
        let before = all.first().unwrap().id;
        let ServerMessage::Older {
            conversation_id,
            messages,
            has_more,
        } = older(&mut lia, GROUP_ROOM, before).await
        else {
            panic!("esperava Older");
        };
        assert_eq!(conversation_id, GROUP_ROOM);
        assert!(messages.len() <= OLDER_PAGE);
        assert!(messages.iter().all(|m| m.id < before));
        all.splice(0..0, messages);
        if !has_more {
            break;
        }
    }

    let texts: Vec<String> = all.into_iter().map(|m| m.text).collect();
    let expected: Vec<String> = (1..=230).map(|n| format!("msg {n}")).collect();
    assert_eq!(texts, expected);
}

#[sqlx::test]
async fn older_pages_have_fifty_messages(pool: PgPool) {
    let url = start_server(pool.clone()).await;
    let (mut lia, _) = join(&url, "lia").await;
    seed_messages(&pool, 120).await;

    let (msg_71,): (i64,) = sqlx::query_as("SELECT id FROM messages WHERE text = 'msg 71'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let ServerMessage::Older {
        messages, has_more, ..
    } = older(&mut lia, GROUP_ROOM, MessageId(msg_71)).await
    else {
        panic!("esperava Older");
    };
    assert_eq!(messages.len(), OLDER_PAGE);
    assert_eq!(messages.first().unwrap().text, "msg 21");
    assert_eq!(messages.last().unwrap().text, "msg 70");
    assert!(has_more);

    // Exatamente 50 antes: vem tudo e avisa que não tem mais.
    let (msg_51,): (i64,) = sqlx::query_as("SELECT id FROM messages WHERE text = 'msg 51'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let ServerMessage::Older {
        messages, has_more, ..
    } = older(&mut lia, GROUP_ROOM, MessageId(msg_51)).await
    else {
        panic!("esperava Older");
    };
    assert_eq!(messages.len(), 50);
    assert!(!has_more);
}

#[sqlx::test]
async fn short_conversation_has_nothing_older(pool: PgPool) {
    let url = start_server(pool.clone()).await;
    let (mut lia, _) = join(&url, "lia").await;
    seed_messages(&pool, 3).await;

    send(
        &mut lia,
        &ClientMessage::OpenConversation {
            conversation_id: GROUP_ROOM,
        },
    )
    .await;
    let ServerMessage::History {
        messages, has_more, ..
    } = recv(&mut lia).await
    else {
        panic!("esperava History");
    };
    assert_eq!(messages.len(), 3);
    assert!(!has_more);
}

#[sqlx::test]
async fn outsiders_cannot_load_older_messages(pool: PgPool) {
    let url = start_server(pool.clone()).await;
    let (_lia, _) = join(&url, "lia").await;
    let (mut jobiss, _) = join(&url, "jobiss").await;

    let (private,): (uuid::Uuid,) = sqlx::query_as(
        "WITH c AS (INSERT INTO conversations (name, kind) VALUES ('Só a Lia', 'group') RETURNING id)
         INSERT INTO conversation_members (conversation_id, user_id)
         SELECT c.id, u.id FROM c, users u WHERE u.username = 'lia'
         RETURNING conversation_id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let reply = older(&mut jobiss, ConversationId(private), MessageId(i64::MAX)).await;
    assert!(
        matches!(reply, ServerMessage::Error { .. }),
        "esperava Error, veio {reply:?}"
    );
}

#[sqlx::test]
async fn only_members_receive_a_message(pool: PgPool) {
    let url = start_server(pool.clone()).await;
    let (mut lia, _) = join(&url, "lia").await;
    let (mut jobiss, _) = join(&url, "jobiss").await;

    // Uma conversa só com a Lia.
    let (private,): (uuid::Uuid,) = sqlx::query_as(
        "WITH c AS (INSERT INTO conversations (name, kind) VALUES ('Só a Lia', 'group') RETURNING id)
         INSERT INTO conversation_members (conversation_id, user_id)
         SELECT c.id, u.id FROM c, users u WHERE u.username = 'lia'
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
    let (mut socket, _) = join(&url, "lia").await;

    say(&mut socket, GROUP_ROOM, "   ").await;
    assert!(matches!(
        recv(&mut socket).await,
        ServerMessage::Error { .. }
    ));
}

#[sqlx::test]
async fn chatting_before_login_gets_an_auth_error(pool: PgPool) {
    let url = start_server(pool).await;
    let (_, reply) = connect(
        &url,
        ClientMessage::OpenConversation {
            conversation_id: GROUP_ROOM,
        },
    )
    .await;
    assert!(matches!(reply, ServerMessage::AuthError { .. }));
}

#[sqlx::test]
async fn login_ignores_case_and_rejects_a_wrong_password(pool: PgPool) {
    let url = start_server(pool).await;
    let (_, welcome) = join(&url, "Jobiss").await;
    let ServerMessage::Welcome { username, .. } = &welcome else {
        unreachable!()
    };
    assert_eq!(username, "jobiss");

    let (mut socket, reply) = connect(&url, login("jobiss", "senha-errada")).await;
    let ServerMessage::AuthError { reason } = reply else {
        panic!("esperava AuthError, veio {reply:?}");
    };
    assert_eq!(reason, "Usuário ou senha incorretos.");

    // A mesma conexão pode tentar de novo.
    send(&mut socket, &login("JOBISS", PASSWORD)).await;
    let reply = recv(&mut socket).await;
    assert!(matches!(reply, ServerMessage::Welcome { .. }), "{reply:?}");

    // Usuário que não existe recebe a mesma resposta de senha errada.
    let (_, reply) = connect(&url, login("ninguem", PASSWORD)).await;
    assert!(matches!(
        reply,
        ServerMessage::AuthError { reason } if reason == "Usuário ou senha incorretos."
    ));
}

#[sqlx::test]
async fn register_validates_the_input(pool: PgPool) {
    let url = start_server(pool).await;
    join(&url, "lia").await;

    let bad = [
        register("LIA"),
        register("jo"),
        register("nome com espaço"),
        ClientMessage::Register {
            username: "nina".into(),
            password: "curta".into(),
        },
    ];
    for msg in bad {
        let (_, reply) = connect(&url, msg.clone()).await;
        assert!(
            matches!(reply, ServerMessage::AuthError { .. }),
            "{msg:?} deveria falhar, veio {reply:?}"
        );
    }
}

#[sqlx::test]
async fn resume_works_until_logout(pool: PgPool) {
    let url = start_server(pool.clone()).await;
    let (mut first, welcome) = join(&url, "nina").await;
    let token = token_of(&welcome);
    assert_eq!(token.len(), 64);

    // O banco guarda o hash, nunca o token.
    let (stored,): (Vec<u8>,) = sqlx::query_as("SELECT token_hash FROM sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_ne!(stored, token.as_bytes());

    let (_, reply) = connect(
        &url,
        ClientMessage::Resume {
            token: token.clone(),
        },
    )
    .await;
    let ServerMessage::Welcome { username, .. } = &reply else {
        panic!("esperava Welcome, veio {reply:?}");
    };
    assert_eq!(username, "nina");

    // Depois do Logout a conexão volta para o login e o token não vale mais.
    send(&mut first, &ClientMessage::Logout).await;
    send(
        &mut first,
        &ClientMessage::OpenConversation {
            conversation_id: GROUP_ROOM,
        },
    )
    .await;
    assert!(matches!(
        recv(&mut first).await,
        ServerMessage::AuthError { .. }
    ));

    let (_, reply) = connect(&url, ClientMessage::Resume { token }).await;
    assert!(
        matches!(reply, ServerMessage::AuthError { .. }),
        "{reply:?}"
    );
}

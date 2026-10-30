//! Limite de tentativas de login (F1-08), de ponta a ponta pelo WebSocket.
//!
//! O servidor sobe com um bloqueio de 2 segundos em vez de 15 minutos, para o
//! teste conseguir esperar o prazo passar.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use protocol::{ClientMessage, ServerMessage};
use server::{AppState, app, login_limit::LoginLimit};
use sqlx::PgPool;
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const PASSWORD: &str = "senha-boa-123";
const LOCKOUT: Duration = Duration::from_secs(2);

async fn start_server(pool: PgPool) -> String {
    let limit = LoginLimit {
        lockout: LOCKOUT,
        ..LoginLimit::default()
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app(AppState::with_login_limit(pool, limit)))
            .await
            .unwrap();
    });
    format!("ws://{addr}/ws")
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

async fn ask(socket: &mut Socket, msg: &ClientMessage) -> ServerMessage {
    let json = serde_json::to_string(msg).unwrap();
    socket.send(Message::Text(json.into())).await.unwrap();
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

fn login(username: &str, password: &str) -> ClientMessage {
    ClientMessage::Login {
        username: username.into(),
        password: password.into(),
    }
}

fn auth_error(reply: ServerMessage) -> String {
    let ServerMessage::AuthError { reason } = reply else {
        panic!("esperava AuthError, veio {reply:?}");
    };
    reason
}

/// Cria a conta numa conexão própria e fecha.
async fn create_account(url: &str, username: &str) {
    let (mut socket, _) = connect_async(url).await.unwrap();
    let reply = ask(
        &mut socket,
        &ClientMessage::Register {
            username: username.into(),
            password: PASSWORD.into(),
        },
    )
    .await;
    assert!(matches!(reply, ServerMessage::Welcome { .. }), "{reply:?}");
}

#[sqlx::test]
async fn five_wrong_passwords_lock_the_user_until_the_deadline(pool: PgPool) {
    let url = start_server(pool).await;
    create_account(&url, "jobiss").await;
    create_account(&url, "nina").await;

    let (mut socket, _) = connect_async(&url).await.unwrap();
    for _ in 0..5 {
        let reason = auth_error(ask(&mut socket, &login("jobiss", "senha-errada")).await);
        assert_eq!(reason, "Usuário ou senha incorretos.");
    }

    // Bloqueado: nem a senha certa entra, e a resposta diz quanto falta.
    let reason = auth_error(ask(&mut socket, &login("Jobiss", PASSWORD)).await);
    assert!(reason.starts_with("Muitas tentativas erradas."), "{reason}");
    assert!(reason.contains(" s."), "{reason}");

    // Outra conexão não escapa do bloqueio, mas outro usuário entra normal.
    let (mut other, _) = connect_async(&url).await.unwrap();
    let reason = auth_error(ask(&mut other, &login("jobiss", PASSWORD)).await);
    assert!(reason.starts_with("Muitas tentativas erradas."), "{reason}");
    let reply = ask(&mut other, &login("nina", PASSWORD)).await;
    assert!(matches!(reply, ServerMessage::Welcome { .. }), "{reply:?}");

    // Depois do prazo, a senha certa volta a funcionar.
    tokio::time::sleep(LOCKOUT + Duration::from_millis(200)).await;
    let reply = ask(&mut socket, &login("jobiss", PASSWORD)).await;
    assert!(matches!(reply, ServerMessage::Welcome { .. }), "{reply:?}");
}

#[sqlx::test]
async fn right_password_resets_the_count(pool: PgPool) {
    let url = start_server(pool).await;
    create_account(&url, "jobiss").await;

    for _ in 0..2 {
        // Uma conexão nova a cada rodada, porque o login certo muda de fase.
        let (mut socket, _) = connect_async(&url).await.unwrap();
        for _ in 0..4 {
            auth_error(ask(&mut socket, &login("jobiss", "senha-errada")).await);
        }
        let reply = ask(&mut socket, &login("jobiss", PASSWORD)).await;
        assert!(matches!(reply, ServerMessage::Welcome { .. }), "{reply:?}");
    }
}

#[sqlx::test]
async fn unknown_users_are_locked_the_same_way(pool: PgPool) {
    // Senão o bloqueio contaria quais usuários existem.
    let url = start_server(pool).await;
    let (mut socket, _) = connect_async(&url).await.unwrap();
    for _ in 0..5 {
        let reason = auth_error(ask(&mut socket, &login("fantasma", "senha-errada")).await);
        assert_eq!(reason, "Usuário ou senha incorretos.");
    }
    let reason = auth_error(ask(&mut socket, &login("fantasma", "senha-errada")).await);
    assert!(reason.starts_with("Muitas tentativas erradas."), "{reason}");
}

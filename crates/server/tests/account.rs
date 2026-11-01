//! Conta: trocar o nome e a senha, com clientes de verdade.
//!
//! Precisa do PostgreSQL ligado, como em `chat.rs`.

use futures_util::{SinkExt, StreamExt};
use protocol::{ClientMessage, ServerMessage, UserId};
use server::{AppState, app};
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

async fn send(socket: &mut Socket, msg: ClientMessage) {
    let json = serde_json::to_string(&msg).unwrap();
    socket.send(Message::Text(json.into())).await.unwrap();
}

/// Próxima resposta que interessa ao teste (pula troca de chaves, presença etc.).
async fn recv(socket: &mut Socket) -> ServerMessage {
    loop {
        let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
            continue;
        };
        let msg: ServerMessage = serde_json::from_str(&text).unwrap();
        if matches!(
            msg,
            ServerMessage::Welcome { .. }
                | ServerMessage::AuthError { .. }
                | ServerMessage::Error { .. }
                | ServerMessage::PasswordChanged
                | ServerMessage::UserRenamed { .. }
        ) {
            return msg;
        }
    }
}

/// Entra (cadastro ou login) e devolve o socket, o id e o token.
async fn enter(url: &str, first: ClientMessage) -> (Socket, UserId, String) {
    let (mut socket, _) = connect_async(url).await.unwrap();
    send(&mut socket, first).await;
    let ServerMessage::Welcome { user_id, token, .. } = recv(&mut socket).await else {
        panic!("esperava Welcome");
    };
    (socket, user_id, token)
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

#[sqlx::test]
async fn rename_reaches_everyone_and_changes_the_login(pool: PgPool) {
    let url = start_server(pool).await;
    let (mut ana, ana_id, _) = enter(&url, register("ana")).await;
    let (mut bia, _, _) = enter(&url, register("bia")).await;

    // Nome de outra pessoa ou fora do formato: erro, nada muda.
    for taken in ["bia", "BIA", "a b"] {
        send(
            &mut ana,
            ClientMessage::Rename {
                username: taken.into(),
            },
        )
        .await;
        assert!(
            matches!(recv(&mut ana).await, ServerMessage::Error { .. }),
            "{taken}"
        );
    }

    send(
        &mut ana,
        ClientMessage::Rename {
            username: "Ana_Nova".into(),
        },
    )
    .await;
    let renamed = ServerMessage::UserRenamed {
        user_id: ana_id,
        username: "ana_nova".into(),
    };
    assert_eq!(recv(&mut ana).await, renamed);
    // As duas estão na Sala do Grupo: a outra fica sabendo na hora.
    assert_eq!(recv(&mut bia).await, renamed);

    let (mut old, _) = connect_async(&url).await.unwrap();
    send(&mut old, login("ana", PASSWORD)).await;
    assert!(matches!(
        recv(&mut old).await,
        ServerMessage::AuthError { .. }
    ));
    let (_, id, _) = enter(&url, login("ana_nova", PASSWORD)).await;
    assert_eq!(id, ana_id);
}

#[sqlx::test]
async fn change_password_needs_the_current_one_and_drops_other_sessions(pool: PgPool) {
    let url = start_server(pool).await;
    let (mut here, _, _) = enter(&url, register("lia")).await;
    let (_elsewhere, _, other_token) = enter(&url, login("lia", PASSWORD)).await;

    send(
        &mut here,
        ClientMessage::ChangePassword {
            current: "senha-errada-1".into(),
            new: "outra-senha-456".into(),
        },
    )
    .await;
    assert!(matches!(recv(&mut here).await, ServerMessage::Error { .. }));

    send(
        &mut here,
        ClientMessage::ChangePassword {
            current: PASSWORD.into(),
            new: "curta".into(),
        },
    )
    .await;
    assert!(matches!(recv(&mut here).await, ServerMessage::Error { .. }));

    send(
        &mut here,
        ClientMessage::ChangePassword {
            current: PASSWORD.into(),
            new: "outra-senha-456".into(),
        },
    )
    .await;
    assert_eq!(recv(&mut here).await, ServerMessage::PasswordChanged);

    // A sessão do outro aparelho não volta mais sozinha.
    let (mut socket, _) = connect_async(&url).await.unwrap();
    send(&mut socket, ClientMessage::Resume { token: other_token }).await;
    assert!(matches!(
        recv(&mut socket).await,
        ServerMessage::AuthError { .. }
    ));

    let (mut socket, _) = connect_async(&url).await.unwrap();
    send(&mut socket, login("lia", PASSWORD)).await;
    assert!(matches!(
        recv(&mut socket).await,
        ServerMessage::AuthError { .. }
    ));
    enter(&url, login("lia", "outra-senha-456")).await;
}

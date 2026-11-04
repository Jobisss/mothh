//! Conta: trocar o nome e a senha, com clientes de verdade.
//!
//! Precisa do PostgreSQL ligado, como em `chat.rs`.

use futures_util::{SinkExt, StreamExt};
use protocol::{ClientMessage, ServerMessage, UserId};
use server::mail::CapturingMailer;
use server::{AppState, app};
use sqlx::PgPool;
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const PASSWORD: &str = "senha-boa-123";

async fn start_server(pool: PgPool) -> String {
    start_with(AppState::new(pool)).await
}

async fn start_with(state: AppState) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app(state)).await.unwrap();
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
                | ServerMessage::ResetRequested
                | ServerMessage::PasswordReset
                | ServerMessage::EmailChanged { .. }
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
        email: Some(format!("{}@teste.com", username)),
    }
}

fn login(username: &str, password: &str) -> ClientMessage {
    ClientMessage::Login {
        username: username.into(),
        password: password.into(),
    }
}

fn email_of(welcome: &ServerMessage) -> Option<String> {
    let ServerMessage::Welcome { email, .. } = welcome else {
        panic!("esperava Welcome, veio {welcome:?}");
    };
    email.clone()
}

/// V6-04: a conta nova precisa de e-mail, e entra com ele ou com o usuário.
#[sqlx::test]
async fn register_needs_an_email_and_login_accepts_it(pool: PgPool) {
    let url = start_server(pool).await;

    let (mut socket, _) = connect_async(&url).await.unwrap();
    for (username, email) in [
        ("ana", None),
        ("ana", Some("sem-arroba")),
        ("ana", Some("a@b")),
    ] {
        send(
            &mut socket,
            ClientMessage::Register {
                username: username.into(),
                password: PASSWORD.into(),
                email: email.map(Into::into),
            },
        )
        .await;
        assert!(
            matches!(recv(&mut socket).await, ServerMessage::AuthError { .. }),
            "{email:?}"
        );
    }

    send(
        &mut socket,
        ClientMessage::Register {
            username: "ana".into(),
            password: PASSWORD.into(),
            email: Some("  Ana@Exemplo.COM ".into()),
        },
    )
    .await;
    let welcome = recv(&mut socket).await;
    assert_eq!(email_of(&welcome), Some("ana@exemplo.com".into()));

    // O mesmo e-mail não serve para outra conta.
    let (mut other, _) = connect_async(&url).await.unwrap();
    send(
        &mut other,
        ClientMessage::Register {
            username: "bia".into(),
            password: PASSWORD.into(),
            email: Some("ana@exemplo.com".into()),
        },
    )
    .await;
    let ServerMessage::AuthError { reason } = recv(&mut other).await else {
        panic!("esperava AuthError");
    };
    assert!(reason.contains("e-mail"), "{reason}");

    // Entra com o e-mail, em qualquer caixa.
    let (_, _, _) = enter(&url, login("ANA@exemplo.com", PASSWORD)).await;
    let (mut wrong, _) = connect_async(&url).await.unwrap();
    send(&mut wrong, login("ana@exemplo.com", "senha-errada-1")).await;
    assert!(matches!(
        recv(&mut wrong).await,
        ServerMessage::AuthError { .. }
    ));
}

/// Conta de antes da V6-04 (sem e-mail) continua entrando e pode pôr um.
#[sqlx::test]
async fn old_account_without_email_logs_in_and_can_add_one(pool: PgPool) {
    let url = start_server(pool.clone()).await;
    let (_, _, _) = enter(&url, register("ana")).await;
    sqlx::query("UPDATE users SET email = NULL WHERE username = 'ana'")
        .execute(&pool)
        .await
        .unwrap();

    let (mut ana, _, _) = enter(&url, login("ana", PASSWORD)).await;
    // `enter` descarta o Welcome; pede de novo o estado pela conexão nova.
    let (mut again, _) = connect_async(&url).await.unwrap();
    send(&mut again, login("ana", PASSWORD)).await;
    assert_eq!(email_of(&recv(&mut again).await), None);

    send(
        &mut ana,
        ClientMessage::SetEmail {
            email: "Ana@Exemplo.com".into(),
        },
    )
    .await;
    assert_eq!(
        recv(&mut ana).await,
        ServerMessage::EmailChanged {
            email: "ana@exemplo.com".into()
        }
    );
    let (mut third, _) = connect_async(&url).await.unwrap();
    send(&mut third, login("ana@exemplo.com", PASSWORD)).await;
    assert_eq!(
        email_of(&recv(&mut third).await),
        Some("ana@exemplo.com".into())
    );
}

/// Esqueci a senha: código por e-mail, senha nova, sessões antigas caem.
#[sqlx::test]
async fn password_reset_by_code(pool: PgPool) {
    let mailer = CapturingMailer::new();
    let url = start_with(AppState::new(pool).with_mailer(mailer.clone())).await;
    let (mut ana, _, old_token) = enter(&url, register("ana")).await;

    let (mut socket, _) = connect_async(&url).await.unwrap();
    // E-mail sem conta: mesma resposta, sem código.
    send(
        &mut socket,
        ClientMessage::RequestPasswordReset {
            email: "ninguem@teste.com".into(),
        },
    )
    .await;
    assert_eq!(recv(&mut socket).await, ServerMessage::ResetRequested);
    assert_eq!(mailer.code_for("ninguem@teste.com"), None);

    send(
        &mut socket,
        ClientMessage::RequestPasswordReset {
            email: "ANA@teste.com".into(),
        },
    )
    .await;
    assert_eq!(recv(&mut socket).await, ServerMessage::ResetRequested);
    let code = mailer.code_for("ana@teste.com").expect("código no e-mail");
    assert_eq!(code.len(), 6);

    // Código errado ou senha curta: nada muda.
    send(
        &mut socket,
        ClientMessage::ResetPassword {
            email: "ana@teste.com".into(),
            code: "000000".into(),
            password: "senha-nova-123".into(),
        },
    )
    .await;
    assert!(matches!(
        recv(&mut socket).await,
        ServerMessage::AuthError { .. }
    ));
    send(
        &mut socket,
        ClientMessage::ResetPassword {
            email: "ana@teste.com".into(),
            code: code.clone(),
            password: "curta".into(),
        },
    )
    .await;
    assert!(matches!(
        recv(&mut socket).await,
        ServerMessage::AuthError { .. }
    ));

    send(
        &mut socket,
        ClientMessage::ResetPassword {
            email: "ana@teste.com".into(),
            code: code.clone(),
            password: "senha-nova-123".into(),
        },
    )
    .await;
    assert_eq!(recv(&mut socket).await, ServerMessage::PasswordReset);

    // O código não serve duas vezes; a senha velha não entra; a nova entra;
    // a sessão antiga caiu.
    send(
        &mut socket,
        ClientMessage::ResetPassword {
            email: "ana@teste.com".into(),
            code,
            password: "outra-senha-123".into(),
        },
    )
    .await;
    assert!(matches!(
        recv(&mut socket).await,
        ServerMessage::AuthError { .. }
    ));
    let (mut old, _) = connect_async(&url).await.unwrap();
    send(&mut old, login("ana", PASSWORD)).await;
    assert!(matches!(
        recv(&mut old).await,
        ServerMessage::AuthError { .. }
    ));
    let (_, _, _) = enter(&url, login("ana", "senha-nova-123")).await;
    let (mut resumed, _) = connect_async(&url).await.unwrap();
    send(&mut resumed, ClientMessage::Resume { token: old_token }).await;
    assert!(matches!(
        recv(&mut resumed).await,
        ServerMessage::AuthError { .. }
    ));
    // A conexão antiga segue aberta até cair, mas não é o caso de testar.
    let _ = ana.close(None).await;
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

//! Fotos de perfil: trocar, avisar quem divide conversa e pedir a de alguém.
//!
//! Precisa do PostgreSQL ligado, como em `chat.rs`.

use futures_util::{SinkExt, StreamExt};
use protocol::{ClientMessage, Photo, ServerMessage, UserId};
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

/// Próxima mensagem que interessa ao teste (pula troca de chaves, presença etc.).
async fn recv_photo(socket: &mut Socket) -> ServerMessage {
    loop {
        let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
            continue;
        };
        let msg: ServerMessage = serde_json::from_str(&text).unwrap();
        if matches!(
            msg,
            ServerMessage::Photo { .. }
                | ServerMessage::PhotoChanged { .. }
                | ServerMessage::Error { .. }
        ) {
            return msg;
        }
    }
}

async fn welcome(socket: &mut Socket) -> (UserId, Option<i64>) {
    loop {
        let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
            continue;
        };
        if let ServerMessage::Welcome {
            user_id,
            photo_version,
            ..
        } = serde_json::from_str(&text).unwrap()
        {
            return (user_id, photo_version);
        }
    }
}

async fn register(url: &str, username: &str) -> (Socket, UserId, Option<i64>) {
    let (mut socket, _) = connect_async(url).await.unwrap();
    send(
        &mut socket,
        ClientMessage::Register {
            username: username.into(),
            password: PASSWORD.into(),
        },
    )
    .await;
    let (id, version) = welcome(&mut socket).await;
    (socket, id, version)
}

/// PNG mínimo com o cabeçalho certo (o servidor não decodifica a imagem).
fn png(side: u32) -> Photo {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend_from_slice(&13u32.to_be_bytes());
    bytes.extend_from_slice(b"IHDR");
    bytes.extend_from_slice(&side.to_be_bytes());
    bytes.extend_from_slice(&side.to_be_bytes());
    bytes.extend_from_slice(&[8, 6, 0, 0, 0, 1, 2, 3, 4]);
    Photo(bytes)
}

#[sqlx::test]
async fn photo_is_shared_with_people_in_the_same_conversation(pool: PgPool) {
    let url = start_server(pool).await;
    // Todo mundo está na Sala do Grupo: as duas dividem uma conversa.
    let (mut ana, ana_id, ana_version) = register(&url, "ana").await;
    let (mut bia, _, _) = register(&url, "bia").await;
    assert_eq!(ana_version, Some(0), "conta nova começa sem foto");

    send(&mut bia, ClientMessage::GetPhoto { user_id: ana_id }).await;
    assert_eq!(
        recv_photo(&mut bia).await,
        ServerMessage::Photo {
            user_id: ana_id,
            version: 0,
            png: None
        }
    );

    let photo = png(128);
    send(&mut ana, ClientMessage::SetPhoto { png: photo.clone() }).await;
    let changed = ServerMessage::PhotoChanged {
        user_id: ana_id,
        version: 1,
    };
    assert_eq!(recv_photo(&mut ana).await, changed);
    assert_eq!(recv_photo(&mut bia).await, changed);

    send(&mut bia, ClientMessage::GetPhoto { user_id: ana_id }).await;
    assert_eq!(
        recv_photo(&mut bia).await,
        ServerMessage::Photo {
            user_id: ana_id,
            version: 1,
            png: Some(photo)
        }
    );

    // Trocar de novo sobe a versão, e quem entra de novo já sabe dela.
    send(&mut ana, ClientMessage::SetPhoto { png: png(64) }).await;
    assert!(matches!(
        recv_photo(&mut ana).await,
        ServerMessage::PhotoChanged { version: 2, .. }
    ));
    let (mut again, _) = connect_async(&url).await.unwrap();
    send(
        &mut again,
        ClientMessage::Login {
            username: "ana".into(),
            password: PASSWORD.into(),
        },
    )
    .await;
    assert_eq!(welcome(&mut again).await, (ana_id, Some(2)));
}

#[sqlx::test]
async fn refuses_a_photo_that_is_not_a_small_png(pool: PgPool) {
    let url = start_server(pool).await;
    let (mut ana, _, _) = register(&url, "ana").await;
    for bad in [Photo(b"nada de png aqui, nem perto".to_vec()), png(512)] {
        send(&mut ana, ClientMessage::SetPhoto { png: bad }).await;
        assert!(
            matches!(recv_photo(&mut ana).await, ServerMessage::Error { .. }),
            "foto inválida tinha que dar erro"
        );
    }
}

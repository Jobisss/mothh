//! Imagens e arquivos (F1-15): upload HTTP, mensagem WS e download autorizado.

use futures_util::{SinkExt, StreamExt};
use image::ImageEncoder;
use protocol::{ATTACHMENTS_PATH, ClientMessage, FileAttachment, ServerMessage, Uploaded};
use reqwest::StatusCode;
use server::{AppState, app, state::GROUP_ROOM};
use sqlx::PgPool;
use tokio_tungstenite::{connect_async, tungstenite::Message};

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn start_server(pool: PgPool) -> String {
    let dir = std::env::temp_dir().join(format!("mothh-files-{}", uuid::Uuid::new_v4()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app(AppState::new(pool).with_attachments_dir(dir)))
            .await
            .unwrap();
    });
    format!("http://{addr}")
}

async fn send(socket: &mut Socket, message: &ClientMessage) {
    socket
        .send(Message::Text(
            serde_json::to_string(message).unwrap().into(),
        ))
        .await
        .unwrap();
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

async fn recv(socket: &mut Socket) -> ServerMessage {
    loop {
        if let Message::Text(text) = socket.next().await.unwrap().unwrap() {
            let msg: ServerMessage = serde_json::from_str(&text).unwrap();
            // A troca de chaves (F2-08) tem os testes dela em `mls.rs`.
            if !is_mls(&msg) {
                return msg;
            }
        }
    }
}

async fn join(base: &str, name: &str) -> (Socket, String) {
    let (mut socket, _) = connect_async(format!("{}/ws", base.replacen("http", "ws", 1)))
        .await
        .unwrap();
    send(
        &mut socket,
        &ClientMessage::Register {
            username: name.into(),
            password: "senha-boa-123".into(),
        },
    )
    .await;
    let ServerMessage::Welcome { token, .. } = recv(&mut socket).await else {
        panic!("esperava Welcome");
    };
    (socket, token)
}

async fn upload(
    base: &str,
    token: &str,
    name: &str,
    mime: &str,
    body: Vec<u8>,
) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{base}{ATTACHMENTS_PATH}"))
        .query(&[("name", name)])
        .bearer_auth(token)
        .header("content-type", mime)
        .body(body)
        .send()
        .await
        .unwrap()
}

fn tiny_png() -> Vec<u8> {
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&[255, 255, 255, 255], 1, 1, image::ExtendedColorType::Rgba8)
        .unwrap();
    png
}

#[sqlx::test]
async fn image_and_pdf_reach_other_client(pool: PgPool) {
    let base = start_server(pool).await;
    let (mut jobiss, token) = join(&base, "jobiss").await;
    let (mut nina, nina_token) = join(&base, "nina").await;
    for (name, mime, bytes) in [
        ("foto.png", "image/png", tiny_png()),
        (
            "documento.pdf",
            "application/pdf",
            b"%PDF-1.4 teste".to_vec(),
        ),
    ] {
        let response = upload(&base, &token, name, mime, bytes.clone()).await;
        assert_eq!(response.status(), StatusCode::OK);
        let uploaded: Uploaded = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
        send(
            &mut jobiss,
            &ClientMessage::SendFile {
                conversation_id: GROUP_ROOM,
                attachment_id: uploaded.id,
            },
        )
        .await;
        let ServerMessage::NewMessage { message } = recv(&mut nina).await else {
            panic!("Nina deveria receber o arquivo");
        };
        assert_eq!(message.audio, None);
        assert_eq!(
            message.file,
            Some(FileAttachment {
                attachment_id: uploaded.id,
                name: name.into(),
                content_type: mime.into(),
                size_bytes: bytes.len() as u32,
            })
        );
        let download = reqwest::Client::new()
            .get(format!("{base}{ATTACHMENTS_PATH}/{}", uploaded.id.0))
            .bearer_auth(&nina_token)
            .send()
            .await
            .unwrap();
        assert_eq!(download.status(), StatusCode::OK);
        assert_eq!(download.bytes().await.unwrap().to_vec(), bytes);
    }
    send(
        &mut nina,
        &ClientMessage::OpenConversation {
            conversation_id: GROUP_ROOM,
        },
    )
    .await;
    let ServerMessage::History { messages, .. } = recv(&mut nina).await else {
        panic!("esperava histórico");
    };
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].file.as_ref().unwrap().name, "foto.png");
    assert_eq!(messages[1].file.as_ref().unwrap().name, "documento.pdf");
}

#[sqlx::test]
async fn rejects_invalid_or_large_uploads(pool: PgPool) {
    let base = start_server(pool).await;
    let (_socket, token) = join(&base, "jobiss").await;
    assert_eq!(
        upload(
            &base,
            &token,
            "../segredo.pdf",
            "application/pdf",
            b"x".to_vec()
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        upload(&base, &token, "foto.png", "image/png", b"falsa".to_vec())
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        upload(&base, &token, "vazio.pdf", "application/pdf", vec![])
            .await
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        upload(
            &base,
            &token,
            "grande.pdf",
            "application/pdf",
            vec![0; 10 * 1024 * 1024 + 1]
        )
        .await
        .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

#[sqlx::test]
async fn cannot_send_another_users_file(pool: PgPool) {
    let base = start_server(pool).await;
    let (_jobiss, token) = join(&base, "jobiss").await;
    let (mut nina, _) = join(&base, "nina").await;
    let response = upload(&base, &token, "foto.png", "image/png", tiny_png()).await;
    let uploaded: Uploaded = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    send(
        &mut nina,
        &ClientMessage::SendFile {
            conversation_id: GROUP_ROOM,
            attachment_id: uploaded.id,
        },
    )
    .await;
    assert!(matches!(recv(&mut nina).await, ServerMessage::Error { .. }));
}

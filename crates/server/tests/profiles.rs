//! Página de perfil: textos pelo WebSocket, imagens por HTTP, aviso para quem
//! divide conversa. Precisa do PostgreSQL ligado, como em `chat.rs`.

use futures_util::{SinkExt, StreamExt};
use image::ImageEncoder;
use protocol::{
    ClientMessage, PROFILE_PATH, Profile, ProfileImage, ProfileText, ServerMessage, Showcase,
    UserId,
};
use reqwest::StatusCode;
use server::{AppState, app};
use sqlx::PgPool;
use tokio_tungstenite::{connect_async, tungstenite::Message};

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn start_server(pool: PgPool) -> String {
    let dir = std::env::temp_dir().join(format!("mothh-perfil-{}", uuid::Uuid::new_v4()));
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

/// Próxima mensagem de perfil (ou erro); pula o resto.
async fn recv_profile(socket: &mut Socket) -> ServerMessage {
    loop {
        if let Message::Text(text) = socket.next().await.unwrap().unwrap() {
            let msg: ServerMessage = serde_json::from_str(&text).unwrap();
            if matches!(
                msg,
                ServerMessage::Profile { .. }
                    | ServerMessage::ProfileChanged { .. }
                    | ServerMessage::Error { .. }
            ) {
                return msg;
            }
        }
    }
}

async fn join(base: &str, name: &str) -> (Socket, UserId, String) {
    let (mut socket, _) = connect_async(format!("{}/ws", base.replacen("http", "ws", 1)))
        .await
        .unwrap();
    send(
        &mut socket,
        &ClientMessage::Register {
            username: name.into(),
            password: "senha-boa-123".into(),
            email: Some(format!("{}@teste.com", name)),
        },
    )
    .await;
    loop {
        if let Message::Text(text) = socket.next().await.unwrap().unwrap()
            && let ServerMessage::Welcome {
                user_id,
                token,
                profiles,
                ..
            } = serde_json::from_str(&text).unwrap()
        {
            assert!(profiles, "servidor novo avisa que guarda perfis");
            return (socket, user_id, token);
        }
    }
}

async fn profile_of(socket: &mut Socket, user_id: UserId) -> Profile {
    send(socket, &ClientMessage::GetProfile { user_id }).await;
    match recv_profile(socket).await {
        ServerMessage::Profile { profile } => profile,
        other => panic!("esperava Profile, veio {other:?}"),
    }
}

fn gif() -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
        for shade in [0u8, 255] {
            let frame = image::Frame::new(image::RgbaImage::from_pixel(
                4,
                4,
                image::Rgba([shade, shade, shade, 255]),
            ));
            encoder.encode_frame(frame).unwrap();
        }
    }
    bytes
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    let pixels = vec![200u8; (width * height * 4) as usize];
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(&pixels, width, height, image::ExtendedColorType::Rgba8)
        .unwrap();
    bytes
}

#[sqlx::test]
async fn texts_and_images_reach_people_who_share_a_conversation(pool: PgPool) {
    let base = start_server(pool).await;
    // Todo mundo entra na Sala do Grupo: as duas dividem uma conversa.
    let (mut ana, ana_id, ana_token) = join(&base, "ana").await;
    let (mut bia, _, bia_token) = join(&base, "bia").await;

    let fresh = profile_of(&mut bia, ana_id).await;
    assert_eq!(fresh.username, "ana");
    assert_eq!(fresh.text, ProfileText::default());
    assert!(fresh.images.is_empty());
    assert_eq!(fresh.version, 0);
    assert!(fresh.member_since_ms > 0);

    let text = ProfileText {
        real_name: "Ana Paula".into(),
        location: "Recife".into(),
        about: "oi, eu sou a ana".into(),
        accent: "#ff8a3d".into(),
        showcases: vec![Showcase::About],
    };
    send(&mut ana, &ClientMessage::SaveProfile { text: text.clone() }).await;
    let changed = ServerMessage::ProfileChanged {
        user_id: ana_id,
        version: 1,
    };
    assert_eq!(recv_profile(&mut ana).await, changed);
    assert_eq!(recv_profile(&mut bia).await, changed);
    let saved = profile_of(&mut bia, ana_id).await;
    assert_eq!(saved.text, text);
    assert_eq!(saved.version, 1);

    // Imagem: a Ana manda um GIF de fundo, a Bia baixa.
    let http = reqwest::Client::new();
    let response = http
        .post(format!("{base}{PROFILE_PATH}/imagens/fundo"))
        .bearer_auth(&ana_token)
        .header("content-type", "image/gif")
        .body(gif())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(matches!(
        recv_profile(&mut bia).await,
        ServerMessage::ProfileChanged { version: 2, .. }
    ));
    let with_image = profile_of(&mut bia, ana_id).await;
    assert_eq!(with_image.images.len(), 1);
    assert_eq!(with_image.images[0].kind, ProfileImage::Background);
    assert_eq!(with_image.images[0].content_type, "image/gif");
    assert_eq!(with_image.images[0].version, 2);
    // Os textos continuam.
    assert_eq!(with_image.text, text);

    let url = format!("{base}{PROFILE_PATH}/{}/imagens/fundo", ana_id.0);
    let download = http.get(&url).bearer_auth(&bia_token).send().await.unwrap();
    assert_eq!(download.status(), StatusCode::OK);
    assert_eq!(download.headers()["content-type"], "image/gif");
    assert_eq!(download.bytes().await.unwrap().to_vec(), gif());
    // Sem token, nada.
    assert_eq!(
        http.get(&url).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );

    // Tirar a imagem: some da página e do download.
    let removed = http
        .delete(format!("{base}{PROFILE_PATH}/imagens/fundo"))
        .bearer_auth(&ana_token)
        .send()
        .await
        .unwrap();
    assert_eq!(removed.status(), StatusCode::NO_CONTENT);
    assert!(matches!(
        recv_profile(&mut bia).await,
        ServerMessage::ProfileChanged { version: 3, .. }
    ));
    assert!(profile_of(&mut bia, ana_id).await.images.is_empty());
    assert_eq!(
        http.get(&url)
            .bearer_auth(&bia_token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[sqlx::test]
async fn refuses_bad_texts_and_images(pool: PgPool) {
    let base = start_server(pool).await;
    let (mut ana, _, token) = join(&base, "ana").await;

    send(
        &mut ana,
        &ClientMessage::SaveProfile {
            text: ProfileText {
                accent: "azul".into(),
                ..ProfileText::default()
            },
        },
    )
    .await;
    assert!(matches!(
        recv_profile(&mut ana).await,
        ServerMessage::Error { .. }
    ));

    let http = reqwest::Client::new();
    let post = |slug: &str, mime: &str, body: Vec<u8>| {
        http.post(format!("{base}{PROFILE_PATH}/imagens/{slug}"))
            .bearer_auth(&token)
            .header("content-type", mime)
            .body(body)
            .send()
    };
    // Foto maior que 512 px, tipo trocado, tipo de imagem que não existe.
    assert_eq!(
        post("foto", "image/png", png(600, 600))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post("arte", "image/png", gif()).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post("banner", "image/png", png(10, 10))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        post("arte-lateral", "image/png", png(300, 900))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
}

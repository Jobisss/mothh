//! Mensagens de áudio (F1-14): o arquivo sobe por HTTP e a mensagem vai pelo
//! WebSocket.

use futures_util::{SinkExt, StreamExt};
use opus_pure::{Application, MAX_PACKET_BYTES, OggOpusWriter, OpusEncoder, OpusHead};
use protocol::{
    ATTACHMENTS_PATH, AUDIO_CONTENT_TYPE, AttachmentId, ClientMessage, ConversationId,
    MAX_AUDIO_BYTES, ServerMessage, Uploaded,
};
use reqwest::StatusCode;
use server::{AppState, app, state::GROUP_ROOM};
use sqlx::PgPool;
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const PASSWORD: &str = "senha-boa-123";

/// Sobe o servidor com uma pasta de anexos só deste teste. Devolve o endereço
/// `http://...` (o WebSocket fica em `/ws`).
async fn start_server(pool: PgPool) -> String {
    let dir = std::env::temp_dir().join(format!("mothh-anexos-{}", uuid::Uuid::new_v4()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let state = AppState::new(pool).with_attachments_dir(dir);
        axum::serve(listener, app(state)).await.unwrap();
    });
    format!("http://{addr}")
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

/// Cria a conta e devolve a conexão e o token.
async fn join(base: &str, username: &str) -> (Socket, String) {
    let url = format!("{}/ws", base.replacen("http", "ws", 1));
    let (mut socket, _) = connect_async(url).await.unwrap();
    let register = ClientMessage::Register {
        username: username.into(),
        password: PASSWORD.into(),
    };
    send(&mut socket, &register).await;
    let ServerMessage::Welcome { token, .. } = recv(&mut socket).await else {
        panic!("esperava Welcome");
    };
    (socket, token)
}

/// `seconds` de um tom de 440 Hz em Ogg Opus, como o app grava.
fn ogg_opus(seconds: f32) -> Vec<u8> {
    let mut encoder = OpusEncoder::new(48_000, 1, Application::Voip).unwrap();
    encoder.bitrate_bps = 24_000;
    let head = OpusHead::for_encoder(&encoder, 48_000);
    let mut writer = OggOpusWriter::new(Vec::new(), head).unwrap();
    let total = (seconds * 48_000.0) as usize;
    let mut packet = vec![0u8; MAX_PACKET_BYTES];
    let mut frame = [0f32; 960];
    for start in (0..total).step_by(960) {
        for (i, sample) in frame.iter_mut().enumerate() {
            let t = (start + i) as f32 / 48_000.0;
            *sample = 0.3 * (t * 440.0 * std::f32::consts::TAU).sin();
        }
        let n = encoder.encode(&frame, 960, &mut packet).unwrap();
        writer.write_packet(&packet[..n]).unwrap();
    }
    writer.finish().unwrap()
}

async fn upload(base: &str, token: &str, bytes: Vec<u8>) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{base}{ATTACHMENTS_PATH}"))
        .bearer_auth(token)
        .header("content-type", AUDIO_CONTENT_TYPE)
        .body(bytes)
        .send()
        .await
        .unwrap()
}

async fn upload_ok(base: &str, token: &str, bytes: Vec<u8>) -> AttachmentId {
    let response = upload(base, token, bytes).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.bytes().await.unwrap();
    serde_json::from_slice::<Uploaded>(&body).unwrap().id
}

async fn download(base: &str, token: Option<&str>, id: AttachmentId) -> reqwest::Response {
    let mut request = reqwest::Client::new().get(format!("{base}{ATTACHMENTS_PATH}/{}", id.0));
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    request.send().await.unwrap()
}

#[sqlx::test]
async fn jobiss_records_ten_seconds_and_nina_gets_it(pool: PgPool) {
    let base = start_server(pool).await;
    let (mut jobiss, jobiss_token) = join(&base, "jobiss").await;
    let (mut nina, nina_token) = join(&base, "nina").await;

    let file = ogg_opus(10.0);
    let id = upload_ok(&base, &jobiss_token, file.clone()).await;
    send(
        &mut jobiss,
        &ClientMessage::SendAudio {
            conversation_id: GROUP_ROOM,
            attachment_id: id,
        },
    )
    .await;

    let ServerMessage::NewMessage { message } = recv(&mut nina).await else {
        panic!("Nina deveria receber o áudio");
    };
    assert_eq!(message.author_name, "jobiss");
    assert_eq!(message.text, "");
    let audio = message.audio.expect("a mensagem deveria ter áudio");
    assert_eq!(audio.attachment_id, id);
    // Este arquivo não desconta o atraso do codificador (o app desconta):
    // fica alguns milissegundos abaixo dos 10 s.
    assert!(
        (9_980..=10_020).contains(&audio.duration_ms),
        "duração {}",
        audio.duration_ms
    );

    // Nina baixa o mesmo arquivo que o Jobiss enviou.
    let response = download(&base, Some(&nina_token), id).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"].to_str().unwrap(),
        AUDIO_CONTENT_TYPE
    );
    assert_eq!(response.bytes().await.unwrap().to_vec(), file);

    // E o áudio continua lá no histórico e na lista de conversas.
    send(
        &mut nina,
        &ClientMessage::OpenConversation {
            conversation_id: GROUP_ROOM,
        },
    )
    .await;
    let ServerMessage::History { messages, .. } = recv(&mut nina).await else {
        panic!("esperava History");
    };
    assert_eq!(messages.last().unwrap().audio, Some(audio));
    let (_lia, lia_token) = join(&base, "lia").await;
    // Lia entrou depois na Sala do Grupo: vê o áudio que já estava lá.
    assert_eq!(
        download(&base, Some(&lia_token), id).await.status(),
        StatusCode::OK
    );
}

#[sqlx::test]
async fn only_people_in_the_conversation_can_download(pool: PgPool) {
    let base = start_server(pool).await;
    let (mut jobiss, jobiss_token) = join(&base, "jobiss").await;
    let (mut nina, nina_token) = join(&base, "nina").await;
    let (_lia, lia_token) = join(&base, "lia").await;

    send(
        &mut jobiss,
        &ClientMessage::OpenDirect {
            username: "nina".into(),
        },
    )
    .await;
    let ServerMessage::DirectOpened { conversation } = recv(&mut jobiss).await else {
        panic!("esperava DirectOpened");
    };
    let ServerMessage::ConversationAdded { .. } = recv(&mut nina).await else {
        panic!("esperava ConversationAdded");
    };

    let id = upload_ok(&base, &jobiss_token, ogg_opus(1.0)).await;
    // Antes de virar mensagem, só quem enviou consegue baixar.
    assert_eq!(
        download(&base, Some(&nina_token), id).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        download(&base, Some(&jobiss_token), id).await.status(),
        StatusCode::OK
    );

    send(
        &mut jobiss,
        &ClientMessage::SendAudio {
            conversation_id: conversation.id,
            attachment_id: id,
        },
    )
    .await;
    let ServerMessage::NewMessage { message } = recv(&mut nina).await else {
        panic!("Nina deveria receber o áudio");
    };
    assert_eq!(message.conversation_id, conversation.id);

    assert_eq!(
        download(&base, Some(&nina_token), id).await.status(),
        StatusCode::OK
    );
    // Lia não está na conversa direta.
    assert_eq!(
        download(&base, Some(&lia_token), id).await.status(),
        StatusCode::NOT_FOUND
    );
    // Sem token, nem chega a procurar.
    assert_eq!(
        download(&base, None, id).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        download(&base, Some("token-inventado"), id).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test]
async fn bad_uploads_are_refused(pool: PgPool) {
    let base = start_server(pool).await;
    let (_jobiss, token) = join(&base, "jobiss").await;

    // Não é Opus.
    let response = upload(&base, &token, b"isso nao e audio".to_vec()).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // Grande demais: o servidor corta antes de ler tudo.
    let response = upload(&base, &token, vec![0u8; MAX_AUDIO_BYTES + 1]).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);

    // Tipo errado.
    let response = reqwest::Client::new()
        .post(format!("{base}{ATTACHMENTS_PATH}"))
        .bearer_auth(&token)
        .header("content-type", "image/png")
        .body(ogg_opus(1.0))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    // Sem entrar.
    let response = reqwest::Client::new()
        .post(format!("{base}{ATTACHMENTS_PATH}"))
        .header("content-type", AUDIO_CONTENT_TYPE)
        .body(ogg_opus(1.0))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn cannot_send_someone_elses_audio(pool: PgPool) {
    let base = start_server(pool).await;
    let (_jobiss, jobiss_token) = join(&base, "jobiss").await;
    let (mut nina, _) = join(&base, "nina").await;

    let id = upload_ok(&base, &jobiss_token, ogg_opus(1.0)).await;
    for attachment_id in [id, AttachmentId(uuid::Uuid::new_v4())] {
        send(
            &mut nina,
            &ClientMessage::SendAudio {
                conversation_id: GROUP_ROOM,
                attachment_id,
            },
        )
        .await;
        let reply = recv(&mut nina).await;
        assert!(
            matches!(reply, ServerMessage::Error { ref reason } if reason == "Esse áudio não existe."),
            "{reply:?}"
        );
    }

    // E não dá para mandar numa conversa da qual não participa.
    let (mut lia, lia_token) = join(&base, "lia").await;
    let id = upload_ok(&base, &lia_token, ogg_opus(1.0)).await;
    send(
        &mut lia,
        &ClientMessage::SendAudio {
            conversation_id: ConversationId(uuid::Uuid::new_v4()),
            attachment_id: id,
        },
    )
    .await;
    assert!(matches!(recv(&mut lia).await, ServerMessage::Error { .. }));
}

#[test]
fn duration_comes_from_the_file() {
    assert_eq!(server::attachments::audio_duration_ms(b"OggS lixo"), None);
    let ms = server::attachments::audio_duration_ms(&ogg_opus(2.5)).unwrap();
    assert!((2_480..=2_520).contains(&ms), "{ms}");
}

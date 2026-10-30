//! Token de chamada (`JoinCall`): só membro recebe, e o token é válido.
//!
//! Precisa do PostgreSQL ligado, como em `chat.rs`.

use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use protocol::{ClientMessage, ConversationId, ServerMessage, UserId};
use server::calls::{CallClaims, LiveKitConfig, room_name};
use server::{AppState, app, state::GROUP_ROOM};
use sqlx::PgPool;
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const PASSWORD: &str = "senha-boa-123";
const URL: &str = "ws://livekit.teste:7880";
const KEY: &str = "chave-de-teste";
const SECRET: &str = "segredo-de-teste-com-mais-de-32-caracteres";

fn livekit() -> LiveKitConfig {
    LiveKitConfig {
        url: URL.into(),
        api_key: KEY.into(),
        api_secret: SECRET.into(),
    }
}

async fn start_server(state: AppState) -> String {
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

async fn recv(socket: &mut Socket) -> ServerMessage {
    loop {
        match socket.next().await.unwrap().unwrap() {
            Message::Text(text) => return serde_json::from_str(&text).unwrap(),
            _ => continue,
        }
    }
}

async fn join(url: &str, username: &str) -> (Socket, UserId) {
    let (mut socket, _) = connect_async(url).await.unwrap();
    send(
        &mut socket,
        ClientMessage::Register {
            username: username.into(),
            password: PASSWORD.into(),
        },
    )
    .await;
    let ServerMessage::Welcome { user_id, .. } = recv(&mut socket).await else {
        panic!("esperava Welcome");
    };
    (socket, user_id)
}

async fn create_group(socket: &mut Socket, name: &str) -> ConversationId {
    send(socket, ClientMessage::CreateGroup { name: name.into() }).await;
    let ServerMessage::GroupCreated { conversation } = recv(socket).await else {
        panic!("esperava GroupCreated");
    };
    conversation.id
}

async fn join_call(socket: &mut Socket, conversation_id: ConversationId) -> ServerMessage {
    send(socket, ClientMessage::JoinCall { conversation_id }).await;
    recv(socket).await
}

/// Confere a assinatura com o segredo, como o LiveKit faz, e devolve o conteúdo.
fn decode(token: &str) -> CallClaims {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.set_issuer(&[KEY]);
    validation.validate_nbf = true;
    jsonwebtoken::decode::<CallClaims>(
        token,
        &DecodingKey::from_secret(SECRET.as_bytes()),
        &validation,
    )
    .expect("token deveria ser válido")
    .claims
}

#[sqlx::test]
async fn member_gets_a_valid_token_for_the_group_room(pool: PgPool) {
    let url = start_server(AppState::new(pool).with_livekit(livekit())).await;
    let (mut jobiss, jobiss_id) = join(&url, "jobiss").await;
    let group = create_group(&mut jobiss, "turma do cs").await;

    let reply = join_call(&mut jobiss, group).await;
    let ServerMessage::CallToken {
        conversation_id,
        url: livekit_url,
        token,
    } = reply
    else {
        panic!("esperava CallToken, veio {reply:?}");
    };
    assert_eq!(conversation_id, group);
    assert_eq!(livekit_url, URL);

    let claims = decode(&token);
    assert_eq!(claims.sub, jobiss_id.0.to_string());
    assert_eq!(claims.name, "jobiss");
    assert_eq!(claims.video.room, room_name(group));
    assert_eq!(claims.video.room, group.0.to_string());
    assert!(claims.video.room_join && claims.video.can_publish && claims.video.can_subscribe);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(claims.nbf <= now && claims.exp > now + 60 * 60);

    // O JSON tem os nomes que o LiveKit lê.
    let raw = serde_json::to_value(&claims).unwrap();
    assert_eq!(raw["video"]["roomJoin"], true);
    assert_eq!(raw["video"]["canPublish"], true);

    // Token assinado com outro segredo não passa.
    let wrong = jsonwebtoken::decode::<CallClaims>(
        &token,
        &DecodingKey::from_secret(b"outro-segredo-qualquer-com-32-caracteres"),
        &Validation::new(Algorithm::HS256),
    );
    assert!(wrong.is_err());
}

#[sqlx::test]
async fn non_member_gets_an_error(pool: PgPool) {
    let url = start_server(AppState::new(pool).with_livekit(livekit())).await;
    let (mut jobiss, _) = join(&url, "jobiss").await;
    let (mut nina, _) = join(&url, "nina").await;
    let group = create_group(&mut jobiss, "só eu").await;

    let reply = join_call(&mut nina, group).await;
    assert!(
        matches!(reply, ServerMessage::Error { ref reason } if reason == "Essa conversa não existe."),
        "{reply:?}"
    );

    // Nem uma conversa que não existe.
    let reply = join_call(&mut nina, ConversationId(uuid::Uuid::new_v4())).await;
    assert!(matches!(reply, ServerMessage::Error { .. }), "{reply:?}");

    // Mas a Sala do Grupo é de todo mundo.
    let reply = join_call(&mut nina, GROUP_ROOM).await;
    assert!(
        matches!(reply, ServerMessage::CallToken { .. }),
        "{reply:?}"
    );
}

#[sqlx::test]
async fn member_who_left_loses_access(pool: PgPool) {
    let url = start_server(AppState::new(pool).with_livekit(livekit())).await;
    let (mut jobiss, _) = join(&url, "jobiss").await;
    let (mut nina, _) = join(&url, "nina").await;
    let group = create_group(&mut jobiss, "turma").await;
    send(
        &mut jobiss,
        ClientMessage::InviteToGroup {
            conversation_id: group,
            username: "nina".into(),
        },
    )
    .await;
    // Nina recebe a conversa e a lista de membros.
    loop {
        if let ServerMessage::Members { .. } = recv(&mut nina).await {
            break;
        }
    }
    assert!(matches!(
        join_call(&mut nina, group).await,
        ServerMessage::CallToken { .. }
    ));

    send(
        &mut nina,
        ClientMessage::LeaveGroup {
            conversation_id: group,
        },
    )
    .await;
    loop {
        if let ServerMessage::ConversationRemoved { .. } = recv(&mut nina).await {
            break;
        }
    }
    assert!(matches!(
        join_call(&mut nina, group).await,
        ServerMessage::Error { .. }
    ));
}

#[sqlx::test]
async fn without_livekit_join_call_is_an_error(pool: PgPool) {
    let url = start_server(AppState::new(pool)).await;
    let (mut jobiss, _) = join(&url, "jobiss").await;
    let reply = join_call(&mut jobiss, GROUP_ROOM).await;
    assert!(
        matches!(reply, ServerMessage::Error { ref reason } if reason.contains("não estão configuradas")),
        "{reply:?}"
    );
}

//! Teste de ponta a ponta: sobe o servidor e conecta dois clientes de verdade.

use futures_util::{SinkExt, StreamExt};
use protocol::{ClientMessage, ServerMessage};
use server::{AppState, app};
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn start_server() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app(AppState::with_default_group()))
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

async fn join(url: &str, name: &str) -> (Socket, ServerMessage) {
    let (mut socket, _) = connect_async(url).await.unwrap();
    send(&mut socket, &ClientMessage::Hello { name: name.into() }).await;
    let welcome = recv(&mut socket).await;
    (socket, welcome)
}

#[tokio::test]
async fn message_from_one_client_reaches_the_other() {
    let url = start_server().await;
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
    assert_eq!(message.author_name, "Jobiss");
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

#[tokio::test]
async fn empty_message_is_rejected() {
    let url = start_server().await;
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

#[tokio::test]
async fn connection_without_hello_gets_an_error() {
    let url = start_server().await;
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
        ServerMessage::Error { .. }
    ));
}

fn uuid_v4() -> uuid::Uuid {
    uuid::Uuid::new_v4()
}

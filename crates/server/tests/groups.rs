//! Grupos: criar, convidar, sair e remover, com clientes de verdade.
//!
//! Precisa do PostgreSQL ligado, como em `chat.rs`.

use futures_util::{SinkExt, StreamExt};
use protocol::{ClientMessage, ConversationId, ConversationSummary, ServerMessage, UserId};
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

/// Alguém conectado: o socket, o id e as conversas do `Welcome`.
struct Person {
    socket: Socket,
    id: UserId,
    conversations: Vec<ConversationSummary>,
}

async fn enter(url: &str, first: ClientMessage) -> Person {
    let (mut socket, _) = connect_async(url).await.unwrap();
    send(&mut socket, first).await;
    let ServerMessage::Welcome {
        user_id,
        conversations,
        ..
    } = recv(&mut socket).await
    else {
        panic!("esperava Welcome");
    };
    Person {
        socket,
        id: user_id,
        conversations,
    }
}

async fn join(url: &str, username: &str) -> Person {
    enter(
        url,
        ClientMessage::Register {
            username: username.into(),
            password: PASSWORD.into(),
        },
    )
    .await
}

async fn login(url: &str, username: &str) -> Person {
    enter(
        url,
        ClientMessage::Login {
            username: username.into(),
            password: PASSWORD.into(),
        },
    )
    .await
}

async fn create_group(person: &mut Person, name: &str) -> ConversationId {
    send(
        &mut person.socket,
        ClientMessage::CreateGroup { name: name.into() },
    )
    .await;
    let ServerMessage::GroupCreated { conversation } = recv(&mut person.socket).await else {
        panic!("esperava GroupCreated");
    };
    assert_eq!(conversation.name, name.trim());
    conversation.id
}

async fn invite(person: &mut Person, conversation_id: ConversationId, username: &str) {
    send(
        &mut person.socket,
        ClientMessage::InviteToGroup {
            conversation_id,
            username: username.into(),
        },
    )
    .await;
}

async fn say(person: &mut Person, conversation_id: ConversationId, text: &str) {
    send(
        &mut person.socket,
        ClientMessage::SendMessage {
            conversation_id,
            text: text.into(),
        },
    )
    .await;
}

/// Espera um `Members` daquele grupo e devolve os nomes, dono primeiro.
async fn members(person: &mut Person, group: ConversationId) -> (Option<UserId>, Vec<String>) {
    let reply = recv(&mut person.socket).await;
    let ServerMessage::Members {
        conversation_id,
        owner_id,
        members,
    } = reply
    else {
        panic!("esperava Members, veio {reply:?}");
    };
    assert_eq!(conversation_id, group);
    (owner_id, members.into_iter().map(|m| m.username).collect())
}

async fn expect_text(person: &mut Person, text: &str) {
    let reply = recv(&mut person.socket).await;
    let ServerMessage::NewMessage { message } = reply else {
        panic!("esperava NewMessage, veio {reply:?}");
    };
    assert_eq!(message.text, text);
}

async fn expect_error(person: &mut Person) -> String {
    let reply = recv(&mut person.socket).await;
    let ServerMessage::Error { reason } = reply else {
        panic!("esperava Error, veio {reply:?}");
    };
    reason
}

#[sqlx::test]
async fn invited_person_sees_the_group_live_and_gets_its_messages(pool: PgPool) {
    let url = start_server(pool).await;
    let mut jobiss = join(&url, "jobiss").await;
    let mut nina = join(&url, "nina").await;
    let mut lia = join(&url, "lia").await;

    let group = create_group(&mut jobiss, "  turma do cs ").await;
    let (owner, names) = {
        send(
            &mut jobiss.socket,
            ClientMessage::ListMembers {
                conversation_id: group,
            },
        )
        .await;
        members(&mut jobiss, group).await
    };
    assert_eq!(owner, Some(jobiss.id));
    assert_eq!(names, ["jobiss"]);

    // O convite chega na hora para a Nina, sem ela reconectar.
    invite(&mut jobiss, group, "NINA").await;
    let ServerMessage::ConversationAdded { conversation } = recv(&mut nina.socket).await else {
        panic!("Nina deveria ver o grupo novo");
    };
    assert_eq!(conversation.id, group);
    assert_eq!(conversation.name, "turma do cs");
    assert_eq!(members(&mut nina, group).await.1, ["jobiss", "nina"]);
    assert_eq!(members(&mut jobiss, group).await.1, ["jobiss", "nina"]);

    // Qualquer membro convida, não só o dono.
    invite(&mut nina, group, "lia").await;
    let ServerMessage::ConversationAdded { .. } = recv(&mut lia.socket).await else {
        panic!("Lia deveria ver o grupo novo");
    };
    let jobiss_id = jobiss.id;
    for person in [&mut jobiss, &mut nina, &mut lia] {
        let (owner, names) = members(person, group).await;
        assert_eq!(owner, Some(jobiss_id));
        assert_eq!(names, ["jobiss", "nina", "lia"]);
    }

    say(&mut nina, group, "bora jogar").await;
    for person in [&mut jobiss, &mut nina, &mut lia] {
        expect_text(person, "bora jogar").await;
    }

    // E o grupo continua na lista depois de entrar de novo.
    let nina_again = login(&url, "nina").await;
    assert!(nina_again.conversations.iter().any(|c| c.id == group));
}

#[sqlx::test]
async fn whoever_leaves_stops_getting_messages(pool: PgPool) {
    let url = start_server(pool).await;
    let mut jobiss = join(&url, "jobiss").await;
    let mut nina = join(&url, "nina").await;

    let group = create_group(&mut jobiss, "turma").await;
    invite(&mut jobiss, group, "nina").await;
    recv(&mut nina.socket).await; // ConversationAdded
    members(&mut nina, group).await;
    members(&mut jobiss, group).await;

    send(
        &mut nina.socket,
        ClientMessage::LeaveGroup {
            conversation_id: group,
        },
    )
    .await;
    let reply = recv(&mut nina.socket).await;
    assert_eq!(
        reply,
        ServerMessage::ConversationRemoved {
            conversation_id: group
        }
    );
    assert_eq!(members(&mut jobiss, group).await.1, ["jobiss"]);

    // A mensagem do grupo não chega; a da Sala do Grupo, mandada depois, chega
    // primeiro.
    say(&mut jobiss, group, "ainda tem alguém?").await;
    say(&mut jobiss, GROUP_ROOM, "oi sala").await;
    expect_text(&mut nina, "oi sala").await;

    // Nem dá para abrir, escrever ou ver os membros.
    send(
        &mut nina.socket,
        ClientMessage::OpenConversation {
            conversation_id: group,
        },
    )
    .await;
    expect_error(&mut nina).await;
    say(&mut nina, group, "voltei?").await;
    expect_error(&mut nina).await;
    send(
        &mut nina.socket,
        ClientMessage::ListMembers {
            conversation_id: group,
        },
    )
    .await;
    expect_error(&mut nina).await;

    let nina_again = login(&url, "nina").await;
    assert!(nina_again.conversations.iter().all(|c| c.id != group));
}

#[sqlx::test]
async fn only_the_owner_removes_people(pool: PgPool) {
    let url = start_server(pool).await;
    let mut jobiss = join(&url, "jobiss").await;
    let mut nina = join(&url, "nina").await;

    let group = create_group(&mut jobiss, "turma").await;
    invite(&mut jobiss, group, "nina").await;
    recv(&mut nina.socket).await; // ConversationAdded
    members(&mut nina, group).await;
    members(&mut jobiss, group).await;

    send(
        &mut nina.socket,
        ClientMessage::RemoveFromGroup {
            conversation_id: group,
            user_id: jobiss.id,
        },
    )
    .await;
    assert_eq!(
        expect_error(&mut nina).await,
        "Só o dono do grupo pode remover pessoas."
    );

    send(
        &mut jobiss.socket,
        ClientMessage::RemoveFromGroup {
            conversation_id: group,
            user_id: nina.id,
        },
    )
    .await;
    assert_eq!(
        recv(&mut nina.socket).await,
        ServerMessage::ConversationRemoved {
            conversation_id: group
        }
    );
    assert_eq!(members(&mut jobiss, group).await.1, ["jobiss"]);

    say(&mut jobiss, group, "só eu").await;
    say(&mut jobiss, GROUP_ROOM, "oi sala").await;
    expect_text(&mut nina, "oi sala").await;
}

#[sqlx::test]
async fn owner_leaving_hands_the_group_to_the_oldest_member(pool: PgPool) {
    let url = start_server(pool.clone()).await;
    let mut jobiss = join(&url, "jobiss").await;
    let mut nina = join(&url, "nina").await;
    let mut lia = join(&url, "lia").await;

    let group = create_group(&mut jobiss, "turma").await;
    invite(&mut jobiss, group, "nina").await;
    recv(&mut nina.socket).await;
    members(&mut nina, group).await;
    members(&mut jobiss, group).await;
    invite(&mut jobiss, group, "lia").await;
    recv(&mut lia.socket).await;
    for person in [&mut jobiss, &mut nina, &mut lia] {
        members(person, group).await;
    }

    send(
        &mut jobiss.socket,
        ClientMessage::LeaveGroup {
            conversation_id: group,
        },
    )
    .await;
    recv(&mut jobiss.socket).await; // ConversationRemoved
    let (owner, names) = members(&mut nina, group).await;
    assert_eq!(owner, Some(nina.id));
    assert_eq!(names, ["nina", "lia"]);
    members(&mut lia, group).await;

    // A Nina agora pode remover; quando a última pessoa sai, o grupo some.
    send(
        &mut nina.socket,
        ClientMessage::RemoveFromGroup {
            conversation_id: group,
            user_id: lia.id,
        },
    )
    .await;
    recv(&mut lia.socket).await; // ConversationRemoved
    members(&mut nina, group).await;
    send(
        &mut nina.socket,
        ClientMessage::LeaveGroup {
            conversation_id: group,
        },
    )
    .await;
    recv(&mut nina.socket).await; // ConversationRemoved

    let (left,): (i64,) = sqlx::query_as("SELECT count(*) FROM conversations WHERE id = $1")
        .bind(group.0)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

#[sqlx::test]
async fn bad_group_requests_get_an_error(pool: PgPool) {
    let url = start_server(pool).await;
    let mut jobiss = join(&url, "jobiss").await;
    let mut nina = join(&url, "nina").await;
    let group = create_group(&mut jobiss, "turma").await;

    // Nome vazio ou comprido demais.
    for name in ["   ", &"a".repeat(41)] {
        send(
            &mut jobiss.socket,
            ClientMessage::CreateGroup { name: name.into() },
        )
        .await;
        expect_error(&mut jobiss).await;
    }

    // Usuário que não existe, e quem já está no grupo.
    invite(&mut jobiss, group, "ninguem").await;
    assert_eq!(
        expect_error(&mut jobiss).await,
        "Ninguém com o usuário \"ninguem\"."
    );
    invite(&mut jobiss, group, "jobiss").await;
    assert_eq!(expect_error(&mut jobiss).await, "jobiss já está no grupo.");

    // Quem está fora não convida nem se coloca no grupo.
    invite(&mut nina, group, "nina").await;
    expect_error(&mut nina).await;

    // A Sala do Grupo não tem dono: ninguém sai nem convida.
    send(
        &mut nina.socket,
        ClientMessage::LeaveGroup {
            conversation_id: GROUP_ROOM,
        },
    )
    .await;
    expect_error(&mut nina).await;
    invite(&mut nina, GROUP_ROOM, "jobiss").await;
    expect_error(&mut nina).await;

    // O dono não se remove: para isso existe "sair do grupo".
    send(
        &mut jobiss.socket,
        ClientMessage::RemoveFromGroup {
            conversation_id: group,
            user_id: jobiss.id,
        },
    )
    .await;
    expect_error(&mut jobiss).await;
}

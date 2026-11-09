//! Vários aparelhos na mesma conta: o aparelho novo espera aprovação, e
//! depois cada aparelho é um membro próprio do grupo MLS, então o que um
//! manda o outro lê.

use std::time::Duration;

use crypto::{Group, Identity, Received};
use futures_util::{SinkExt, StreamExt};
use protocol::{
    ClientMessage, ConversationId, ConversationSummary, DeviceId, Sealed, ServerMessage, UserId,
};
use server::{AppState, app};
use sqlx::PgPool;
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};
use uuid::Uuid;

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

async fn send(socket: &mut Socket, msg: &ClientMessage) {
    let json = serde_json::to_string(msg).unwrap();
    socket.send(Message::Text(json.into())).await.unwrap();
}

/// A próxima mensagem; `None` quando a conexão fechou.
async fn recv(socket: &mut Socket) -> Option<ServerMessage> {
    loop {
        match socket.next().await? {
            Ok(Message::Text(text)) => return Some(serde_json::from_str(&text).unwrap()),
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => continue,
        }
    }
}

/// Um aparelho com o app aberto.
struct Device {
    socket: Socket,
    install: Uuid,
    user: UserId,
    device: DeviceId,
    token: String,
    code: Option<String>,
    conversations: Vec<ConversationSummary>,
    me: Identity,
    groups: std::collections::HashMap<ConversationId, Group>,
}

impl Device {
    /// Conecta, manda o `Hello` e `first`, e espera o `Welcome`.
    async fn enter(url: &str, install: Uuid, first: ClientMessage) -> Self {
        let (mut socket, _) = connect_async(url).await.unwrap();
        send(
            &mut socket,
            &ClientMessage::Hello {
                install,
                name: "pc de teste".into(),
            },
        )
        .await;
        send(&mut socket, &first).await;
        loop {
            match recv(&mut socket)
                .await
                .expect("conexão fechou antes do Welcome")
            {
                ServerMessage::Welcome {
                    user_id,
                    token,
                    conversations,
                    device_id,
                    approval_code,
                    ..
                } => {
                    let device = device_id.unwrap();
                    let me = Identity::in_memory(&format!("{}:{}", user_id.0, device.0)).unwrap();
                    return Self {
                        socket,
                        install,
                        user: user_id,
                        device,
                        token,
                        code: approval_code,
                        conversations,
                        me,
                        groups: Default::default(),
                    };
                }
                ServerMessage::AuthError { reason } => panic!("não entrou: {reason}"),
                _ => continue,
            }
        }
    }

    async fn register(url: &str, username: &str) -> Self {
        Self::enter(
            url,
            Uuid::new_v4(),
            ClientMessage::Register {
                username: username.into(),
                password: PASSWORD.into(),
                email: Some(format!("{username}@teste.com")),
            },
        )
        .await
    }

    async fn login(url: &str, username: &str) -> Self {
        Self::enter(
            url,
            Uuid::new_v4(),
            ClientMessage::Login {
                username: username.into(),
                password: PASSWORD.into(),
            },
        )
        .await
    }

    /// Volta com o mesmo token e a mesma instalação (o app reabrindo).
    async fn resume(self, url: &str) -> Self {
        let mut again = Self::enter(
            url,
            self.install,
            ClientMessage::Resume {
                token: self.token.clone(),
            },
        )
        .await;
        again.me = self.me;
        again
    }

    async fn send(&mut self, msg: ClientMessage) {
        send(&mut self.socket, &msg).await;
    }

    async fn wait<T>(&mut self, mut pick: impl FnMut(ServerMessage) -> Option<T>) -> T {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let msg = recv(&mut self.socket).await.expect("a conexão fechou");
                if let Some(found) = pick(msg) {
                    return found;
                }
            }
        })
        .await
        .expect("a mensagem esperada não chegou")
    }

    /// Espera a conexão fechar (descartando o que vier antes).
    async fn closed(&mut self) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while recv(&mut self.socket).await.is_some() {}
        })
        .await
        .expect("a conexão não fechou");
    }

    async fn upload_packages(&mut self, n: usize) {
        let packages = (0..n)
            .map(|_| Sealed(self.me.key_package().unwrap()))
            .collect();
        self.send(ClientMessage::UploadKeyPackages {
            packages,
            replace: true,
        })
        .await;
        self.wait(|m| match m {
            ServerMessage::KeyPackageStock { count } if count as usize >= n => Some(()),
            _ => None,
        })
        .await;
    }

    async fn commit_ok(&mut self, conversation: ConversationId) {
        self.wait(|m| match m {
            ServerMessage::CommitAccepted {
                conversation_id, ..
            } if conversation_id == conversation => Some(()),
            ServerMessage::CommitRejected { .. } => panic!("commit recusado"),
            ServerMessage::Error { reason } => panic!("erro do servidor: {reason}"),
            _ => None,
        })
        .await;
    }

    async fn accept_welcome(&mut self, conversation: ConversationId) {
        let welcome = self
            .wait(|m| match m {
                ServerMessage::GroupWelcome {
                    conversation_id,
                    welcome,
                    ..
                } if conversation_id == conversation => Some(welcome),
                _ => None,
            })
            .await;
        let group = self.me.join(&welcome.0).unwrap();
        self.groups.insert(conversation, group);
    }

    async fn say(&mut self, conversation: ConversationId, text: &str) {
        let sealed = self
            .groups
            .get_mut(&conversation)
            .unwrap()
            .encrypt(&self.me, text.as_bytes())
            .unwrap();
        self.send(ClientMessage::SendMessage {
            conversation_id: conversation,
            sealed: Sealed(sealed),
        })
        .await;
    }

    /// A próxima mensagem de texto da conversa que este aparelho não mandou,
    /// decifrada: (quem assinou no MLS, texto).
    async fn hear(&mut self, conversation: ConversationId, author: UserId) -> (String, String) {
        let message = self
            .wait(|m| match m {
                ServerMessage::NewMessage { message }
                    if message.conversation_id == conversation && message.author_id == author =>
                {
                    Some(message)
                }
                _ => None,
            })
            .await;
        let group = self.groups.get_mut(&conversation).unwrap();
        let Received::Text { from, text } =
            group.receive(&self.me, &message.sealed.unwrap().0).unwrap()
        else {
            panic!("esperava texto");
        };
        (from, String::from_utf8(text).unwrap())
    }

    async fn devices(
        &mut self,
        mut ready: impl FnMut(&[protocol::DeviceInfo]) -> bool,
    ) -> Vec<protocol::DeviceInfo> {
        self.wait(|m| match m {
            ServerMessage::Devices { devices } if ready(&devices) => Some(devices),
            _ => None,
        })
        .await
    }
}

#[sqlx::test]
async fn aparelho_novo_espera_aprovacao_e_depois_le_o_que_o_outro_manda(pool: PgPool) {
    let url = start_server(pool).await;
    let mut sala = Device::register(&url, "ana").await;
    // O primeiro aparelho da conta entra direto.
    assert_eq!(sala.code, None);
    let mut bia = Device::register(&url, "bia").await;

    // O notebook entra com a senha e fica esperando, sem ver conversa.
    let mut note = Device::login(&url, "ana").await;
    assert_eq!(note.user, sala.user);
    assert_ne!(note.device, sala.device);
    let code = note.code.clone().expect("o aparelho novo espera aprovação");
    assert_eq!(code.len(), 6);
    assert!(note.conversations.is_empty());
    note.send(ClientMessage::OpenDirect {
        username: "bia".into(),
    })
    .await;
    note.wait(|m| matches!(m, ServerMessage::Error { .. }).then_some(()))
        .await;

    // A sala vê o pedido, com o mesmo código.
    let note_id = note.device;
    let devices = sala
        .devices(|d| d.iter().any(|d| d.id == note_id && !d.approved))
        .await;
    let pending = devices.iter().find(|d| d.id == note_id).unwrap();
    assert_eq!(pending.code.as_deref(), Some(code.as_str()));
    // Bia não é da conta: não pode aprovar o notebook de Ana.
    bia.send(ClientMessage::ApproveDevice { device_id: note_id })
        .await;
    bia.wait(|m| matches!(m, ServerMessage::Error { .. }).then_some(()))
        .await;

    sala.send(ClientMessage::ApproveDevice { device_id: note_id })
        .await;
    note.wait(|m| matches!(m, ServerMessage::DeviceApproved).then_some(()))
        .await;
    note.closed().await;
    let mut note = note.resume(&url).await;
    assert_eq!(note.device, note_id);
    assert_eq!(note.code, None);
    assert!(!note.conversations.is_empty());

    // Os três aparelhos publicam chaves; a sala cria a conversa com Bia e
    // põe os outros dois aparelhos (o notebook e o de Bia).
    for d in [&mut sala, &mut note, &mut bia] {
        d.upload_packages(3).await;
    }
    sala.send(ClientMessage::OpenDirect {
        username: "bia".into(),
    })
    .await;
    let direct = sala
        .wait(|m| match m {
            ServerMessage::DirectOpened { conversation } => Some(conversation.id),
            _ => None,
        })
        .await;
    let (sala_id, bia_id) = (sala.device, bia.device);
    let pending_add = sala
        .wait(|m| match m {
            ServerMessage::GroupState {
                conversation_id,
                pending_add,
                ..
            } if conversation_id == direct && pending_add.len() == 3 => Some(pending_add),
            _ => None,
        })
        .await;
    for d in [sala_id, note_id, bia_id] {
        assert!(pending_add.contains(&d), "{pending_add:?}");
    }
    let group = sala.me.create_group(direct.0.as_bytes()).unwrap();
    sala.groups.insert(direct, group);
    sala.send(ClientMessage::Commit {
        conversation_id: direct,
        epoch: 0,
        commit: None,
        welcome: None,
        added: vec![],
        removed: vec![],
    })
    .await;
    sala.commit_ok(direct).await;
    sala.send(ClientMessage::ClaimKeyPackages {
        conversation_id: direct,
        device_ids: vec![note_id, bia_id],
    })
    .await;
    let packages = sala
        .wait(|m| match m {
            ServerMessage::KeyPackages { packages, .. } => Some(packages),
            _ => None,
        })
        .await;
    assert_eq!(packages.len(), 2);
    let me = &sala.me;
    let pending = sala
        .groups
        .get_mut(&direct)
        .unwrap()
        .commit(
            me,
            &packages
                .iter()
                .map(|p| p.package.0.clone())
                .collect::<Vec<_>>(),
            &[],
        )
        .unwrap();
    sala.send(ClientMessage::Commit {
        conversation_id: direct,
        epoch: 0,
        commit: Some(Sealed(pending.commit)),
        welcome: pending.welcome.map(Sealed),
        added: packages.iter().map(|p| p.device_id).collect(),
        removed: vec![],
    })
    .await;
    sala.commit_ok(direct).await;
    let me = &sala.me;
    sala.groups.get_mut(&direct).unwrap().confirm(me).unwrap();
    note.accept_welcome(direct).await;
    bia.accept_welcome(direct).await;

    // O que a sala manda, o notebook (mesma conta) e Bia leem.
    let ana = sala.user;
    sala.say(direct, "mandei do pc da sala").await;
    let signed_by = format!("{}:{}", ana.0, sala_id.0);
    assert_eq!(
        note.hear(direct, ana).await,
        (signed_by.clone(), "mandei do pc da sala".into())
    );
    assert_eq!(
        bia.hear(direct, ana).await,
        (signed_by, "mandei do pc da sala".into())
    );
    // E o que Bia responde chega nos dois aparelhos de Ana.
    bia.say(direct, "chegou nos dois?").await;
    let bia_user = bia.user;
    assert_eq!(note.hear(direct, bia_user).await.1, "chegou nos dois?");
    assert_eq!(sala.hear(direct, bia_user).await.1, "chegou nos dois?");

    // A sala desconecta o notebook: ele cai e fica para sair do grupo.
    sala.send(ClientMessage::RemoveDevice { device_id: note_id })
        .await;
    note.wait(|m| matches!(m, ServerMessage::DeviceRemoved).then_some(()))
        .await;
    note.closed().await;
    sala.wait(|m| match m {
        ServerMessage::GroupState {
            conversation_id,
            pending_remove,
            ..
        } if conversation_id == direct && pending_remove == [note_id] => Some(()),
        _ => None,
    })
    .await;
    // E o token dele não vale mais.
    let (mut socket, _) = connect_async(&url).await.unwrap();
    send(
        &mut socket,
        &ClientMessage::Hello {
            install: note.install,
            name: "x".into(),
        },
    )
    .await;
    send(&mut socket, &ClientMessage::Resume { token: note.token }).await;
    assert!(matches!(
        recv(&mut socket).await,
        Some(ServerMessage::AuthError { .. })
    ));
}

#[sqlx::test]
async fn sem_o_outro_aparelho_entra_desconectando_os_outros(pool: PgPool) {
    let url = start_server(pool).await;
    let mut antigo = Device::register(&url, "ana").await;
    let mut novo = Device::login(&url, "ana").await;
    assert!(novo.code.is_some());
    // Um aparelho esperando não pode tirar ninguém.
    let antigo_id = antigo.device;
    novo.send(ClientMessage::RemoveDevice {
        device_id: antigo_id,
    })
    .await;
    novo.wait(|m| matches!(m, ServerMessage::Error { .. }).then_some(()))
        .await;

    novo.send(ClientMessage::TakeOverAccount).await;
    novo.wait(|m| matches!(m, ServerMessage::DeviceApproved).then_some(()))
        .await;
    antigo
        .wait(|m| matches!(m, ServerMessage::DeviceRemoved).then_some(()))
        .await;
    antigo.closed().await;
    novo.closed().await;
    let mut novo = novo.resume(&url).await;
    assert_eq!(novo.code, None);
    let novo_id = novo.device;
    let devices = novo.devices(|d| d.len() == 1).await;
    assert_eq!(devices[0].id, novo_id);
}

#[sqlx::test]
async fn token_de_um_aparelho_nao_serve_em_outra_instalacao(pool: PgPool) {
    let url = start_server(pool).await;
    let ana = Device::register(&url, "ana").await;
    let (mut socket, _) = connect_async(&url).await.unwrap();
    send(
        &mut socket,
        &ClientMessage::Hello {
            install: Uuid::new_v4(),
            name: "copiou o token".into(),
        },
    )
    .await;
    send(&mut socket, &ClientMessage::Resume { token: ana.token }).await;
    assert!(matches!(
        recv(&mut socket).await,
        Some(ServerMessage::AuthError { .. })
    ));
}

#[sqlx::test]
async fn app_antigo_que_se_atualiza_continua_o_mesmo_aparelho(pool: PgPool) {
    let url = start_server(pool).await;
    // O app antigo entra sem `Hello`: o aparelho dele tem o id da conta.
    let (mut socket, _) = connect_async(&url).await.unwrap();
    send(
        &mut socket,
        &ClientMessage::Register {
            username: "nina".into(),
            password: PASSWORD.into(),
            email: Some("nina@teste.com".into()),
        },
    )
    .await;
    let (user, token, device) = loop {
        if let Some(ServerMessage::Welcome {
            user_id,
            token,
            device_id,
            ..
        }) = recv(&mut socket).await
        {
            break (user_id, token, device_id.unwrap());
        }
    };
    assert_eq!(device.0, user.0);
    drop(socket);

    // Atualizado, volta com o mesmo token e agora diz a instalação: é o
    // mesmo aparelho (os grupos dele continuam) e não espera aprovação.
    let updated = Device::enter(&url, Uuid::new_v4(), ClientMessage::Resume { token }).await;
    assert_eq!(updated.device, device);
    assert_eq!(updated.code, None);
}

//! F2-08: o servidor entrega pacotes de chaves, commits e welcomes do MLS e
//! guarda só texto cifrado. Os clientes daqui usam a crate `crypto` de
//! verdade, como o app.

use std::time::Duration;

use crypto::{Group, Identity, Received};
use futures_util::{SinkExt, StreamExt};
use protocol::{
    ClientMessage, ConversationId, ConversationSummary, DeviceId, EpochCommit, Sealed,
    ServerMessage, UserId,
};
use server::{AppState, app, state::GROUP_ROOM};
use sqlx::PgPool;
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

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

/// Uma pessoa com o app aberto: conexão, identidade MLS e os grupos dela.
struct Person {
    socket: Socket,
    id: UserId,
    me: Identity,
    groups: std::collections::HashMap<ConversationId, Group>,
}

impl Person {
    async fn register(url: &str, username: &str) -> Self {
        let (mut socket, _) = connect_async(url).await.unwrap();
        send(
            &mut socket,
            &ClientMessage::Register {
                username: username.into(),
                password: "senha-boa-123".into(),
                email: Some(format!("{}@teste.com", username)),
            },
        )
        .await;
        let id = loop {
            if let ServerMessage::Welcome { user_id, .. } = recv(&mut socket).await {
                break user_id;
            }
        };
        let me = Identity::in_memory(&id.0.to_string()).unwrap();
        Self {
            socket,
            id,
            me,
            groups: Default::default(),
        }
    }

    async fn send(&mut self, msg: ClientMessage) {
        send(&mut self.socket, &msg).await;
    }

    /// A próxima mensagem que satisfaz `pick`; as outras são descartadas.
    async fn wait<T>(&mut self, mut pick: impl FnMut(ServerMessage) -> Option<T>) -> T {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(found) = pick(recv(&mut self.socket).await) {
                    return found;
                }
            }
        })
        .await
        .expect("a mensagem esperada não chegou")
    }

    async fn upload_packages(&mut self, n: usize) {
        let packages = (0..n)
            .map(|_| Sealed(self.me.key_package().unwrap()))
            .collect();
        self.send(ClientMessage::UploadKeyPackages {
            packages,
            replace: false,
        })
        .await;
        // O login já manda um `KeyPackageStock` (com 0): espera o que conta
        // os pacotes que acabaram de subir.
        self.wait(|m| match m {
            ServerMessage::KeyPackageStock { count } if count as usize >= n => Some(()),
            _ => None,
        })
        .await;
    }

    /// Espera o `GroupState` da conversa que satisfaz `ready`.
    async fn group_state(
        &mut self,
        conversation: ConversationId,
        mut ready: impl FnMut(&ServerMessage) -> bool,
    ) -> ServerMessage {
        self.wait(|m| {
            (matches!(&m, ServerMessage::GroupState { conversation_id, .. } if *conversation_id == conversation)
                && ready(&m))
                .then_some(m)
        })
        .await
    }

    /// Cria o grupo MLS da conversa e põe quem estiver pendente, como o app
    /// faz quando é o `committer`.
    async fn create_and_add(&mut self, conversation: ConversationId, others: &[UserId]) {
        let group = self.me.create_group(conversation.0.as_bytes()).unwrap();
        self.groups.insert(conversation, group);
        self.send(ClientMessage::Commit {
            conversation_id: conversation,
            epoch: 0,
            commit: None,
            welcome: None,
            added: vec![],
            removed: vec![],
        })
        .await;
        let accepted = self.commit_reply(conversation).await;
        assert_eq!(
            accepted,
            ServerMessage::CommitAccepted {
                conversation_id: conversation,
                epoch: 0
            }
        );
        self.add(conversation, others).await;
    }

    async fn add(&mut self, conversation: ConversationId, others: &[UserId]) {
        if others.is_empty() {
            return;
        }
        self.send(ClientMessage::ClaimKeyPackages {
            conversation_id: conversation,
            device_ids: others.iter().map(|u| dev(*u)).collect(),
        })
        .await;
        let packages = self
            .wait(|m| match m {
                ServerMessage::KeyPackages { packages, .. } => Some(packages),
                _ => None,
            })
            .await;
        assert_eq!(packages.len(), others.len());
        let group = self.groups.get_mut(&conversation).unwrap();
        let epoch = group.epoch();
        let pending = group
            .commit(
                &self.me,
                &packages
                    .iter()
                    .map(|p| p.package.0.clone())
                    .collect::<Vec<_>>(),
                &[],
            )
            .unwrap();
        self.send(ClientMessage::Commit {
            conversation_id: conversation,
            epoch,
            commit: Some(Sealed(pending.commit)),
            welcome: pending.welcome.map(Sealed),
            added: packages.iter().map(|p| p.device_id).collect(),
            removed: vec![],
        })
        .await;
        let reply = self.commit_reply(conversation).await;
        assert_eq!(
            reply,
            ServerMessage::CommitAccepted {
                conversation_id: conversation,
                epoch: epoch + 1
            }
        );
        self.groups
            .get_mut(&conversation)
            .unwrap()
            .confirm(&self.me)
            .unwrap();
    }

    async fn commit_reply(&mut self, conversation: ConversationId) -> ServerMessage {
        self.wait(|m| match &m {
            ServerMessage::CommitAccepted {
                conversation_id, ..
            }
            | ServerMessage::CommitRejected {
                conversation_id, ..
            } if *conversation_id == conversation => Some(m),
            ServerMessage::Error { reason } => panic!("erro do servidor: {reason}"),
            _ => None,
        })
        .await
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

    /// A próxima mensagem de texto da conversa, decifrada: (autor, texto).
    async fn hear(&mut self, conversation: ConversationId) -> (UserId, String) {
        let me = self.id;
        // A própria mensagem também volta (o MLS não decifra a própria).
        let message = self
            .wait(|m| match m {
                ServerMessage::NewMessage { message }
                    if message.conversation_id == conversation && message.author_id != me =>
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
        // Quem assinou no MLS é quem o servidor diz que mandou.
        assert_eq!(from, message.author_id.0.to_string());
        (message.author_id, String::from_utf8(text).unwrap())
    }
}

async fn send(socket: &mut Socket, msg: &ClientMessage) {
    let json = serde_json::to_string(msg).unwrap();
    socket.send(Message::Text(json.into())).await.unwrap();
}

async fn recv(socket: &mut Socket) -> ServerMessage {
    loop {
        if let Message::Text(text) = socket.next().await.unwrap().unwrap() {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

fn state_parts(
    state: &ServerMessage,
) -> (u64, Vec<UserId>, Vec<UserId>, Vec<UserId>, Option<UserId>) {
    let ServerMessage::GroupState {
        epoch,
        members,
        pending_add,
        pending_remove,
        committer,
        ..
    } = state
    else {
        panic!("esperava GroupState");
    };
    // Sem `Hello` (como um app antigo), o aparelho tem o id da conta.
    let users = |devices: &[DeviceId]| devices.iter().map(|d| UserId(d.0)).collect();
    (
        *epoch,
        users(members),
        users(pending_add),
        users(pending_remove),
        committer.map(|d| UserId(d.0)),
    )
}

/// O aparelho de quem entrou sem `Hello`: o mesmo id da conta.
fn dev(user: UserId) -> DeviceId {
    DeviceId(user.0)
}

/// Abre a conversa direta e devolve o id.
async fn open_direct(person: &mut Person, username: &str) -> ConversationId {
    person
        .send(ClientMessage::OpenDirect {
            username: username.into(),
        })
        .await;
    person
        .wait(|m| match m {
            ServerMessage::DirectOpened {
                conversation: ConversationSummary { id, .. },
            } => Some(id),
            _ => None,
        })
        .await
}

#[sqlx::test]
async fn conversa_direta_cifrada_de_ponta_a_ponta(pool: PgPool) {
    let url = start_server(pool.clone()).await;
    let mut ana = Person::register(&url, "ana").await;
    let mut bia = Person::register(&url, "bia").await;
    ana.upload_packages(3).await;
    bia.upload_packages(3).await;

    let direct = open_direct(&mut ana, "bia").await;
    // O servidor diz: o grupo não existe, Bia (e Ana) podem entrar, e quem
    // cria é a de menor id entre as duas online.
    let state = ana
        .group_state(direct, |s| state_parts(s).2.len() == 2)
        .await;
    let (epoch, members, pending_add, _, committer) = state_parts(&state);
    assert_eq!(epoch, 0);
    assert!(members.is_empty());
    assert_eq!(committer, Some(ana.id.min_by_uuid(bia.id)));

    // Quem quer que seja o escolhido, aqui Ana faz a criação.
    let others: Vec<UserId> = pending_add.into_iter().filter(|u| *u != ana.id).collect();
    assert_eq!(others, [bia.id]);
    ana.create_and_add(direct, &others).await;
    bia.accept_welcome(direct).await;

    // O estado agora tem as duas e nada pendente.
    let state = bia.group_state(direct, |s| state_parts(s).0 == 1).await;
    let (_, members, pending_add, pending_remove, _) = state_parts(&state);
    assert_eq!(members.len(), 2);
    assert!(pending_add.is_empty() && pending_remove.is_empty());

    ana.say(direct, "segredo entre nós duas").await;
    assert_eq!(
        bia.hear(direct).await,
        (ana.id, "segredo entre nós duas".into())
    );
    bia.say(direct, "ninguém mais lê").await;
    assert_eq!(ana.hear(direct).await, (bia.id, "ninguém mais lê".into()));

    // O que a F2-08 promete: olhando a tabela, não dá para ler nada.
    let rows: Vec<(Option<Vec<u8>>,)> = sqlx::query_as("SELECT sealed FROM messages")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    for (sealed,) in rows {
        let sealed = sealed.unwrap();
        for plain in ["segredo", "ninguém"] {
            assert!(!sealed.windows(plain.len()).any(|w| w == plain.as_bytes()));
        }
    }
    let columns: Vec<(String,)> = sqlx::query_as(
        "SELECT column_name::TEXT FROM information_schema.columns WHERE table_name = 'messages'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(!columns.iter().any(|(c,)| c == "text"), "{columns:?}");
}

trait MinByUuid {
    fn min_by_uuid(self, other: Self) -> Self;
}

impl MinByUuid for UserId {
    fn min_by_uuid(self, other: Self) -> Self {
        if self.0 <= other.0 { self } else { other }
    }
}

#[sqlx::test]
async fn commit_de_epoca_velha_e_recusado_e_quem_atrasou_alcanca(pool: PgPool) {
    let url = start_server(pool).await;
    let mut ana = Person::register(&url, "ana").await;
    let mut bia = Person::register(&url, "bia").await;
    let mut caio = Person::register(&url, "caio").await;
    for p in [&mut ana, &mut bia, &mut caio] {
        p.upload_packages(3).await;
    }
    ana.send(ClientMessage::CreateGroup {
        name: "turma".into(),
    })
    .await;
    let group = ana
        .wait(|m| match m {
            ServerMessage::GroupCreated { conversation } => Some(conversation.id),
            _ => None,
        })
        .await;
    for name in ["bia", "caio"] {
        ana.send(ClientMessage::InviteToGroup {
            conversation_id: group,
            username: name.into(),
        })
        .await;
    }
    ana.group_state(group, |s| state_parts(s).2.len() == 3)
        .await;
    ana.create_and_add(group, &[bia.id]).await;
    bia.accept_welcome(group).await;
    // Época 1: Ana e Bia. Ana põe Caio (época 1 → 2).
    ana.add(group, &[caio.id]).await;
    caio.accept_welcome(group).await;

    // Bia, ainda na época 1, tenta mudar o grupo: recusado com a época atual.
    bia.send(ClientMessage::Commit {
        conversation_id: group,
        epoch: 1,
        commit: Some(Sealed(vec![1, 2, 3])),
        welcome: None,
        added: vec![],
        removed: vec![dev(caio.id)],
    })
    .await;
    assert_eq!(
        bia.commit_reply(group).await,
        ServerMessage::CommitRejected {
            conversation_id: group,
            epoch: 2
        }
    );

    // Bia pede o que perdeu e alcança a época 2.
    bia.send(ClientMessage::FetchCommits {
        conversation_id: group,
        from: 1,
    })
    .await;
    let commits: Vec<EpochCommit> = bia
        .wait(|m| match m {
            ServerMessage::Commits { commits, .. } => Some(commits),
            _ => None,
        })
        .await;
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].epoch, 1);
    let bia_group = bia.groups.get_mut(&group).unwrap();
    bia_group.receive(&bia.me, &commits[0].commit.0).unwrap();
    assert_eq!(bia_group.epoch(), 2);

    caio.say(group, "cheguei").await;
    assert_eq!(ana.hear(group).await.1, "cheguei");
    assert_eq!(bia.hear(group).await.1, "cheguei");
}

#[sqlx::test]
async fn quem_sai_do_grupo_fica_pendente_para_sair_do_mls(pool: PgPool) {
    let url = start_server(pool).await;
    let mut ana = Person::register(&url, "ana").await;
    let mut bia = Person::register(&url, "bia").await;
    ana.upload_packages(2).await;
    bia.upload_packages(2).await;
    ana.send(ClientMessage::CreateGroup {
        name: "dupla".into(),
    })
    .await;
    let group = ana
        .wait(|m| match m {
            ServerMessage::GroupCreated { conversation } => Some(conversation.id),
            _ => None,
        })
        .await;
    ana.send(ClientMessage::InviteToGroup {
        conversation_id: group,
        username: "bia".into(),
    })
    .await;
    ana.group_state(group, |s| state_parts(s).2.contains(&bia.id))
        .await;
    ana.create_and_add(group, &[bia.id]).await;
    bia.accept_welcome(group).await;

    bia.send(ClientMessage::LeaveGroup {
        conversation_id: group,
    })
    .await;
    let state = ana
        .group_state(group, |s| !state_parts(s).3.is_empty())
        .await;
    let (_, _, _, pending_remove, committer) = state_parts(&state);
    assert_eq!(pending_remove, [bia.id]);
    assert_eq!(committer, Some(ana.id));

    // Ana tira Bia do MLS; quem saiu não recebe mais nada.
    let group_ana = ana.groups.get_mut(&group).unwrap();
    let epoch = group_ana.epoch();
    let pending = group_ana
        .commit(&ana.me, &[], &[bia.id.0.to_string()])
        .unwrap();
    ana.send(ClientMessage::Commit {
        conversation_id: group,
        epoch,
        commit: Some(Sealed(pending.commit)),
        welcome: None,
        added: vec![],
        removed: vec![dev(bia.id)],
    })
    .await;
    assert!(matches!(
        ana.commit_reply(group).await,
        ServerMessage::CommitAccepted { .. }
    ));
    let state = ana
        .group_state(group, |s| state_parts(s).0 == epoch + 1)
        .await;
    let (_, members, _, pending_remove, _) = state_parts(&state);
    assert_eq!(members, [ana.id]);
    assert!(pending_remove.is_empty());
}

#[sqlx::test]
async fn so_quem_esta_no_grupo_pega_pacotes_e_commits(pool: PgPool) {
    let url = start_server(pool).await;
    let mut ana = Person::register(&url, "ana").await;
    let mut bia = Person::register(&url, "bia").await;
    bia.upload_packages(1).await;

    // A Sala do Grupo ainda não tem grupo MLS: Ana não está nele.
    ana.send(ClientMessage::ClaimKeyPackages {
        conversation_id: GROUP_ROOM,
        device_ids: vec![dev(bia.id)],
    })
    .await;
    assert!(
        ana.wait(|m| match m {
            ServerMessage::Error { .. } => Some(true),
            ServerMessage::KeyPackages { .. } => Some(false),
            _ => None,
        })
        .await
    );
    ana.send(ClientMessage::FetchCommits {
        conversation_id: GROUP_ROOM,
        from: 0,
    })
    .await;
    ana.wait(|m| matches!(m, ServerMessage::Error { .. }).then_some(()))
        .await;

    // Criar de novo um grupo que já existe é recusado.
    ana.create_and_add(GROUP_ROOM, &[bia.id]).await;
    bia.send(ClientMessage::Commit {
        conversation_id: GROUP_ROOM,
        epoch: 0,
        commit: None,
        welcome: None,
        added: vec![],
        removed: vec![],
    })
    .await;
    assert_eq!(
        bia.commit_reply(GROUP_ROOM).await,
        ServerMessage::CommitRejected {
            conversation_id: GROUP_ROOM,
            epoch: 1
        }
    );

    // Pacote de chaves só sai uma vez: Bia publicou um e ele foi usado.
    ana.send(ClientMessage::ClaimKeyPackages {
        conversation_id: GROUP_ROOM,
        device_ids: vec![dev(bia.id)],
    })
    .await;
    let packages = ana
        .wait(|m| match m {
            ServerMessage::KeyPackages { packages, .. } => Some(packages),
            _ => None,
        })
        .await;
    assert!(packages.is_empty());
}

#[sqlx::test]
async fn welcome_guardado_chega_ao_entrar_de_novo(pool: PgPool) {
    let url = start_server(pool).await;
    let mut ana = Person::register(&url, "ana").await;
    let mut bia = Person::register(&url, "bia").await;
    bia.upload_packages(1).await;
    // Bia fecha o app; a identidade (o "arquivo de estado") fica.
    let Person { socket, me, id, .. } = bia;
    drop(socket);

    // Ana, sozinha online, cria o grupo da Sala e põe Bia. Ela tem pacotes,
    // como todo app com MLS: é o que a faz poder criar o grupo.
    ana.upload_packages(1).await;
    let ana_id = ana.id;
    ana.group_state(GROUP_ROOM, |s| state_parts(s).4 == Some(ana_id))
        .await;
    ana.create_and_add(GROUP_ROOM, &[id]).await;
    ana.say(GROUP_ROOM, "bem-vinda, Bia").await;

    // Bia volta: o welcome vem logo depois do Welcome.
    let (mut socket, _) = connect_async(&url).await.unwrap();
    send(
        &mut socket,
        &ClientMessage::Login {
            username: "bia".into(),
            password: "senha-boa-123".into(),
        },
    )
    .await;
    let mut bia = Person {
        socket,
        id,
        me,
        groups: Default::default(),
    };
    bia.accept_welcome(GROUP_ROOM).await;
    bia.send(ClientMessage::OpenConversation {
        conversation_id: GROUP_ROOM,
    })
    .await;
    let history = bia
        .wait(|m| match m {
            ServerMessage::History { messages, .. } => Some(messages),
            _ => None,
        })
        .await;
    let group = bia.groups.get_mut(&GROUP_ROOM).unwrap();
    let Received::Text { text, .. } = group
        .receive(&bia.me, &history[0].sealed.clone().unwrap().0)
        .unwrap()
    else {
        panic!("esperava texto");
    };
    assert_eq!(text, "bem-vinda, Bia".as_bytes());
}

/// Quem foi escolhido para mudar o grupo e não age dentro do prazo perde a
/// vez para o próximo online, quando alguém cutuca (`NudgeGroup`).
#[sqlx::test]
async fn committer_parado_passa_a_vez_a_outro_online(pool: PgPool) {
    let url = start_with(AppState::new(pool).with_stall_timeout(Duration::from_secs(1))).await;
    let mut ana = Person::register(&url, "ana").await;
    let mut bia = Person::register(&url, "bia").await;
    ana.upload_packages(2).await;
    bia.upload_packages(2).await;

    // Sala do Grupo: o grupo MLS ainda não existe e as duas podem criá-lo.
    let first = ana
        .group_state(GROUP_ROOM, |m| {
            matches!(m, ServerMessage::GroupState { pending_add, committer: Some(_), .. } if pending_add.len() == 2)
        })
        .await;
    let ServerMessage::GroupState {
        committer: Some(chosen),
        ..
    } = first
    else {
        unreachable!()
    };
    let other = if chosen == dev(ana.id) {
        bia.id
    } else {
        ana.id
    };

    // Ninguém age. Antes do prazo, cutucar não troca ninguém.
    bia.send(ClientMessage::NudgeGroup {
        conversation_id: GROUP_ROOM,
    })
    .await;
    bia.group_state(
        GROUP_ROOM,
        |m| matches!(m, ServerMessage::GroupState { committer: Some(c), .. } if *c == chosen),
    )
    .await;

    // Passado o prazo, a vez vai para a outra.
    tokio::time::sleep(Duration::from_millis(1300)).await;
    bia.send(ClientMessage::NudgeGroup {
        conversation_id: GROUP_ROOM,
    })
    .await;
    for person in [&mut ana, &mut bia] {
        person
            .group_state(GROUP_ROOM, |m| {
                matches!(m, ServerMessage::GroupState { committer: Some(c), .. } if *c == dev(other))
            })
            .await;
    }

    // E quem não é da conversa não cutuca.
    let mut socket = connect_async(&url).await.unwrap().0;
    send(
        &mut socket,
        &ClientMessage::Register {
            username: "zoe".into(),
            password: "senha-boa-123".into(),
            email: Some("zoe@teste.com".into()),
        },
    )
    .await;
    let private = ConversationId(uuid::Uuid::new_v4());
    send(
        &mut socket,
        &ClientMessage::NudgeGroup {
            conversation_id: private,
        },
    )
    .await;
    loop {
        match recv(&mut socket).await {
            ServerMessage::Error { .. } => break,
            ServerMessage::Welcome { .. }
            | ServerMessage::KeyPackageStock { .. }
            | ServerMessage::Devices { .. }
            | ServerMessage::GroupState { .. } => continue,
            other => panic!("inesperado: {other:?}"),
        }
    }
}

#[sqlx::test]
async fn app_sem_mls_online_nao_vira_quem_cria_o_grupo(pool: PgPool) {
    let url = start_server(pool).await;
    let mut ana = Person::register(&url, "ana").await;
    // Bia está online com um app de antes do MLS: nunca manda pacotes.
    let bia = Person::register(&url, "bia").await;
    ana.upload_packages(3).await;

    let direct = open_direct(&mut ana, "bia").await;
    let state = ana
        .group_state(direct, |s| state_parts(s).4.is_some())
        .await;
    let (_, members, pending_add, _, committer) = state_parts(&state);
    assert!(members.is_empty());
    assert_eq!(pending_add, [ana.id]);
    // Quem cria é Ana, mesmo que o id de Bia seja menor.
    assert_eq!(committer, Some(ana.id));
    drop(bia);
}

/// Outra instalação da mesma conta deixou pacotes no servidor; o aparelho
/// novo troca todos pelos dele, e quem o põe no grupo pega um que abre aqui.
#[sqlx::test]
async fn pacotes_de_outra_instalacao_sao_trocados(pool: PgPool) {
    let url = start_server(pool).await;
    let mut ana = Person::register(&url, "ana").await;
    let mut bia = Person::register(&url, "bia").await;
    bia.upload_packages(3).await;
    ana.upload_packages(1).await;

    // Bia instala de novo: identidade nova, pacotes novos que trocam os velhos.
    bia.me = Identity::in_memory(&bia.id.0.to_string()).unwrap();
    let fresh = Sealed(bia.me.key_package().unwrap());
    bia.send(ClientMessage::UploadKeyPackages {
        packages: vec![fresh],
        replace: true,
    })
    .await;
    bia.wait(|m| match m {
        ServerMessage::KeyPackageStock { count: 1 } => Some(()),
        _ => None,
    })
    .await;

    // Quem quer que seja o escolhido, aqui Ana faz a criação.
    ana.group_state(GROUP_ROOM, |s| state_parts(s).4.is_some())
        .await;
    ana.create_and_add(GROUP_ROOM, &[bia.id]).await;
    bia.accept_welcome(GROUP_ROOM).await;
    ana.say(GROUP_ROOM, "abre aí").await;
    assert_eq!(bia.hear(GROUP_ROOM).await, (ana.id, "abre aí".into()));
}

/// O grupo foi criado e o único aparelho dentro saiu (recomeçou o grupo
/// daqui): sem ninguém, ninguém mais mudaria a época, e a criação só vale na
/// época 0. O servidor recomeça o grupo e alguém cria de novo.
#[sqlx::test]
async fn grupo_vazio_depois_de_criado_recomeca(pool: PgPool) {
    let url = start_server(pool).await;
    let mut ana = Person::register(&url, "ana").await;
    ana.upload_packages(1).await;
    let ana_id = ana.id;
    ana.group_state(GROUP_ROOM, |s| state_parts(s).4 == Some(ana_id))
        .await;
    ana.create_and_add(GROUP_ROOM, &[]).await;

    ana.groups.remove(&GROUP_ROOM);
    ana.me.delete_group(GROUP_ROOM.0.as_bytes()).unwrap();
    ana.send(ClientMessage::RejoinGroup {
        conversation_id: GROUP_ROOM,
    })
    .await;
    let state = ana
        .group_state(GROUP_ROOM, |s| state_parts(s).1.is_empty())
        .await;
    assert_eq!(state_parts(&state).0, 0);
    assert_eq!(state_parts(&state).4, Some(ana_id));
    ana.create_and_add(GROUP_ROOM, &[]).await;
}

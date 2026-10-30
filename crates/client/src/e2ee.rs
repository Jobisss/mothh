//! Criptografia ponta a ponta das conversas (F2-08), do lado do app.
//!
//! Cada conversa é um grupo MLS (crate `crypto`) com o id da conversa. O
//! servidor só ordena e repassa: diz em que época cada grupo está, quem
//! falta pôr ou tirar e quem deve fazer isso (`GroupState`). Este módulo
//! reage a essas mensagens e devolve o que mandar de volta.
//!
//! O MLS não decifra a mesma mensagem duas vezes (as chaves de cada mensagem
//! são apagadas depois de usadas) nem a própria mensagem. Por isso o texto de
//! cada mensagem já lida fica guardado aqui, cifrado com a chave do aparelho,
//! e o histórico que vem do servidor é preenchido a partir dessa cópia.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crypto::{FileVault, Group, Identity, Received, SystemVault, Vault};
use protocol::{
    ChatMessage, ClientMessage, ConversationId, EpochCommit, KeyPackageOf, MAX_MESSAGE_CHARS,
    MessageId, Sealed, UserId,
};

use crate::session::profile_file;

/// Abaixo disso o app publica mais pacotes de chaves, até `KEY_PACKAGES`.
const KEY_PACKAGES_LOW: u32 = 10;
const KEY_PACKAGES: u32 = 20;

/// Rótulo do segredo exportado para a chave do SFrame (F2-09).
const SFRAME_LABEL: &str = "mothh sframe";
const SFRAME_KEY_LEN: usize = 32;

/// O que aparece no lugar de uma mensagem que este aparelho não lê: de antes
/// de ele entrar na conversa, ou de uma instalação anterior do app.
pub const UNREADABLE: &str = "[cifrada: este aparelho não tem a chave desta mensagem]";
/// Mensagem de uma época que ainda não chegou; some quando o commit chegar.
pub const DECRYPTING: &str = "[decifrando...]";
/// O MLS diz que outra pessoa escreveu: o servidor mentiu sobre o autor.
pub const FORGED: &str = "[mensagem com autor trocado: ignorada]";

/// O que o servidor disse, no último `GroupState`, sobre o grupo de uma conversa.
#[derive(Debug, Clone, Default)]
pub struct ServerGroup {
    pub epoch: u64,
    pub members: Vec<UserId>,
    pub pending_add: Vec<UserId>,
    pub pending_remove: Vec<UserId>,
    pub committer: Option<UserId>,
}

pub struct E2ee {
    me: Identity,
    user: UserId,
    groups: HashMap<ConversationId, Group>,
    server: HashMap<ConversationId, ServerGroup>,
    /// Texto das mensagens já lidas, e onde ele fica salvo.
    texts: HashMap<MessageId, String>,
    texts_file: Option<PathBuf>,
    /// Mensagens que mandamos e ainda não voltaram do servidor com id.
    sent: HashMap<Vec<u8>, String>,
    /// Textos esperando o grupo ficar pronto para sair.
    outbox: HashMap<ConversationId, Vec<String>>,
    /// Mensagens de época futura, para tentar de novo depois de um commit.
    waiting: HashSet<MessageId>,
    /// Já pedimos algo e esperamos a resposta: não pede de novo.
    creating: HashSet<ConversationId>,
    claiming: HashSet<ConversationId>,
    fetching: HashSet<ConversationId>,
    /// Pedimos para sair e entrar de novo (uma vez por conversa por sessão).
    rejoining: HashSet<ConversationId>,
}

impl E2ee {
    /// Abre as chaves de `user` neste perfil. A chave de assinatura fica no
    /// cofre do sistema; sem cofre (um Linux sem Secret Service), num arquivo
    /// na pasta de configuração.
    pub fn open(profile: &str, user: UserId) -> Result<Self, String> {
        let user_id = user.0.to_string();
        let state_file =
            profile_file(profile, &format!("{user_id}.mls")).ok_or("sem pasta de configuração")?;
        let file_vault = profile_file(profile, "cofre.json").ok_or("sem pasta de configuração")?;

        // O cofre do sistema às vezes usa um runtime próprio por baixo: abre
        // fora do Tokio, numa thread comum.
        let profile = profile.to_string();
        let me = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    // Quem já caiu no arquivo uma vez continua nele, senão a
                    // chave do estado salvo não bate mais.
                    if !file_vault.exists() {
                        match Identity::open(&SystemVault::new(&profile), &user_id, &state_file) {
                            Ok(me) => return Ok(me),
                            Err(crypto::Error::Vault(err)) => {
                                eprintln!(
                                    "cofre do sistema indisponível ({err}); \
                                     as chaves ficam em {}",
                                    file_vault.display()
                                );
                            }
                            Err(err) => return Err(err),
                        }
                    }
                    let vault: &dyn Vault = &FileVault::new(&file_vault);
                    Identity::open(vault, &user_id, &state_file)
                })
                .join()
                .unwrap_or_else(|_| Err(crypto::Error::Vault("o cofre travou".into())))
        })
        .map_err(|err| err.to_string())?;

        let texts_file = profile_file(&profile, &format!("{}.textos", user.0));
        let texts = texts_file
            .as_ref()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| me.open_local(&bytes).ok())
            .and_then(|plain| serde_json::from_slice(&plain).ok())
            .unwrap_or_default();
        Ok(Self::with_identity(me, user, texts, texts_file))
    }

    /// Só em memória, para os testes.
    #[cfg(test)]
    pub fn in_memory(user: UserId) -> Self {
        let me = Identity::in_memory(&user.0.to_string()).unwrap();
        Self::with_identity(me, user, HashMap::new(), None)
    }

    fn with_identity(
        me: Identity,
        user: UserId,
        texts: HashMap<MessageId, String>,
        texts_file: Option<PathBuf>,
    ) -> Self {
        Self {
            me,
            user,
            groups: HashMap::new(),
            server: HashMap::new(),
            texts,
            texts_file,
            sent: HashMap::new(),
            outbox: HashMap::new(),
            waiting: HashSet::new(),
            creating: HashSet::new(),
            claiming: HashSet::new(),
            fetching: HashSet::new(),
            rejoining: HashSet::new(),
        }
    }

    pub fn user(&self) -> UserId {
        self.user
    }

    /// O servidor disse quantos pacotes de chaves ainda temos lá.
    pub fn key_package_stock(&mut self, count: u32, out: &mut Vec<ClientMessage>) {
        if count >= KEY_PACKAGES_LOW {
            return;
        }
        let packages: Result<Vec<Sealed>, _> = (count..KEY_PACKAGES)
            .map(|_| self.me.key_package().map(Sealed))
            .collect();
        match packages {
            Ok(packages) => {
                // A parte secreta de cada pacote precisa estar salva antes de
                // alguém usá-lo.
                self.save();
                out.push(ClientMessage::UploadKeyPackages { packages });
            }
            Err(err) => eprintln!("pacotes de chaves: {err}"),
        }
    }

    /// Um `GroupState` chegou. Devolve `true` se a época do nosso grupo mudou.
    pub fn group_state(
        &mut self,
        conversation: ConversationId,
        info: ServerGroup,
        out: &mut Vec<ClientMessage>,
    ) -> bool {
        let committer = info.committer == Some(self.user);
        let in_group = info.members.contains(&self.user);
        let empty = info.members.is_empty();
        self.server.insert(conversation, info);
        let mut changed = false;

        if !in_group {
            // Esperando a resposta da criação: o estado pode ser de antes dela.
            if self.creating.contains(&conversation) {
                return false;
            }
            // Fomos tirados, perdemos a corrida da criação, ou pedimos para
            // entrar de novo: o grupo daqui não vale mais. Um grupo de época
            // maior que a do estado é de um welcome que chegou depois dele.
            let epoch = self.server[&conversation].epoch;
            if self.group(conversation).is_some_and(|g| g.epoch() <= epoch) {
                self.forget(conversation);
                changed = true;
            }
            if empty && committer {
                match self.me.create_group(conversation.0.as_bytes()) {
                    Ok(group) => {
                        self.groups.insert(conversation, group);
                        self.creating.insert(conversation);
                        out.push(ClientMessage::Commit {
                            conversation_id: conversation,
                            epoch: 0,
                            commit: None,
                            welcome: None,
                            added: vec![],
                            removed: vec![],
                        });
                    }
                    Err(err) => eprintln!("criar o grupo: {err}"),
                }
            }
            return changed;
        }

        let server_epoch = self.server[&conversation].epoch;
        let Some(group) = self.group(conversation) else {
            // O servidor diz que estamos no grupo, mas este aparelho não tem
            // ele: pede para ser posto de novo.
            self.rejoin(conversation, out);
            return false;
        };
        if !group.is_active() {
            self.forget(conversation);
            self.rejoin(conversation, out);
            return true;
        }
        let local_epoch = group.epoch();
        if local_epoch < server_epoch {
            self.fetch(conversation, local_epoch, out);
        } else if local_epoch == server_epoch && committer && !group.has_pending_commit() {
            let info = &self.server[&conversation];
            if !info.pending_add.is_empty() {
                if self.claiming.insert(conversation) {
                    out.push(ClientMessage::ClaimKeyPackages {
                        conversation_id: conversation,
                        user_ids: info.pending_add.clone(),
                    });
                }
            } else if !info.pending_remove.is_empty() {
                self.commit(conversation, &[], out);
            }
        }
        self.flush(conversation, out);
        changed
    }

    /// Chegaram os pacotes que pedimos: põe essas pessoas (e tira quem saiu).
    pub fn key_packages(
        &mut self,
        conversation: ConversationId,
        packages: Vec<KeyPackageOf>,
        out: &mut Vec<ClientMessage>,
    ) {
        self.claiming.remove(&conversation);
        let Some(info) = self.server.get(&conversation) else {
            return;
        };
        let (epoch, committer) = (info.epoch, info.committer == Some(self.user));
        let ready = committer
            && self
                .group(conversation)
                .is_some_and(|g| g.epoch() == epoch && !g.has_pending_commit());
        if !ready {
            // Os pacotes se perdem; o próximo `GroupState` pede outros.
            return;
        }
        self.commit(conversation, &packages, out);
    }

    /// Prepara e manda uma mudança: põe quem veio em `packages` e tira quem
    /// está em `pending_remove`, além das folhas velhas de quem volta.
    fn commit(
        &mut self,
        conversation: ConversationId,
        packages: &[KeyPackageOf],
        out: &mut Vec<ClientMessage>,
    ) {
        let info = &self.server[&conversation];
        let group = self.groups.get_mut(&conversation).expect("conferido antes");
        let in_tree = group.members();
        let mut removed = info.pending_remove.clone();
        // Quem volta com pacote novo ainda pode ter a folha antiga na árvore.
        removed.extend(
            packages
                .iter()
                .map(|p| p.user_id)
                .filter(|u| in_tree.contains(&u.0.to_string()) && !info.pending_remove.contains(u)),
        );
        let epoch = group.epoch();
        let result = group.commit(
            &self.me,
            &packages
                .iter()
                .map(|p| p.package.0.clone())
                .collect::<Vec<_>>(),
            &removed.iter().map(|u| u.0.to_string()).collect::<Vec<_>>(),
        );
        match result {
            Ok(pending) => {
                self.save();
                out.push(ClientMessage::Commit {
                    conversation_id: conversation,
                    epoch,
                    commit: Some(Sealed(pending.commit)),
                    welcome: pending.welcome.map(Sealed),
                    added: packages.iter().map(|p| p.user_id).collect(),
                    removed,
                });
            }
            Err(err) => eprintln!("mudar o grupo: {err}"),
        }
    }

    /// O servidor aceitou a nossa mudança. `true`: a época mudou.
    pub fn accepted(&mut self, conversation: ConversationId, out: &mut Vec<ClientMessage>) -> bool {
        if self.creating.remove(&conversation) {
            self.save();
            return false;
        }
        let Some(group) = self.groups.get_mut(&conversation) else {
            return false;
        };
        if let Err(err) = group.confirm(&self.me) {
            eprintln!("aplicar a mudança aceita: {err}");
            return false;
        }
        self.save();
        self.flush(conversation, out);
        true
    }

    /// Outra mudança chegou antes da nossa.
    pub fn rejected(
        &mut self,
        conversation: ConversationId,
        epoch: u64,
        out: &mut Vec<ClientMessage>,
    ) {
        if self.creating.remove(&conversation) {
            self.forget(conversation);
            return;
        }
        let Some(group) = self.groups.get_mut(&conversation) else {
            return;
        };
        let _ = group.discard(&self.me);
        let local = group.epoch();
        self.save();
        if local < epoch {
            self.fetch(conversation, local, out);
        }
    }

    /// Fomos postos no grupo. `true`: temos grupo novo.
    pub fn welcome(
        &mut self,
        conversation: ConversationId,
        epoch: u64,
        welcome: &Sealed,
        out: &mut Vec<ClientMessage>,
    ) -> bool {
        // Um welcome guardado de antes, que já usamos.
        if self
            .group(conversation)
            .is_some_and(|g| g.is_active() && g.epoch() >= epoch)
        {
            return false;
        }
        self.forget(conversation);
        match self.me.join(&welcome.0) {
            Ok(group) => {
                self.groups.insert(conversation, group);
                self.rejoining.remove(&conversation);
                self.save();
                if let Some(info) = self.server.get(&conversation)
                    && info.epoch > epoch
                {
                    self.fetch(conversation, epoch, out);
                }
                self.flush(conversation, out);
                true
            }
            Err(err) => {
                // O pacote era de uma instalação anterior do app.
                eprintln!("entrar no grupo: {err}");
                self.rejoin(conversation, out);
                false
            }
        }
    }

    /// Commits que chegaram (um ao vivo, ou vários de um `FetchCommits`).
    /// `true`: a época mudou.
    pub fn commits(
        &mut self,
        conversation: ConversationId,
        commits: &[EpochCommit],
        fetched: bool,
        out: &mut Vec<ClientMessage>,
    ) -> bool {
        if fetched {
            self.fetching.remove(&conversation);
        }
        let mut changed = false;
        for commit in commits {
            let Some(group) = self.group(conversation) else {
                return changed;
            };
            let local = group.epoch();
            if commit.epoch < local {
                continue;
            }
            if commit.epoch > local {
                // Faltou algum no meio: pede a partir de onde estamos.
                self.fetch(conversation, local, out);
                break;
            }
            let group = self.groups.get_mut(&conversation).expect("carregado acima");
            match group.receive(&self.me, &commit.commit.0) {
                Ok(_) => changed = true,
                Err(err) => {
                    eprintln!("aplicar commit: {err}");
                    self.forget(conversation);
                    self.rejoin(conversation, out);
                    return true;
                }
            }
            if !group.is_active() {
                // O commit nos tirou do grupo.
                self.forget(conversation);
                return true;
            }
        }
        if changed {
            self.save();
            // Alcançamos a época do servidor: talvez sejamos nós a mudar algo.
            if let Some(info) = self.server.get(&conversation).cloned()
                && self
                    .group(conversation)
                    .is_some_and(|g| g.epoch() == info.epoch)
            {
                self.group_state(conversation, info, out);
            }
            self.flush(conversation, out);
        }
        changed
    }

    /// Cifra e manda, ou guarda até o grupo ficar pronto. `Err` com o motivo
    /// para a tela quando nem dá para guardar.
    pub fn send(
        &mut self,
        conversation: ConversationId,
        text: String,
        out: &mut Vec<ClientMessage>,
    ) -> Result<(), String> {
        let text = text.trim().to_string();
        if text.is_empty() {
            return Ok(());
        }
        if text.chars().count() > MAX_MESSAGE_CHARS {
            return Err(format!(
                "A mensagem passou de {MAX_MESSAGE_CHARS} caracteres."
            ));
        }
        self.outbox.entry(conversation).or_default().push(text);
        self.flush(conversation, out);
        Ok(())
    }

    /// Quantas mensagens estão esperando a troca de chaves.
    pub fn queued(&self) -> usize {
        self.outbox.values().map(Vec::len).sum()
    }

    /// O grupo está na época do servidor, sem mudança pendente e com todo
    /// mundo que dá para pôr já dentro (senão quem falta não leria).
    fn ready(&mut self, conversation: ConversationId) -> bool {
        let Some(epoch) = self
            .server
            .get(&conversation)
            .filter(|s| s.pending_add.is_empty())
            .map(|s| s.epoch)
        else {
            return false;
        };
        self.group(conversation)
            .is_some_and(|g| g.is_active() && !g.has_pending_commit() && g.epoch() == epoch)
    }

    fn flush(&mut self, conversation: ConversationId, out: &mut Vec<ClientMessage>) {
        if !self.ready(conversation) {
            return;
        }
        let Some(texts) = self.outbox.remove(&conversation) else {
            return;
        };
        let group = self.groups.get_mut(&conversation).expect("pronto");
        for text in texts {
            match group.encrypt(&self.me, text.as_bytes()) {
                Ok(sealed) => {
                    self.sent.insert(sealed.clone(), text);
                    out.push(ClientMessage::SendMessage {
                        conversation_id: conversation,
                        sealed: Sealed(sealed),
                    });
                }
                Err(err) => eprintln!("cifrar: {err}"),
            }
        }
        self.save();
    }

    /// Preenche o `text` de uma mensagem que veio do servidor.
    pub fn reveal(&mut self, message: &mut ChatMessage) {
        let Some(sealed) = &message.sealed else {
            return;
        };
        if let Some(text) = self.texts.get(&message.id) {
            message.text = text.clone();
            return;
        }
        if message.author_id == self.user {
            // O MLS não decifra a própria mensagem: o texto é o que mandamos.
            message.text = match self.sent.remove(&sealed.0) {
                Some(text) => {
                    self.remember(message.id, &text);
                    text
                }
                None => UNREADABLE.into(),
            };
            return;
        }
        if self.group(message.conversation_id).is_none() {
            message.text = UNREADABLE.into();
            return;
        }
        let group = self
            .groups
            .get_mut(&message.conversation_id)
            .expect("carregado acima");
        match group.receive(&self.me, &sealed.0) {
            Ok(Received::Text { from, text }) => {
                self.waiting.remove(&message.id);
                let text = if from == message.author_id.0.to_string() {
                    String::from_utf8_lossy(&text).into_owned()
                } else {
                    FORGED.into()
                };
                self.save();
                self.remember(message.id, &text);
                message.text = text;
            }
            Err(crypto::Error::FutureEpoch) => {
                self.waiting.insert(message.id);
                message.text = DECRYPTING.into();
            }
            Ok(_) | Err(_) => {
                self.waiting.remove(&message.id);
                message.text = UNREADABLE.into();
            }
        }
    }

    /// A mensagem ainda espera um commit para abrir.
    pub fn is_waiting(&self, id: MessageId) -> bool {
        self.waiting.contains(&id)
    }

    /// Saímos da conversa: o grupo daqui some.
    pub fn forget(&mut self, conversation: ConversationId) {
        self.groups.remove(&conversation);
        if let Err(err) = self.me.delete_group(conversation.0.as_bytes()) {
            eprintln!("apagar o grupo: {err}");
        }
        self.save();
    }

    /// A chave da época atual do grupo, para o SFrame da chamada (F2-09).
    /// `None` enquanto o grupo não está pronto.
    #[allow(dead_code)]
    pub fn media_key(&mut self, conversation: ConversationId) -> Option<(u64, Vec<u8>)> {
        self.group(conversation)?;
        let group = &self.groups[&conversation];
        let epoch = group.epoch();
        let key = group
            .export_secret(&self.me, SFRAME_LABEL, SFRAME_KEY_LEN)
            .ok()?;
        Some((epoch, key))
    }

    /// O grupo da conversa, do mapa ou do estado salvo.
    fn group(&mut self, conversation: ConversationId) -> Option<&Group> {
        if !self.groups.contains_key(&conversation) {
            match self.me.load_group(conversation.0.as_bytes()) {
                Ok(Some(group)) => {
                    self.groups.insert(conversation, group);
                }
                Ok(None) => return None,
                Err(err) => {
                    eprintln!("abrir o grupo: {err}");
                    return None;
                }
            }
        }
        self.groups.get(&conversation)
    }

    fn fetch(&mut self, conversation: ConversationId, from: u64, out: &mut Vec<ClientMessage>) {
        if self.fetching.insert(conversation) {
            out.push(ClientMessage::FetchCommits {
                conversation_id: conversation,
                from,
            });
        }
    }

    fn rejoin(&mut self, conversation: ConversationId, out: &mut Vec<ClientMessage>) {
        if self.rejoining.insert(conversation) {
            out.push(ClientMessage::RejoinGroup {
                conversation_id: conversation,
            });
        }
    }

    fn remember(&mut self, id: MessageId, text: &str) {
        self.texts.insert(id, text.to_string());
        let Some(path) = &self.texts_file else {
            return;
        };
        let saved = serde_json::to_vec(&self.texts)
            .ok()
            .and_then(|plain| self.me.seal_local(&plain).ok());
        if let Some(bytes) = saved {
            let temp = path.with_extension("tmp");
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if std::fs::write(&temp, bytes).is_ok() {
                let _ = std::fs::rename(&temp, path);
            }
        }
    }

    fn save(&self) {
        if let Err(err) = self.me.save() {
            eprintln!("salvar as chaves: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn user(n: u128) -> UserId {
        UserId(Uuid::from_u128(n))
    }

    fn take(out: &mut Vec<ClientMessage>) -> Vec<ClientMessage> {
        std::mem::take(out)
    }

    fn message(
        id: i64,
        conversation: ConversationId,
        author: UserId,
        sealed: &Sealed,
    ) -> ChatMessage {
        ChatMessage {
            id: MessageId(id),
            conversation_id: conversation,
            author_id: author,
            author_name: String::new(),
            text: String::new(),
            sealed: Some(sealed.clone()),
            sent_at_ms: 0,
            audio: None,
            file: None,
        }
    }

    /// O caminho inteiro que o servidor conduz: criar, pôr a outra pessoa,
    /// mandar e ler, com a mensagem esperando enquanto o grupo não existe.
    #[test]
    fn duas_pessoas_criam_o_grupo_e_conversam() {
        let conv = ConversationId(Uuid::from_u128(77));
        let (a, b) = (user(1), user(2));
        let mut ana = E2ee::in_memory(a);
        let mut bia = E2ee::in_memory(b);
        let mut out = Vec::new();

        // Pacotes publicados.
        bia.key_package_stock(0, &mut out);
        let [ClientMessage::UploadKeyPackages { packages }] = &take(&mut out)[..] else {
            panic!()
        };
        assert_eq!(packages.len(), KEY_PACKAGES as usize);
        let bia_package = packages[0].clone();

        // Ana escreve antes de existir grupo: a mensagem espera.
        ana.send(conv, "  oi bia  ".into(), &mut out).unwrap();
        assert!(out.is_empty());
        assert_eq!(ana.queued(), 1);

        // O grupo não existe e Ana é quem cria.
        let state = |epoch, members: Vec<UserId>, pending_add: Vec<UserId>| ServerGroup {
            epoch,
            members,
            pending_add,
            pending_remove: vec![],
            committer: Some(a),
        };
        ana.group_state(conv, state(0, vec![], vec![a, b]), &mut out);
        assert!(matches!(
            &take(&mut out)[..],
            [ClientMessage::Commit {
                epoch: 0,
                commit: None,
                ..
            }]
        ));
        assert!(!ana.accepted(conv, &mut out));
        // Bia não é a escolhida: não faz nada.
        bia.group_state(conv, state(0, vec![a], vec![b]), &mut out);
        assert!(out.is_empty());

        ana.group_state(conv, state(0, vec![a], vec![b]), &mut out);
        // Grupo criado, mas Bia ainda falta: a mensagem continua esperando.
        let sent = take(&mut out);
        assert!(
            matches!(&sent[..], [ClientMessage::ClaimKeyPackages { user_ids, .. }] if *user_ids == [b])
        );
        ana.key_packages(
            conv,
            vec![KeyPackageOf {
                user_id: b,
                package: bia_package,
            }],
            &mut out,
        );
        let [
            ClientMessage::Commit {
                epoch: 0,
                welcome: Some(welcome),
                added,
                ..
            },
        ] = &take(&mut out)[..]
        else {
            panic!()
        };
        assert_eq!(*added, [b]);
        // Com a mudança pendente, nada é cifrado.
        ana.send(conv, "segunda".into(), &mut out).unwrap();
        assert!(out.is_empty());
        assert!(ana.accepted(conv, &mut out));
        ana.group_state(conv, state(1, vec![a, b], vec![]), &mut out);
        let sent = take(&mut out);
        let [
            ClientMessage::SendMessage { sealed: first, .. },
            ClientMessage::SendMessage { sealed, .. },
        ] = &sent[..]
        else {
            panic!("{sent:?}")
        };
        assert_eq!(ana.queued(), 0);

        assert!(bia.welcome(conv, 1, welcome, &mut out));
        bia.group_state(conv, state(1, vec![a, b], vec![]), &mut out);
        assert!(out.is_empty());
        let mut received = message(9, conv, a, first);
        bia.reveal(&mut received);
        assert_eq!(received.text, "oi bia");
        let mut received = message(10, conv, a, sealed);
        bia.reveal(&mut received);
        assert_eq!(received.text, "segunda");
        // De novo (o histórico chega depois): vem da cópia guardada.
        let mut again = message(10, conv, a, sealed);
        bia.reveal(&mut again);
        assert_eq!(again.text, "segunda");
        // A própria mensagem volta do servidor: o texto é o que foi mandado.
        let mut own = message(10, conv, a, sealed);
        ana.reveal(&mut own);
        assert_eq!(own.text, "segunda");
        // O servidor dizendo que foi outra pessoa: não cola.
        let sealed = {
            bia.send(conv, "eu".into(), &mut out).unwrap();
            let [ClientMessage::SendMessage { sealed, .. }] = &take(&mut out)[..] else {
                panic!()
            };
            sealed.clone()
        };
        let mut forged = message(11, conv, user(3), &sealed);
        ana.reveal(&mut forged);
        assert_eq!(forged.text, FORGED);

        // As duas exportam a mesma chave de mídia.
        assert_eq!(ana.media_key(conv), bia.media_key(conv));
        assert_eq!(ana.media_key(conv).unwrap().0, 1);

        // Muito longo é recusado antes de cifrar.
        assert!(
            ana.send(conv, "a".repeat(MAX_MESSAGE_CHARS + 1), &mut out)
                .is_err()
        );
    }
}

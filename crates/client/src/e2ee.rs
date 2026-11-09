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
use std::time::{Duration, Instant};

use crypto::{FileVault, Group, Identity, Received, SystemVault, Vault};
use protocol::{
    ChatMessage, ClientMessage, ConversationId, DeviceId, EpochCommit, KeyPackageOf,
    MAX_MESSAGE_CHARS, MessageId, Sealed, UserId,
};

use crate::session::profile_file;

/// Abaixo disso o app publica mais pacotes de chaves, até `KEY_PACKAGES`.
const KEY_PACKAGES_LOW: u32 = 10;
const KEY_PACKAGES: u32 = 20;

/// O que aparece no lugar de uma mensagem que este aparelho não lê: de antes
/// de ele entrar na conversa, ou de uma instalação anterior do app.
pub const UNREADABLE: &str = "[cifrada: este aparelho não tem a chave desta mensagem]";
/// Mensagem de uma época que ainda não chegou; some quando o commit chegar.
pub const DECRYPTING: &str = "[decifrando...]";
/// O MLS diz que outra pessoa escreveu: o servidor mentiu sobre o autor.
pub const FORGED: &str = "[mensagem com autor trocado: ignorada]";

/// O que o servidor disse, no último `GroupState`, sobre o grupo de uma
/// conversa. Os membros são aparelhos: cada aparelho de cada pessoa.
#[derive(Debug, Clone, Default)]
pub struct ServerGroup {
    pub epoch: u64,
    pub members: Vec<DeviceId>,
    pub pending_add: Vec<DeviceId>,
    pub pending_remove: Vec<DeviceId>,
    pub committer: Option<DeviceId>,
}

/// A credencial MLS de um aparelho: `conta:aparelho`. O app antigo (e o
/// servidor antigo) tinha só a conta, e aí o aparelho tem o mesmo id dela.
fn credential(user: UserId, device: DeviceId) -> String {
    if user.0 == device.0 {
        user.0.to_string()
    } else {
        format!("{}:{}", user.0, device.0)
    }
}

/// A conta de quem assinou, pela credencial.
fn user_of(credential: &str) -> &str {
    credential.split(':').next().unwrap_or_default()
}

/// O aparelho de quem assinou, pela credencial.
fn device_of(credential: &str) -> &str {
    credential.rsplit(':').next().unwrap_or_default()
}

pub struct E2ee {
    me: Identity,
    user: UserId,
    /// Este aparelho, o membro dos grupos MLS.
    device: DeviceId,
    groups: HashMap<ConversationId, Group>,
    server: HashMap<ConversationId, ServerGroup>,
    /// Texto das mensagens já lidas, e onde ele fica salvo.
    texts: HashMap<MessageId, String>,
    texts_file: Option<PathBuf>,
    /// Prévia de link que veio cifrada junto do texto (`links.rs`).
    previews: HashMap<MessageId, crate::links::Preview>,
    /// Mensagens que mandamos e ainda não voltaram do servidor com id.
    sent: HashMap<Vec<u8>, String>,
    /// Textos esperando o grupo ficar pronto para sair.
    outbox: HashMap<ConversationId, Vec<String>>,
    /// Mensagens de época futura, para tentar de novo depois de um commit.
    waiting: HashMap<MessageId, ConversationId>,
    /// Já pedimos algo e esperamos a resposta: não pede de novo.
    creating: HashSet<ConversationId>,
    claiming: HashSet<ConversationId>,
    /// Quando pedimos os commits que faltam. Se a resposta se perde (a
    /// conexão caiu no meio), pede de novo depois de [`REFETCH`]: antes, o
    /// pedido ficava marcado para sempre e o aparelho parava numa época
    /// velha, com a chave velha da chamada (uma pessoa deixava de ouvir a
    /// outra até fechar o app).
    fetching: HashMap<ConversationId, Instant>,
    /// Pedimos para sair e entrar de novo e esperamos o welcome.
    rejoining: HashSet<ConversationId>,
    /// Welcomes que não abriram, por conversa: tenta de novo até
    /// [`WELCOME_RETRIES`] vezes.
    failed_welcomes: HashMap<ConversationId, u8>,
    /// Já trocamos os pacotes do servidor pelos deste aparelho nesta sessão.
    packages_replaced: bool,
}

/// Quantas vezes pede para entrar de novo quando um welcome não abre.
const WELCOME_RETRIES: u8 = 3;

/// Depois de quanto tempo sem resposta pede os commits de novo.
const REFETCH: Duration = Duration::from_secs(10);

impl E2ee {
    /// Abre as chaves de `user` neste perfil. A chave de assinatura fica no
    /// cofre do sistema; sem cofre (um Linux sem Secret Service), num arquivo
    /// na pasta de configuração.
    ///
    /// Cada aparelho tem as próprias chaves e o próprio estado (o arquivo
    /// leva o id do aparelho): um aparelho novo, ou o mesmo tirado da conta
    /// e entrado de novo, começa do zero.
    pub fn open(profile: &str, user: UserId, device: DeviceId) -> Result<Self, String> {
        let user_id = credential(user, device);
        // Sempre um arquivo novo por aparelho, mesmo no aparelho "da conta"
        // (app que se atualizou): os grupos de antes de haver aparelhos
        // foram recomeçados no servidor, e um grupo velho guardado aqui
        // faria este aparelho ignorar o convite para o grupo novo.
        let state_name = format!("{}.{}.mls", user.0, device.0);
        let state_file = profile_file(profile, &state_name).ok_or("sem pasta de configuração")?;
        // O texto já lido fica cifrado com a chave da conta (a de antes de
        // haver vários aparelhos), não com a do aparelho: assim ele continua
        // aberto quando este aparelho troca de chaves.
        let local_name = user.0.to_string();
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
                        match Identity::open_with_local(
                            &SystemVault::new(&profile),
                            &user_id,
                            &state_file,
                            &local_name,
                        ) {
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
                    Identity::open_with_local(vault, &user_id, &state_file, &local_name)
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
        Ok(Self::with_identity(me, user, device, texts, texts_file))
    }

    /// Só em memória, para os testes.
    #[cfg(test)]
    pub fn in_memory(user: UserId) -> Self {
        Self::in_memory_on(user, DeviceId(user.0))
    }

    /// Só em memória, num aparelho que não é o "da conta".
    #[cfg(test)]
    pub fn in_memory_on(user: UserId, device: DeviceId) -> Self {
        let me = Identity::in_memory(&credential(user, device)).unwrap();
        Self::with_identity(me, user, device, HashMap::new(), None)
    }

    fn with_identity(
        me: Identity,
        user: UserId,
        device: DeviceId,
        texts: HashMap<MessageId, String>,
        texts_file: Option<PathBuf>,
    ) -> Self {
        Self {
            me,
            user,
            device,
            groups: HashMap::new(),
            server: HashMap::new(),
            texts,
            texts_file,
            sent: HashMap::new(),
            previews: HashMap::new(),
            outbox: HashMap::new(),
            waiting: HashMap::new(),
            creating: HashSet::new(),
            claiming: HashSet::new(),
            fetching: HashMap::new(),
            rejoining: HashSet::new(),
            failed_welcomes: HashMap::new(),
            packages_replaced: false,
        }
    }

    pub fn user(&self) -> UserId {
        self.user
    }

    pub fn device(&self) -> DeviceId {
        self.device
    }

    /// O servidor disse quantos pacotes de chaves ainda temos lá.
    pub fn key_package_stock(&mut self, count: u32, out: &mut Vec<ClientMessage>) {
        // Na primeira vez da sessão troca tudo: o que está lá pode ser de
        // outra instalação, e um welcome feito com aquilo não abriria aqui.
        let replace = !self.packages_replaced;
        let count = if replace { 0 } else { count };
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
                self.packages_replaced = true;
                out.push(ClientMessage::UploadKeyPackages { packages, replace });
            }
            Err(err) => {
                crate::diag::report(format!("erro na troca de chaves, pacotes de chaves: {err}"))
            }
        }
    }

    /// Um `GroupState` chegou. Devolve `true` se a época do nosso grupo mudou.
    pub fn group_state(
        &mut self,
        conversation: ConversationId,
        info: ServerGroup,
        out: &mut Vec<ClientMessage>,
    ) -> bool {
        let committer = info.committer == Some(self.device);
        let in_group = info.members.contains(&self.device);
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
                    Err(err) => crate::diag::report(format!(
                        "erro na troca de chaves, criar o grupo: {err}"
                    )),
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
                        device_ids: info.pending_add.clone(),
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
        let (epoch, committer) = (info.epoch, info.committer == Some(self.device));
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
        let in_tree_has = |d: &DeviceId| {
            let d = d.0.to_string();
            in_tree.iter().any(|c| device_of(c) == d)
        };
        let mut removed = info.pending_remove.clone();
        // Quem volta com pacote novo ainda pode ter a folha antiga na árvore.
        removed.extend(
            packages
                .iter()
                .map(|p| p.device_id)
                .filter(|d| in_tree_has(d) && !info.pending_remove.contains(d)),
        );
        // As folhas da árvore desses aparelhos, pela credencial.
        let leaves: Vec<String> = in_tree
            .iter()
            .filter(|c| removed.iter().any(|d| device_of(c) == d.0.to_string()))
            .cloned()
            .collect();
        let epoch = group.epoch();
        let result = group.commit(
            &self.me,
            &packages
                .iter()
                .map(|p| p.package.0.clone())
                .collect::<Vec<_>>(),
            &leaves,
        );
        match result {
            Ok(pending) => {
                self.save();
                out.push(ClientMessage::Commit {
                    conversation_id: conversation,
                    epoch,
                    commit: Some(Sealed(pending.commit)),
                    welcome: pending.welcome.map(Sealed),
                    added: packages.iter().map(|p| p.device_id).collect(),
                    removed,
                });
            }
            Err(err) => {
                crate::diag::report(format!("erro na troca de chaves, mudar o grupo: {err}"))
            }
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
            crate::diag::report(format!(
                "erro na troca de chaves, aplicar a mudança aceita: {err}"
            ));
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
                self.failed_welcomes.remove(&conversation);
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
                // O pacote era de uma instalação anterior do app (ou de outro
                // perfil com a mesma conta). Pede para ser posto de novo: os
                // pacotes do servidor já são os deste aparelho.
                crate::diag::report(format!("erro na troca de chaves, entrar no grupo: {err}"));
                let tries = self.failed_welcomes.entry(conversation).or_default();
                if *tries < WELCOME_RETRIES {
                    *tries += 1;
                    self.rejoining.remove(&conversation);
                }
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
                    crate::diag::report(format!("erro na troca de chaves, aplicar commit: {err}"));
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
        if crate::links::split(&text).0.chars().count() > MAX_MESSAGE_CHARS {
            return Err(format!(
                "A mensagem passou de {MAX_MESSAGE_CHARS} caracteres."
            ));
        }
        self.outbox.entry(conversation).or_default().push(text);
        self.flush(conversation, out);
        Ok(())
    }

    /// Cifra o texto novo de uma mensagem já enviada (`EditMessage`). Ao
    /// contrário de `send`, não guarda para depois: ou o grupo está pronto
    /// e sai agora, ou avisa para tentar de novo.
    pub fn seal(&mut self, conversation: ConversationId, text: &str) -> Result<Sealed, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("A mensagem está vazia.".into());
        }
        if crate::links::split(text).0.chars().count() > MAX_MESSAGE_CHARS {
            return Err(format!(
                "A mensagem passou de {MAX_MESSAGE_CHARS} caracteres."
            ));
        }
        if !self.ready(conversation) {
            return Err("trocando chaves... tente editar de novo em instantes".into());
        }
        let group = self.groups.get_mut(&conversation).expect("pronto");
        let sealed = group
            .encrypt(&self.me, text.as_bytes())
            .map_err(|err| format!("não deu para cifrar: {err}"))?;
        // O MLS não decifra a própria mensagem: quando ela voltar editada,
        // `reveal` acha o texto aqui.
        self.sent.insert(sealed.clone(), text.to_string());
        self.save();
        Ok(Sealed(sealed))
    }

    /// A mensagem foi editada: o texto guardado era o antigo. Sem isto,
    /// `reveal` devolveria ele em vez de abrir o `sealed` novo.
    pub fn forget_text(&mut self, id: MessageId) {
        self.texts.remove(&id);
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
        if let Some(text) = self.texts.get(&message.id).cloned() {
            self.show(message, &text);
            return;
        }
        if message.author_id == self.user
            && let Some(text) = self.sent.remove(&sealed.0)
        {
            // O MLS não decifra a própria mensagem: o texto é o que mandamos.
            self.remember(message.id, &text);
            self.show(message, &text);
            return;
        }
        // Daqui para baixo, mesmo sendo da própria conta: foi mandada de
        // outro aparelho, que é outro membro do grupo, e decifra como a de
        // qualquer pessoa. (Uma deste aparelho sem a cópia, de antes de
        // reinstalar, não abre e fica como ilegível.)
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
                let text = if user_of(&from) == message.author_id.0.to_string() {
                    String::from_utf8_lossy(&text).into_owned()
                } else {
                    FORGED.into()
                };
                self.save();
                self.remember(message.id, &text);
                self.show(message, &text);
            }
            Err(crypto::Error::FutureEpoch) => {
                self.waiting.insert(message.id, message.conversation_id);
                message.text = DECRYPTING.into();
            }
            Ok(_) | Err(_) => {
                self.waiting.remove(&message.id);
                message.text = UNREADABLE.into();
            }
        }
    }

    /// Separa o texto que aparece da prévia de link que veio junto.
    fn show(&mut self, message: &mut ChatMessage, plain: &str) {
        let (text, preview) = crate::links::split(plain);
        message.text = text.to_string();
        match preview {
            Some(preview) => {
                self.previews.insert(message.id, preview);
            }
            None => {
                self.previews.remove(&message.id);
            }
        }
    }

    /// A prévia de link de uma mensagem já aberta.
    pub fn preview(&self, id: MessageId) -> Option<&crate::links::Preview> {
        self.previews.get(&id)
    }

    /// A mensagem ainda espera um commit para abrir.
    pub fn is_waiting(&self, id: MessageId) -> bool {
        self.waiting.contains_key(&id)
    }

    /// Conversas em que algo está parado esperando a troca de chaves: texto
    /// na fila para sair, mensagem recebida que ainda não abre, este
    /// aparelho esperando ser posto (de novo) no grupo, ou mudança pendente
    /// que cabe a outra pessoa fazer. O app cutuca o servidor por elas
    /// (`NudgeGroup`) a cada ping; se quem devia agir não agiu, a vez passa.
    pub fn stuck(&self) -> Vec<ConversationId> {
        let mut stuck: Vec<ConversationId> = self
            .outbox
            .iter()
            .filter(|(_, texts)| !texts.is_empty())
            .map(|(id, _)| *id)
            .chain(self.waiting.values().copied())
            .chain(self.rejoining.iter().copied())
            .chain(self.server.iter().filter_map(|(id, info)| {
                let waiting_for_me = info.pending_add.contains(&self.device);
                let someone_else = info.committer != Some(self.device)
                    && (info.members.is_empty()
                        || !info.pending_add.is_empty()
                        || !info.pending_remove.is_empty());
                (waiting_for_me || someone_else).then_some(*id)
            }))
            .collect();
        stuck.sort_by_key(|id| id.0);
        stuck.dedup();
        stuck
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
    pub fn media_key(&mut self, conversation: ConversationId) -> Option<(u64, Vec<u8>)> {
        self.group(conversation)?;
        let group = &self.groups[&conversation];
        let epoch = group.epoch();
        let key = group
            .export_secret(&self.me, crate::sframe::KEY_LABEL, crate::sframe::KEY_LEN)
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
                    crate::diag::report(format!("erro na troca de chaves, abrir o grupo: {err}"));
                    return None;
                }
            }
        }
        self.groups.get(&conversation)
    }

    /// A conexão com o servidor voltou: as respostas que esperávamos podem
    /// ter se perdido, então pode pedir tudo de novo.
    pub fn reconnected(&mut self) {
        self.fetching.clear();
        self.claiming.clear();
        self.rejoining.clear();
    }

    /// Durante uma chamada: confere com o servidor se perdemos algum commit
    /// do grupo. A chave da chamada vem da época do grupo; um aparelho que
    /// ficou para trás cifra e decifra com a chave velha, e os outros não o
    /// ouvem (nem ele os ouve). Sem grupo, pede para entrar de novo.
    pub fn catch_up(&mut self, conversation: ConversationId, out: &mut Vec<ClientMessage>) {
        let local = match self.group(conversation) {
            Some(group) if group.is_active() => group.epoch(),
            _ => {
                self.rejoin(conversation, out);
                return;
            }
        };
        self.fetch(conversation, local, out);
    }

    fn fetch(&mut self, conversation: ConversationId, from: u64, out: &mut Vec<ClientMessage>) {
        let now = Instant::now();
        let asked = self.fetching.get(&conversation);
        if asked.is_none_or(|at| now.duration_since(*at) >= REFETCH) {
            self.fetching.insert(conversation, now);
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

    /// O aparelho "da conta" (o de um app sem `Hello`).
    fn dev(user: UserId) -> DeviceId {
        DeviceId(user.0)
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
            edited: false,
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
        let [ClientMessage::UploadKeyPackages { packages, .. }] = &take(&mut out)[..] else {
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
            members: members.into_iter().map(dev).collect(),
            pending_add: pending_add.into_iter().map(dev).collect(),
            pending_remove: vec![],
            committer: Some(dev(a)),
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
            matches!(&sent[..], [ClientMessage::ClaimKeyPackages { device_ids, .. }] if *device_ids == [dev(b)])
        );
        ana.key_packages(
            conv,
            vec![KeyPackageOf {
                device_id: dev(b),
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
        assert_eq!(*added, [dev(b)]);
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

    #[test]
    fn pedido_de_commits_perdido_pede_de_novo() {
        let conv = ConversationId(Uuid::from_u128(78));
        let mut e2ee = E2ee::in_memory(user(1));
        let mut out = Vec::new();
        let fetches = |out: &mut Vec<ClientMessage>| {
            take(out)
                .into_iter()
                .filter(|m| matches!(m, ClientMessage::FetchCommits { .. }))
                .count()
        };
        e2ee.fetch(conv, 3, &mut out);
        e2ee.fetch(conv, 3, &mut out);
        assert_eq!(fetches(&mut out), 1, "pede uma vez enquanto espera");
        // A conexão caiu antes da resposta: ao voltar, pede de novo.
        e2ee.reconnected();
        e2ee.fetch(conv, 3, &mut out);
        assert_eq!(fetches(&mut out), 1);
        // Sem resposta por mais de REFETCH, também pede de novo.
        e2ee.fetching
            .insert(conv, Instant::now() - REFETCH - Duration::from_secs(1));
        e2ee.fetch(conv, 3, &mut out);
        assert_eq!(fetches(&mut out), 1);
    }

    #[test]
    fn na_chamada_sem_grupo_pede_para_entrar_de_novo() {
        let conv = ConversationId(Uuid::from_u128(79));
        let mut e2ee = E2ee::in_memory(user(1));
        let mut out = Vec::new();
        e2ee.catch_up(conv, &mut out);
        assert!(matches!(
            take(&mut out).as_slice(),
            [ClientMessage::RejoinGroup { .. }]
        ));
    }

    /// A mesma conta em dois computadores: cada um é um membro do grupo, o
    /// que um manda o outro lê, e quem manda do outro aparelho não é tido
    /// como falsificação (a conta da credencial é a do autor).
    #[test]
    fn dois_aparelhos_da_mesma_conta_leem_um_ao_outro() {
        let conv = ConversationId(Uuid::from_u128(80));
        let (ana, bia) = (user(1), user(2));
        let (sala, note, bia_pc) = (
            DeviceId(Uuid::from_u128(11)),
            DeviceId(Uuid::from_u128(12)),
            DeviceId(Uuid::from_u128(21)),
        );
        let mut on_sala = E2ee::in_memory_on(ana, sala);
        let mut on_note = E2ee::in_memory_on(ana, note);
        let mut on_bia = E2ee::in_memory_on(bia, bia_pc);
        let mut out = Vec::new();
        let package = |e: &mut E2ee, out: &mut Vec<ClientMessage>| {
            e.key_package_stock(0, out);
            let [ClientMessage::UploadKeyPackages { packages, .. }] = &take(out)[..] else {
                panic!()
            };
            packages[0].clone()
        };
        let note_package = package(&mut on_note, &mut out);
        let bia_package = package(&mut on_bia, &mut out);
        let state = |epoch, members: Vec<DeviceId>, pending_add: Vec<DeviceId>| ServerGroup {
            epoch,
            members,
            pending_add,
            pending_remove: vec![],
            committer: Some(sala),
        };

        on_sala.group_state(conv, state(0, vec![], vec![sala, note, bia_pc]), &mut out);
        take(&mut out);
        on_sala.accepted(conv, &mut out);
        on_sala.group_state(conv, state(0, vec![sala], vec![note, bia_pc]), &mut out);
        let sent = take(&mut out);
        assert!(
            matches!(&sent[..], [ClientMessage::ClaimKeyPackages { device_ids, .. }] if *device_ids == [note, bia_pc])
        );
        on_sala.key_packages(
            conv,
            vec![
                KeyPackageOf {
                    device_id: note,
                    package: note_package,
                },
                KeyPackageOf {
                    device_id: bia_pc,
                    package: bia_package,
                },
            ],
            &mut out,
        );
        let [
            ClientMessage::Commit {
                welcome: Some(welcome),
                added,
                ..
            },
        ] = &take(&mut out)[..]
        else {
            panic!()
        };
        assert_eq!(*added, [note, bia_pc]);
        assert!(on_sala.accepted(conv, &mut out));
        let ready = state(1, vec![sala, note, bia_pc], vec![]);
        on_sala.group_state(conv, ready.clone(), &mut out);
        assert!(on_note.welcome(conv, 1, welcome, &mut out));
        on_note.group_state(conv, ready.clone(), &mut out);
        assert!(on_bia.welcome(conv, 1, welcome, &mut out));
        on_bia.group_state(conv, ready, &mut out);
        take(&mut out);

        // Ana manda do pc da sala: o notebook dela (mesma conta) lê.
        on_sala
            .send(conv, "do pc da sala".into(), &mut out)
            .unwrap();
        let [ClientMessage::SendMessage { sealed, .. }] = &take(&mut out)[..] else {
            panic!()
        };
        let mut on_notebook = message(30, conv, ana, sealed);
        on_note.reveal(&mut on_notebook);
        assert_eq!(on_notebook.text, "do pc da sala");
        let mut at_bia = message(30, conv, ana, sealed);
        on_bia.reveal(&mut at_bia);
        assert_eq!(at_bia.text, "do pc da sala");
        // E o pc da sala continua vendo a própria.
        let mut own = message(30, conv, ana, sealed);
        on_sala.reveal(&mut own);
        assert_eq!(own.text, "do pc da sala");

        // Assinada pelo notebook de Ana mas dita como de Bia: falsificação.
        on_note.send(conv, "do notebook".into(), &mut out).unwrap();
        let [ClientMessage::SendMessage { sealed, .. }] = &take(&mut out)[..] else {
            panic!()
        };
        let mut forged = message(31, conv, bia, sealed);
        on_bia.reveal(&mut forged);
        assert_eq!(forged.text, FORGED);
        let mut real = message(31, conv, ana, sealed);
        on_sala.reveal(&mut real);
        assert_eq!(real.text, "do notebook");

        // Os três aparelhos têm a mesma chave da chamada.
        assert_eq!(on_sala.media_key(conv), on_note.media_key(conv));
        assert_eq!(on_sala.media_key(conv), on_bia.media_key(conv));
    }

    /// O notebook saiu da conta: o próximo commit tira a folha dele pela
    /// credencial `conta:aparelho`.
    #[test]
    fn aparelho_tirado_da_conta_sai_do_grupo() {
        let conv = ConversationId(Uuid::from_u128(81));
        let ana = user(1);
        let (sala, note) = (DeviceId(Uuid::from_u128(11)), DeviceId(Uuid::from_u128(12)));
        let mut on_sala = E2ee::in_memory_on(ana, sala);
        let mut on_note = E2ee::in_memory_on(ana, note);
        let mut out = Vec::new();
        on_note.key_package_stock(0, &mut out);
        let [ClientMessage::UploadKeyPackages { packages, .. }] = &take(&mut out)[..] else {
            panic!()
        };
        let note_package = packages[0].clone();
        let state = |epoch, members: Vec<DeviceId>, pending_add, pending_remove| ServerGroup {
            epoch,
            members,
            pending_add,
            pending_remove,
            committer: Some(sala),
        };
        on_sala.group_state(conv, state(0, vec![], vec![sala, note], vec![]), &mut out);
        on_sala.accepted(conv, &mut out);
        on_sala.group_state(conv, state(0, vec![sala], vec![note], vec![]), &mut out);
        take(&mut out);
        on_sala.key_packages(
            conv,
            vec![KeyPackageOf {
                device_id: note,
                package: note_package,
            }],
            &mut out,
        );
        take(&mut out);
        assert!(on_sala.accepted(conv, &mut out));
        on_sala.group_state(
            conv,
            state(1, vec![sala, note], vec![], vec![note]),
            &mut out,
        );
        let [ClientMessage::Commit { removed, .. }] = &take(&mut out)[..] else {
            panic!()
        };
        assert_eq!(*removed, [note]);
        assert!(on_sala.accepted(conv, &mut out));
        let members = on_sala.groups[&conv].members();
        assert_eq!(members, [credential(ana, sala)]);
    }
}

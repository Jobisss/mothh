//! Um grupo MLS: convidar, remover, cifrar e decifrar.

use openmls::prelude::tls_codec::Deserialize;
use openmls::prelude::*;

use crate::Error;
use crate::identity::Identity;

/// O que sai de [`Group::add`]: o `commit` vai para quem já está no grupo
/// (eles passam por [`Group::receive`]) e o `welcome` para quem entrou
/// (eles passam por [`Identity::join`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invitation {
    pub commit: Vec<u8>,
    pub welcome: Vec<u8>,
}

/// O que sai de [`Group::commit`]: o `commit` vai para quem já está no grupo
/// e o `welcome`, quando alguém entrou, para quem entrou.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingCommit {
    pub commit: Vec<u8>,
    pub welcome: Option<Vec<u8>>,
}

/// O que [`Group::receive`] encontrou numa mensagem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Received {
    /// Uma mensagem de `from`, já decifrada.
    Text { from: String, text: Vec<u8> },
    /// Alguém mudou o grupo (entrou ou saiu gente); as chaves já trocaram.
    /// `members` é a lista depois da mudança.
    Commit { members: Vec<String> },
    /// Uma mensagem nossa que o servidor devolveu. O MLS não decifra a
    /// própria mensagem; o app já tem o texto.
    Own,
    /// Uma proposta ou outra coisa que o mothh não usa.
    Ignored,
}

/// Um grupo MLS. Quem cifra uma mensagem não consegue decifrar a própria
/// cópia (o MLS é assim): o app guarda o texto que mandou.
pub struct Group {
    inner: MlsGroup,
}

impl Group {
    pub(crate) fn new(inner: MlsGroup) -> Self {
        Self { inner }
    }

    /// O id dado na criação (o id da conversa).
    pub fn id(&self) -> &[u8] {
        self.inner.group_id().as_slice()
    }

    /// Os ids de usuário de quem está no grupo, na ordem da árvore.
    pub fn members(&self) -> Vec<String> {
        self.inner
            .members()
            .map(|m| identity_of(&m.credential))
            .collect()
    }

    /// Quantas vezes as chaves já trocaram. Serve para o app saber se uma
    /// mensagem é de uma época passada.
    pub fn epoch(&self) -> u64 {
        self.inner.epoch().as_u64()
    }

    /// `false` depois que este aparelho foi removido: nada mais funciona.
    pub fn is_active(&self) -> bool {
        self.inner.is_active()
    }

    /// Põe pessoas no grupo pelos pacotes de chaves delas
    /// ([`Identity::key_package`]). Troca as chaves do grupo na hora; o app
    /// usa [`Group::commit`], que espera o servidor aceitar.
    pub fn add(&mut self, me: &Identity, key_packages: &[Vec<u8>]) -> Result<Invitation, Error> {
        if key_packages.is_empty() {
            return Err(Error::Message("nenhum pacote de chaves".into()));
        }
        let invitation = self.commit(me, key_packages, &[])?;
        self.confirm(me)?;
        Ok(Invitation {
            commit: invitation.commit,
            welcome: invitation.welcome.unwrap_or_default(),
        })
    }

    /// Tira `user_id` do grupo e troca as chaves na hora. Devolve o commit
    /// para os outros (inclusive para quem saiu, que assim fica sabendo).
    pub fn remove(&mut self, me: &Identity, user_id: &str) -> Result<Vec<u8>, Error> {
        if !self.members().iter().any(|m| m == user_id) {
            return Err(Error::NotMember(user_id.to_string()));
        }
        let pending = self.commit(me, &[], &[user_id.to_string()])?;
        self.confirm(me)?;
        Ok(pending.commit)
    }

    /// Prepara uma mudança no grupo: põe quem tem pacote em `key_packages` e
    /// tira todas as cópias de quem está em `remove` (uma pessoa que perdeu o
    /// estado e volta com pacote novo aparece nas duas listas). A mudança fica
    /// pendente até [`Group::confirm`] (o servidor aceitou) ou
    /// [`Group::discard`] (outro membro mudou o grupo antes). Enquanto isso o
    /// grupo não cifra mensagens.
    pub fn commit(
        &mut self,
        me: &Identity,
        key_packages: &[Vec<u8>],
        remove: &[String],
    ) -> Result<PendingCommit, Error> {
        if !self.inner.is_active() {
            return Err(Error::Inactive);
        }
        let packages = key_packages
            .iter()
            .map(|bytes| {
                KeyPackageIn::tls_deserialize_exact(bytes)
                    .map_err(|err| Error::Message(err.to_string()))?
                    .validate(me.provider().crypto(), ProtocolVersion::Mls10)
                    .map_err(|err| Error::Message(err.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let own = self.inner.own_leaf_index();
        let removals: Vec<LeafNodeIndex> = self
            .inner
            .members()
            .filter(|m| m.index != own && remove.contains(&identity_of(&m.credential)))
            .map(|m| m.index)
            .collect();
        if packages.is_empty() && removals.is_empty() {
            return Err(Error::Message("nada para mudar no grupo".into()));
        }
        let bundle = self
            .inner
            .commit_builder()
            .propose_adds(packages)
            .propose_removals(removals)
            .load_psks(me.provider().storage())
            .map_err(Error::mls)?
            .build(
                me.provider().rand(),
                me.provider().crypto(),
                me.signer(),
                |_| true,
            )
            .map_err(Error::mls)?
            .stage_commit(me.provider())
            .map_err(Error::mls)?;
        let (commit, welcome, _group_info) = bundle.into_messages();
        Ok(PendingCommit {
            commit: commit.to_bytes().map_err(Error::mls)?,
            welcome: welcome
                .map(|w| w.to_bytes())
                .transpose()
                .map_err(Error::mls)?,
        })
    }

    /// O servidor aceitou a mudança de [`Group::commit`]: as chaves trocam.
    pub fn confirm(&mut self, me: &Identity) -> Result<(), Error> {
        self.inner
            .merge_pending_commit(me.provider())
            .map_err(Error::mls)
    }

    /// Esquece a mudança pendente: outro membro mudou o grupo primeiro.
    pub fn discard(&mut self, me: &Identity) -> Result<(), Error> {
        self.inner
            .clear_pending_commit(me.provider().storage())
            .map_err(Error::mls)
    }

    /// Há uma mudança esperando o servidor.
    pub fn has_pending_commit(&self) -> bool {
        self.inner.pending_commit().is_some()
    }

    /// Um segredo da época atual, igual em todos os membros e diferente a
    /// cada troca de chaves. A chamada deriva daqui a chave do SFrame.
    pub fn export_secret(&self, me: &Identity, label: &str, len: usize) -> Result<Vec<u8>, Error> {
        self.inner
            .export_secret(me.provider().crypto(), label, &[], len)
            .map_err(Error::mls)
    }

    /// Cifra `text` para o grupo. Os bytes vão para o servidor como estão.
    pub fn encrypt(&mut self, me: &Identity, text: &[u8]) -> Result<Vec<u8>, Error> {
        if !self.inner.is_active() {
            return Err(Error::Inactive);
        }
        // Quem entrar com a mudança pendente não leria a mensagem: espera.
        if self.has_pending_commit() {
            return Err(Error::PendingCommit);
        }
        self.inner
            .create_message(me.provider(), me.signer(), text)
            .map_err(Error::mls)?
            .to_bytes()
            .map_err(Error::mls)
    }

    /// Lê uma mensagem que chegou do servidor: um texto cifrado, ou um
    /// commit de quem mudou o grupo.
    pub fn receive(&mut self, me: &Identity, bytes: &[u8]) -> Result<Received, Error> {
        if !self.inner.is_active() {
            return Err(Error::Inactive);
        }
        let message = MlsMessageIn::tls_deserialize_exact(bytes)
            .map_err(|err| Error::Message(err.to_string()))?
            .try_into_protocol_message()
            .map_err(|err| Error::Message(err.to_string()))?;
        if message.epoch() > self.inner.epoch() {
            return Err(Error::FutureEpoch);
        }
        let processed = self
            .inner
            .process_message(me.provider(), message)
            .map_err(Error::mls)?;
        let from = identity_of(processed.credential());
        match processed.into_content() {
            ProcessedMessageContent::ApplicationMessage(text) => Ok(Received::Text {
                from,
                text: text.into_bytes(),
            }),
            ProcessedMessageContent::StagedCommitMessage(commit) => {
                // Outro membro mudou o grupo antes de nós: a nossa mudança
                // pendente perdeu e o servidor vai recusá-la.
                if self.inner.pending_commit().is_some() {
                    self.discard(me)?;
                }
                self.inner
                    .merge_staged_commit(me.provider(), *commit)
                    .map_err(Error::mls)?;
                Ok(Received::Commit {
                    members: self.members(),
                })
            }
            // Um commit nosso que o servidor devolveu: já foi aplicado em
            // `add`/`remove`, mas se sobrou algo pendente, aplica.
            ProcessedMessageContent::OwnPendingCommit => {
                if self.inner.pending_commit().is_some() {
                    self.inner
                        .merge_pending_commit(me.provider())
                        .map_err(Error::mls)?;
                }
                Ok(Received::Commit {
                    members: self.members(),
                })
            }
            ProcessedMessageContent::OwnPrivateMessage => Ok(Received::Own),
            ProcessedMessageContent::ProposalMessage(_)
            | ProcessedMessageContent::ExternalJoinProposalMessage(_) => Ok(Received::Ignored),
        }
    }
}

/// O id de usuário dentro de uma credencial básica.
fn identity_of(credential: &Credential) -> String {
    String::from_utf8_lossy(credential.serialized_content()).into_owned()
}

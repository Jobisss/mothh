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
    /// ([`Identity::key_package`]). Troca as chaves do grupo.
    pub fn add(&mut self, me: &Identity, key_packages: &[Vec<u8>]) -> Result<Invitation, Error> {
        if key_packages.is_empty() {
            return Err(Error::Message("nenhum pacote de chaves".into()));
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
        let (commit, welcome, _group_info) = self
            .inner
            .add_members(me.provider(), me.signer(), &packages)
            .map_err(Error::mls)?;
        self.inner
            .merge_pending_commit(me.provider())
            .map_err(Error::mls)?;
        Ok(Invitation {
            commit: commit.to_bytes().map_err(Error::mls)?,
            welcome: welcome.to_bytes().map_err(Error::mls)?,
        })
    }

    /// Tira `user_id` do grupo e troca as chaves. Devolve o commit para os
    /// outros (inclusive para quem saiu, que assim fica sabendo).
    pub fn remove(&mut self, me: &Identity, user_id: &str) -> Result<Vec<u8>, Error> {
        let index = self
            .inner
            .members()
            .find(|m| identity_of(&m.credential) == user_id)
            .map(|m| m.index)
            .ok_or_else(|| Error::NotMember(user_id.to_string()))?;
        let (commit, _welcome, _group_info) = self
            .inner
            .remove_members(me.provider(), me.signer(), &[index])
            .map_err(Error::mls)?;
        self.inner
            .merge_pending_commit(me.provider())
            .map_err(Error::mls)?;
        commit.to_bytes().map_err(Error::mls)
    }

    /// Cifra `text` para o grupo. Os bytes vão para o servidor como estão.
    pub fn encrypt(&mut self, me: &Identity, text: &[u8]) -> Result<Vec<u8>, Error> {
        if !self.inner.is_active() {
            return Err(Error::Inactive);
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

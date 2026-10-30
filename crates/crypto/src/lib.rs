//! Criptografia ponta a ponta do mothh (F2-07): um embrulho fino do OpenMLS
//! (RFC 9420) com o que o app precisa e nada feito à mão.
//!
//! - [`Identity`]: a pessoa neste aparelho. Guarda a chave de assinatura no
//!   cofre do sistema ([`SystemVault`]: Credential Manager no Windows,
//!   Keychain no macOS, Secret Service no Linux) e o estado dos grupos num
//!   arquivo cifrado com uma chave que também fica no cofre.
//! - [`Group`]: um grupo MLS. Criar, convidar (`add`), remover, cifrar e
//!   decifrar. Uma conversa de duas pessoas é um grupo de dois.
//!
//! Como o MLS funciona, do ponto de vista de quem usa a crate:
//!
//! 1. Cada pessoa publica um "pacote de chaves" ([`Identity::key_package`]),
//!    um convite em branco que outros usam para pôr ela num grupo.
//! 2. Quem convida chama [`Group::add`] com os pacotes e recebe um *commit*
//!    (para quem já está no grupo) e um *welcome* (para quem entra). O
//!    servidor só repassa os dois; não lê nada.
//! 3. Quem entra chama [`Identity::join`] com o welcome. Quem já estava
//!    passa o commit por [`Group::receive`].
//! 4. Mensagens vão por [`Group::encrypt`] e voltam por [`Group::receive`].
//!    Remover alguém ([`Group::remove`]) troca as chaves: quem saiu não
//!    decifra mais nada dali em diante.
//!
//! O que sai daqui são bytes opacos; a crate `protocol` decide como eles
//! viajam. Ver `docs/seguranca.md`.

mod group;
mod identity;
mod vault;

pub use group::{Group, Invitation, Received};
pub use identity::Identity;
pub use vault::{MemoryVault, SystemVault, Vault};

/// O que pode dar errado. As mensagens são para o log; a tela mostra algo
/// mais curto.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cofre do sistema: {0}")]
    Vault(String),
    #[error("arquivo de estado: {0}")]
    Io(#[from] std::io::Error),
    #[error("o arquivo de estado não abriu: chave errada ou arquivo mexido")]
    Corrupted,
    #[error("MLS: {0}")]
    Mls(String),
    #[error("mensagem inválida: {0}")]
    Message(String),
    #[error("{0} não está no grupo")]
    NotMember(String),
    #[error("este aparelho não faz mais parte do grupo")]
    Inactive,
}

impl Error {
    /// Qualquer erro do OpenMLS vira texto: o app não decide nada a partir
    /// do tipo exato, só registra.
    pub(crate) fn mls(err: impl std::fmt::Display) -> Self {
        Self::Mls(err.to_string())
    }
}

//! A pessoa neste aparelho: chave de assinatura, credencial e o estado dos
//! grupos MLS.

use std::path::{Path, PathBuf};

use chacha20poly1305::aead::Aead;
use chacha20poly1305::{ChaCha20Poly1305, Key, KeyInit, Nonce};
use openmls::prelude::tls_codec::{Deserialize, Serialize};
use openmls::prelude::*;
use openmls_basic_credential::SignatureKeyPair;
use openmls_memory_storage::MemoryStorage;
use openmls_rust_crypto::RustCrypto;
use openmls_traits::OpenMlsProvider;
use openmls_traits::random::OpenMlsRand;

use crate::Error;
use crate::group::Group;
use crate::vault::Vault;

/// A suíte do mothh: X25519 para acordo de chaves, AES-128-GCM para cifrar,
/// SHA-256 e Ed25519 para assinar. É a suíte obrigatória do RFC 9420.
pub(crate) const CIPHERSUITE: Ciphersuite =
    Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;

/// Quantas épocas passadas cada aparelho guarda para decifrar mensagens que
/// chegaram atrasadas (enviadas pouco antes de alguém entrar ou sair).
const MAX_PAST_EPOCHS: usize = 16;

/// Quantas mensagens de uma mesma pessoa podem chegar fora de ordem, e o
/// maior salto para a frente: o histórico às vezes chega depois da última
/// mensagem da lista.
fn sender_ratchet() -> SenderRatchetConfiguration {
    SenderRatchetConfiguration::new(1000, 10_000)
}

/// Tamanho da chave que cifra o arquivo de estado (ChaCha20-Poly1305).
const FILE_KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;

/// O que o OpenMLS precisa: criptografia (RustCrypto) e um armazenamento. O
/// armazenamento fica em memória e é salvo no arquivo cifrado por
/// [`Identity::save`].
#[derive(Default)]
pub(crate) struct Provider {
    crypto: RustCrypto,
    storage: MemoryStorage,
}

impl OpenMlsProvider for Provider {
    type CryptoProvider = RustCrypto;
    type RandProvider = RustCrypto;
    type StorageProvider = MemoryStorage;

    fn storage(&self) -> &Self::StorageProvider {
        &self.storage
    }

    fn crypto(&self) -> &Self::CryptoProvider {
        &self.crypto
    }

    fn rand(&self) -> &Self::RandProvider {
        &self.crypto
    }
}

/// Onde o estado dos grupos é salvo, e com que chave.
struct StateFile {
    path: PathBuf,
    key: [u8; FILE_KEY_LEN],
}

/// A pessoa neste aparelho, com a chave que assina tudo que ela manda.
pub struct Identity {
    user_id: String,
    provider: Provider,
    signer: SignatureKeyPair,
    credential: CredentialWithKey,
    state: Option<StateFile>,
    /// A chave do arquivo de estado, ou uma aleatória sem arquivo.
    local_key: [u8; FILE_KEY_LEN],
}

impl Identity {
    /// Uma identidade só em memória, sem cofre nem arquivo. Para testes e
    /// para experimentar.
    pub fn in_memory(user_id: &str) -> Result<Self, Error> {
        let signer =
            SignatureKeyPair::new(CIPHERSUITE.signature_algorithm()).map_err(Error::mls)?;
        Self::build(user_id, signer, MemoryStorage::default(), None)
    }

    /// Abre (ou cria) a identidade de `user_id`. A chave de assinatura e a
    /// chave do arquivo ficam no `vault`; os grupos ficam em `state_file`,
    /// cifrado. Chame [`Identity::save`] depois de mexer em qualquer grupo.
    pub fn open(vault: &dyn Vault, user_id: &str, state_file: &Path) -> Result<Self, Error> {
        let provider = Provider::default();
        let signer = match vault.get(&format!("{user_id}/assinatura"))? {
            Some(saved) => serde_json::from_slice(&saved).map_err(|_| Error::Corrupted)?,
            None => {
                let signer =
                    SignatureKeyPair::new(CIPHERSUITE.signature_algorithm()).map_err(Error::mls)?;
                let saved = serde_json::to_vec(&signer).map_err(Error::mls)?;
                vault.set(&format!("{user_id}/assinatura"), &saved)?;
                signer
            }
        };
        let key: [u8; FILE_KEY_LEN] = match vault.get(&format!("{user_id}/arquivo-chave"))? {
            Some(key) => key.try_into().map_err(|_| Error::Corrupted)?,
            None => {
                let key = provider
                    .rand()
                    .random_array::<FILE_KEY_LEN>()
                    .map_err(Error::mls)?;
                vault.set(&format!("{user_id}/arquivo-chave"), &key)?;
                key
            }
        };
        let storage = match std::fs::read(state_file) {
            Ok(bytes) => storage_from_bytes(&decrypt(&key, &bytes)?)?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => MemoryStorage::default(),
            Err(err) => return Err(err.into()),
        };
        Self::build(
            user_id,
            signer,
            storage,
            Some(StateFile {
                path: state_file.to_path_buf(),
                key,
            }),
        )
    }

    fn build(
        user_id: &str,
        signer: SignatureKeyPair,
        storage: MemoryStorage,
        state: Option<StateFile>,
    ) -> Result<Self, Error> {
        let provider = Provider {
            crypto: RustCrypto::default(),
            storage,
        };
        // O OpenMLS lê a chave de assinatura do armazenamento ao assinar
        // mensagens de grupo; guardar de novo é inofensivo.
        signer.store(provider.storage()).map_err(Error::mls)?;
        let local_key = match &state {
            Some(state) => state.key,
            None => provider
                .rand()
                .random_array::<FILE_KEY_LEN>()
                .map_err(Error::mls)?,
        };
        let credential = CredentialWithKey {
            credential: BasicCredential::new(user_id.as_bytes().to_vec()).into(),
            signature_key: signer.to_public_vec().into(),
        };
        Ok(Self {
            user_id: user_id.to_string(),
            provider,
            signer,
            credential,
            state,
            local_key,
        })
    }

    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    /// Salva o estado dos grupos no arquivo cifrado. Sem arquivo
    /// ([`Identity::in_memory`]) não faz nada.
    pub fn save(&self) -> Result<(), Error> {
        let Some(state) = &self.state else {
            return Ok(());
        };
        let plain = storage_to_bytes(self.provider.storage());
        let bytes = encrypt(&state.key, &plain, self.provider.rand())?;
        // Escreve ao lado e renomeia: ou o arquivo novo inteiro, ou o velho.
        let temp = state.path.with_extension("tmp");
        if let Some(dir) = state.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&temp, bytes)?;
        std::fs::rename(&temp, &state.path)?;
        Ok(())
    }

    /// Um pacote de chaves novo, para alguém pôr esta pessoa num grupo. Cada
    /// pacote serve uma vez só; a parte secreta fica no estado (salve).
    pub fn key_package(&self) -> Result<Vec<u8>, Error> {
        let bundle = KeyPackage::builder()
            .build(
                CIPHERSUITE,
                &self.provider,
                &self.signer,
                self.credential.clone(),
            )
            .map_err(Error::mls)?;
        bundle
            .key_package()
            .tls_serialize_detached()
            .map_err(Error::mls)
    }

    /// Cria um grupo só com esta pessoa, com o id dado (o id da conversa).
    pub fn create_group(&self, group_id: &[u8]) -> Result<Group, Error> {
        let config = MlsGroupCreateConfig::builder()
            .ciphersuite(CIPHERSUITE)
            .use_ratchet_tree_extension(true)
            .max_past_epochs(MAX_PAST_EPOCHS)
            .sender_ratchet_configuration(sender_ratchet())
            .build();
        let inner = MlsGroup::new_with_group_id(
            &self.provider,
            &self.signer,
            &config,
            GroupId::from_slice(group_id),
            self.credential.clone(),
        )
        .map_err(Error::mls)?;
        Ok(Group::new(inner))
    }

    /// Entra num grupo pelo welcome que alguém mandou ([`crate::Invitation`]).
    pub fn join(&self, welcome: &[u8]) -> Result<Group, Error> {
        let message = MlsMessageIn::tls_deserialize_exact(welcome)
            .map_err(|err| Error::Message(err.to_string()))?;
        let MlsMessageBodyIn::Welcome(welcome) = message.extract() else {
            return Err(Error::Message("não é um convite".into()));
        };
        let config = MlsGroupJoinConfig::builder()
            .use_ratchet_tree_extension(true)
            .max_past_epochs(MAX_PAST_EPOCHS)
            .sender_ratchet_configuration(sender_ratchet())
            .build();
        let inner = StagedWelcome::new_from_welcome(&self.provider, &config, welcome, None)
            .map_err(Error::mls)?
            .into_group(&self.provider)
            .map_err(Error::mls)?;
        Ok(Group::new(inner))
    }

    /// Um grupo já conhecido, do estado salvo.
    pub fn load_group(&self, group_id: &[u8]) -> Result<Option<Group>, Error> {
        let inner = MlsGroup::load(self.provider.storage(), &GroupId::from_slice(group_id))
            .map_err(Error::mls)?;
        Ok(inner.map(Group::new))
    }

    /// Apaga o grupo deste aparelho (saiu dele, ou vai entrar de novo por
    /// outro welcome). Não faz nada se ele não existir.
    pub fn delete_group(&self, group_id: &[u8]) -> Result<(), Error> {
        if let Some(mut inner) =
            MlsGroup::load(self.provider.storage(), &GroupId::from_slice(group_id))
                .map_err(Error::mls)?
        {
            inner.delete(self.provider.storage()).map_err(Error::mls)?;
        }
        Ok(())
    }

    /// Cifra bytes para guardar neste aparelho (o app guarda assim o texto
    /// das mensagens já lidas, porque o MLS não decifra a mesma mensagem duas
    /// vezes). Usa a chave do arquivo de estado; sem cofre, uma chave que
    /// some quando o processo acaba.
    pub fn seal_local(&self, plain: &[u8]) -> Result<Vec<u8>, Error> {
        encrypt(&self.local_key, plain, self.provider.rand())
    }

    /// O contrário de [`Identity::seal_local`]. Falha com
    /// [`Error::Corrupted`] se a chave não for a mesma.
    pub fn open_local(&self, sealed: &[u8]) -> Result<Vec<u8>, Error> {
        decrypt(&self.local_key, sealed)
    }

    pub(crate) fn provider(&self) -> &Provider {
        &self.provider
    }

    pub(crate) fn signer(&self) -> &SignatureKeyPair {
        &self.signer
    }
}

/// O armazenamento do OpenMLS é um mapa de bytes para bytes. No arquivo
/// vai, para cada par: tamanho da chave, chave, tamanho do valor, valor
/// (tamanhos em 4 bytes, big-endian), com a quantidade de pares na frente.
fn storage_to_bytes(storage: &MemoryStorage) -> Vec<u8> {
    let values = storage.values.read().unwrap();
    let mut out = Vec::new();
    out.extend_from_slice(&(values.len() as u32).to_be_bytes());
    for (key, value) in values.iter() {
        out.extend_from_slice(&(key.len() as u32).to_be_bytes());
        out.extend_from_slice(key);
        out.extend_from_slice(&(value.len() as u32).to_be_bytes());
        out.extend_from_slice(value);
    }
    out
}

fn storage_from_bytes(bytes: &[u8]) -> Result<MemoryStorage, Error> {
    struct Cursor<'a>(&'a [u8]);
    impl<'a> Cursor<'a> {
        fn take(&mut self, len: usize) -> Result<&'a [u8], Error> {
            if self.0.len() < len {
                return Err(Error::Corrupted);
            }
            let (head, tail) = self.0.split_at(len);
            self.0 = tail;
            Ok(head)
        }
        fn len(&mut self) -> Result<usize, Error> {
            let raw: [u8; 4] = self.take(4)?.try_into().map_err(|_| Error::Corrupted)?;
            Ok(u32::from_be_bytes(raw) as usize)
        }
    }
    let mut cursor = Cursor(bytes);
    let count = cursor.len()?;
    let mut values = std::collections::HashMap::new();
    for _ in 0..count {
        let key_len = cursor.len()?;
        let key = cursor.take(key_len)?.to_vec();
        let value_len = cursor.len()?;
        let value = cursor.take(value_len)?.to_vec();
        values.insert(key, value);
    }
    if !cursor.0.is_empty() {
        return Err(Error::Corrupted);
    }
    Ok(MemoryStorage {
        values: std::sync::RwLock::new(values),
    })
}

/// Arquivo de estado: nonce aleatório seguido do texto cifrado
/// (ChaCha20-Poly1305).
fn encrypt(key: &[u8; FILE_KEY_LEN], plain: &[u8], rand: &RustCrypto) -> Result<Vec<u8>, Error> {
    let nonce = rand.random_array::<NONCE_LEN>().map_err(Error::mls)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let sealed = cipher
        .encrypt(Nonce::from_slice(&nonce), plain)
        .map_err(|_| Error::Corrupted)?;
    let mut out = nonce.to_vec();
    out.extend_from_slice(&sealed);
    Ok(out)
}

fn decrypt(key: &[u8; FILE_KEY_LEN], bytes: &[u8]) -> Result<Vec<u8>, Error> {
    if bytes.len() < NONCE_LEN {
        return Err(Error::Corrupted);
    }
    let (nonce, sealed) = bytes.split_at(NONCE_LEN);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .decrypt(Nonce::from_slice(nonce), sealed)
        .map_err(|_| Error::Corrupted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arquivo_cifrado_volta_igual_e_chave_errada_falha() {
        let rand = RustCrypto::default();
        let key = [7u8; FILE_KEY_LEN];
        let bytes = encrypt(&key, b"estado", &rand).unwrap();
        assert!(!bytes.windows(6).any(|w| w == b"estado"));
        assert_eq!(decrypt(&key, &bytes).unwrap(), b"estado");
        assert!(matches!(
            decrypt(&[8u8; FILE_KEY_LEN], &bytes),
            Err(Error::Corrupted)
        ));
        assert!(matches!(decrypt(&key, &bytes[..5]), Err(Error::Corrupted)));
    }

    #[test]
    fn armazenamento_vai_e_volta() {
        let storage = MemoryStorage::default();
        storage
            .values
            .write()
            .unwrap()
            .insert(b"chave".to_vec(), b"valor".to_vec());
        storage
            .values
            .write()
            .unwrap()
            .insert(vec![], vec![1, 2, 3]);
        let bytes = storage_to_bytes(&storage);
        let back = storage_from_bytes(&bytes).unwrap();
        assert_eq!(
            *back.values.read().unwrap(),
            *storage.values.read().unwrap()
        );
        assert!(matches!(
            storage_from_bytes(&bytes[..bytes.len() - 1]),
            Err(Error::Corrupted)
        ));
        assert!(matches!(
            storage_from_bytes(&[0, 0, 0, 1]),
            Err(Error::Corrupted)
        ));
    }
}

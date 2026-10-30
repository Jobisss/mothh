//! Onde as chaves ficam: o cofre do sistema, ou memória nos testes.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::Error;

/// Um lugar para guardar segredos pequenos por nome. O [`SystemVault`] é o
/// cofre do sistema operacional; o [`MemoryVault`] serve para testes.
pub trait Vault: Send + Sync {
    /// O segredo com este nome, se existir.
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>, Error>;
    /// Guarda (ou troca) o segredo com este nome.
    fn set(&self, name: &str, value: &[u8]) -> Result<(), Error>;
    /// Apaga o segredo, se existir.
    fn delete(&self, name: &str) -> Result<(), Error>;
}

/// Cofre só em memória: some quando o processo acaba.
#[derive(Debug, Default)]
pub struct MemoryVault {
    entries: Mutex<HashMap<String, Vec<u8>>>,
}

impl MemoryVault {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Vault for MemoryVault {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>, Error> {
        Ok(self.entries.lock().unwrap().get(name).cloned())
    }

    fn set(&self, name: &str, value: &[u8]) -> Result<(), Error> {
        self.entries
            .lock()
            .unwrap()
            .insert(name.to_string(), value.to_vec());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), Error> {
        self.entries.lock().unwrap().remove(name);
        Ok(())
    }
}

/// O cofre do sistema: Credential Manager no Windows, Keychain no macOS,
/// Secret Service (GNOME Keyring, KWallet) no Linux. Cada segredo é uma
/// entrada do serviço `mothh`, com o nome prefixado pelo `scope` (o perfil
/// do app), para duas contas na mesma máquina não se misturarem.
#[derive(Debug, Clone)]
pub struct SystemVault {
    scope: String,
}

/// Nome do serviço no cofre do sistema.
const SERVICE: &str = "mothh";

impl SystemVault {
    pub fn new(scope: &str) -> Self {
        Self {
            scope: scope.to_string(),
        }
    }

    fn entry(&self, name: &str) -> Result<keyring::Entry, Error> {
        keyring::Entry::new(SERVICE, &format!("{}/{}", self.scope, name))
            .map_err(|err| Error::Vault(err.to_string()))
    }
}

impl Vault for SystemVault {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>, Error> {
        match self.entry(name)?.get_secret() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(Error::Vault(err.to_string())),
        }
    }

    fn set(&self, name: &str, value: &[u8]) -> Result<(), Error> {
        self.entry(name)?
            .set_secret(value)
            .map_err(|err| Error::Vault(err.to_string()))
    }

    fn delete(&self, name: &str) -> Result<(), Error> {
        match self.entry(name)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(Error::Vault(err.to_string())),
        }
    }
}

/// Cofre num arquivo comum, para quando o sistema não tem cofre (um Linux
/// sem Secret Service, por exemplo). Protege bem menos: quem lê o arquivo
/// lê as chaves. O app só cai nele se o [`SystemVault`] falhar, e avisa no
/// log. O arquivo é um JSON de nome para bytes.
#[derive(Debug)]
pub struct FileVault {
    path: std::path::PathBuf,
    lock: Mutex<()>,
}

impl FileVault {
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            path: path.into(),
            lock: Mutex::new(()),
        }
    }

    fn read(&self) -> Result<HashMap<String, Vec<u8>>, Error> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| Error::Corrupted),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
            Err(err) => Err(err.into()),
        }
    }

    fn write(&self, entries: &HashMap<String, Vec<u8>>) -> Result<(), Error> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let temp = self.path.with_extension("tmp");
        std::fs::write(&temp, serde_json::to_vec(entries).map_err(Error::mls)?)?;
        std::fs::rename(&temp, &self.path)?;
        Ok(())
    }
}

impl Vault for FileVault {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>, Error> {
        let _guard = self.lock.lock().unwrap();
        Ok(self.read()?.remove(name))
    }

    fn set(&self, name: &str, value: &[u8]) -> Result<(), Error> {
        let _guard = self.lock.lock().unwrap();
        let mut entries = self.read()?;
        entries.insert(name.to_string(), value.to_vec());
        self.write(&entries)
    }

    fn delete(&self, name: &str) -> Result<(), Error> {
        let _guard = self.lock.lock().unwrap();
        let mut entries = self.read()?;
        if entries.remove(name).is_some() {
            self.write(&entries)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memoria_guarda_troca_e_apaga() {
        let vault = MemoryVault::new();
        assert_eq!(vault.get("a").unwrap(), None);
        vault.set("a", b"1").unwrap();
        assert_eq!(vault.get("a").unwrap(), Some(b"1".to_vec()));
        vault.set("a", b"2").unwrap();
        assert_eq!(vault.get("a").unwrap(), Some(b"2".to_vec()));
        vault.delete("a").unwrap();
        assert_eq!(vault.get("a").unwrap(), None);
    }

    #[test]
    fn arquivo_guarda_e_volta_depois_de_reabrir() {
        let path = std::env::temp_dir().join(format!("mothh-cofre-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let vault = FileVault::new(&path);
            assert_eq!(vault.get("a").unwrap(), None);
            vault.set("a", &[1, 2, 3]).unwrap();
        }
        let vault = FileVault::new(&path);
        assert_eq!(vault.get("a").unwrap(), Some(vec![1, 2, 3]));
        vault.delete("a").unwrap();
        assert_eq!(vault.get("a").unwrap(), None);
        std::fs::remove_file(&path).unwrap();
    }

    /// Mexe no cofre de verdade, por isso fica fora do `cargo test` normal:
    /// `cargo test -p crypto -- --ignored cofre`.
    #[test]
    #[ignore]
    fn cofre_do_sistema_guarda_e_le() {
        let vault = SystemVault::new("teste-crypto");
        vault.delete("segredo").unwrap();
        assert_eq!(vault.get("segredo").unwrap(), None);
        vault.set("segredo", &[0, 1, 2, 255]).unwrap();
        assert_eq!(vault.get("segredo").unwrap(), Some(vec![0, 1, 2, 255]));
        vault.delete("segredo").unwrap();
        assert_eq!(vault.get("segredo").unwrap(), None);
    }
}

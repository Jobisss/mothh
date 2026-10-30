//! Token da sessão salvo em disco, para não pedir a senha toda vez.
//!
//! Cada perfil (`MOTHH_PERFIL`) tem o seu arquivo, assim dá para abrir duas
//! contas na mesma máquina. No Windows fica em `%APPDATA%\mothh\`.

use std::path::PathBuf;

pub struct SavedSession {
    path: Option<PathBuf>,
}

impl SavedSession {
    pub fn for_profile(profile: &str) -> Self {
        // Só letras, números, - e _ no nome do arquivo.
        let profile: String = profile
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        let profile = if profile.is_empty() {
            "padrao".into()
        } else {
            profile
        };
        let path = dirs::config_dir().map(|dir| dir.join("mothh").join(format!("{profile}.token")));
        Self { path }
    }

    pub fn load(&self) -> Option<String> {
        let token = std::fs::read_to_string(self.path.as_ref()?).ok()?;
        let token = token.trim();
        (!token.is_empty()).then(|| token.to_owned())
    }

    /// Falhar ao salvar não impede de usar o app: só vai pedir a senha de novo.
    pub fn save(&self, token: &str) {
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, token);
    }

    pub fn delete(&self) {
        if let Some(path) = &self.path {
            let _ = std::fs::remove_file(path);
        }
    }
}

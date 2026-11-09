//! Token da sessão salvo em disco, para não pedir a senha toda vez.
//!
//! Cada perfil (`MOTHH_PERFIL`) tem o seu arquivo, assim dá para abrir duas
//! contas na mesma máquina. No Windows fica em `%APPDATA%\mothh\`.

use std::path::PathBuf;

/// Arquivo do perfil na pasta de configuração, ex.: `padrao.token`.
pub fn profile_file(profile: &str, extension: &str) -> Option<PathBuf> {
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
    dirs::config_dir().map(|dir| dir.join("mothh").join(format!("{profile}.{extension}")))
}

pub struct SavedSession {
    path: Option<PathBuf>,
    profile: String,
}

impl SavedSession {
    pub fn for_profile(profile: &str) -> Self {
        Self {
            path: profile_file(profile, "token"),
            profile: profile.to_string(),
        }
    }

    /// O nome do perfil, para os outros arquivos dele (as chaves, F2-08).
    pub fn profile(&self) -> &str {
        &self.profile
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

    /// O id desta instalação do app (do perfil), criado na primeira vez. É
    /// por ele que o servidor sabe qual aparelho da conta está entrando.
    pub fn install(&self) -> uuid::Uuid {
        let path = profile_file(&self.profile, "aparelho");
        if let Some(saved) = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|text| text.trim().parse().ok())
        {
            return saved;
        }
        let install = uuid::Uuid::new_v4();
        if let Some(path) = &path {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(path, install.to_string());
        }
        install
    }

    pub fn delete(&self) {
        if let Some(path) = &self.path {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// O nome deste computador, para a lista de aparelhos da conta.
pub fn device_name() -> String {
    let name = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty());
    match name {
        Some(name) => name,
        None if cfg!(target_os = "windows") => "PC com Windows".into(),
        None => "computador".into(),
    }
}

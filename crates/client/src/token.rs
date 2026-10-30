//! Guarda o token da sessão para entrar sem senha da próxima vez.
//!
//! Fica num arquivo por perfil na pasta de configuração do sistema
//! (no Windows, `%APPDATA%\mothh`). Perfis diferentes deixam duas contas
//! abertas no mesmo computador, uma em cada janela.

use std::path::PathBuf;

/// Onde fica o token do perfil. `None` se o sistema não tem pasta de
/// configuração; aí o app só não lembra do login.
fn path(profile: &str) -> Option<PathBuf> {
    Some(
        dirs::config_dir()?
            .join("mothh")
            .join(format!("sessao-{profile}.txt")),
    )
}

pub fn load(profile: &str) -> Option<String> {
    let token = std::fs::read_to_string(path(profile)?).ok()?;
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_owned())
}

pub fn save(profile: &str, token: &str) {
    let Some(path) = path(profile) else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, token);
}

pub fn forget(profile: &str) {
    if let Some(path) = path(profile) {
        let _ = std::fs::remove_file(path);
    }
}

/// Limpa o nome do perfil para ele servir de nome de arquivo.
pub fn profile_name(raw: &str) -> String {
    let name: String = raw
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(32)
        .collect();
    if name.is_empty() {
        "padrao".into()
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_name_cannot_escape_the_folder() {
        assert_eq!(profile_name("Nina"), "nina");
        assert_eq!(profile_name("../../etc"), "etc");
        assert_eq!(profile_name("  "), "padrao");
    }
}

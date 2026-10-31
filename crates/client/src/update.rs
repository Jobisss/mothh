//! Atualização automática.
//!
//! O app pergunta às Releases do GitHub (Jobisss/mothh) se saiu versão nova:
//! ao abrir e depois a cada 6 horas. Achou uma:
//! - instalado pelo instalador (Windows): baixa o instalador em segundo plano,
//!   confere o SHA-256 com o que o GitHub informa e avisa no rodapé
//!   "v0.4.0 pronta · reiniciar". Clicar fecha o app, instala em silêncio e
//!   abre de novo. Se a pessoa só fechar o app, instala na saída, sem abrir.
//!   O login fica: os dados estão em `%APPDATA%\mothh`, que o instalador não
//!   toca;
//! - versão portátil (zip) ou outro sistema: só avisa, com um link para a
//!   página da release.
//!
//! Só liga no app gerado pelo workflow Release, que informa a versão em
//! `MOTHH_VERSAO` ao compilar. No `cargo run` não há versão e nada acontece.
//! `MOTHH_SEM_ATUALIZAR=1` desliga.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::AppWindow;

/// A versão deste app (a tag da release, ex.: `v0.4.0`). `None` fora da release.
pub const VERSION: Option<&str> = option_env!("MOTHH_VERSAO");

const RELEASES: &str = "https://api.github.com/repos/Jobisss/mothh/releases?per_page=10";
/// Só baixa deste endereço: a URL do instalador vem da resposta da API.
const DOWNLOADS: &str = "https://github.com/Jobisss/mothh/releases/download/";
const INSTALLER: &str = "mothh-setup-windows-x64.exe";
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// Se a checagem falhar (sem internet), tenta de novo antes das 6 horas.
const RETRY: Duration = Duration::from_secs(15 * 60);

/// Uma release mais nova que esta.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub tag: String,
    /// Página da release, para quem usa a versão portátil.
    pub page: String,
    pub installer_url: Option<String>,
    /// SHA-256 do instalador em hexadecimal, informado pelo GitHub.
    pub installer_sha256: Option<String>,
}

/// Compartilhado entre a thread que checa e o `main`.
#[derive(Default)]
pub struct Updater {
    /// O instalador baixado e conferido, pronto para rodar.
    ready: Mutex<Option<PathBuf>>,
    /// Já mandou instalar (não roda duas vezes: clique e saída).
    launched: Mutex<bool>,
}

impl Updater {
    /// Começa a checar em segundo plano, se esta for uma versão de release.
    pub fn start(ui: slint::Weak<AppWindow>) -> Arc<Self> {
        let updater = Arc::new(Self::default());
        let Some(version) = VERSION else {
            return updater;
        };
        if std::env::var_os("MOTHH_SEM_ATUALIZAR").is_some_and(|v| v != "0") {
            return updater;
        }
        let shared = updater.clone();
        std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(err) => return eprintln!("atualização: sem runtime: {err}"),
            };
            runtime.block_on(watch(version, shared, ui));
        });
        updater
    }

    /// Roda o instalador baixado, em silêncio. `reopen`: abre o app depois.
    /// Devolve `true` se o instalador começou (aí o app precisa fechar).
    pub fn install(&self, reopen: bool) -> bool {
        let Some(path) = self.ready.lock().unwrap().clone() else {
            return false;
        };
        let mut launched = self.launched.lock().unwrap();
        if *launched {
            return false;
        }
        let mut args = vec![
            "/VERYSILENT".to_string(),
            "/SUPPRESSMSGBOXES".into(),
            "/NORESTART".into(),
            // Se o mothh ainda estiver aberto (outra janela, ou esta demorando
            // a fechar), o instalador fecha antes de trocar o .exe.
            "/CLOSEAPPLICATIONS".into(),
        ];
        if let Some(log) = path.parent().map(|dir| dir.join("instalacao.log")) {
            args.push(format!("/LOG={}", log.display()));
        }
        if reopen {
            args.push("/relancar=1".into());
        }
        match std::process::Command::new(&path).args(&args).spawn() {
            Ok(_) => {
                *launched = true;
                true
            }
            Err(err) => {
                eprintln!("atualização: o instalador não abriu: {err}");
                false
            }
        }
    }

    /// Ao fechar o app: se havia atualização baixada, instala sem reabrir.
    pub fn install_on_exit(&self) {
        self.install(false);
    }
}

async fn watch(version: &'static str, updater: Arc<Updater>, ui: slint::Weak<AppWindow>) {
    let Some(current) = parse_version(version) else {
        return eprintln!("atualização: versão inválida {version:?}");
    };
    let dir = dirs::data_dir().map(|d| d.join("mothh").join("atualizacao"));
    let installed = installed_by_setup();
    let client = match reqwest::Client::builder()
        .user_agent(format!("mothh/{version}"))
        .timeout(Duration::from_secs(10 * 60))
        .build()
    {
        Ok(client) => client,
        Err(err) => return eprintln!("atualização: {err}"),
    };
    // Instaladores de atualizações já aplicadas.
    if let Some(dir) = &dir {
        let _ = std::fs::remove_dir_all(dir);
    }

    loop {
        let wait = match check(&client, current).await {
            Ok(None) => EVERY,
            Ok(Some(release)) => {
                if installed && let (Some(dir), Some(_)) = (&dir, &release.installer_url) {
                    match download(&client, &release, dir).await {
                        Ok(path) => {
                            eprintln!("atualização: {} baixada e conferida", release.tag);
                            *updater.ready.lock().unwrap() = Some(path);
                            show(&ui, &release, true);
                            // Pronta: falta só a pessoa reiniciar ou fechar.
                            return;
                        }
                        Err(err) => {
                            eprintln!("atualização: {}: {err}", release.tag);
                            RETRY
                        }
                    }
                } else {
                    show(&ui, &release, false);
                    EVERY
                }
            }
            Err(err) => {
                eprintln!("atualização: não deu para checar: {err}");
                RETRY
            }
        };
        tokio::time::sleep(wait).await;
    }
}

fn show(ui: &slint::Weak<AppWindow>, release: &Release, ready: bool) {
    let (tag, page) = (release.tag.clone(), release.page.clone());
    let _ = ui.upgrade_in_event_loop(move |ui| {
        ui.set_update_version(tag.into());
        ui.set_update_page(page.into());
        ui.set_update_ready(ready);
    });
}

/// A release mais nova que `current`, se houver.
async fn check(
    client: &reqwest::Client,
    current: (u64, u64, u64),
) -> Result<Option<Release>, String> {
    let body = client
        .get(RELEASES)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;
    let list: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    Ok(newest(&list, current))
}

/// Escolhe, na resposta da API, a release publicada de versão mais alta,
/// se for maior que `current`. Pré-releases contam (as v0.x são todas).
fn newest(list: &serde_json::Value, current: (u64, u64, u64)) -> Option<Release> {
    let mut best: Option<((u64, u64, u64), Release)> = None;
    for item in list.as_array()? {
        if item["draft"].as_bool().unwrap_or(true) {
            continue;
        }
        let Some(tag) = item["tag_name"].as_str() else {
            continue;
        };
        let Some(version) = parse_version(tag) else {
            continue;
        };
        if version <= current || best.as_ref().is_some_and(|(v, _)| version <= *v) {
            continue;
        }
        let asset = item["assets"]
            .as_array()
            .and_then(|assets| assets.iter().find(|a| a["name"] == INSTALLER));
        let installer_url = asset
            .and_then(|a| a["browser_download_url"].as_str())
            .filter(|url| url.starts_with(DOWNLOADS))
            .map(String::from);
        let installer_sha256 = asset
            .and_then(|a| a["digest"].as_str())
            .and_then(|d| d.strip_prefix("sha256:"))
            .filter(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
            .map(str::to_ascii_lowercase);
        let release = Release {
            tag: tag.to_string(),
            page: item["html_url"].as_str().unwrap_or_default().to_string(),
            installer_url,
            installer_sha256,
        };
        best = Some((version, release));
    }
    best.map(|(_, release)| release)
}

/// Baixa o instalador para `dir` e confere o SHA-256. Sem o hash do GitHub
/// para comparar, não instala.
async fn download(
    client: &reqwest::Client,
    release: &Release,
    dir: &Path,
) -> Result<PathBuf, String> {
    let (Some(url), Some(expected)) = (&release.installer_url, &release.installer_sha256) else {
        return Err("a release não tem instalador com hash".into());
    };
    let bytes = client
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;
    let got = hex(&Sha256::digest(&bytes));
    if &got != expected {
        return Err(format!("hash não confere: esperado {expected}, veio {got}"));
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("mothh-setup-{}.exe", release.tag));
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

/// O app veio do instalador (e não do zip portátil): o desinstalador do Inno
/// Setup fica na mesma pasta do `mothh.exe`.
fn installed_by_setup() -> bool {
    cfg!(windows)
        && std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("unins000.exe").exists()))
            .unwrap_or(false)
}

/// Abre a página da release no navegador (versão portátil).
pub fn open_page(url: &str) {
    if !url.starts_with("https://github.com/Jobisss/mothh/") {
        return;
    }
    #[cfg(windows)]
    let opener = "explorer";
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let opener = "xdg-open";
    let _ = std::process::Command::new(opener).arg(url).spawn();
}

/// `v0.4.1` ou `0.4.1` (o que vier depois de `-` é ignorado) → `(0, 4, 1)`.
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let text = text.trim().trim_start_matches('v');
    let text = text.split(['-', '+']).next()?;
    let mut parts = text.split('.').map(|p| p.parse::<u64>().ok());
    let version = (
        parts.next()??,
        parts.next()??,
        parts.next().unwrap_or(Some(0))?,
    );
    parts.next().is_none().then_some(version)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v0.3.2"), Some((0, 3, 2)));
        assert_eq!(parse_version("1.10.0"), Some((1, 10, 0)));
        assert_eq!(parse_version("v0.4"), Some((0, 4, 0)));
        assert_eq!(parse_version("v0.4.0-beta"), Some((0, 4, 0)));
        assert_eq!(parse_version("v0.4.0.1"), None);
        assert_eq!(parse_version("latest"), None);
        assert!(parse_version("v0.10.0") > parse_version("v0.9.9"));
    }

    fn sample() -> serde_json::Value {
        let asset = |tag: &str| {
            serde_json::json!({
                "name": INSTALLER,
                "browser_download_url": format!("{DOWNLOADS}{tag}/{INSTALLER}"),
                "digest": format!("sha256:{}", "AB".repeat(32)),
            })
        };
        serde_json::json!([
            { "tag_name": "v0.5.0", "draft": true, "html_url": "", "assets": [asset("v0.5.0")] },
            { "tag_name": "v0.3.1", "draft": false, "html_url": "p31", "assets": [asset("v0.3.1")] },
            { "tag_name": "v0.4.0", "draft": false, "html_url": "p40", "assets": [asset("v0.4.0")] },
            { "tag_name": "v0.3.2", "draft": false, "html_url": "p32", "assets": [] },
        ])
    }

    #[test]
    fn picks_newest_published() {
        let release = newest(&sample(), (0, 3, 1)).unwrap();
        assert_eq!(release.tag, "v0.4.0");
        assert_eq!(release.page, "p40");
        assert_eq!(
            release.installer_url.as_deref(),
            Some(
                "https://github.com/Jobisss/mothh/releases/download/v0.4.0/mothh-setup-windows-x64.exe"
            )
        );
        assert_eq!(release.installer_sha256, Some("ab".repeat(32)));
        assert_eq!(newest(&sample(), (0, 4, 0)), None);
    }

    #[test]
    fn ignores_installer_from_elsewhere() {
        let list = serde_json::json!([{
            "tag_name": "v9.0.0", "draft": false, "html_url": "p",
            "assets": [{ "name": INSTALLER, "browser_download_url": "https://evil.example/x.exe", "digest": "sha256:00" }],
        }]);
        let release = newest(&list, (0, 1, 0)).unwrap();
        assert_eq!(release.installer_url, None);
        assert_eq!(release.installer_sha256, None);
    }

    /// Fala com o GitHub de verdade: `cargo test -p client -- --ignored update`.
    #[test]
    #[ignore]
    fn real_github_download_and_hash() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let client = reqwest::Client::builder()
                .user_agent("mothh/teste")
                .build()
                .unwrap();
            let release = check(&client, (0, 0, 1))
                .await
                .unwrap()
                .expect("alguma release");
            println!("mais nova: {release:?}");
            let dir = std::env::temp_dir().join("mothh-teste-atualizacao");
            let path = download(&client, &release, &dir).await.unwrap();
            assert!(std::fs::metadata(&path).unwrap().len() > 1_000_000);
            let _ = std::fs::remove_dir_all(&dir);
            // Hash errado: recusa.
            let mut wrong = release.clone();
            wrong.installer_sha256 = Some("0".repeat(64));
            assert!(
                download(&client, &wrong, &dir)
                    .await
                    .unwrap_err()
                    .contains("hash")
            );
        });
    }
}

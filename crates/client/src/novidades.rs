//! Novidades de cada versão, para quem usa o app (`novidades.md`, que vai
//! para dentro do executável). Depois de uma atualização a janela abre
//! sozinha com o que mudou desde a última versão que a pessoa abriu; nas
//! configurações, "ver novidades" mostra tudo.

use std::path::PathBuf;
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{AppWindow, NewsLine, update};

const TEXT: &str = include_str!("../novidades.md");

type Version = (u64, u64, u64);

struct Entry {
    version: Version,
    title: String,
    items: Vec<String>,
}

/// As seções `## X.Y.Z` com as linhas `- ` de cada uma, da mais nova para
/// a mais velha. Uma linha recuada continua a novidade de cima.
fn parse(text: &str) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    for line in text.lines() {
        if let Some(title) = line.strip_prefix("## ") {
            let title = title.trim();
            if let Some(version) = title
                .split_whitespace()
                .next()
                .and_then(update::parse_version)
            {
                entries.push(Entry {
                    version,
                    title: title.to_string(),
                    items: Vec::new(),
                });
            }
        } else if let Some(entry) = entries.last_mut() {
            if let Some(item) = line.strip_prefix("- ") {
                entry.items.push(item.trim().to_string());
            } else if line.starts_with("  ")
                && !line.trim().is_empty()
                && let Some(last) = entry.items.last_mut()
            {
                last.push(' ');
                last.push_str(line.trim());
            }
        }
    }
    entries.sort_by_key(|e| std::cmp::Reverse(e.version));
    entries
}

/// O que mostrar ao abrir a versão `current`, tendo aberto `seen` da última
/// vez. Sem registro (a primeira vez com esta janela), só a versão atual.
fn unseen(entries: &[Entry], current: Version, seen: Option<Version>) -> Vec<&Entry> {
    let mut list: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.version <= current && seen.is_none_or(|s| e.version > s))
        .filter(|e| !e.items.is_empty())
        .collect();
    if seen.is_none() {
        list.truncate(1);
    }
    list
}

fn lines(entries: &[&Entry]) -> ModelRc<NewsLine> {
    let mut rows = Vec::new();
    for entry in entries {
        rows.push(NewsLine {
            text: SharedString::from(format!("versão {}", entry.title)),
            heading: true,
        });
        rows.extend(entry.items.iter().map(|item| NewsLine {
            text: item.into(),
            heading: false,
        }));
    }
    ModelRc::from(Rc::new(VecModel::from(rows)))
}

/// Onde fica a última versão cujas novidades a pessoa já viu (uma para a
/// máquina toda, não por perfil).
fn seen_file() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("mothh").join("novidades-vistas.txt"))
}

/// Liga o botão das configurações e, numa release com novidades que a
/// pessoa ainda não viu, abre a janela (fica embaixo da mariposa da
/// abertura até ela sair).
pub fn start(ui: &AppWindow) {
    let weak = ui.as_weak();
    ui.on_show_news(move || {
        if let Some(ui) = weak.upgrade() {
            let entries = parse(TEXT);
            let all: Vec<&Entry> = entries.iter().filter(|e| !e.items.is_empty()).collect();
            ui.set_news(lines(&all));
            ui.set_news_open(true);
        }
    });

    let Some(current) = update::VERSION.and_then(update::parse_version) else {
        return;
    };
    let path = seen_file();
    let seen = path
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|text| update::parse_version(text.trim()));
    if seen.is_some_and(|s| s >= current) {
        return;
    }
    let entries = parse(TEXT);
    let list = unseen(&entries, current, seen);
    if !list.is_empty() {
        ui.set_news(lines(&list));
        ui.set_news_open(true);
    }
    if let Some(path) = path {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, format!("{}.{}.{}", current.0, current.1, current.2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn o_arquivo_do_app_tem_novidades() {
        let entries = parse(TEXT);
        assert!(!entries.is_empty());
        assert!(entries.iter().all(|e| !e.items.is_empty()));
        assert!(entries.windows(2).all(|w| w[0].version > w[1].version));
    }

    #[test]
    fn mostra_so_o_que_a_pessoa_nao_viu() {
        let text =
            "intro\n- não é novidade\n## 0.3.0\n- c\n  continua\n## 0.1.0\n- a\n## 0.2.0\n- b\n";
        let entries = parse(text);
        let versions = |list: Vec<&Entry>| list.iter().map(|e| e.version).collect::<Vec<_>>();
        assert_eq!(entries[0].items, ["c continua"]);
        // Pulou da 0.1 para a 0.3: vê a 0.3 e a 0.2.
        assert_eq!(
            versions(unseen(&entries, (0, 3, 0), Some((0, 1, 0)))),
            [(0, 3, 0), (0, 2, 0)]
        );
        // A release saiu como 0.4.0 com a seção ainda chamada 0.3.0: aparece.
        assert_eq!(
            versions(unseen(&entries, (0, 4, 0), Some((0, 2, 0)))),
            [(0, 3, 0)]
        );
        // Primeira vez com a janela: só a mais nova até a versão atual.
        assert_eq!(versions(unseen(&entries, (0, 2, 5), None)), [(0, 2, 0)]);
        // Nada novo.
        assert!(unseen(&entries, (0, 3, 0), Some((0, 3, 0))).is_empty());
    }
}

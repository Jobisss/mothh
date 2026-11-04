//! As fontes da interface, escolhidas nas configurações (V6-05).
//!
//! O app tem duas famílias: a "pixel" (títulos, botões, lista de conversas,
//! nomes) e a "sans" (texto das mensagens). Cada uma tem algumas opções,
//! todas OFL e embutidas no binário pelo `app.slint`. A escolha fica num
//! arquivo do perfil (`padrao.fontes.json`), guardada pelo nome da família,
//! então reordenar a lista não troca a fonte de ninguém.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Uma opção de fonte: o nome da família (como está dentro do `.ttf`) e o
/// que aparece na lista das configurações.
pub struct Face {
    pub family: &'static str,
    pub label: &'static str,
    /// Fator sobre os tamanhos do `app.slint`, pensados para a VT323 e a
    /// Geist: fonte mais larga ou mais alta entra menor, para caber no
    /// mesmo lugar.
    pub scale: f32,
}

/// Fonte de terminal, para títulos e botões. A primeira é o padrão.
pub const PIXEL: [Face; 4] = [
    Face {
        family: "VT323",
        label: "VT323 (padrão)",
        scale: 1.0,
    },
    Face {
        family: "Share Tech Mono",
        label: "Share Tech Mono",
        scale: 0.78,
    },
    Face {
        family: "Silkscreen",
        label: "Silkscreen",
        scale: 0.58,
    },
    Face {
        family: "DotGothic16",
        label: "DotGothic16",
        scale: 0.8,
    },
];

/// Fonte legível, para o texto das mensagens. A primeira é o padrão.
pub const SANS: [Face; 4] = [
    Face {
        family: "Geist",
        label: "Geist (padrão)",
        scale: 1.0,
    },
    Face {
        family: "Atkinson Hyperlegible",
        label: "Atkinson Hyperlegible",
        scale: 0.95,
    },
    Face {
        family: "Fira Sans",
        label: "Fira Sans",
        scale: 0.95,
    },
    Face {
        family: "Lato",
        label: "Lato",
        scale: 1.0,
    },
];

/// Os temas, na ordem da lista das configurações. O primeiro é o padrão.
pub const THEMES: [&str; 2] = ["escuro (padrão)", "claro"];

/// Qual das famílias: a de títulos ou a das mensagens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Pixel,
    Sans,
}

impl Kind {
    pub fn from_id(id: &str) -> Option<Kind> {
        match id {
            "pixel" => Some(Kind::Pixel),
            "sans" => Some(Kind::Sans),
            _ => None,
        }
    }

    pub fn faces(self) -> &'static [Face] {
        match self {
            Kind::Pixel => &PIXEL,
            Kind::Sans => &SANS,
        }
    }
}

/// As escolhas, compartilhadas entre a interface e quem abre outras
/// janelas (o overlay). Clonar é barato.
#[derive(Clone)]
pub struct Fonts {
    path: Option<PathBuf>,
    /// Índice em `PIXEL`, em `SANS` e em `THEMES`.
    choice: Arc<Mutex<[usize; 3]>>,
}

impl Fonts {
    pub fn for_profile(profile: &str) -> Self {
        Self::at(crate::session::profile_file(profile, "fontes.json"))
    }

    fn at(path: Option<PathBuf>) -> Self {
        let choice = path
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map(|text| parse(&text))
            .unwrap_or_default();
        Self {
            path,
            choice: Arc::new(Mutex::new(choice)),
        }
    }

    /// O índice escolhido na lista daquela família.
    pub fn choice(&self, kind: Kind) -> usize {
        self.choice.lock().unwrap()[slot(kind)]
    }

    /// O nome da família escolhida, para o `font-family` do Slint.
    pub fn family(&self, kind: Kind) -> &'static str {
        kind.faces()[self.choice(kind)].family
    }

    /// O fator de tamanho da fonte escolhida.
    pub fn scale(&self, kind: Kind) -> f32 {
        kind.faces()[self.choice(kind)].scale
    }

    /// O índice do tema em `THEMES`.
    pub fn theme(&self) -> usize {
        self.choice.lock().unwrap()[2]
    }

    /// Tema claro escolhido.
    pub fn light(&self) -> bool {
        self.theme() == 1
    }

    pub fn set_theme(&self, index: usize) {
        if index >= THEMES.len() {
            return;
        }
        let mut choice = self.choice.lock().unwrap();
        choice[2] = index;
        self.persist(&choice);
    }

    /// Troca e já grava. Índice fora da lista é ignorado.
    pub fn set(&self, kind: Kind, index: usize) {
        if index >= kind.faces().len() {
            return;
        }
        let mut choice = self.choice.lock().unwrap();
        choice[slot(kind)] = index;
        self.persist(&choice);
    }

    fn persist(&self, choice: &[usize; 3]) {
        // Falhar ao salvar não trava o app: só volta o padrão na próxima vez.
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, render(choice));
    }
}

fn slot(kind: Kind) -> usize {
    match kind {
        Kind::Pixel => 0,
        Kind::Sans => 1,
    }
}

/// `{"pixel": "VT323", "sans": "Geist", "tema": "escuro"}`. Família ou tema
/// desconhecido (ou arquivo estragado) vale como o padrão.
fn parse(text: &str) -> [usize; 3] {
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str(text) else {
        return [0, 0, 0];
    };
    let find = |kind: Kind, key: &str| {
        let family = map.get(key).and_then(|v| v.as_str())?;
        kind.faces().iter().position(|face| face.family == family)
    };
    let theme = match map.get("tema").and_then(|v| v.as_str()) {
        Some("claro") => 1,
        _ => 0,
    };
    [
        find(Kind::Pixel, "pixel").unwrap_or(0),
        find(Kind::Sans, "sans").unwrap_or(0),
        theme,
    ]
}

fn render(choice: &[usize; 3]) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "pixel": PIXEL[choice[0]].family,
        "sans": SANS[choice[1]].family,
        "tema": if choice[2] == 1 { "claro" } else { "escuro" },
    }))
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mothh-teste-{}", std::process::id()));
        let path = dir.join(name);
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn padrao_sem_arquivo() {
        let fonts = Fonts::at(Some(temp_file("nenhuma.json")));
        assert_eq!(fonts.family(Kind::Pixel), "VT323");
        assert_eq!(fonts.family(Kind::Sans), "Geist");
    }

    #[test]
    fn escolha_sobrevive_a_reabrir() {
        let path = temp_file("fontes.json");
        let fonts = Fonts::at(Some(path.clone()));
        fonts.set(Kind::Pixel, 2);
        fonts.set(Kind::Sans, 3);
        fonts.set(Kind::Sans, 99); // fora da lista: ignorado

        let reopened = Fonts::at(Some(path));
        assert_eq!(reopened.family(Kind::Pixel), "Silkscreen");
        assert_eq!(reopened.family(Kind::Sans), "Lato");
        assert_eq!(reopened.choice(Kind::Sans), 3);
        assert!(!reopened.light());
    }

    #[test]
    fn tema_claro_sobrevive_a_reabrir() {
        let path = temp_file("tema.json");
        let fonts = Fonts::at(Some(path.clone()));
        fonts.set_theme(1);
        fonts.set_theme(7); // fora da lista: ignorado
        let reopened = Fonts::at(Some(path));
        assert!(reopened.light());
        assert_eq!(reopened.family(Kind::Pixel), "VT323");
    }

    #[test]
    fn arquivo_estragado_ou_familia_desconhecida_vale_padrao() {
        assert_eq!(parse("isso não é json"), [0, 0, 0]);
        assert_eq!(
            parse(r#"{"pixel": "Comic Sans", "sans": "Fira Sans", "tema": "roxo"}"#),
            [0, 2, 0]
        );
        assert_eq!(parse(r#"{"tema": "claro"}"#), [0, 0, 1]);
    }
}

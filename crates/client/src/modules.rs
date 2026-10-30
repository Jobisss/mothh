//! Registro de módulos: cada recurso opcional do app tem um interruptor.
//!
//! O que está ligado fica num arquivo do perfil, ao lado do token da sessão
//! (`padrao.modulos.json`), e vale na hora: quem toca um som ou desenha o
//! fundo pergunta `is_on` antes. Módulo que não está no arquivo vem ligado.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Um recurso que dá para desligar. Para criar um módulo novo, some aqui e
/// em `ALL`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Module {
    /// Fundo em dither atrás da lista de conversas.
    Dither,
    /// Som curto ao enviar uma mensagem.
    SentSound,
    /// Som curto ao receber uma mensagem.
    ReceivedSound,
}

impl Module {
    /// Na ordem em que aparecem na tela de configurações.
    pub const ALL: [Module; 3] = [Module::Dither, Module::SentSound, Module::ReceivedSound];

    /// Nome no arquivo. Não muda depois de lançado, senão o que a pessoa
    /// desligou volta ligado.
    pub fn id(self) -> &'static str {
        match self {
            Module::Dither => "dither",
            Module::SentSound => "som-enviada",
            Module::ReceivedSound => "som-recebida",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Module::Dither => "fundo em dither",
            Module::SentSound => "som ao enviar",
            Module::ReceivedSound => "som ao receber",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Module::Dither => "padrão animado atrás da lista de conversas",
            Module::SentSound => "bipe curto quando a sua mensagem sai",
            Module::ReceivedSound => "bipe curto quando chega mensagem",
        }
    }

    pub fn from_id(id: &str) -> Option<Module> {
        Module::ALL.into_iter().find(|module| module.id() == id)
    }
}

/// Os interruptores, compartilhados entre a interface e a rede. Clonar é
/// barato: todos os clones veem a mesma coisa.
#[derive(Clone)]
pub struct Modules {
    path: Option<PathBuf>,
    off: Arc<Mutex<Vec<Module>>>,
}

impl Modules {
    pub fn for_profile(profile: &str) -> Self {
        Self::at(crate::session::profile_file(profile, "modulos.json"))
    }

    fn at(path: Option<PathBuf>) -> Self {
        let off = path
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map(|text| parse(&text))
            .unwrap_or_default();
        Self {
            path,
            off: Arc::new(Mutex::new(off)),
        }
    }

    pub fn is_on(&self, module: Module) -> bool {
        !self.off.lock().unwrap().contains(&module)
    }

    /// Liga ou desliga e já grava no arquivo.
    pub fn set(&self, module: Module, on: bool) {
        let mut off = self.off.lock().unwrap();
        off.retain(|m| *m != module);
        if !on {
            off.push(module);
        }
        // Falhar ao salvar não trava o app: só volta ligado na próxima vez.
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, render(&off));
    }
}

/// `{"dither": false, ...}`. Arquivo estragado vale como tudo ligado.
fn parse(text: &str) -> Vec<Module> {
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str(text) else {
        return Vec::new();
    };
    map.iter()
        .filter(|(_, on)| on.as_bool() == Some(false))
        .filter_map(|(id, _)| Module::from_id(id))
        .collect()
}

/// Grava todos os módulos, para quem abrir o arquivo ver as opções.
fn render(off: &[Module]) -> String {
    let map: serde_json::Map<String, serde_json::Value> = Module::ALL
        .into_iter()
        .map(|m| (m.id().to_owned(), (!off.contains(&m)).into()))
        .collect();
    serde_json::to_string_pretty(&map).unwrap_or_default()
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
    fn tudo_ligado_sem_arquivo() {
        let modules = Modules::at(Some(temp_file("nenhum.json")));
        assert!(Module::ALL.into_iter().all(|m| modules.is_on(m)));
    }

    #[test]
    fn desligado_continua_desligado_ao_abrir_de_novo() {
        let path = temp_file("reabrir.json");
        let modules = Modules::at(Some(path.clone()));
        modules.set(Module::Dither, false);
        modules.set(Module::ReceivedSound, false);
        modules.set(Module::ReceivedSound, true);

        let reopened = Modules::at(Some(path));
        assert!(!reopened.is_on(Module::Dither));
        assert!(reopened.is_on(Module::SentSound));
        assert!(reopened.is_on(Module::ReceivedSound));
    }

    #[test]
    fn clone_ve_a_mudanca_na_hora() {
        let modules = Modules::at(None);
        let other = modules.clone();
        modules.set(Module::SentSound, false);
        assert!(!other.is_on(Module::SentSound));
    }

    #[test]
    fn arquivo_estragado_ou_desconhecido_fica_ligado() {
        assert!(parse("isso não é json").is_empty());
        assert_eq!(
            parse(r#"{"dither": false, "sala-3d": false, "som-enviada": true}"#),
            vec![Module::Dither]
        );
    }
}

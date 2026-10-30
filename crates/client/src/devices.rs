//! Dispositivos da chamada: qual microfone, qual saída de áudio e qual
//! câmera usar, e se a tela compartilhada leva o som junto.
//!
//! A escolha fica num arquivo do perfil (`padrao.dispositivos.json`), como
//! os módulos. Nada escolhido (ou o escolhido desconectado) vale como o
//! padrão do sistema.
//!
//! O microfone e a saída são guardados pelo id do WebRTC, que não muda ao
//! tirar e pôr o fone, e pelo nome, que é o que o `cpal` das mensagens de
//! áudio enxerga. A câmera é guardada pelo nome: a posição dela na lista
//! muda quando outra câmera é ligada.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use nokhwa::utils::{ApiBackend, CameraIndex};
use serde_json::{Map, Value, json};

/// Um dispositivo escolhido.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Choice {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prefs {
    pub mic: Option<Choice>,
    pub speaker: Option<Choice>,
    pub camera: Option<Choice>,
    /// Compartilhar o som junto com a tela.
    pub share_audio: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            mic: None,
            speaker: None,
            camera: None,
            share_audio: true,
        }
    }
}

/// Que dispositivo uma escolha das configurações troca.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Mic,
    Speaker,
    Camera,
}

impl Kind {
    pub fn from_id(id: &str) -> Option<Kind> {
        match id {
            "mic" => Some(Kind::Mic),
            "speaker" => Some(Kind::Speaker),
            "camera" => Some(Kind::Camera),
            _ => None,
        }
    }
}

/// As escolhas, compartilhadas entre a interface, a rede e a chamada.
/// Clonar é barato: todos os clones veem a mesma coisa.
#[derive(Clone)]
pub struct Devices {
    path: Option<PathBuf>,
    prefs: Arc<Mutex<Prefs>>,
}

impl Devices {
    pub fn for_profile(profile: &str) -> Self {
        Self::at(crate::session::profile_file(profile, "dispositivos.json"))
    }

    fn at(path: Option<PathBuf>) -> Self {
        let prefs = path
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map(|text| parse(&text))
            .unwrap_or_default();
        Self {
            path,
            prefs: Arc::new(Mutex::new(prefs)),
        }
    }

    pub fn get(&self) -> Prefs {
        self.prefs.lock().unwrap().clone()
    }

    /// Troca um dispositivo (`None`: o padrão do sistema) e já grava.
    pub fn set(&self, kind: Kind, choice: Option<Choice>) {
        self.change(|prefs| match kind {
            Kind::Mic => prefs.mic = choice,
            Kind::Speaker => prefs.speaker = choice,
            Kind::Camera => prefs.camera = choice,
        });
    }

    pub fn set_share_audio(&self, on: bool) {
        self.change(|prefs| prefs.share_audio = on);
    }

    fn change(&self, edit: impl FnOnce(&mut Prefs)) {
        let mut prefs = self.prefs.lock().unwrap();
        edit(&mut prefs);
        // Falhar ao salvar não trava o app: só volta o padrão na próxima vez.
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, render(&prefs));
    }
}

/// Arquivo estragado vale como tudo no padrão.
fn parse(text: &str) -> Prefs {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text) else {
        return Prefs::default();
    };
    let choice = |key: &str| {
        let entry = map.get(key)?;
        let name = entry.get("nome")?.as_str()?.to_owned();
        let id = entry
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        Some(Choice { id, name })
    };
    Prefs {
        mic: choice("microfone"),
        speaker: choice("saida"),
        camera: choice("camera"),
        share_audio: map
            .get("som-da-tela")
            .and_then(Value::as_bool)
            .unwrap_or(true),
    }
}

fn render(prefs: &Prefs) -> String {
    let choice = |c: &Option<Choice>| match c {
        Some(c) => json!({ "id": c.id, "nome": c.name }),
        None => Value::Null,
    };
    let mut map = Map::new();
    map.insert("microfone".into(), choice(&prefs.mic));
    map.insert("saida".into(), choice(&prefs.speaker));
    map.insert("camera".into(), choice(&prefs.camera));
    map.insert("som-da-tela".into(), prefs.share_audio.into());
    serde_json::to_string_pretty(&Value::Object(map)).unwrap_or_default()
}

/// Os dispositivos ligados agora, para as listas das configurações.
#[derive(Debug, Clone, Default)]
pub struct Available {
    pub mics: Vec<Choice>,
    pub speakers: Vec<Choice>,
    pub cameras: Vec<Choice>,
}

/// Procura os microfones, as saídas e as câmeras. Demora um pouco (abre o
/// som do WebRTC e pergunta às câmeras), então roda fora da thread da tela.
pub fn available() -> Available {
    let mut found = Available::default();
    match livekit::PlatformAudio::new() {
        Ok(audio) => {
            found.mics = dedup(
                audio
                    .recording_devices()
                    .map(|d| Choice {
                        id: d.id.as_str().to_owned(),
                        name: d.name,
                    })
                    .collect(),
            );
            found.speakers = dedup(
                audio
                    .playout_devices()
                    .map(|d| Choice {
                        id: d.id.as_str().to_owned(),
                        name: d.name,
                    })
                    .collect(),
            );
        }
        Err(err) => eprintln!("não deu para listar o som: {err}"),
    }
    match nokhwa::query(ApiBackend::Auto) {
        Ok(cameras) => {
            found.cameras = dedup(
                cameras
                    .into_iter()
                    .map(|c| Choice {
                        id: c.index().to_string(),
                        name: c.human_name(),
                    })
                    .collect(),
            );
        }
        Err(err) => eprintln!("não deu para listar as câmeras: {err}"),
    }
    found
}

/// Tira os repetidos e as entradas "Default" e "Communication" que o WebRTC
/// do Windows põe na lista: elas são o "padrão do sistema", que já é a
/// primeira opção da tela.
fn dedup(list: Vec<Choice>) -> Vec<Choice> {
    let mut out: Vec<Choice> = Vec::new();
    for choice in list {
        let name = choice.name.trim();
        if name.is_empty()
            || name.starts_with("Default - ")
            || name.starts_with("Communication - ")
            || out.iter().any(|c| c.name == choice.name)
        {
            continue;
        }
        out.push(choice);
    }
    out
}

/// Onde a escolha está na lista: 0 é o padrão do sistema, 1 em diante é a
/// lista. Acha pelo id e, se ele mudou, pelo nome. Desconectado vale como 0.
pub fn position(list: &[Choice], chosen: &Option<Choice>) -> usize {
    let Some(chosen) = chosen else { return 0 };
    list.iter()
        .position(|c| !chosen.id.is_empty() && c.id == chosen.id)
        .or_else(|| list.iter().position(|c| c.name == chosen.name))
        .map_or(0, |i| i + 1)
}

/// A câmera para o `nokhwa`: a escolhida, se estiver ligada, ou a primeira.
pub fn camera_index(chosen: &Option<Choice>) -> CameraIndex {
    let first = CameraIndex::Index(0);
    let Some(chosen) = chosen else { return first };
    let Ok(cameras) = nokhwa::query(ApiBackend::Auto) else {
        return first;
    };
    cameras
        .into_iter()
        .find(|c| c.human_name() == chosen.name)
        .map_or(first, |c| c.index().clone())
}

/// O nome do microfone padrão do sistema, como o `cpal` vê. No Windows é o
/// mesmo nome amigável que o WebRTC mostra.
pub fn default_mic_name() -> Option<String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    cpal::default_host().default_input_device()?.name().ok()
}

pub fn default_speaker_name() -> Option<String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    cpal::default_host().default_output_device()?.name().ok()
}

/// O dispositivo do `cpal` para as mensagens de áudio: o escolhido, achado
/// pelo nome, ou o padrão do sistema.
pub fn cpal_input(chosen: &Option<Choice>) -> Option<cpal::Device> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    chosen
        .as_ref()
        .and_then(|c| {
            host.input_devices()
                .ok()?
                .find(|d| d.name().is_ok_and(|n| n == c.name))
        })
        .or_else(|| host.default_input_device())
}

pub fn cpal_output(chosen: &Option<Choice>) -> Option<cpal::Device> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    chosen
        .as_ref()
        .and_then(|c| {
            host.output_devices()
                .ok()?
                .find(|d| d.name().is_ok_and(|n| n == c.name))
        })
        .or_else(|| host.default_output_device())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: &str, name: &str) -> Choice {
        Choice {
            id: id.into(),
            name: name.into(),
        }
    }

    fn temp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mothh-disp-{}", std::process::id()));
        let path = dir.join(name);
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn sem_arquivo_tudo_no_padrao_e_som_ligado() {
        let devices = Devices::at(Some(temp_file("nenhum.json")));
        assert_eq!(devices.get(), Prefs::default());
        assert!(devices.get().share_audio);
    }

    #[test]
    fn escolha_continua_ao_abrir_de_novo() {
        let path = temp_file("reabrir.json");
        let devices = Devices::at(Some(path.clone()));
        devices.set(Kind::Mic, Some(c("{guid-1}", "Microfone (USB)")));
        devices.set(Kind::Camera, Some(c("1", "Webcam C920")));
        devices.set(Kind::Camera, None);
        devices.set_share_audio(false);

        let prefs = Devices::at(Some(path)).get();
        assert_eq!(prefs.mic, Some(c("{guid-1}", "Microfone (USB)")));
        assert_eq!(prefs.speaker, None);
        assert_eq!(prefs.camera, None);
        assert!(!prefs.share_audio);
    }

    #[test]
    fn arquivo_estragado_fica_no_padrao() {
        assert_eq!(parse("isso não é json"), Prefs::default());
        assert_eq!(parse(r#"{"microfone": 3}"#), Prefs::default());
    }

    #[test]
    fn posicao_acha_pelo_id_e_depois_pelo_nome() {
        let list = [c("a", "Fone"), c("b", "Microfone (USB)")];
        assert_eq!(position(&list, &None), 0);
        assert_eq!(position(&list, &Some(c("b", "outro nome"))), 2);
        // O id mudou (Windows reinstalou o driver), o nome não.
        assert_eq!(position(&list, &Some(c("z", "Fone"))), 1);
        // Desconectado: o padrão.
        assert_eq!(position(&list, &Some(c("z", "Headset"))), 0);
    }

    #[test]
    fn lista_sem_repetidos_nem_entradas_de_padrao() {
        let list = dedup(vec![
            c("0", "Default - Fone"),
            c("1", "Communication - Fone"),
            c("2", "Fone"),
            c("3", "Fone"),
            c("4", " "),
        ]);
        assert_eq!(list, [c("2", "Fone")]);
    }
}

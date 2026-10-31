//! Volume de cada pessoa na chamada: a voz e o som da tela que ela mostra,
//! cada um de 0% a 200%.
//!
//! A escolha vale para quem ouve (só muda o som aqui, não o dos outros) e
//! fica guardada por pessoa num arquivo do perfil (`padrao.volumes.json`),
//! então quem fala baixo continua mais alto na próxima chamada.
//!
//! O som de quem está na chamada é tocado pelo próprio WebRTC, que mistura
//! todo mundo antes do alto-falante e usa essa mistura no cancelamento de
//! eco. O volume entra antes da mistura, em cada faixa (`cpp/volume.cpp`).

use std::collections::HashMap;
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use cxx::SharedPtr;
use livekit::webrtc::audio_track::RtcAudioTrack;
use serde_json::{Map, Value, json};
use tokio::sync::Notify;
use webrtc_sys::audio_track::ffi::AudioTrack;

/// 100%: o som como chega.
pub const NORMAL: i32 = 100;
pub const MAX: i32 = 200;

/// Qual som da pessoa.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Voice,
    Screen,
}

/// Os volumes do perfil aberto, um só para o app inteiro: a tela, a rede e
/// a chamada leem e mexem nele.
static SHARED: OnceLock<Volumes> = OnceLock::new();

/// Abre os volumes do perfil. Chamar uma vez, ao abrir o app.
pub fn init(profile: &str) {
    let _ = SHARED.set(Volumes::for_profile(profile));
}

/// Os volumes do perfil (sem `init`, valem só enquanto o app está aberto).
pub fn shared() -> &'static Volumes {
    SHARED.get_or_init(|| Volumes::at(None))
}

pub struct Volumes {
    path: Option<PathBuf>,
    /// Por id da pessoa: (voz, tela), em %.
    levels: Arc<Mutex<HashMap<String, (i32, i32)>>>,
    /// Acorda a chamada para aplicar o que mudou.
    changed: Arc<Notify>,
}

impl Volumes {
    pub fn for_profile(profile: &str) -> Self {
        Self::at(crate::session::profile_file(profile, "volumes.json"))
    }

    fn at(path: Option<PathBuf>) -> Self {
        let levels = path
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map(|text| parse(&text))
            .unwrap_or_default();
        Self {
            path,
            levels: Arc::new(Mutex::new(levels)),
            changed: Arc::new(Notify::new()),
        }
    }

    /// O volume (em %) de um som de alguém; 100% se nunca mexeu.
    pub fn get(&self, person: &str, kind: Kind) -> i32 {
        let levels = self.levels.lock().unwrap();
        let (voice, screen) = levels.get(person).copied().unwrap_or((NORMAL, NORMAL));
        match kind {
            Kind::Voice => voice,
            Kind::Screen => screen,
        }
    }

    /// Muda, grava e avisa a chamada.
    pub fn set(&self, person: &str, kind: Kind, percent: i32) {
        let percent = percent.clamp(0, MAX);
        {
            let mut levels = self.levels.lock().unwrap();
            let entry = levels.entry(person.to_owned()).or_insert((NORMAL, NORMAL));
            match kind {
                Kind::Voice => entry.0 = percent,
                Kind::Screen => entry.1 = percent,
            }
            // Quem voltou para o normal nos dois não precisa ficar no arquivo.
            if *entry == (NORMAL, NORMAL) {
                levels.remove(person);
            }
            // Falhar ao salvar não trava o app: só não lembra na próxima vez.
            if let Some(path) = &self.path {
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(path, render(&levels));
            }
        }
        self.changed.notify_one();
    }

    /// Espera alguém mexer num volume.
    pub async fn changed(&self) {
        self.changed.notified().await;
    }
}

/// Arquivo estragado vale como tudo em 100%.
fn parse(text: &str) -> HashMap<String, (i32, i32)> {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text) else {
        return HashMap::new();
    };
    let level = |entry: &Value, key: &str| {
        entry
            .get(key)
            .and_then(Value::as_i64)
            .map_or(NORMAL, |v| v.clamp(0, MAX.into()) as i32)
    };
    map.iter()
        .map(|(id, entry)| (id.clone(), (level(entry, "voz"), level(entry, "tela"))))
        .collect()
}

fn render(levels: &HashMap<String, (i32, i32)>) -> String {
    let map: Map<String, Value> = levels
        .iter()
        .map(|(id, (voice, screen))| (id.clone(), json!({ "voz": voice, "tela": screen })))
        .collect();
    serde_json::to_string_pretty(&Value::Object(map)).unwrap_or_default()
}

unsafe extern "C" {
    fn mothh_set_track_volume(livekit_track: *const c_void, volume: f64) -> bool;
}

// O `RtcAudioTrack` do libwebrtc é só um invólucro de um campo em volta do
// `SharedPtr<AudioTrack>` do webrtc-sys, mas não deixa ninguém de fora pegar
// nele. As versões estão presas no Cargo.toml; se o desenho mudar, isto
// para de compilar em vez de ler a memória errada.
const _: () = assert!(size_of::<RtcAudioTrack>() == size_of::<SharedPtr<AudioTrack>>());

/// Põe o volume (em %) numa faixa de áudio recebida.
pub fn apply(track: &RtcAudioTrack, percent: i32) -> bool {
    // SAFETY: mesmo tamanho (conferido acima) e o único campo é o
    // `SharedPtr`. O clone soma uma referência e o `SharedPtr` devolve ela
    // ao sair daqui.
    let handle: SharedPtr<AudioTrack> = unsafe { std::mem::transmute(track.clone()) };
    let Some(native) = handle.as_ref() else {
        return false;
    };
    let gain = f64::from(percent.clamp(0, MAX)) / f64::from(NORMAL);
    // SAFETY: `native` vive enquanto `handle` existir; a função só lê a
    // faixa e chama o `SetVolume` do WebRTC, que serve de qualquer thread.
    unsafe { mothh_set_track_volume(std::ptr::from_ref(native).cast(), gain) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guarda_e_le_de_volta() {
        let dir = std::env::temp_dir().join(format!("mothh-volumes-{}", std::process::id()));
        let path = dir.join("teste.volumes.json");
        let volumes = Volumes::at(Some(path.clone()));
        assert_eq!(volumes.get("ana", Kind::Voice), NORMAL);
        volumes.set("ana", Kind::Voice, 40);
        volumes.set("ana", Kind::Screen, 999);
        let again = Volumes::at(Some(path.clone()));
        assert_eq!(again.get("ana", Kind::Voice), 40);
        assert_eq!(again.get("ana", Kind::Screen), MAX);
        assert_eq!(again.get("bia", Kind::Screen), NORMAL);
        // Voltou tudo para 100%: sai do arquivo.
        again.set("ana", Kind::Voice, NORMAL);
        again.set("ana", Kind::Screen, NORMAL);
        assert_eq!(
            parse(&std::fs::read_to_string(&path).unwrap()),
            HashMap::new()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn arquivo_estragado_vale_100() {
        assert!(parse("isso não é json").is_empty());
        let levels = parse(r#"{"ana": {"voz": -5}}"#);
        assert_eq!(levels["ana"], (0, NORMAL));
    }

    /// O volume muda mesmo o som que sai no alto-falante. Precisa de um
    /// LiveKit (`MOTHH_LIVEKIT_URL`/`KEY`/`SECRET`) e de um PulseAudio com a
    /// saída padrão num "null sink", cujo `.monitor` o teste grava com o
    /// `parec` (ver o teste da F2-04 nas notas do projeto).
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "precisa de um LiveKit e de um PulseAudio"]
    async fn volume_muda_o_que_sai_no_alto_falante() {
        use std::io::Read;
        use std::process::{Command, Stdio};
        use std::time::Duration;

        use livekit::options::TrackPublishOptions;
        use livekit::prelude::*;
        use livekit::webrtc::audio_frame::AudioFrame;
        use livekit::webrtc::audio_source::native::NativeAudioSource;
        use livekit::webrtc::audio_source::{AudioSourceOptions, RtcAudioSource};

        const RATE: u32 = 48_000;

        let var = |name| std::env::var(name).unwrap_or_else(|_| panic!("falta {name}"));
        let url = var("MOTHH_LIVEKIT_URL");
        let api_key = var("MOTHH_LIVEKIT_KEY");
        let api_secret = var("MOTHH_LIVEKIT_SECRET");
        let room_name = format!("volume-{}", uuid::Uuid::new_v4());
        let token = |who: &str| {
            livekit_api::access_token::AccessToken::with_api_key(&api_key, &api_secret)
                .with_identity(who)
                .with_grants(livekit_api::access_token::VideoGrants {
                    room_join: true,
                    room: room_name.clone(),
                    ..Default::default()
                })
                .to_jwt()
                .unwrap()
        };

        // Quem ouve toca no alto-falante, como o app.
        let _audio = PlatformAudio::new().unwrap();
        let (fala, _) = Room::connect(&url, &token("fala"), RoomOptions::default())
            .await
            .unwrap();
        let (_ouve, mut events) = Room::connect(&url, &token("ouve"), RoomOptions::default())
            .await
            .unwrap();

        let source = NativeAudioSource::new(
            AudioSourceOptions {
                echo_cancellation: false,
                noise_suppression: false,
                auto_gain_control: false,
            },
            RATE,
            1,
            100,
        );
        let track =
            LocalAudioTrack::create_audio_track("tom", RtcAudioSource::Native(source.clone()));
        fala.local_participant()
            .publish_track(
                LocalTrack::Audio(track),
                TrackPublishOptions {
                    source: TrackSource::Microphone,
                    dtx: false,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let tone = tokio::spawn(async move {
            let mut n = 0u64;
            loop {
                let mut frame = AudioFrame::new(RATE, 1, RATE / 100);
                for sample in frame.data.to_mut() {
                    let t = n as f32 / RATE as f32;
                    *sample = ((t * 440.0 * std::f32::consts::TAU).sin() * 6000.0) as i16;
                    n += 1;
                }
                source.capture_frame(&frame).await.unwrap();
            }
        });

        let heard = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if let Some(RoomEvent::TrackSubscribed {
                    track: RemoteTrack::Audio(track),
                    ..
                }) = events.recv().await
                {
                    return track;
                }
            }
        })
        .await
        .unwrap();

        // Grava um segundo do que sai no alto-falante e mede o volume (RMS).
        let loudness = || {
            let mut rec = Command::new("parec")
                .args(["--raw", "--format=s16le", "--rate=48000", "--channels=1"])
                .arg(format!(
                    "--device={}.monitor",
                    std::env::var("MOTHH_TESTE_SAIDA").unwrap_or("saida".into())
                ))
                .stdout(Stdio::piped())
                .spawn()
                .expect("falta o parec");
            std::thread::sleep(Duration::from_millis(1200));
            rec.kill().unwrap();
            let mut raw = Vec::new();
            rec.stdout.take().unwrap().read_to_end(&mut raw).unwrap();
            let _ = rec.wait();
            // Pula o começo, que pode ter som de antes da troca.
            let samples: Vec<f64> = raw
                .as_chunks::<2>()
                .0
                .iter()
                .skip(RATE as usize / 5)
                .map(|b| f64::from(i16::from_le_bytes(*b)))
                .collect();
            (samples.iter().map(|s| s * s).sum::<f64>() / samples.len().max(1) as f64).sqrt()
        };
        let settle = || tokio::time::sleep(Duration::from_millis(600));

        settle().await;
        settle().await;
        let normal = tokio::task::block_in_place(loudness);
        assert!(normal > 500.0, "não saiu som nenhum: {normal}");

        assert!(apply(&heard.rtc_track(), 25));
        settle().await;
        let low = tokio::task::block_in_place(loudness);
        assert!(apply(&heard.rtc_track(), 0));
        settle().await;
        let muted = tokio::task::block_in_place(loudness);
        assert!(apply(&heard.rtc_track(), 200));
        settle().await;
        let loud = tokio::task::block_in_place(loudness);
        eprintln!("100%: {normal:.0}  25%: {low:.0}  0%: {muted:.0}  200%: {loud:.0}");
        tone.abort();

        assert!(
            (0.15..0.35).contains(&(low / normal)),
            "25% saiu {low} contra {normal}"
        );
        assert!(muted < normal * 0.02, "0% ainda toca: {muted}");
        assert!(
            (1.6..2.4).contains(&(loud / normal)),
            "200% saiu {loud} contra {normal}"
        );
    }
}

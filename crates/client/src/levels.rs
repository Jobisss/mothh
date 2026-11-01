//! Nível da voz de cada pessoa na chamada, medido aqui no app.
//!
//! O LiveKit também diz quem fala (`ActiveSpeakersChanged`), mas quem mede é
//! o servidor dele, com suavização: o destaque chegava atrasado em relação à
//! voz. Aqui o nível sai do próprio áudio:
//! - dos outros, de um "sink" em cada faixa de voz recebida (`call.rs`), que
//!   o WebRTC alimenta na hora em que mistura o som para o alto-falante, então
//!   o nível anda junto com o que se ouve;
//! - o seu, do microfone, no mesmo desvio da redução de ruído (`noise.rs`).
//!
//! Cada pedaço de 10 ms vira um número de 0 a 1 (`level_of`). A janela lê
//! esses números umas 30 vezes por segundo e faz a borda da foto crescer
//! (`main.rs`); a chamada usa os mesmos números para dizer quem está falando.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Chave do seu microfone (o resto é o id de cada pessoa).
pub const ME: &str = "";

/// Abaixo disso é silêncio (dBFS do pedaço de 10 ms).
const FLOOR_DB: f32 = -50.0;
/// Daqui para cima é o máximo da borda.
const TOP_DB: f32 = -14.0;
/// A partir deste nível conta como "falando".
pub const SPEAKING: f32 = 0.22;
/// Nível que não foi atualizado há mais que isso já é silêncio (a faixa
/// parou de mandar áudio, a pessoa mutou...).
const STALE: Duration = Duration::from_millis(150);

static LEVELS: Mutex<Option<HashMap<String, (f32, Instant)>>> = Mutex::new(None);
static MUTED: AtomicBool = AtomicBool::new(false);

/// Número de 0 a 1 para um pedaço de áudio (amostras de 16 bits).
pub fn level_of(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let energy: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    let rms = (energy / samples.len() as f64).sqrt() / 32768.0;
    let db = 20.0 * (rms.max(1e-9)).log10() as f32;
    ((db - FLOOR_DB) / (TOP_DB - FLOOR_DB)).clamp(0.0, 1.0)
}

/// Guarda o nível de agora de alguém (`ME` para o microfone).
pub fn feed(id: &str, samples: &[i16]) {
    let level = if id == ME && MUTED.load(Ordering::Relaxed) {
        0.0
    } else {
        level_of(samples)
    };
    if let Ok(mut levels) = LEVELS.lock() {
        levels
            .get_or_insert_with(HashMap::new)
            .insert(id.to_string(), (level, Instant::now()));
    }
}

/// Microfone mudo: o seu nível fica zero mesmo que o som continue chegando.
pub fn set_muted(muted: bool) {
    MUTED.store(muted, Ordering::Relaxed);
}

/// Esquece alguém (saiu da chamada) ou todo mundo (`None`, a chamada acabou).
pub fn forget(id: Option<&str>) {
    if let Ok(mut levels) = LEVELS.lock()
        && let Some(map) = levels.as_mut()
    {
        match id {
            Some(id) => {
                map.remove(id);
            }
            None => map.clear(),
        }
    }
}

/// O nível de cada um agora; quem não mandou áudio há pouco vale 0.
pub fn snapshot() -> HashMap<String, f32> {
    let now = Instant::now();
    LEVELS
        .lock()
        .ok()
        .and_then(|levels| levels.clone())
        .unwrap_or_default()
        .into_iter()
        .map(|(id, (level, at))| (id, if now - at > STALE { 0.0 } else { level }))
        .collect()
}

/// Se temos medição recente de alguém (para saber se dá para confiar nela
/// em vez do aviso do LiveKit).
pub fn measured(id: &str) -> bool {
    let now = Instant::now();
    LEVELS
        .lock()
        .ok()
        .and_then(|levels| {
            levels
                .as_ref()
                .and_then(|m| m.get(id).map(|&(_, at)| now - at < Duration::from_secs(1)))
        })
        .unwrap_or(false)
}

/// Suaviza o nível para a tela: sobe na hora e desce aos poucos, para a
/// borda não tremer entre uma sílaba e outra.
pub fn smooth(shown: f32, now: f32) -> f32 {
    if now >= shown {
        now
    } else {
        (shown * 0.78).max(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_is_zero_and_loud_is_one() {
        assert_eq!(level_of(&[0; 480]), 0.0);
        assert_eq!(level_of(&[]), 0.0);
        let loud: Vec<i16> = (0..480)
            .map(|i| if i % 2 == 0 { 20000 } else { -20000 })
            .collect();
        assert_eq!(level_of(&loud), 1.0);
        // Voz normal (uns -26 dBFS) fica no meio.
        let normal: Vec<i16> = (0..480)
            .map(|i| if i % 2 == 0 { 1600 } else { -1600 })
            .collect();
        let l = level_of(&normal);
        assert!(l > SPEAKING && l < 1.0, "{l}");
    }

    #[test]
    fn muted_microphone_reads_zero() {
        let loud = [20000i16; 480];
        set_muted(true);
        feed(ME, &loud);
        assert_eq!(snapshot()[ME], 0.0);
        set_muted(false);
        feed(ME, &loud);
        assert_eq!(snapshot()[ME], 1.0);
        forget(Some(ME));
        assert!(!snapshot().contains_key(ME));
    }

    #[test]
    fn smoothing_rises_at_once_and_falls_slowly() {
        assert_eq!(smooth(0.1, 0.9), 0.9);
        let after = smooth(0.9, 0.0);
        assert!(after > 0.6 && after < 0.9, "{after}");
    }
}

//! Sons escolhidos para mensagem recebida e digitação.
//!
//! Os MP3 originais estão em assets/sounds. Os PCM mono são a versão pronta
//! para tocar: uma thread segura a saída de áudio sem travar a interface.

use std::sync::OnceLock;
use std::sync::mpsc::{self, Sender};

use rodio::buffer::SamplesBuffer;
use slint::ComponentHandle;

use crate::{AppWindow, Modules};

const SAMPLE_RATE: u32 = 24_000;
const RECEIVED_PCM: &[u8] = include_bytes!("../assets/sounds/received.pcm");
const TYPING_PCM: &[u8] = include_bytes!("../assets/sounds/typing.pcm");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sound {
    Received,
    Typing,
}

/// Consulta o interruptor na thread da interface, para valer imediatamente.
pub fn play_if_on(ui: &slint::Weak<AppWindow>, sound: Sound) {
    let _ = ui.upgrade_in_event_loop(move |ui| {
        let modules = ui.global::<Modules>();
        let on = match sound {
            Sound::Received => modules.get_received_sound(),
            Sound::Typing => modules.get_typing_sound(),
        };
        if on {
            play(sound);
        }
    });
}

fn play(sound: Sound) {
    static PLAYER: OnceLock<Sender<Sound>> = OnceLock::new();
    let player = PLAYER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Sound>();
        std::thread::spawn(move || {
            let Ok(mut stream) = rodio::OutputStreamBuilder::open_default_stream() else {
                return;
            };
            stream.log_on_drop(false);
            for sound in rx {
                let volume = match sound {
                    Sound::Received => 1.0,
                    // Teclas rápidas podem sobrepor vários sons.
                    Sound::Typing => 0.35,
                };
                let samples: Vec<f32> = samples(sound).iter().map(|s| s * volume).collect();
                stream
                    .mixer()
                    .add(SamplesBuffer::new(1, SAMPLE_RATE, samples));
            }
        });
        tx
    });
    let _ = player.send(sound);
}

fn samples(sound: Sound) -> &'static [f32] {
    static RECEIVED: OnceLock<Vec<f32>> = OnceLock::new();
    static TYPING: OnceLock<Vec<f32>> = OnceLock::new();
    match sound {
        Sound::Received => RECEIVED.get_or_init(|| decode_pcm(RECEIVED_PCM)),
        Sound::Typing => TYPING.get_or_init(|| decode_pcm(TYPING_PCM)),
    }
}

fn decode_pcm(bytes: &[u8]) -> Vec<f32> {
    let (chunks, remainder) = bytes.as_chunks::<2>();
    debug_assert!(remainder.is_empty());
    chunks
        .iter()
        .map(|chunk| i16::from_le_bytes(*chunk) as f32 / 32768.0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_sounds_have_expected_lengths() {
        let received_ms = samples(Sound::Received).len() * 1000 / SAMPLE_RATE as usize;
        let typing_ms = samples(Sound::Typing).len() * 1000 / SAMPLE_RATE as usize;
        assert!((500..=700).contains(&received_ms), "{received_ms}ms");
        assert!((100..=250).contains(&typing_ms), "{typing_ms}ms");
    }

    #[test]
    fn bundled_sounds_are_distinct_and_in_range() {
        assert_ne!(samples(Sound::Received), samples(Sound::Typing));
        for sound in [Sound::Received, Sound::Typing] {
            assert!(samples(sound).iter().all(|s| s.abs() <= 1.0));
        }
    }
}

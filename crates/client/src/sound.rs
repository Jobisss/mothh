//! Sons curtos com cara de chip 8-bit: mensagem enviada e recebida.
//!
//! Os sons são gerados aqui mesmo (onda quadrada), sem arquivo de áudio.
//! Uma thread própria segura a saída de áudio: no Windows ela não pode mudar
//! de thread, e assim tocar um som nunca trava a interface nem a rede.
//! Sem placa de som, a thread desiste e os sons ficam mudos, sem erro.

use std::sync::OnceLock;
use std::sync::mpsc::{self, Sender};

use rodio::buffer::SamplesBuffer;
use slint::ComponentHandle;

use crate::{AppWindow, Modules};

const SAMPLE_RATE: u32 = 44_100;
/// Volume da onda quadrada. Quadrada soa alto; 0.15 já basta.
const VOLUME: f32 = 0.15;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sound {
    /// O servidor confirmou a mensagem que eu mandei.
    Sent,
    /// Chegou mensagem de outra pessoa.
    Received,
}

impl Sound {
    /// Notas do som: (frequência em Hz, duração em ms). Enviada sobe,
    /// recebida desce, para dar para distinguir sem olhar.
    fn notes(self) -> &'static [(f32, u32)] {
        match self {
            Sound::Sent => &[(660.0, 50), (990.0, 80)],
            Sound::Received => &[(1175.0, 60), (880.0, 60), (659.0, 110)],
        }
    }
}

/// Toca o som se o interruptor dele estiver ligado. Pode ser chamada de
/// qualquer thread: a decisão é feita na thread da interface, lendo o
/// `Modules` do Slint, então desligar vale na hora.
pub fn play_if_on(ui: &slint::Weak<AppWindow>, sound: Sound) {
    let _ = ui.upgrade_in_event_loop(move |ui| {
        let modules = ui.global::<Modules>();
        let on = match sound {
            Sound::Sent => modules.get_sent_sound(),
            Sound::Received => modules.get_received_sound(),
        };
        if on {
            play(sound);
        }
    });
}

/// Manda o som para a thread de áudio, que nasce no primeiro uso.
fn play(sound: Sound) {
    static PLAYER: OnceLock<Sender<Sound>> = OnceLock::new();
    let player = PLAYER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Sound>();
        std::thread::spawn(move || {
            let Ok(mut stream) = rodio::OutputStreamBuilder::open_default_stream() else {
                return;
            };
            // Sem isto o rodio escreve no terminal quando o app fecha.
            stream.log_on_drop(false);
            for sound in rx {
                stream
                    .mixer()
                    .add(SamplesBuffer::new(1, SAMPLE_RATE, synth(sound)));
            }
        });
        tx
    });
    let _ = player.send(sound);
}

/// Amostras do som, em mono.
fn synth(sound: Sound) -> Vec<f32> {
    let mut samples = Vec::new();
    for &(freq, ms) in sound.notes() {
        let len = (SAMPLE_RATE * ms / 1000) as usize;
        // Subida e descida rápidas nas pontas de cada nota, para não estalar.
        let attack = (SAMPLE_RATE as usize) * 2 / 1000;
        let release = (SAMPLE_RATE as usize) * 15 / 1000;
        let period = SAMPLE_RATE as f32 / freq;
        for i in 0..len {
            let wave = if (i as f32 % period) < period / 2.0 {
                1.0
            } else {
                -1.0
            };
            let envelope = (i as f32 / attack as f32)
                .min((len - i) as f32 / release as f32)
                .min(1.0);
            samples.push(wave * envelope * VOLUME);
        }
    }
    samples
}

#[cfg(test)]
mod tests {
    use super::*;

    fn duration_ms(sound: Sound) -> usize {
        synth(sound).len() * 1000 / SAMPLE_RATE as usize
    }

    #[test]
    fn sounds_are_at_most_400ms() {
        for sound in [Sound::Sent, Sound::Received] {
            let ms = duration_ms(sound);
            assert!(ms > 0 && ms <= 400, "{sound:?} tem {ms}ms");
        }
    }

    #[test]
    fn samples_stay_within_volume() {
        for sound in [Sound::Sent, Sound::Received] {
            assert!(synth(sound).iter().all(|s| s.abs() <= VOLUME));
        }
    }

    #[test]
    fn notes_start_and_end_silent() {
        let samples = synth(Sound::Sent);
        assert_eq!(samples[0], 0.0);
        assert!(samples.last().unwrap().abs() < 0.01);
    }

    #[test]
    fn sent_and_received_sound_different() {
        assert_ne!(synth(Sound::Sent), synth(Sound::Received));
    }
}

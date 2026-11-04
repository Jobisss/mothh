//! Som de mensagem recebida, escolhido pelo Jobiss, e os dois toques curtos
//! de quando alguém entra ou sai da chamada (gerados aqui, sem arquivo).
//!
//! O MP3 original está em assets/sounds; o `.pcm` (mono, 16 bits, 24 kHz) é a
//! versão pronta para tocar, embutida no executável.
//!
//! Só toca para mensagem de outra pessoa; enviar e digitar ficam em silêncio.
//! Uma thread própria segura a saída de áudio, assim tocar nunca trava a
//! interface. Sem placa de som, a thread desiste e o app fica mudo, sem erro.

use std::sync::OnceLock;
use std::sync::mpsc::{self, Sender};

use rodio::buffer::SamplesBuffer;
use slint::ComponentHandle;

use crate::{AppWindow, Modules};

const SAMPLE_RATE: u32 = 24_000;
const RECEIVED_PCM: &[u8] = include_bytes!("../assets/sounds/received.pcm");

/// Toca o som de recebida se o interruptor estiver ligado. A decisão é feita
/// na thread da interface, lendo o `Modules` do Slint, então desligar vale na hora.
pub fn play_received_if_on(ui: &slint::Weak<AppWindow>) {
    let _ = ui.upgrade_in_event_loop(|ui| {
        if ui.global::<Modules>().get_received_sound() {
            play(samples().to_vec());
        }
    });
}

/// Entrada ou saída de alguém na chamada (inclusive a própria pessoa), e o
/// toque de uma chamada 1:1 chegando.
#[derive(Clone, Copy)]
pub enum Cue {
    Joined,
    Left,
    Ring,
}

/// Toca o toque de entrada/saída se o interruptor "som da chamada" estiver ligado.
pub fn play_call_if_on(ui: &slint::Weak<AppWindow>, cue: Cue) {
    let _ = ui.upgrade_in_event_loop(move |ui| {
        if ui.global::<Modules>().get_call_sound() {
            play(call_cue(cue).to_vec());
        }
    });
}

fn play(sound: Vec<f32>) {
    static PLAYER: OnceLock<Sender<Vec<f32>>> = OnceLock::new();
    let player = PLAYER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Vec<f32>>();
        std::thread::spawn(move || {
            let Ok(mut stream) = rodio::OutputStreamBuilder::open_default_stream() else {
                return;
            };
            // Sem isto o rodio escreve no terminal quando o app fecha.
            stream.log_on_drop(false);
            for sound in rx {
                stream
                    .mixer()
                    .add(SamplesBuffer::new(1, SAMPLE_RATE, sound));
            }
        });
        tx
    });
    let _ = player.send(sound);
}

fn samples() -> &'static [f32] {
    static RECEIVED: OnceLock<Vec<f32>> = OnceLock::new();
    RECEIVED.get_or_init(|| {
        let (chunks, remainder) = RECEIVED_PCM.as_chunks::<2>();
        debug_assert!(remainder.is_empty());
        chunks
            .iter()
            .map(|chunk| i16::from_le_bytes(*chunk) as f32 / 32768.0)
            .collect()
    })
}

/// Duas notas curtas e baixinhas: subindo ao entrar, descendo ao sair.
fn call_cue(cue: Cue) -> &'static [f32] {
    static JOINED: OnceLock<Vec<f32>> = OnceLock::new();
    static LEFT: OnceLock<Vec<f32>> = OnceLock::new();
    static RING: OnceLock<Vec<f32>> = OnceLock::new();
    const LOW: f32 = 523.25; // dó
    const HIGH: f32 = 783.99; // sol
    match cue {
        Cue::Joined => JOINED.get_or_init(|| two_notes(LOW, HIGH)),
        Cue::Left => LEFT.get_or_init(|| two_notes(HIGH, LOW)),
        // Toque de chamada 1:1: o "plim" de entrada duas vezes, como um
        // telefone discreto. Curto de propósito: toca uma vez só.
        Cue::Ring => RING.get_or_init(|| {
            let mut out = two_notes(LOW, HIGH);
            out.extend(std::iter::repeat_n(0.0, SAMPLE_RATE as usize * 220 / 1000));
            out.extend(two_notes(LOW, HIGH));
            out
        }),
    }
}

fn two_notes(first: f32, second: f32) -> Vec<f32> {
    let mut out = note(first, 0.075);
    out.extend(std::iter::repeat_n(0.0, SAMPLE_RATE as usize * 15 / 1000));
    out.extend(note(second, 0.11));
    out
}

/// Seno com um pouco da oitava, ataque de 4 ms e queda suave: soa como um
/// "plim" de marimba, sem estalo no começo nem no fim.
fn note(freq: f32, seconds: f32) -> Vec<f32> {
    use std::f32::consts::TAU;
    const VOLUME: f32 = 0.16;
    let len = (SAMPLE_RATE as f32 * seconds) as usize;
    let attack = SAMPLE_RATE as f32 * 0.004;
    (0..len)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            let rise = (i as f32 / attack).min(1.0);
            let fall = 1.0 - i as f32 / len as f32;
            let env = rise * fall * fall;
            let wave = (TAU * freq * t).sin() + 0.25 * (TAU * 2.0 * freq * t).sin();
            VOLUME * env * wave / 1.25
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn received_sound_has_expected_length_and_range() {
        let ms = samples().len() * 1000 / SAMPLE_RATE as usize;
        assert!((500..=700).contains(&ms), "{ms}ms");
        assert!(samples().iter().all(|s| s.abs() <= 1.0));
    }

    #[test]
    fn call_cues_are_short_and_quiet() {
        for cue in [Cue::Joined, Cue::Left] {
            let sound = call_cue(cue);
            let ms = sound.len() * 1000 / SAMPLE_RATE as usize;
            assert!((150..=250).contains(&ms), "{ms}ms");
            assert!(sound.iter().all(|s| s.abs() <= 0.2));
        }
        // O toque é dois "plins" com uma pausa: menos de um segundo.
        let ring = call_cue(Cue::Ring);
        let ms = ring.len() * 1000 / SAMPLE_RATE as usize;
        assert!((500..=800).contains(&ms), "{ms}ms");
        assert!(ring.iter().all(|s| s.abs() <= 0.2));
    }
}

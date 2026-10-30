//! Som de mensagem recebida: um bipe curto e baixo, gerado no código.
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
/// Uma nota só, senoidal: discreta, sem cara de alarme.
const FREQ_HZ: f32 = 880.0;
const LENGTH_MS: u32 = 90;
const VOLUME: f32 = 0.12;

/// Toca o bipe de recebida se o interruptor estiver ligado. A decisão é feita
/// na thread da interface, lendo o `Modules` do Slint, então desligar vale na hora.
pub fn play_received_if_on(ui: &slint::Weak<AppWindow>) {
    let _ = ui.upgrade_in_event_loop(|ui| {
        if ui.global::<Modules>().get_received_sound() {
            play();
        }
    });
}

fn play() {
    static PLAYER: OnceLock<Sender<()>> = OnceLock::new();
    let player = PLAYER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<()>();
        std::thread::spawn(move || {
            let Ok(mut stream) = rodio::OutputStreamBuilder::open_default_stream() else {
                return;
            };
            // Sem isto o rodio escreve no terminal quando o app fecha.
            stream.log_on_drop(false);
            for () in rx {
                stream
                    .mixer()
                    .add(SamplesBuffer::new(1, SAMPLE_RATE, blip()));
            }
        });
        tx
    });
    let _ = player.send(());
}

/// Amostras do bipe: seno com subida rápida e descida suave, para não estalar.
fn blip() -> Vec<f32> {
    let len = (SAMPLE_RATE * LENGTH_MS / 1000) as usize;
    let attack = (SAMPLE_RATE * 5 / 1000) as usize;
    (0..len)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            let rise = (i as f32 / attack as f32).min(1.0);
            let fall = 1.0 - i as f32 / len as f32;
            (std::f32::consts::TAU * FREQ_HZ * t).sin() * rise * fall * fall * VOLUME
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blip_is_short_and_quiet() {
        let samples = blip();
        let ms = samples.len() * 1000 / SAMPLE_RATE as usize;
        assert!((50..=150).contains(&ms), "{ms}ms");
        assert!(samples.iter().all(|s| s.abs() <= VOLUME));
    }

    #[test]
    fn blip_starts_and_ends_silent() {
        let samples = blip();
        assert_eq!(samples[0], 0.0);
        assert!(samples.last().unwrap().abs() < 0.001);
    }
}

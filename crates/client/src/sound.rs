//! Som de mensagem recebida, escolhido pelo Jobiss.
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
                    .add(SamplesBuffer::new(1, SAMPLE_RATE, samples().to_vec()));
            }
        });
        tx
    });
    let _ = player.send(());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn received_sound_has_expected_length_and_range() {
        let ms = samples().len() * 1000 / SAMPLE_RATE as usize;
        assert!((500..=700).contains(&ms), "{ms}ms");
        assert!(samples().iter().all(|s| s.abs() <= 1.0));
    }
}

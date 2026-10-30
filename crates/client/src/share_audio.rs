//! Som da tela compartilhada (só no Windows por enquanto).
//!
//! O Windows 10 (2004 em diante) deixa gravar o som de um programa e dos
//! filhos dele, ou de tudo menos um programa (o "process loopback" do
//! WASAPI). Assim:
//! - tela inteira: todo o som do PC menos o do próprio mothh, senão quem
//!   está na chamada ia ouvir a própria voz de volta;
//! - janela: só o som do programa dono dela (o Chrome com o vídeo, por
//!   exemplo), mesmo que outros estejam tocando.
//!
//! O som vai como uma faixa à parte (`ScreenshareAudio`), estéreo a 48 kHz,
//! sem o cancelamento de eco e a redução de ruído do microfone, que
//! estragam música. Do outro lado ela toca sozinha, como as vozes.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use livekit::webrtc::audio_source::native::NativeAudioSource;

pub const RATE: u32 = 48_000;
pub const CHANNELS: u32 = 2;

/// De onde vem o som.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Tudo menos o mothh.
    Screen,
    /// Só o programa dono desta janela (o id da janela no `DesktopCapturer`,
    /// que no Windows é o HWND).
    Window(u64),
}

/// Este sistema sabe gravar o som da tela.
pub fn supported() -> bool {
    cfg!(windows)
}

/// O som sendo gravado. Largar para a gravação.
pub struct SoundCapture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for SoundCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Começa a gravar o som de `target` numa thread própria e manda para
/// `source`. `runtime` é o Tokio da chamada: entregar o som ao WebRTC é
/// assíncrono. `failed` recebe a frase de erro se não der para gravar.
pub fn start(
    target: Target,
    source: NativeAudioSource,
    runtime: tokio::runtime::Handle,
    failed: impl FnOnce(String) + Send + 'static,
) -> SoundCapture {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let thread = std::thread::spawn(move || {
        if let Err(err) = capture(target, &source, &runtime, &flag) {
            failed(err);
        }
    });
    SoundCapture {
        stop,
        thread: Some(thread),
    }
}

/// Amostras `f32` intercaladas para o `i16` que o WebRTC recebe.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn to_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|s| (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16)
        .collect()
}

#[cfg(not(windows))]
fn capture(
    _target: Target,
    _source: &NativeAudioSource,
    _runtime: &tokio::runtime::Handle,
    _stop: &AtomicBool,
) -> Result<(), String> {
    Err("o som da tela só funciona no Windows por enquanto".into())
}

#[cfg(windows)]
fn capture(
    target: Target,
    source: &NativeAudioSource,
    runtime: &tokio::runtime::Handle,
    stop: &AtomicBool,
) -> Result<(), String> {
    use std::collections::VecDeque;

    use livekit::webrtc::audio_frame::AudioFrame;
    use wasapi::{AudioClient, Direction, SampleType, StreamMode, WaveFormat};

    let failed = |err: wasapi::WasapiError| format!("o som da tela não abriu: {err}");
    let _ = wasapi::initialize_mta().ok();

    let me = std::process::id();
    let (pid, include) = match target {
        Target::Screen => (me, false),
        Target::Window(hwnd) => match window_process(hwnd) {
            // A janela é do próprio mothh: grava tudo menos ele, como a tela.
            Some(pid) if pid != me => (pid, true),
            _ => (me, false),
        },
    };
    let format = WaveFormat::new(
        32,
        32,
        &SampleType::Float,
        RATE as usize,
        CHANNELS as usize,
        None,
    );
    let mut client = AudioClient::new_application_loopback_client(pid, include).map_err(failed)?;
    client
        .initialize_client(
            &format,
            &Direction::Capture,
            &StreamMode::EventsShared {
                autoconvert: true,
                buffer_duration_hns: 0,
            },
        )
        .map_err(failed)?;
    let event = client.set_get_eventhandle().map_err(failed)?;
    let reader = client.get_audiocaptureclient().map_err(failed)?;
    client.start_stream().map_err(failed)?;

    // O WebRTC quer pedaços de 10 ms.
    let frame_bytes = 4 * CHANNELS as usize;
    let chunk = (RATE / 100) as usize;
    let mut queue: VecDeque<u8> = VecDeque::new();
    let mut floats = vec![0f32; chunk * CHANNELS as usize];
    let mut bytes = Vec::with_capacity(chunk * frame_bytes);
    while !stop.load(Ordering::Relaxed) {
        // Sem nada tocando não chega aviso; o tempo limite só serve para
        // olhar o `stop` de vez em quando.
        if event.wait_for_event(200).is_err() {
            continue;
        }
        while reader.get_next_packet_size().map_err(failed)?.unwrap_or(0) > 0 {
            reader
                .read_from_device_to_deque(&mut queue)
                .map_err(failed)?;
        }
        while queue.len() >= chunk * frame_bytes {
            bytes.clear();
            bytes.extend(queue.drain(..chunk * frame_bytes));
            for (sample, raw) in floats.iter_mut().zip(bytes.as_chunks::<4>().0) {
                *sample = f32::from_le_bytes(*raw);
            }
            let frame = AudioFrame {
                data: to_i16(&floats).into(),
                sample_rate: RATE,
                num_channels: CHANNELS,
                samples_per_channel: chunk as u32,
            };
            let _ = runtime.block_on(source.capture_frame(&frame));
        }
    }
    let _ = client.stop_stream();
    Ok(())
}

/// O processo dono da janela.
#[cfg(windows)]
fn window_process(hwnd: u64) -> Option<u32> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

    let mut pid = 0u32;
    // SAFETY: só lê o dono da janela; um HWND que não existe mais devolve 0.
    unsafe { GetWindowThreadProcessId(HWND(hwnd as usize as *mut _), Some(&mut pid)) };
    (pid != 0).then_some(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amostras_viram_i16_sem_estourar() {
        assert_eq!(
            to_i16(&[0.0, 1.0, -1.0, 2.0, -3.0]),
            [0, 32767, -32767, 32767, -32767]
        );
        assert_eq!(to_i16(&[0.5]), [16383]);
    }
}

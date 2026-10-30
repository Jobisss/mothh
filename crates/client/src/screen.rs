//! Captura da tela (ou de uma janela) para compartilhar na chamada.
//!
//! O capturador do próprio WebRTC (`DesktopCapturer`) não pode trocar de
//! thread, então cada captura ganha uma thread só dela, que pede um quadro
//! `FPS` vezes por segundo e entrega para o `NativeVideoSource` da chamada.
//! Largar a `Capture` para a thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use livekit::webrtc::desktop_capturer::{
    CaptureError, DesktopCaptureSourceType, DesktopCapturer, DesktopCapturerOptions, DesktopFrame,
};
use livekit::webrtc::native::yuv_helper;
use livekit::webrtc::prelude::{I420Buffer, VideoFrame, VideoResolution, VideoRotation};
use livekit::webrtc::video_source::native::NativeVideoSource;

/// Quadros por segundo da tela. Código e documentos mudam pouco; vale mais
/// gastar a banda em resolução (para dar para ler) do que em fluidez.
pub const FPS: u64 = 15;

/// Banda máxima do vídeo da tela, em bits por segundo.
pub const MAX_BITRATE: u64 = 4_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Screen,
    Window,
}

/// Uma tela ou janela que dá para compartilhar.
#[derive(Clone, Debug)]
pub struct Source {
    pub kind: Kind,
    pub id: u64,
    pub title: String,
}

fn capturer(kind: Kind) -> Option<DesktopCapturer> {
    let mut options = DesktopCapturerOptions::new(match kind {
        Kind::Screen => DesktopCaptureSourceType::Screen,
        Kind::Window => DesktopCaptureSourceType::Window,
    });
    options.set_include_cursor(true);
    DesktopCapturer::new(options)
}

/// As telas primeiro, depois as janelas abertas. Pode demorar um pouco
/// (o sistema lista as janelas uma a uma): chame fora da thread da interface.
pub fn sources() -> Vec<Source> {
    let mut all = Vec::new();
    for kind in [Kind::Screen, Kind::Window] {
        let Some(capturer) = capturer(kind) else {
            continue;
        };
        let list = capturer.get_source_list();
        let count = list.len();
        for (n, source) in list.into_iter().enumerate() {
            let title = match kind {
                // O sistema costuma não dar nome às telas.
                Kind::Screen if count == 1 => "tela inteira".to_string(),
                Kind::Screen => format!("tela {}", n + 1),
                Kind::Window => fix_latin1(source.title()),
            };
            if title.trim().is_empty() {
                continue;
            }
            all.push(Source {
                kind,
                id: source.id(),
                title,
            });
        }
    }
    all
}

/// No Linux (X11) o título da janela vem em UTF-8 lido como Latin-1
/// ("mothh Â· ana"). Desfaz isso quando dá; senão deixa como veio.
fn fix_latin1(title: String) -> String {
    let bytes: Option<Vec<u8>> = title.chars().map(|c| u8::try_from(c).ok()).collect();
    bytes
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or(title)
}

/// Uma captura em andamento.
pub struct Capture {
    stop: Arc<AtomicBool>,
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Começa a capturar `source` para dentro de `video`. `on_end` é chamado
/// (na thread da captura) se ela parar sozinha, por exemplo quando a janela
/// compartilhada é fechada.
pub fn start(
    source: Source,
    video: NativeVideoSource,
    on_end: impl FnOnce(String) + Send + 'static,
) -> Capture {
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    std::thread::spawn(move || {
        if let Err(reason) = run(source, video, &stopped) {
            on_end(reason);
        }
    });
    Capture { stop }
}

fn run(source: Source, video: NativeVideoSource, stop: &AtomicBool) -> Result<(), String> {
    let mut capturer =
        capturer(source.kind).ok_or("este sistema não deixou capturar a tela".to_string())?;
    let source_handle = capturer
        .get_source_list()
        .into_iter()
        .find(|s| s.id() == source.id)
        .ok_or("essa tela ou janela não existe mais".to_string())?;

    // O retorno do capturador chega por callback, às vezes em outra thread;
    // a falha permanente volta para cá por esta bandeira.
    let failed = Arc::new(AtomicBool::new(false));
    let failed_flag = failed.clone();
    let started = Instant::now();
    capturer.start_capture(Some(source_handle), move |result| match result {
        Ok(frame) => {
            let buffer = to_i420(&frame);
            let mut frame = VideoFrame::new(VideoRotation::VideoRotation0, buffer);
            frame.timestamp_us = started.elapsed().as_micros() as i64;
            video.capture_frame(&frame);
        }
        Err(CaptureError::Permanent) => failed_flag.store(true, Ordering::Relaxed),
        // Acontece quando a janela está minimizada: tenta no próximo quadro.
        Err(CaptureError::Temporary) => {}
    });

    let period = Duration::from_millis(1000 / FPS);
    let mut next = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        capturer.capture_frame();
        if failed.load(Ordering::Relaxed) {
            return Err("o compartilhamento de tela parou (a janela fechou?)".into());
        }
        next += period;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            // Atrasou (máquina ocupada): não tenta compensar de uma vez.
            next = now;
        }
    }
    Ok(())
}

/// O quadro do capturador vem em BGRA (o "ARGB" do libyuv), com possível
/// folga no fim de cada linha.
fn to_i420(frame: &DesktopFrame) -> I420Buffer {
    let width = frame.width().max(1) as u32;
    let height = frame.height().max(1) as u32;
    let mut buffer = I420Buffer::new(width, height);
    let (stride_y, stride_u, stride_v) = buffer.strides();
    let (y, u, v) = buffer.data_mut();
    yuv_helper::argb_to_i420(
        frame.data(),
        frame.stride(),
        y,
        stride_y,
        u,
        stride_u,
        v,
        stride_v,
        width as i32,
        height as i32,
    );
    buffer
}

/// Resolução inicial do vídeo da tela. O WebRTC ajusta sozinho ao tamanho
/// dos quadros que chegam; isto é só a primeira estimativa.
pub fn resolution() -> VideoResolution {
    VideoResolution {
        width: 1920,
        height: 1080,
    }
}

#[cfg(test)]
mod tests {
    use super::fix_latin1;

    #[test]
    fn titulo_lido_como_latin1_volta_a_ser_utf8() {
        assert_eq!(fix_latin1("mothh Â· ana".into()), "mothh · ana");
    }

    #[test]
    fn titulo_certo_fica_igual() {
        assert_eq!(
            fix_latin1("código.rs — editor".into()),
            "código.rs — editor"
        );
        assert_eq!(fix_latin1("ação".into()), "ação");
    }
}

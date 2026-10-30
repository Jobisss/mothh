//! Compartilhar tela (F2-06): escolher uma tela ou janela e mandar os
//! quadros dela para a chamada.
//!
//! A captura é a do próprio libwebrtc (`DesktopCapturer`), a mesma que os
//! navegadores usam: DXGI no Windows, X11 ou PipeWire no Linux. Ela roda
//! numa thread própria porque, no Windows, o capturador precisa ser criado
//! e usado na mesma thread. Cada quadro vira I420, é reduzido até caber em
//! `WIDTH` x `HEIGHT` e vai para a `NativeVideoSource` publicada no LiveKit,
//! como a câmera em `video.rs`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use livekit::webrtc::desktop_capturer::{
    CaptureError, DesktopCaptureSourceType, DesktopCapturer, DesktopCapturerOptions, DesktopFrame,
};
use livekit::webrtc::native::yuv_helper;
use livekit::webrtc::prelude::{
    I420Buffer, VideoBuffer, VideoFormatType, VideoFrame, VideoRotation,
};
use livekit::webrtc::video_source::native::NativeVideoSource;
use slint::{Rgba8Pixel, SharedPixelBuffer};

use crate::AppWindow;
use crate::video;

/// Até 1080p: para ler um código compartilhado o texto precisa ficar
/// nítido. E 30 quadros por segundo, para um vídeo ou um jogo na tela não
/// ficar travado (com 10 ficava).
pub const WIDTH: u32 = 1920;
pub const HEIGHT: u32 = 1080;
pub const FPS: u64 = 30;
/// 1080p a 30 com 4 Mbps: o `H1080_FPS30` do LiveKit dá 3, que borra o
/// texto quando a tela inteira mexe.
pub const ENCODING: livekit::options::VideoEncoding = livekit::options::VideoEncoding {
    max_bitrate: 4_000_000,
    max_framerate: FPS as f64,
};

/// A prévia da própria tela no tile é menor que o que vai para a chamada:
/// o tile é pequeno e converter 1080p 30 vezes por segundo só para ele
/// gastaria à toa.
const PREVIEW_WIDTH: u32 = 960;
const PREVIEW_HEIGHT: u32 = 540;

/// Id do tile da própria tela. Os tiles das telas dos outros terminam com
/// `TILE_SUFFIX` depois do id da pessoa.
pub const ME: &str = "#tela";
pub const TILE_SUFFIX: &str = "#tela";

/// Uma tela ou janela que dá para compartilhar, como a lista mostra.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareSource {
    /// `s:<id>` para uma tela, `w:<id>` para uma janela.
    pub id: String,
    pub name: String,
    pub is_screen: bool,
}

/// Uma foto pequena de uma tela ou janela, para a lista mostrar o que cada
/// opção é de verdade. `width` e `height` são o tamanho real dela.
pub struct Preview {
    pub id: String,
    pub width: u32,
    pub height: u32,
    pub pixels: SharedPixelBuffer<Rgba8Pixel>,
}

/// Tamanho máximo da foto de cada opção na lista.
const THUMB_WIDTH: u32 = 320;
const THUMB_HEIGHT: u32 = 180;

/// Lista as telas e janelas abertas numa thread própria. A lista vai para
/// `done` assim que sai (telas primeiro); depois tira uma foto de cada
/// opção e manda uma por uma para `preview`, que demora mais.
pub fn list_sources(
    done: impl FnOnce(Vec<ShareSource>) + Send + 'static,
    preview: impl Fn(Preview) + Send + 'static,
) {
    std::thread::spawn(move || {
        let list = sources();
        done(list.clone());
        for source in list {
            if let Some(found) = take_preview(&source.id) {
                preview(found);
            }
        }
    });
}

fn sources() -> Vec<ShareSource> {
    let mut list = Vec::new();
    if let Some(capturer) = DesktopCapturer::new(DesktopCapturerOptions::new(
        DesktopCaptureSourceType::Screen,
    )) {
        for (i, screen) in capturer.get_source_list().iter().enumerate() {
            list.push(ShareSource {
                id: format!("s:{}", screen.id()),
                name: format!("tela {}", i + 1),
                is_screen: true,
            });
        }
    }
    if let Some(capturer) = DesktopCapturer::new(DesktopCapturerOptions::new(
        DesktopCaptureSourceType::Window,
    )) {
        for window in capturer.get_source_list() {
            let title = window.title().trim().to_string();
            // Janelas sem título são as invisíveis do sistema.
            if title.is_empty() {
                continue;
            }
            list.push(ShareSource {
                id: format!("w:{}", window.id()),
                name: title,
                is_screen: false,
            });
        }
    }
    list
}

/// Captura um quadro de `id` e reduz para a lista. Algumas tentativas: o
/// primeiro quadro da tela às vezes não vem (no Windows, o DXGI ainda está
/// ligando).
fn take_preview(id: &str) -> Option<Preview> {
    let (kind, number) = parse_source(id)?;
    let mut capturer = DesktopCapturer::new(DesktopCapturerOptions::new(kind))?;
    let chosen = capturer
        .get_source_list()
        .into_iter()
        .find(|s| s.id() == number)?;
    let (tx, rx) = channel();
    capturer.start_capture(Some(chosen), move |result| {
        if let Ok(frame) = result {
            let width = frame.width().max(0) as u32;
            let height = frame.height().max(0) as u32;
            let _ = tx.send(thumbnail(frame.data(), frame.stride(), width, height));
        }
    });
    for _ in 0..5 {
        capturer.capture_frame();
        if let Ok(Some((width, height, (tw, th, rgba)))) =
            rx.recv_timeout(Duration::from_millis(200))
        {
            let mut pixels = SharedPixelBuffer::<Rgba8Pixel>::new(tw, th);
            pixels.make_mut_bytes().copy_from_slice(&rgba);
            return Some(Preview {
                id: id.to_string(),
                width,
                height,
                pixels,
            });
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

/// Um quadro BGRA para a foto da lista, em RGBA, sem passar de
/// `THUMB_WIDTH` x `THUMB_HEIGHT`. Cada pixel da foto é a média do bloco
/// que ele cobre, para o texto miúdo virar cinza em vez de chuvisco.
/// Devolve o tamanho real e a foto com o tamanho dela; `None` se vazio.
#[allow(clippy::type_complexity)]
pub fn thumbnail(
    bgra: &[u8],
    stride: u32,
    width: u32,
    height: u32,
) -> Option<(u32, u32, (u32, u32, Vec<u8>))> {
    if width == 0 || height == 0 || (stride as usize) < width as usize * 4 {
        return None;
    }
    if bgra.len() < stride as usize * (height as usize - 1) + width as usize * 4 {
        return None;
    }
    let (tw, th) = fit(width, height, THUMB_WIDTH, THUMB_HEIGHT);
    let (tw, th) = (tw.max(1), th.max(1));
    let mut rgba = Vec::with_capacity((tw * th * 4) as usize);
    for ty in 0..th {
        let y0 = ty * height / th;
        let y1 = ((ty + 1) * height / th).max(y0 + 1);
        for tx in 0..tw {
            let x0 = tx * width / tw;
            let x1 = ((tx + 1) * width / tw).max(x0 + 1);
            let mut sum = [0u32; 3];
            // Até 4x4 amostras por bloco: o bastante para a média e barato.
            let (sy, sx) = (((y1 - y0) / 4).max(1), ((x1 - x0) / 4).max(1));
            let mut n = 0;
            let mut y = y0;
            while y < y1 {
                let mut x = x0;
                while x < x1 {
                    let i = y as usize * stride as usize + x as usize * 4;
                    sum[0] += u32::from(bgra[i + 2]);
                    sum[1] += u32::from(bgra[i + 1]);
                    sum[2] += u32::from(bgra[i]);
                    n += 1;
                    x += sx;
                }
                y += sy;
            }
            rgba.extend_from_slice(&[
                (sum[0] / n) as u8,
                (sum[1] / n) as u8,
                (sum[2] / n) as u8,
                255,
            ]);
        }
    }
    Some((width, height, (tw, th, rgba)))
}

/// Interpreta o id de um `ShareSource`.
fn parse_source(id: &str) -> Option<(DesktopCaptureSourceType, u64)> {
    let (kind, number) = id.split_once(':')?;
    let kind = match kind {
        "s" => DesktopCaptureSourceType::Screen,
        "w" => DesktopCaptureSourceType::Window,
        _ => return None,
    };
    Some((kind, number.parse().ok()?))
}

/// A tela sendo compartilhada. Largar para a captura.
pub struct ScreenCapture {
    stop: Arc<AtomicBool>,
    /// Quantos quadros já foram para a chamada. Só o teste de captura lê.
    #[cfg_attr(not(test), allow(dead_code))]
    frames: Arc<AtomicU64>,
    thread: Option<JoinHandle<()>>,
}

impl ScreenCapture {
    /// Só o teste de captura olha; a tela não mostra contagem.
    #[cfg(test)]
    pub fn frames(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }
}

impl Drop for ScreenCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Começa a capturar a tela ou janela `source_id` (o id de um
/// `ShareSource`), mandando os quadros para `source` e a prévia para o tile
/// da própria tela. `failed` recebe a frase de erro se não der para
/// capturar ou a janela sumir.
pub fn start_share(
    source_id: String,
    source: NativeVideoSource,
    ui: slint::Weak<AppWindow>,
    failed: impl FnOnce(String) + Send + 'static,
) -> ScreenCapture {
    let stop = Arc::new(AtomicBool::new(false));
    let frames = Arc::new(AtomicU64::new(0));
    let flag = stop.clone();
    let counter = frames.clone();
    let thread = std::thread::spawn(move || {
        if let Err(err) = capture(&source_id, source, ui, &flag, counter) {
            failed(err);
        }
    });
    ScreenCapture {
        stop,
        frames,
        thread: Some(thread),
    }
}

fn capture(
    source_id: &str,
    source: NativeVideoSource,
    ui: slint::Weak<AppWindow>,
    stop: &AtomicBool,
    frames: Arc<AtomicU64>,
) -> Result<(), String> {
    let (kind, id) =
        parse_source(source_id).ok_or_else(|| "tela ou janela inválida".to_string())?;
    let mut options = DesktopCapturerOptions::new(kind);
    options.set_include_cursor(true);
    let mut capturer = DesktopCapturer::new(options)
        .ok_or_else(|| "não dá para capturar a tela neste sistema".to_string())?;
    let chosen = capturer
        .get_source_list()
        .into_iter()
        .find(|s| s.id() == id)
        .ok_or_else(|| "a tela ou janela escolhida não existe mais".to_string())?;

    // O capturador avisa o resultado de cada quadro por callback; os erros
    // voltam por aqui para a thread da captura parar.
    let (errors, failed): (Sender<String>, Receiver<String>) = channel();
    let started = Instant::now();
    let tile = video::TileFeed::new(&ui, ME);
    capturer.start_capture(Some(chosen), move |result| match result {
        Ok(frame) => {
            if push(&frame, &source, &tile, started) {
                frames.fetch_add(1, Ordering::Relaxed);
            }
        }
        // Um quadro perdido acontece (a janela estava minimizada, a tela
        // trocou de modo); o próximo vem.
        Err(CaptureError::Temporary) => {}
        Err(CaptureError::Permanent) => {
            let _ = errors.send("a tela ou janela compartilhada fechou".into());
        }
    });

    let period = Duration::from_millis(1000 / FPS);
    while !stop.load(Ordering::Relaxed) {
        let tick = Instant::now();
        capturer.capture_frame();
        if let Ok(reason) = failed.try_recv() {
            return Err(reason);
        }
        std::thread::sleep(period.saturating_sub(tick.elapsed()));
    }
    Ok(())
}

/// Converte um quadro capturado para o que a chamada e a prévia usam.
/// `false` se o quadro era vazio (janela minimizada, por exemplo).
fn push(
    frame: &DesktopFrame,
    source: &NativeVideoSource,
    tile: &video::TileFeed,
    started: Instant,
) -> bool {
    // O I420 guarda a cor em blocos de 2x2: corta uma linha ou coluna
    // ímpar, que ninguém vê.
    let width = (frame.width().max(0) as u32) & !1;
    let height = (frame.height().max(0) as u32) & !1;
    if width < 2 || height < 2 {
        return false;
    }
    let mut i420 = I420Buffer::new(width, height);
    let (stride_y, stride_u, stride_v) = i420.strides();
    let (y, u, v) = i420.data_mut();
    // O quadro do libwebrtc é BGRA (4 bytes por pixel); no libyuv isso é
    // "ARGB", a palavra de 32 bits.
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
    let (w, h) = fit(width, height, WIDTH, HEIGHT);
    if (w, h) != (width, height) {
        i420 = i420.scale(w as i32, h as i32);
    }

    let (pw, ph) = fit(w, h, PREVIEW_WIDTH, PREVIEW_HEIGHT);
    let preview = if (pw, ph) != (w, h) {
        pixels(&i420.scale(pw as i32, ph as i32))
    } else {
        pixels(&i420)
    };

    let mut video_frame = VideoFrame::new(VideoRotation::VideoRotation0, i420);
    video_frame.timestamp_us = started.elapsed().as_micros() as i64;
    source.capture_frame(&video_frame);
    tile.show(preview);
    true
}

/// Um I420 para a imagem que o Slint mostra.
fn pixels(buffer: &I420Buffer) -> SharedPixelBuffer<Rgba8Pixel> {
    let (width, height) = (buffer.width(), buffer.height());
    let mut pixels = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    // `to_argb` vem de um trait interno do libwebrtc, que só aparece pelo
    // objeto `dyn VideoBuffer`.
    let buffer: &dyn VideoBuffer = buffer;
    buffer.to_argb(
        VideoFormatType::ABGR,
        pixels.make_mut_bytes(),
        width * 4,
        width as i32,
        height as i32,
    );
    pixels
}

/// O maior tamanho que cabe em `max_w` x `max_h` sem mudar a proporção,
/// sempre par.
pub fn fit(width: u32, height: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if width <= max_w && height <= max_h {
        return (width & !1, height & !1);
    }
    let scale = f64::min(
        f64::from(max_w) / f64::from(width),
        f64::from(max_h) / f64::from(height),
    );
    let w = (f64::from(width) * scale).round() as u32;
    let h = (f64::from(height) * scale).round() as u32;
    (w.max(2) & !1, h.max(2) & !1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_source_reads_kind_and_id() {
        assert_eq!(
            parse_source("s:7"),
            Some((DesktopCaptureSourceType::Screen, 7))
        );
        assert_eq!(
            parse_source("w:123456"),
            Some((DesktopCaptureSourceType::Window, 123456))
        );
        assert_eq!(parse_source("x:1"), None);
        assert_eq!(parse_source("s:abc"), None);
        assert_eq!(parse_source(""), None);
    }

    #[test]
    fn thumbnail_reduz_e_troca_bgra_por_rgba() {
        // 640x360 todo azul em BGRA, com sobra no fim de cada linha.
        let stride = 640 * 4 + 16;
        let mut bgra = vec![0u8; stride * 360];
        for y in 0..360 {
            for x in 0..640 {
                bgra[y * stride + x * 4..][..4].copy_from_slice(&[200, 10, 20, 255]);
            }
        }
        let (w, h, (tw, th, rgba)) = thumbnail(&bgra, stride as u32, 640, 360).unwrap();
        assert_eq!((w, h, tw, th), (640, 360, 320, 180));
        assert_eq!(rgba.len(), 320 * 180 * 4);
        assert_eq!(&rgba[..4], &[20, 10, 200, 255]);
        assert!(thumbnail(&bgra, stride as u32, 0, 360).is_none());
        assert!(thumbnail(&bgra[..100], stride as u32, 640, 360).is_none());
    }

    #[test]
    fn fit_keeps_1080p_and_shrinks_4k() {
        assert_eq!(fit(1920, 1080, WIDTH, HEIGHT), (1920, 1080));
        assert_eq!(fit(3840, 2160, WIDTH, HEIGHT), (1920, 1080));
        // Monitor vertical: limitado pela altura.
        assert_eq!(fit(1080, 1920, WIDTH, HEIGHT), (608, 1080));
    }

    #[test]
    fn fit_is_always_even() {
        let (w, h) = fit(1279, 721, WIDTH, HEIGHT);
        assert_eq!((w % 2, h % 2), (0, 0));
        let (w, h) = fit(3001, 1999, WIDTH, HEIGHT);
        assert_eq!((w % 2, h % 2), (0, 0));
    }

    /// Precisa de uma sessão gráfica: lista as telas e captura alguns
    /// quadros da primeira (o `NativeVideoSource` precisa do Tokio).
    /// `cargo test -p client -- --ignored captura`.
    #[tokio::test]
    #[ignore]
    async fn captura_a_primeira_tela() {
        let list = sources();
        println!("{list:#?}");
        let screen = list
            .iter()
            .find(|s| s.id.starts_with("s:"))
            .expect("nenhuma tela encontrada");
        let source = NativeVideoSource::new(
            livekit::webrtc::video_source::VideoResolution {
                width: WIDTH,
                height: HEIGHT,
            },
            true,
        );
        let failed = Arc::new(std::sync::Mutex::new(None));
        let report = failed.clone();
        let capture = start_share(
            screen.id.clone(),
            source,
            slint::Weak::default(),
            move |e| {
                *report.lock().unwrap() = Some(e);
            },
        );
        std::thread::sleep(Duration::from_millis(1500));
        let frames = capture.frames();
        drop(capture);
        assert_eq!(*failed.lock().unwrap(), None);
        println!("{frames} quadros em 1,5 s");
        assert!(frames >= 5, "só {frames} quadros capturados");
    }
}

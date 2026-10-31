//! Vídeo da chamada: a câmera (F2-05) e os quadros na tela.
//!
//! A câmera roda numa thread própria, porque ler a webcam trava a thread até
//! o próximo quadro chegar. Cada quadro vira I420 (o formato que o WebRTC
//! codifica) e vai para a `NativeVideoSource` publicada no LiveKit.
//!
//! Os quadros recebidos não passam pela rede do app: vão direto para a lista
//! `video-tiles` da tela, porque chegam muitas vezes por segundo. A tela
//! compartilhada (`screen.rs`) usa os mesmos tiles; um deles pode estar
//! ampliado (`big-tile`), por clique ou porque é a tela de alguém.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use livekit::webrtc::native::yuv_helper;
use livekit::webrtc::prelude::{
    BoxVideoFrame, I420Buffer, VideoFormatType, VideoFrame, VideoRotation,
};
use livekit::webrtc::video_source::native::NativeVideoSource;
use nokhwa::Camera;
use nokhwa::pixel_format::RgbAFormat;
use nokhwa::utils::{
    CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType, Resolution,
};
use slint::{Model, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};

use crate::{AppWindow, VideoTile};

/// 360p por padrão, para caber na VPS (F2-05). A câmera é pedida perto
/// disso e o quadro é reduzido até caber aqui dentro.
pub const WIDTH: u32 = 640;
pub const HEIGHT: u32 = 360;
/// 30 quadros por segundo. Tem que ser um valor que as webcams oferecem:
/// o nokhwa escolhe a taxa mais perto da pedida, e com 20 ele caía nos
/// 15 que quase toda webcam tem.
pub const FPS: u32 = 30;
/// 360p a 30 quadros por segundo. O `H360` do LiveKit é a 20.
pub const ENCODING: livekit::options::VideoEncoding = livekit::options::VideoEncoding {
    max_bitrate: 700_000,
    max_framerate: FPS as f64,
};

/// Id do quadro da própria câmera na lista de tiles.
pub const ME: &str = "";

/// Quadro de vídeo do WebRTC para uma imagem que o Slint mostra.
pub fn frame_to_pixels(frame: &BoxVideoFrame) -> SharedPixelBuffer<Rgba8Pixel> {
    let (width, height) = (frame.buffer.width(), frame.buffer.height());
    let mut pixels = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    // No libyuv, "ABGR" é a palavra de 32 bits; na memória fica R, G, B, A.
    frame.buffer.to_argb(
        VideoFormatType::ABGR,
        pixels.make_mut_bytes(),
        width * 4,
        width as i32,
        height as i32,
    );
    pixels
}

/// RGBA (4 bytes por pixel) para I420, o formato que vai para o codificador.
pub fn rgba_to_i420(rgba: &[u8], width: u32, height: u32) -> I420Buffer {
    let mut i420 = I420Buffer::new(width, height);
    let (stride_y, stride_u, stride_v) = i420.strides();
    let (y, u, v) = i420.data_mut();
    yuv_helper::abgr_to_i420(
        rgba,
        width * 4,
        y,
        stride_y,
        u,
        stride_u,
        v,
        stride_v,
        width as i32,
        height as i32,
    );
    i420
}

/// O maior tamanho que cabe em `WIDTH` x `HEIGHT` sem mudar a proporção.
/// Fica par, porque o I420 guarda a cor em blocos de 2x2.
pub fn fit(width: u32, height: u32) -> (u32, u32) {
    if width <= WIDTH && height <= HEIGHT {
        return (width & !1, height & !1);
    }
    let scale = f64::min(
        f64::from(WIDTH) / f64::from(width),
        f64::from(HEIGHT) / f64::from(height),
    );
    let w = (f64::from(width) * scale).round() as u32;
    let h = (f64::from(height) * scale).round() as u32;
    (w.max(2) & !1, h.max(2) & !1)
}

/// A câmera ligada. Largar desliga e solta a webcam.
pub struct CameraCapture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for CameraCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Abre a câmera `camera` e manda os quadros para `source`, mostrando cada
/// um também no tile da própria pessoa. `failed` recebe a frase de erro, se
/// a câmera não abrir ou parar no meio.
pub fn start_camera(
    camera: CameraIndex,
    source: NativeVideoSource,
    ui: slint::Weak<AppWindow>,
    failed: impl FnOnce(String) + Send + 'static,
) -> CameraCapture {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let thread = std::thread::spawn(move || {
        if let Err(err) = capture(camera, &source, &ui, &flag) {
            failed(err);
        }
    });
    CameraCapture {
        stop,
        thread: Some(thread),
    }
}

fn capture(
    camera: CameraIndex,
    source: &NativeVideoSource,
    ui: &slint::Weak<AppWindow>,
    stop: &AtomicBool,
) -> Result<(), String> {
    let mut frames = Frames::open(camera)?;
    let tile = TileFeed::new(ui, ME);
    let started = std::time::Instant::now();
    while !stop.load(Ordering::Relaxed) {
        let (width, height, rgba) = frames.next_good()?;
        let (w, h) = fit(width, height);
        let mut i420 = rgba_to_i420(&rgba, width, height);
        if (w, h) != (width, height) {
            i420 = i420.scale(w as i32, h as i32);
        }
        let mut frame = VideoFrame::new(VideoRotation::VideoRotation0, i420);
        frame.timestamp_us = started.elapsed().as_micros() as i64;
        source.capture_frame(&frame);

        let mut preview = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
        preview.make_mut_bytes().copy_from_slice(&rgba);
        tile.show(preview);
    }
    Ok(())
}

/// Teste da câmera nas configurações: abre a câmera `camera` só para
/// mostrar a imagem na prévia (`camera-test-frame`), sem chamada nenhuma.
/// O `camera-test-status` diz qual câmera abriu e em que formato, ou por
/// que não abriu. Largar o `CameraCapture` para o teste.
pub fn start_test(camera: CameraIndex, ui: slint::Weak<AppWindow>) -> CameraCapture {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let thread = std::thread::spawn(move || {
        let status = |text: String| {
            let _ = ui.upgrade_in_event_loop(move |ui| ui.set_camera_test_status(text.into()));
        };
        status("abrindo a câmera...".into());
        let result = (|| {
            let mut frames = Frames::open(camera)?;
            let feed = TileFeed::test(&ui);
            let mut shown = false;
            while !flag.load(Ordering::Relaxed) {
                let (width, height, rgba) = frames.next_good()?;
                if !shown {
                    shown = true;
                    status(frames.describe());
                }
                let mut preview = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
                preview.make_mut_bytes().copy_from_slice(&rgba);
                feed.show(preview);
            }
            Ok::<(), String>(())
        })();
        if let Err(err) = result
            && !flag.load(Ordering::Relaxed)
        {
            eprintln!("teste da câmera: {err}");
            let _ = ui.upgrade_in_event_loop(move |ui| {
                ui.set_camera_testing(false);
                ui.set_camera_test_frame(slint::Image::default());
                ui.set_camera_test_status(err.into());
            });
        }
    });
    CameraCapture {
        stop,
        thread: Some(thread),
    }
}

/// Quanto tempo a câmera pode ficar mandando só quadros que não dão para
/// ler antes de virar erro. Sem isso, uma câmera num formato que o app não
/// entende fica com a tela preta para sempre, sem aviso.
const NO_GOOD_FRAME: std::time::Duration = std::time::Duration::from_secs(5);

/// De onde vêm os quadros: a webcam ou, com `MOTHH_CAMERA_TESTE=1`, uma
/// imagem gerada. A de teste serve para abrir vários apps na mesma máquina
/// (só um consegue usar a webcam) ou numa máquina sem câmera.
enum Frames {
    Webcam(Box<Camera>),
    Test { started: std::time::Instant },
}

impl Frames {
    fn open(index: CameraIndex) -> Result<Self, String> {
        if std::env::var("MOTHH_CAMERA_TESTE").is_ok_and(|v| v == "1") {
            return Ok(Self::Test {
                started: std::time::Instant::now(),
            });
        }
        let mut camera = open_camera(index)?;
        camera
            .open_stream()
            .map_err(|err| format!("a câmera não abriu: {err}"))?;
        Ok(Self::Webcam(Box::new(camera)))
    }

    /// Qual câmera abriu e em que formato, para o teste das configurações.
    fn describe(&self) -> String {
        match self {
            Self::Webcam(camera) => {
                let format = camera.camera_format();
                format!(
                    "{} · {}x{} {} {} qps",
                    camera.info().human_name(),
                    format.width(),
                    format.height(),
                    format.format(),
                    format.frame_rate()
                )
            }
            Self::Test { .. } => "câmera de teste (MOTHH_CAMERA_TESTE)".into(),
        }
    }

    /// O próximo quadro que deu para ler. Pula os quebrados, mas se nenhum
    /// prestar por `NO_GOOD_FRAME`, desiste com o formato no erro.
    fn next_good(&mut self) -> Result<(u32, u32, Vec<u8>), String> {
        let since = std::time::Instant::now();
        loop {
            if let Some(frame) = self.next()? {
                return Ok(frame);
            }
            if since.elapsed() > NO_GOOD_FRAME {
                let format = match self {
                    Self::Webcam(camera) => camera.camera_format().to_string(),
                    Self::Test { .. } => String::new(),
                };
                return Err(format!(
                    "a câmera abriu, mas os quadros dela não dão para ler ({format})"
                ));
            }
        }
    }

    /// O próximo quadro em RGBA. A webcam trava até ele chegar; a de teste
    /// espera o tempo de um quadro.
    fn next(&mut self) -> Result<Option<(u32, u32, Vec<u8>)>, String> {
        match self {
            Self::Webcam(camera) => {
                let buffer = camera
                    .frame()
                    .map_err(|err| format!("a câmera parou: {err}"))?;
                Ok(decode(&buffer))
            }
            Self::Test { started } => {
                std::thread::sleep(std::time::Duration::from_millis(u64::from(1000 / FPS)));
                Ok(Some((
                    WIDTH,
                    HEIGHT,
                    test_pattern(started.elapsed().as_secs_f32()),
                )))
            }
        }
    }
}

/// Abre a webcam `index` em 360p a `FPS`, de preferência em MJPEG (o mais
/// leve de ler); se não tiver, em YUYV ou NV12 (a Iriun Webcam só tem YUYV
/// e RGB). O `Closest` do nokhwa só aceita o formato pedido e só acerta a
/// taxa quando a resolução existe; sem nada disso, pede a maior resolução
/// que dá `FPS`, que o `fit` depois reduz; por último, qualquer formato.
fn open_camera(index: CameraIndex) -> Result<Camera, String> {
    let closest = [FrameFormat::MJPEG, FrameFormat::YUYV, FrameFormat::NV12].map(|format| {
        RequestedFormatType::Closest(CameraFormat::new(
            Resolution::new(WIDTH, HEIGHT),
            format,
            FPS,
        ))
    });
    let tries = closest.into_iter().chain([
        RequestedFormatType::HighestFrameRate(FPS),
        RequestedFormatType::AbsoluteHighestFrameRate,
    ]);
    let mut last = String::new();
    for kind in tries {
        let format = RequestedFormat::new::<RgbAFormat>(kind);
        match Camera::new(index.clone(), format) {
            Ok(camera) => return Ok(camera),
            Err(err) => last = err.to_string(),
        }
    }
    Err(format!("a câmera não abriu: {last}"))
}

/// Listras diagonais em cinza andando para o lado, em `WIDTH` x `HEIGHT`.
fn test_pattern(t: f32) -> Vec<u8> {
    let shift = (t * 60.0) as u32;
    let mut rgba = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let v = if ((x + y + shift) / 40).is_multiple_of(2) {
                230
            } else {
                40
            };
            rgba.extend_from_slice(&[v, v, v, 255]);
        }
    }
    rgba
}

/// Quadro cru da webcam para RGBA. O MJPEG (o mais comum nas webcams USB) é
/// lido pelo `image`, que o app já usa; o resto, pelo próprio nokhwa.
fn decode(buffer: &nokhwa::Buffer) -> Option<(u32, u32, Vec<u8>)> {
    if buffer.source_frame_format() == FrameFormat::MJPEG {
        let image =
            image::load_from_memory_with_format(buffer.buffer(), image::ImageFormat::Jpeg).ok()?;
        let rgba = image.to_rgba8();
        return Some((rgba.width(), rgba.height(), rgba.into_raw()));
    }
    let image = buffer.decode_image::<RgbAFormat>().ok()?;
    Some((image.width(), image.height(), image.into_raw()))
}

/// Abre o tile `id`, ainda sem imagem. O da própria pessoa fica sempre
/// primeiro; os outros entram no fim. Com `enlarge`, ele já abre ampliado
/// (a tela compartilhada por alguém: quem assiste quer ler o que tem nela).
/// Se o tile já estava aberto esperando alguém clicar em assistir
/// (`offer_tile`), ele vira um vídeo de verdade no mesmo lugar.
///
/// Só quem abre e fecha tiles é a tarefa da chamada, em ordem. Os quadros
/// (`TileFeed`) só trocam a imagem de um tile que já existe: um quadro
/// atrasado que chega depois do `remove_tile` não traz o tile de volta.
pub fn add_tile(ui: &slint::Weak<AppWindow>, id: &str, name: &str, enlarge: bool) {
    open_tile(ui, id, name, false, enlarge);
}

/// Abre o tile de uma tela que alguém está mostrando, mas sem o vídeo: só
/// o convite para assistir. Ninguém baixa a tela de ninguém sem clicar, e
/// quem mostra nem manda nada enquanto ninguém assiste (`call.rs`).
pub fn offer_tile(ui: &slint::Weak<AppWindow>, id: &str, name: &str) {
    open_tile(ui, id, name, true, false);
}

fn open_tile(ui: &slint::Weak<AppWindow>, id: &str, name: &str, waiting: bool, enlarge: bool) {
    let id = SharedString::from(id);
    let name = SharedString::from(name);
    let _ = ui.upgrade_in_event_loop(move |ui| {
        let model = ui.get_video_tiles();
        let Some(tiles) = model.as_any().downcast_ref::<VecModel<VideoTile>>() else {
            return;
        };
        if let Some(row) = tiles.iter().position(|t| t.id == id) {
            let mut tile = tiles.row_data(row).unwrap_or_default();
            if tile.waiting != waiting {
                tile.waiting = waiting;
                tile.name = name;
                tile.frame = slint::Image::default();
                tiles.set_row_data(row, tile);
            }
        } else {
            let tile = VideoTile {
                id: id.clone(),
                name,
                frame: slint::Image::default(),
                person: tile_person(&id).into(),
                waiting,
            };
            if id == ME {
                tiles.insert(0, tile);
            } else {
                tiles.push(tile);
            }
        }
        if enlarge {
            ui.set_big_tile(id);
        } else if waiting {
            // Um convite não fica ampliado nem em tela cheia: não tem vídeo.
            if ui.get_big_tile() == id {
                ui.set_big_tile(SharedString::default());
            }
            if ui.get_full_tile() == id {
                ui.set_full_tile(SharedString::default());
            }
        }
    });
}

/// Leva os quadros de um vídeo para o tile `id`, sem deixar fila.
///
/// Os quadros chegam 30 vezes por segundo. Se a thread da tela demora (está
/// desenhando o dither, por exemplo) e cada quadro vira uma tarefa nela, as
/// tarefas se acumulam e o vídeo fica atrasado e aos trancos. Aqui só o
/// último quadro espera: um que chega antes da tela pegar o anterior toma
/// o lugar dele, e só existe uma tarefa por vez na thread da tela.
pub struct TileFeed {
    ui: slint::Weak<AppWindow>,
    /// O tile com este id, ou, com `None`, a prévia do teste da câmera.
    id: Option<SharedString>,
    next: Arc<Mutex<Option<SharedPixelBuffer<Rgba8Pixel>>>>,
}

impl TileFeed {
    pub fn new(ui: &slint::Weak<AppWindow>, id: &str) -> Self {
        Self {
            ui: ui.clone(),
            id: Some(SharedString::from(id)),
            next: Arc::new(Mutex::new(None)),
        }
    }

    /// Os quadros vão para a prévia do teste da câmera nas configurações.
    pub fn test(ui: &slint::Weak<AppWindow>) -> Self {
        Self {
            ui: ui.clone(),
            id: None,
            next: Arc::new(Mutex::new(None)),
        }
    }

    /// Troca o quadro do tile, se ele estiver aberto.
    pub fn show(&self, pixels: SharedPixelBuffer<Rgba8Pixel>) {
        let waiting = self
            .next
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replace(pixels)
            .is_some();
        if waiting {
            // A tarefa da tela ainda não rodou: ela vai levar este quadro.
            return;
        }
        let slot = self.next.clone();
        let take = move || slot.lock().unwrap_or_else(|e| e.into_inner()).take();
        let Some(id) = self.id.clone() else {
            let _ = self.ui.upgrade_in_event_loop(move |ui| {
                // Um quadro atrasado depois de parar o teste não volta a
                // imagem.
                if let Some(pixels) = take()
                    && ui.get_camera_testing()
                {
                    ui.set_camera_test_frame(slint::Image::from_rgba8(pixels));
                }
            });
            return;
        };
        with_tiles(&self.ui, move |tiles| {
            let Some(pixels) = take() else {
                return;
            };
            if let Some(row) = tiles.iter().position(|t| t.id == id && !t.waiting) {
                let mut tile = tiles.row_data(row).unwrap_or_default();
                tile.frame = slint::Image::from_rgba8(pixels);
                tiles.set_row_data(row, tile);
            }
        });
    }
}

/// Fecha o tile `id`. Se era o ampliado, a tela volta ao normal.
pub fn remove_tile(ui: &slint::Weak<AppWindow>, id: &str) {
    let id = SharedString::from(id);
    let _ = ui.upgrade_in_event_loop(move |ui| {
        let model = ui.get_video_tiles();
        if let Some(tiles) = model.as_any().downcast_ref::<VecModel<VideoTile>>()
            && let Some(row) = tiles.iter().position(|t| t.id == id)
        {
            tiles.remove(row);
        }
        if ui.get_big_tile() == id {
            ui.set_big_tile(SharedString::default());
        }
        if ui.get_full_tile() == id {
            ui.set_full_tile(SharedString::default());
        }
    });
}

/// De quem é o tile: o id da pessoa, sem o sufixo da tela. Vazio nos nossos.
fn tile_person(id: &str) -> &str {
    if id == ME || id == crate::screen::ME {
        return "";
    }
    id.strip_suffix(crate::screen::TILE_SUFFIX).unwrap_or(id)
}

pub fn clear_tiles(ui: &slint::Weak<AppWindow>) {
    let _ = ui.upgrade_in_event_loop(|ui| {
        let model = ui.get_video_tiles();
        if let Some(tiles) = model.as_any().downcast_ref::<VecModel<VideoTile>>() {
            tiles.set_vec(Vec::new());
        }
        ui.set_big_tile(SharedString::default());
        ui.set_full_tile(SharedString::default());
    });
}

/// Mexe na lista de tiles dentro da thread da tela. O `main` põe um
/// `VecModel` nela ao abrir a janela; trocar só a linha que mudou evita
/// redesenhar todos os tiles a cada quadro.
fn with_tiles(
    ui: &slint::Weak<AppWindow>,
    change: impl FnOnce(&VecModel<VideoTile>) + Send + 'static,
) {
    let _ = ui.upgrade_in_event_loop(move |ui| {
        let model = ui.get_video_tiles();
        if let Some(tiles) = model.as_any().downcast_ref::<VecModel<VideoTile>>() {
            change(tiles);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_e_da_pessoa_sem_o_sufixo() {
        assert_eq!(tile_person("ana"), "ana");
        assert_eq!(tile_person("ana#tela"), "ana");
        assert_eq!(tile_person(ME), "");
        assert_eq!(tile_person(crate::screen::ME), "");
    }

    #[test]
    fn fit_keeps_small_frames() {
        assert_eq!(fit(320, 240), (320, 240));
    }

    #[test]
    fn fit_shrinks_to_360p_keeping_aspect() {
        assert_eq!(fit(1280, 720), (640, 360));
        assert_eq!(fit(1920, 1080), (640, 360));
        // 4:3 fica limitado pela altura.
        assert_eq!(fit(640, 480), (480, 360));
    }

    #[test]
    fn fit_is_always_even() {
        let (w, h) = fit(1279, 721);
        assert_eq!((w % 2, h % 2), (0, 0));
    }
}

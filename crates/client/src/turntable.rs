//! Liga a cena 3D ao Slint. Rasterização fora da thread da interface; sem
//! atualizações contínuas quando o painel está escondido ou a faixa pausada.
use crate::{
    AppWindow,
    turntable_mesh::{PITCH, PRIMARY, Pose, Scene, YAW, pixel_filter},
};
use slint::{ComponentHandle, Rgba8Pixel, SharedPixelBuffer};
use std::{
    sync::{Arc, Mutex, mpsc},
    thread::JoinHandle,
    time::{Duration, Instant},
};

#[derive(Clone)]
struct State {
    visible: bool,
    playing: bool,
    yaw: f32,
    pitch: f32,
    version: u64,
    stop: bool,
    video_id: String,
    cover: Option<Arc<image::RgbaImage>>,
}

pub struct Renderer {
    _visibility_timer: slint::Timer,
    state: Arc<Mutex<State>>,
    wake: mpsc::SyncSender<()>,
    thread: Option<JoinHandle<()>>,
    cover_requests: tokio::sync::watch::Sender<Option<String>>,
    cover_thread: Option<JoinHandle<()>>,
}
impl Drop for Renderer {
    fn drop(&mut self) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).stop = true;
        let _ = self.cover_requests.send(None);
        let _ = self.wake.try_send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        if let Some(thread) = self.cover_thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn connect(ui: &AppWindow) -> Renderer {
    let state = Arc::new(Mutex::new(State {
        visible: false,
        playing: false,
        yaw: YAW,
        pitch: PITCH,
        version: 1,
        stop: false,
        video_id: String::new(),
        cover: None,
    }));
    let (wake, rx) = mpsc::sync_channel(1);
    let (cover_requests, requests) = tokio::sync::watch::channel(Some(String::new()));
    let cover_state = state.clone();
    let cover_signal = wake.clone();
    let cover_thread =
        std::thread::spawn(move || cover_worker(cover_state, cover_signal, requests));
    let shared = state.clone();
    let signal = wake.clone();
    let request = cover_requests.clone();
    ui.on_turntable_track(move |video_id| {
        let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
        if state.video_id != video_id.as_str() {
            state.video_id = video_id.to_string();
            state.cover = None;
            state.version += 1;
            let _ = request.send(Some(state.video_id.clone()));
            let _ = signal.try_send(());
        }
    });
    let shared = state.clone();
    let signal = wake.clone();
    ui.on_turntable_visibility(move |visible| {
        let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
        if state.visible != visible {
            state.visible = visible;
            state.version += 1;
            let _ = signal.try_send(());
        }
    });
    let shared = state.clone();
    let signal = wake.clone();
    ui.on_turntable_playing(move |playing| {
        let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
        if state.playing != playing {
            state.playing = playing;
            state.version += 1;
            let _ = signal.try_send(());
        }
    });
    let shared = state.clone();
    let signal = wake.clone();
    ui.on_turntable_orbit(move |dx, dy| {
        let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
        state.yaw = (state.yaw + dx * 0.012).rem_euclid(std::f32::consts::TAU);
        state.pitch = (state.pitch + dy * 0.008).clamp(0.12, 1.15);
        state.version += 1;
        let _ = signal.try_send(());
    });
    let shared = state.clone();
    let signal = wake.clone();
    ui.on_turntable_reset(move || {
        let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
        state.yaw = YAW;
        state.pitch = PITCH;
        state.version += 1;
        let _ = signal.try_send(());
    });
    let shared = state.clone();
    let weak = ui.as_weak();
    let thread = std::thread::spawn(move || {
        let scene = match Scene::load(PRIMARY) {
            Ok(scene) => scene,
            Err(reason) => {
                eprintln!("toca-discos: {reason}");
                return;
            }
        };
        let (mut version, mut angle, mut was_playing) = (0, 0.0, false);
        let mut arm = 1.0f32;
        let mut previous = Instant::now();
        loop {
            let options = shared.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if options.stop {
                break;
            }
            let now = Instant::now();
            let elapsed = now.duration_since(previous).as_secs_f32().min(0.3);
            let target_arm = if options.playing { 0.0 } else { 1.0 };
            let previous_arm = arm;
            arm += (target_arm - arm).clamp(-elapsed / 0.3, elapsed / 0.3);
            let arm_changed = (arm - previous_arm).abs() > 0.0001;
            if options.visible && options.playing && was_playing {
                angle = (angle + elapsed * std::f32::consts::TAU * 33.333 / 60.0)
                    .rem_euclid(std::f32::consts::TAU);
            }
            previous = now;
            was_playing = options.visible && options.playing;
            if options.visible && (options.version != version || options.playing || arm_changed) {
                let mut pixels = if arm == 0.0 && options.cover.is_none() {
                    scene.render(96, 90, options.yaw, options.pitch, angle)
                } else {
                    scene.render_pose(
                        96,
                        90,
                        &Pose {
                            yaw: options.yaw,
                            pitch: options.pitch,
                            spin: angle,
                            arm_lift: arm,
                            cover: options.cover.as_deref(),
                        },
                    )
                };
                pixel_filter(&mut pixels, 96);
                if weak
                    .upgrade_in_event_loop(move |ui| {
                        let frame =
                            SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&pixels, 96, 90);
                        ui.set_turntable_frame(slint::Image::from_rgba8(frame));
                    })
                    .is_err()
                {
                    break;
                }
                version = options.version;
            }
            if options.visible && (options.playing || (arm - target_arm).abs() > 0.0001) {
                let _ = rx.recv_timeout(Duration::from_millis(100).saturating_sub(now.elapsed()));
            } else if rx.recv().is_err() {
                break;
            }
            while rx.try_recv().is_ok() {}
        }
    });
    // Observar as propriedades derivadas também quando o antigo painel não
    // existe: o Slint pode avaliar bindings de visibilidade de forma preguiçosa.
    let visibility_timer = slint::Timer::default();
    let visibility_ui = ui.as_weak();
    visibility_timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(100),
        move || {
            if let Some(ui) = visibility_ui.upgrade() {
                ui.invoke_turntable_visibility(ui.get_turntable_visible());
                ui.invoke_turntable_playing(ui.get_activity_playing());
            }
        },
    );
    Renderer {
        _visibility_timer: visibility_timer,
        state,
        wake,
        thread: Some(thread),
        cover_requests,
        cover_thread: Some(cover_thread),
    }
}

fn cover_worker(
    state: Arc<Mutex<State>>,
    wake: mpsc::SyncSender<()>,
    mut requests: tokio::sync::watch::Receiver<Option<String>>,
) {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return;
    };
    runtime.block_on(async {
        let Ok(client) = reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none())
            .build()
        else {
            return;
        };
        loop {
            let Some(id) = requests.borrow_and_update().clone() else {
                break;
            };
            let result = tokio::select! {
                changed = requests.changed() => { if changed.is_err() { break; } continue; }
                image = fetch_cover(&client,&id) => image,
            };
            if let Some(image) = result {
                let mut current = state.lock().unwrap_or_else(|e| e.into_inner());
                if current.video_id == id && !current.stop {
                    current.cover = Some(Arc::new(image));
                    current.version += 1;
                    let _ = wake.try_send(());
                }
            }
            if requests.changed().await.is_err() {
                break;
            }
        }
    });
}

async fn fetch_cover(client: &reqwest::Client, id: &str) -> Option<image::RgbaImage> {
    if id.is_empty() {
        return None;
    }
    // O preview é local e nunca acessa a rede; pode receber uma fixture explícita.
    if false {
        let path = std::env::var_os("MOTHH_TURNTABLE_PREVIEW_COVER")?;
        return Some(image::open(path).ok()?.thumbnail(192, 192).into_rgba8());
    }
    if id.len() != 11
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return None;
    }
    let mut response = client
        .get(format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg"))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() || response.content_length().is_some_and(|n| n > 512 * 1024)
    {
        return None;
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if bytes.len() + chunk.len() > 512 * 1024 {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(2048);
    limits.max_image_height = Some(2048);
    limits.max_alloc = Some(16 * 1024 * 1024);
    reader.limits(limits);
    Some(reader.decode().ok()?.thumbnail(192, 192).into_rgba8())
}

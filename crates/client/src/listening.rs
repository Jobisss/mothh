//! Metadados da sessão Spotify no Windows. Sem áudio ou autenticação Spotify.
use crate::{
    Command,
    modules::{Module, Modules},
};
use protocol::ListeningActivity;
use tokio::sync::mpsc::UnboundedSender;

pub fn start(commands: UnboundedSender<Command>, modules: Modules) {
    std::thread::spawn(move || {
        #[cfg(windows)]
        let initialized = unsafe {
            windows::Win32::System::WinRT::RoInitialize(
                windows::Win32::System::WinRT::RO_INIT_MULTITHREADED,
            )
        }
        .is_ok();
        loop {
            let activity = if modules.is_on(Module::Music) {
                detect()
            } else {
                None
            };
            if commands.send(Command::ListeningSample(activity)).is_err() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(10));
        }
        #[cfg(windows)]
        if initialized {
            unsafe { windows::Win32::System::WinRT::RoUninitialize() };
        }
    });
}

#[cfg(all(windows, debug_assertions))]
pub fn probe() -> Result<String, String> {
    use windows::{
        Media::Control::GlobalSystemMediaTransportControlsSessionManager as Manager,
        Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
    };
    unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.map_err(|e| e.to_string())?;
    let result = (|| {
        Manager::RequestAsync()
            .map_err(|e| e.to_string())?
            .join()
            .map_err(|e| e.to_string())?;
        Ok(format!(
            "API de mídia disponível; Spotify tocando: {}",
            detect().is_some()
        ))
    })();
    unsafe { RoUninitialize() };
    result
}

#[cfg(windows)]
fn detect() -> Option<ListeningActivity> {
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSessionManager as Manager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
    };
    let manager = Manager::RequestAsync().ok()?.join().ok()?;
    for session in manager.GetSessions().ok()? {
        let app = session
            .SourceAppUserModelId()
            .ok()?
            .to_string()
            .to_lowercase();
        if !(app == "spotify.exe" || app.starts_with("spotifyab.spotifymusic_")) {
            continue;
        }
        if session.GetPlaybackInfo().ok()?.PlaybackStatus().ok()? != Status::Playing {
            continue;
        }
        let properties = session.TryGetMediaPropertiesAsync().ok()?.join().ok()?;
        let title = properties.Title().ok()?.to_string();
        if title.trim().is_empty() {
            continue;
        }
        let timeline = session.GetTimelineProperties().ok()?;
        let duration = ((timeline.EndTime().ok()?.Duration - timeline.StartTime().ok()?.Duration)
            .max(0)
            / 10_000) as u64;
        let position = ((timeline.Position().ok()?.Duration - timeline.StartTime().ok()?.Duration)
            .max(0)
            / 10_000) as u64;
        return Some(ListeningActivity {
            title: title
                .chars()
                .filter(|c| !c.is_control())
                .take(120)
                .collect(),
            artist: properties
                .Artist()
                .ok()?
                .to_string()
                .chars()
                .filter(|c| !c.is_control())
                .take(120)
                .collect(),
            duration_ms: duration.min(86_400_000),
            position_ms: position.min(duration.min(86_400_000)),
        });
    }
    None
}

#[cfg(not(windows))]
fn detect() -> Option<ListeningActivity> {
    None
}

pub fn open(title: &str, artist: &str) {
    if title.trim().is_empty() {
        return;
    }
    let query = format!("{title} {artist}");
    let encoded: String = query
        .as_bytes()
        .iter()
        .map(|b| format!("%{b:02X}"))
        .collect();
    let url = format!("https://open.spotify.com/search/{encoded}");
    #[cfg(windows)]
    let opener = "explorer";
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let opener = "xdg-open";
    let _ = std::process::Command::new(opener).arg(url).spawn();
}

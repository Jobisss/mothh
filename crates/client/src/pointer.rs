//! Ponteiro dos amigos: quem mostra a tela deixa quem assiste marcar pontos
//! nela, para ajudar numa tarefa ("clica aqui", "é esse botão").
//!
//! A opção fica na lista de compartilhar, junto do som. Ligada, a faixa da
//! tela sai com o nome [`TRACK_NAME`], e quem assiste fica sabendo que pode
//! clicar na tela ampliada ou em tela cheia. Cada clique vira um pacote de
//! dados do LiveKit ([`TOPIC`]) para a sala toda, cifrado ponta a ponta com a
//! mesma chave da mídia (o SDK cifra os pacotes de dados quando a sala tem
//! cifra, `sframe.rs`). O ponto vai de 0 a 1 na imagem, então não depende do
//! tamanho da tela de ninguém.
//!
//! Quem mostra vê a marca por cima da tela de verdade: uma janela do tamanho
//! da tela (ou da janela) compartilhada, sempre por cima, que não pega
//! clique nem foco e fica fora da captura, para a marca não aparecer duas
//! vezes no vídeo. Isso só no Windows por enquanto; nos outros sistemas a
//! marca aparece só na prévia da própria tela, no app. Quem assiste junto vê
//! a marca no tile da tela.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::{AppWindow, PointerPing};

/// Tópico dos pacotes de dados do ponteiro.
pub const TOPIC: &str = "mothh/ponteiro";
/// Nome da faixa da tela quando o ponteiro está ligado.
pub const TRACK_NAME: &str = "tela+ponteiro";
/// Quanto tempo uma marca fica na tela.
const LIFE: Duration = Duration::from_millis(2600);
/// Marcas ao mesmo tempo, no máximo; a mais velha sai primeiro.
const MAX: usize = 24;
/// Intervalo mínimo entre duas marcas de quem assiste (clique segurado ou
/// repetido não vira enxurrada).
const MIN_GAP: Duration = Duration::from_millis(120);

/// O ponto `x`, `y` (0 a 1) na tela que `to` mostra.
pub fn encode(to: &str, x: f32, y: f32) -> Vec<u8> {
    serde_json::json!({ "to": to, "x": x, "y": y })
        .to_string()
        .into_bytes()
}

/// O contrário de [`encode`]. Ponto fora da imagem ou pacote estranho: `None`.
pub fn decode(payload: &[u8]) -> Option<(String, f32, f32)> {
    let value: serde_json::Value = serde_json::from_slice(payload).ok()?;
    let to = value.get("to")?.as_str()?.to_string();
    let x = value.get("x")?.as_f64()? as f32;
    let y = value.get("y")?.as_f64()? as f32;
    let inside = |v: f32| (0.0..=1.0).contains(&v);
    (inside(x) && inside(y) && !to.is_empty()).then_some((to, x, y))
}

thread_local! {
    static NEXT_KEY: Cell<i32> = const { Cell::new(1) };
    static LAST_SENT: Cell<Option<Instant>> = const { Cell::new(None) };
    static OVERLAY: RefCell<Option<Overlay>> = const { RefCell::new(None) };
}

/// Se já dá para mandar outra marca (respeita [`MIN_GAP`]). Thread da tela.
pub fn allow_now() -> bool {
    let now = Instant::now();
    LAST_SENT.with(|last| {
        if last.get().is_some_and(|t| now.duration_since(t) < MIN_GAP) {
            return false;
        }
        last.set(Some(now));
        true
    })
}

/// Mostra uma marca no tile `tile` e, com `on_screen`, por cima da tela de
/// verdade. Pode ser chamada de qualquer thread.
pub fn show(
    ui: &slint::Weak<AppWindow>,
    tile: String,
    x: f32,
    y: f32,
    name: String,
    on_screen: bool,
) {
    let _ = ui.upgrade_in_event_loop(move |ui| show_now(&ui, tile, x, y, name, on_screen));
}

/// O mesmo que [`show`], já na thread da tela.
pub fn show_now(ui: &AppWindow, tile: String, x: f32, y: f32, name: String, on_screen: bool) {
    let key = NEXT_KEY.with(|k| {
        let key = k.get();
        k.set(key.wrapping_add(1));
        key
    });
    let ping = PointerPing {
        key,
        tile: tile.into(),
        x,
        y,
        name: SharedString::from(name),
    };
    push(&ui.get_pointer_pings(), ping.clone());
    if on_screen {
        OVERLAY.with(|o| {
            if let Some(overlay) = o.borrow().as_ref() {
                push(&overlay.window.get_pings(), ping);
            }
        });
    }
    let weak = ui.as_weak();
    slint::Timer::single_shot(LIFE, move || {
        if let Some(ui) = weak.upgrade() {
            remove(&ui.get_pointer_pings(), key);
        }
        OVERLAY.with(|o| {
            if let Some(overlay) = o.borrow().as_ref() {
                remove(&overlay.window.get_pings(), key);
            }
        });
    });
}

fn push(model: &ModelRc<PointerPing>, ping: PointerPing) {
    if let Some(list) = model.as_any().downcast_ref::<VecModel<PointerPing>>() {
        if list.row_count() >= MAX {
            list.remove(0);
        }
        list.push(ping);
    }
}

fn remove(model: &ModelRc<PointerPing>, key: i32) {
    if let Some(list) = model.as_any().downcast_ref::<VecModel<PointerPing>>()
        && let Some(row) = list.iter().position(|p| p.key == key)
    {
        list.remove(row);
    }
}

/// O modelo das marcas da janela principal. O `main` chama ao abrir.
pub fn init(ui: &AppWindow) {
    ui.set_pointer_pings(ModelRc::new(VecModel::<PointerPing>::default()));
}

/// A janela por cima da tela compartilhada, enquanto o ponteiro está ligado.
struct Overlay {
    window: crate::PointerOverlay,
    /// Acompanha a tela ou janela: se a janela compartilhada anda, as
    /// marcas andam junto.
    _timer: slint::Timer,
}

/// Abre a janela das marcas por cima de `source_id` (o id de um
/// `screen::ShareSource`). Qualquer thread.
pub fn open_overlay(ui: &slint::Weak<AppWindow>, source_id: String) {
    let _ = ui.upgrade_in_event_loop(move |ui| {
        close_now();
        if let Err(err) = open_now(&ui, source_id) {
            eprintln!("ponteiro: a janela das marcas não abriu: {err}");
        }
    });
}

/// Fecha a janela das marcas, se aberta. Qualquer thread.
pub fn close_overlay(ui: &slint::Weak<AppWindow>) {
    let _ = ui.upgrade_in_event_loop(|_| close_now());
}

fn close_now() {
    if let Some(overlay) = OVERLAY.with(|o| o.borrow_mut().take()) {
        let _ = overlay.window.hide();
    }
}

#[cfg(not(windows))]
fn open_now(_ui: &AppWindow, _source_id: String) -> Result<(), slint::PlatformError> {
    Ok(())
}

#[cfg(windows)]
fn open_now(ui: &AppWindow, source_id: String) -> Result<(), slint::PlatformError> {
    use slint::winit_030::WinitWindowAccessor;

    let Some(area) = win::area(&source_id) else {
        eprintln!("ponteiro: não achei onde fica {source_id}");
        return Ok(());
    };
    let window = crate::PointerOverlay::new()?;
    let theme = ui.global::<crate::Theme>();
    let mine = window.global::<crate::Theme>();
    mine.set_pixel(theme.get_pixel());
    mine.set_pixel_scale(theme.get_pixel_scale());
    window.set_pings(ModelRc::new(VecModel::<PointerPing>::default()));
    place(&window, area);
    window.show()?;
    window.window().with_winit_window(|w| {
        use slint::winit_030::winit::platform::windows::WindowExtWindows;
        use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        w.set_skip_taskbar(true);
        let _ = w.set_cursor_hittest(false);
        if let Ok(handle) = w.window_handle()
            && let RawWindowHandle::Win32(h) = handle.as_raw()
        {
            win::make_overlay(h.hwnd.get());
        }
    });
    place(&window, area);
    // Abrir a janela tira o foco do app; devolve.
    ui.window().with_winit_window(|w| w.focus_window());

    let weak = window.as_weak();
    let last = Cell::new(area);
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(250),
        move || {
            let (Some(window), Some(area)) = (weak.upgrade(), win::area(&source_id)) else {
                return;
            };
            if area != last.get() {
                last.set(area);
                place(&window, area);
            }
        },
    );
    OVERLAY.with(|o| {
        *o.borrow_mut() = Some(Overlay {
            window,
            _timer: timer,
        })
    });
    Ok(())
}

#[cfg(windows)]
fn place(window: &crate::PointerOverlay, area: win::Area) {
    window
        .window()
        .set_position(slint::PhysicalPosition::new(area.x, area.y));
    // Um pixel a menos na altura: uma janela por cima do monitor inteiro
    // faz o Windows achar que é um jogo em tela cheia (esconde a barra).
    window.window().set_size(slint::PhysicalSize::new(
        area.width.max(1) as u32,
        (area.height - 1).max(1) as u32,
    ));
}

#[cfg(windows)]
mod win {
    use windows::Win32::Foundation::{COLORREF, HWND, RECT};
    use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
    use windows::Win32::Graphics::Gdi::{
        DEVMODEW, DISPLAY_DEVICEW, ENUM_CURRENT_SETTINGS, EnumDisplayDevicesW,
        EnumDisplaySettingsExW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, IsIconic, IsWindow, LWA_COLORKEY,
        SetLayeredWindowAttributes, SetWindowDisplayAffinity, SetWindowLongPtrW,
        WDA_EXCLUDEFROMCAPTURE, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        WS_EX_TRANSPARENT,
    };
    use windows::core::PCWSTR;

    /// Um retângulo em pixels da área de trabalho.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Area {
        pub x: i32,
        pub y: i32,
        pub width: i32,
        pub height: i32,
    }

    /// Onde está a tela (`s:<id>`) ou a janela (`w:<id>`) compartilhada.
    pub fn area(source_id: &str) -> Option<Area> {
        if let Some(id) = source_id.strip_prefix("s:") {
            screen(id.parse().ok()?)
        } else {
            window(source_id.strip_prefix("w:")?.parse().ok()?)
        }
    }

    /// O id de uma tela no capturador do WebRTC é a posição dela na lista
    /// do `EnumDisplayDevices`; o lugar dela vem das configurações atuais.
    fn screen(index: u32) -> Option<Area> {
        let mut device = DISPLAY_DEVICEW {
            cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32,
            ..Default::default()
        };
        // SAFETY: as estruturas são nossas e com o tamanho preenchido.
        unsafe {
            if !EnumDisplayDevicesW(PCWSTR::null(), index, &mut device, 0).as_bool() {
                return None;
            }
            let mut mode = DEVMODEW {
                dmSize: std::mem::size_of::<DEVMODEW>() as u16,
                ..Default::default()
            };
            if !EnumDisplaySettingsExW(
                PCWSTR(device.DeviceName.as_ptr()),
                ENUM_CURRENT_SETTINGS,
                &mut mode,
                Default::default(),
            )
            .as_bool()
            {
                return None;
            }
            let at = mode.Anonymous1.Anonymous2.dmPosition;
            Some(Area {
                x: at.x,
                y: at.y,
                width: mode.dmPelsWidth as i32,
                height: mode.dmPelsHeight as i32,
            })
        }
    }

    /// O id de uma janela no capturador é o HWND dela. Minimizada ou
    /// fechada: `None` (a janela das marcas fica onde estava).
    fn window(id: u64) -> Option<Area> {
        let hwnd = HWND(id as usize as *mut _);
        let mut rect = RECT::default();
        // SAFETY: só lê; um HWND que não existe mais dá erro.
        unsafe {
            if !IsWindow(Some(hwnd)).as_bool() || IsIconic(hwnd).as_bool() {
                return None;
            }
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                &mut rect as *mut RECT as *mut _,
                std::mem::size_of::<RECT>() as u32,
            )
            .ok()?;
        }
        Some(Area {
            x: rect.left,
            y: rect.top,
            width: rect.right - rect.left,
            height: rect.bottom - rect.top,
        })
    }

    /// Deixa a janela das marcas transparente (a cor de fundo, magenta, vira
    /// buraco), sem pegar clique nem foco, fora da barra e fora da captura.
    pub fn make_overlay(hwnd: isize) {
        let hwnd = HWND(hwnd as *mut _);
        // SAFETY: a janela é nossa e está aberta.
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let extra = (WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW).0;
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | extra as isize);
            // #ff00ff, o fundo do `PointerOverlay`.
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0x00ff00ff), 0, LWA_COLORKEY);
            // Fora da captura (Windows 10 2004 ou mais novo): quem assiste já
            // vê a marca no tile, sem ela vir de novo no vídeo.
            if SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE).is_err() {
                eprintln!("ponteiro: a janela das marcas vai aparecer na captura");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ponto_vai_e_volta() {
        let bytes = encode("ana", 0.25, 0.75);
        assert_eq!(decode(&bytes), Some(("ana".into(), 0.25, 0.75)));
    }

    #[test]
    fn ponto_fora_da_tela_ou_pacote_estranho_nao_vale() {
        assert_eq!(decode(&encode("ana", 1.5, 0.5)), None);
        assert_eq!(decode(&encode("ana", 0.5, -0.1)), None);
        assert_eq!(decode(&encode("", 0.5, 0.5)), None);
        assert_eq!(decode(b"oi"), None);
        assert_eq!(decode(br#"{"to":"ana","x":"a","y":0}"#), None);
    }
}

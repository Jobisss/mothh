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
//! da tela (ou da janela) compartilhada, sempre por cima, transparente pixel
//! a pixel (desenhada aqui mesmo, sem o Slint), que não pega clique nem foco
//! e fica fora da captura, para a marca não aparecer duas vezes no vídeo. Isso só no Windows por enquanto; nos outros sistemas a
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
    if on_screen {
        mark_on_screen(x, y, &name);
    }
    let ping = PointerPing {
        key,
        tile: tile.into(),
        x,
        y,
        name: SharedString::from(name),
    };
    push(&ui.get_pointer_pings(), ping);
    let weak = ui.as_weak();
    slint::Timer::single_shot(LIFE, move || {
        if let Some(ui) = weak.upgrade() {
            remove(&ui.get_pointer_pings(), key);
        }
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

/// A camada por cima da tela compartilhada, enquanto o ponteiro está ligado.
struct Overlay {
    #[cfg(windows)]
    layer: win::Layer,
    /// Acompanha a tela ou janela: se a janela compartilhada anda, as
    /// marcas andam junto.
    _follow: slint::Timer,
    /// Redesenha enquanto alguma marca está viva.
    #[cfg(windows)]
    anim: slint::Timer,
}

/// Abre a camada das marcas por cima de `source_id` (o id de um
/// `screen::ShareSource`). Qualquer thread.
pub fn open_overlay(ui: &slint::Weak<AppWindow>, source_id: String) {
    let _ = ui.upgrade_in_event_loop(move |_| {
        close_now();
        open_now(source_id);
    });
}

/// Fecha a camada das marcas, se aberta. Qualquer thread.
pub fn close_overlay(ui: &slint::Weak<AppWindow>) {
    let _ = ui.upgrade_in_event_loop(|_| close_now());
}

fn close_now() {
    drop(OVERLAY.with(|o| o.borrow_mut().take()));
}

#[cfg(not(windows))]
fn open_now(_source_id: String) {}

#[cfg(not(windows))]
fn mark_on_screen(_x: f32, _y: f32, _name: &str) {}

#[cfg(windows)]
fn open_now(source_id: String) {
    let Some(area) = win::area(&source_id) else {
        eprintln!("ponteiro: não achei onde fica {source_id}");
        return;
    };
    let Some(layer) = win::Layer::open(area) else {
        eprintln!("ponteiro: a camada das marcas não abriu");
        return;
    };
    let follow = slint::Timer::default();
    follow.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(250),
        move || {
            let Some(area) = win::area(&source_id) else {
                return;
            };
            OVERLAY.with(|o| {
                if let Some(overlay) = o.borrow_mut().as_mut() {
                    overlay.layer.move_to(area);
                }
            });
        },
    );
    OVERLAY.with(|o| {
        *o.borrow_mut() = Some(Overlay {
            layer,
            _follow: follow,
            anim: slint::Timer::default(),
        })
    });
}

#[cfg(windows)]
fn mark_on_screen(x: f32, y: f32, name: &str) {
    OVERLAY.with(|o| {
        let mut o = o.borrow_mut();
        let Some(overlay) = o.as_mut() else {
            return;
        };
        overlay.layer.add(x, y, name);
        if overlay.layer.len() > MAX {
            overlay.layer.drop_oldest();
        }
        if !overlay.anim.running() {
            overlay.anim.start(
                slint::TimerMode::Repeated,
                Duration::from_millis(33),
                || {
                    OVERLAY.with(|o| {
                        if let Some(overlay) = o.borrow_mut().as_mut()
                            && !overlay.layer.draw()
                        {
                            overlay.anim.stop();
                        }
                    });
                },
            );
        }
    });
}

#[cfg(windows)]
mod win {
    use std::time::Instant;

    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
    use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
    use windows::Win32::Graphics::Gdi::{
        AC_SRC_ALPHA, AC_SRC_OVER, ANTIALIASED_QUALITY, AddFontMemResourceEx, BI_RGB, BITMAPINFO,
        BITMAPINFOHEADER, BLENDFUNCTION, CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateDIBSection,
        CreateFontW, DEFAULT_CHARSET, DEVMODEW, DIB_RGB_COLORS, DISPLAY_DEVICEW, DeleteDC,
        DeleteObject, ENUM_CURRENT_SETTINGS, EnumDisplayDevicesW, EnumDisplaySettingsExW, GdiFlush,
        GetTextExtentPoint32W, HBITMAP, HDC, HFONT, HGDIOBJ, OUT_DEFAULT_PRECIS, SelectObject,
        SetBkMode, SetTextColor, TRANSPARENT, TextOutW,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, HTTRANSPARENT, HWND_TOPMOST, IsIconic,
        IsWindow, RegisterClassW, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SetWindowDisplayAffinity,
        SetWindowPos, ShowWindow, ULW_ALPHA, UpdateLayeredWindow, WDA_EXCLUDEFROMCAPTURE,
        WM_NCHITTEST, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
        WS_EX_TRANSPARENT, WS_POPUP,
    };
    use windows::core::{PCWSTR, w};

    /// Quanto dura uma marca na tela de quem mostra.
    const LIFE: f32 = 2.4;
    const WHITE: u32 = 0xffff_ffff;
    const BLACK: u32 = 0xff00_0000;
    static VT323: &[u8] = include_bytes!("../ui/fonts/VT323-Regular.ttf");

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
    /// fechada: `None` (a camada fica onde estava).
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

    extern "system" fn procedure(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if msg == WM_NCHITTEST {
            return LRESULT(HTTRANSPARENT as isize);
        }
        // SAFETY: repassa a mensagem como veio.
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    struct Mark {
        born: Instant,
        x: f32,
        y: f32,
        name: Vec<u16>,
    }

    /// Uma janela "em camadas" com transparência por pixel: o que não é
    /// marca é 100% transparente na tela e na captura, seja qual for o
    /// desenho do app (o Slint não desenha aqui). Não pega clique nem foco
    /// e, no Windows 10 2004 ou mais novo, fica fora da captura.
    pub struct Layer {
        hwnd: HWND,
        area: Area,
        dc: HDC,
        bitmap: HBITMAP,
        old_bitmap: HGDIOBJ,
        pixels: *mut u32,
        font: HFONT,
        scale: f32,
        marks: Vec<Mark>,
    }

    impl Layer {
        pub fn open(area: Area) -> Option<Self> {
            let area = shrink(area);
            // SAFETY: chamadas Win32 com estruturas nossas; tudo o que é
            // criado aqui é solto no `Drop`.
            unsafe {
                let instance = GetModuleHandleW(None).ok()?;
                let class = w!("mothh-ponteiro");
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(procedure),
                    hInstance: instance.into(),
                    lpszClassName: class,
                    ..Default::default()
                };
                // Já registrada numa vez anterior: tudo bem.
                RegisterClassW(&wc);
                let hwnd = CreateWindowExW(
                    WS_EX_LAYERED
                        | WS_EX_TRANSPARENT
                        | WS_EX_TOPMOST
                        | WS_EX_TOOLWINDOW
                        | WS_EX_NOACTIVATE,
                    class,
                    w!("mothh · ponteiro"),
                    WS_POPUP,
                    area.x,
                    area.y,
                    area.width,
                    area.height,
                    None,
                    None,
                    Some(instance.into()),
                    None,
                )
                .ok()?;
                if SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE).is_err() {
                    eprintln!("ponteiro: as marcas vão aparecer também na captura");
                }
                let scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
                let fonts = 0;
                AddFontMemResourceEx(
                    VT323.as_ptr() as *const _,
                    VT323.len() as u32,
                    None,
                    &fonts,
                );
                let font = CreateFontW(
                    -(22.0 * scale) as i32,
                    0,
                    0,
                    0,
                    400,
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET,
                    OUT_DEFAULT_PRECIS,
                    CLIP_DEFAULT_PRECIS,
                    ANTIALIASED_QUALITY,
                    0,
                    w!("VT323"),
                );
                let dc = CreateCompatibleDC(None);
                let mut layer = Layer {
                    hwnd,
                    area,
                    dc,
                    bitmap: HBITMAP::default(),
                    old_bitmap: HGDIOBJ::default(),
                    pixels: std::ptr::null_mut(),
                    font,
                    scale,
                    marks: Vec::new(),
                };
                if !layer.resize() {
                    return None;
                }
                SelectObject(dc, font.into());
                SetBkMode(dc, TRANSPARENT);
                SetTextColor(dc, windows::Win32::Foundation::COLORREF(0x00ff_ffff));
                // Uma camada nunca mostrada precisa de um quadro antes.
                layer.draw();
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                Some(layer)
            }
        }

        /// Cria o quadro do tamanho da área. `false` se não deu.
        fn resize(&mut self) -> bool {
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: self.area.width,
                    // Negativo: a primeira linha é a de cima.
                    biHeight: -self.area.height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            // SAFETY: o quadro antigo sai do DC antes de ser apagado.
            unsafe {
                let Ok(bitmap) =
                    CreateDIBSection(Some(self.dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
                else {
                    return false;
                };
                let old = SelectObject(self.dc, bitmap.into());
                if self.bitmap.is_invalid() {
                    self.old_bitmap = old;
                } else {
                    let _ = DeleteObject(self.bitmap.into());
                }
                self.bitmap = bitmap;
            }
            self.pixels = bits as *mut u32;
            true
        }

        pub fn move_to(&mut self, area: Area) {
            let area = shrink(area);
            if area == self.area {
                return;
            }
            let resized = (area.width, area.height) != (self.area.width, self.area.height);
            self.area = area;
            if resized && !self.resize() {
                return;
            }
            // SAFETY: a janela é nossa.
            unsafe {
                let _ = SetWindowPos(
                    self.hwnd,
                    Some(HWND_TOPMOST),
                    area.x,
                    area.y,
                    area.width,
                    area.height,
                    SWP_NOACTIVATE,
                );
            }
            self.draw();
        }

        pub fn add(&mut self, x: f32, y: f32, name: &str) {
            self.marks.push(Mark {
                born: Instant::now(),
                x,
                y,
                name: name.encode_utf16().collect(),
            });
        }

        pub fn len(&self) -> usize {
            self.marks.len()
        }

        pub fn drop_oldest(&mut self) {
            if !self.marks.is_empty() {
                self.marks.remove(0);
            }
        }

        /// Desenha as marcas vivas (e apaga as vencidas). `true` enquanto
        /// ainda tem marca na tela.
        pub fn draw(&mut self) -> bool {
            let now = Instant::now();
            self.marks
                .retain(|m| now.duration_since(m.born).as_secs_f32() < LIFE);
            let (w, h) = (self.area.width, self.area.height);
            let len = (w.max(0) * h.max(0)) as usize;
            if self.pixels.is_null() || len == 0 {
                return false;
            }
            // SAFETY: `pixels` é o quadro `w` x `h` do DIB selecionado.
            let buf = unsafe {
                let _ = GdiFlush();
                std::slice::from_raw_parts_mut(self.pixels, len)
            };
            buf.fill(0);
            let s = self.scale;
            let px = |v: f32| (v * s).round() as i32;
            for mark in &self.marks {
                let t = now.duration_since(mark.born).as_secs_f32() / LIFE;
                let cx = (mark.x * w as f32) as i32;
                let cy = (mark.y * h as f32) as i32;
                // O anel que cresce: preto por fora, branco no meio, preto
                // por dentro. Três pulsos e some.
                let ring = px(14.0 + 30.0 * (t * 3.0).fract()) + 2;
                let (rx, ry) = (cx - ring / 2, cy - ring / 2);
                frame(buf, w, h, rx, ry, ring, ring, px(5.0).max(3), BLACK);
                let b = px(1.0).max(1);
                frame(
                    buf,
                    w,
                    h,
                    rx + b,
                    ry + b,
                    ring - 2 * b,
                    ring - 2 * b,
                    px(3.0).max(2),
                    WHITE,
                );
                // O centro, parado.
                let c = px(10.0);
                fill(buf, w, h, cx - c / 2, cy - c / 2, c, c, BLACK);
                let i = px(6.0);
                fill(buf, w, h, cx - i / 2, cy - i / 2, i, i, WHITE);
                // O nome de quem marcou, numa caixa preta.
                let mut size = SIZE::default();
                // SAFETY: o DC é nosso, com a fonte selecionada.
                unsafe {
                    let _ = GetTextExtentPoint32W(self.dc, &mark.name, &mut size);
                }
                let (lx, ly) = (cx + px(26.0), cy + px(18.0));
                let (lw, lh) = (size.cx + px(10.0), size.cy + px(2.0));
                fill(buf, w, h, lx, ly, lw, lh, WHITE);
                fill(buf, w, h, lx + b, ly + b, lw - 2 * b, lh - 2 * b, BLACK);
                // SAFETY: idem.
                unsafe {
                    let _ = TextOutW(self.dc, lx + px(5.0), ly + px(1.0), &mark.name);
                    let _ = GdiFlush();
                }
                // O GDI escreve o texto com alfa 0; dentro da caixa tudo é opaco.
                opaque(buf, w, h, lx, ly, lw, lh);
            }
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let at = POINT {
                x: self.area.x,
                y: self.area.y,
            };
            let size = SIZE { cx: w, cy: h };
            let origin = POINT::default();
            // SAFETY: janela e DC nossos.
            unsafe {
                let _ = UpdateLayeredWindow(
                    self.hwnd,
                    None,
                    Some(&at),
                    Some(&size),
                    Some(self.dc),
                    Some(&origin),
                    windows::Win32::Foundation::COLORREF(0),
                    Some(&blend),
                    ULW_ALPHA,
                );
            }
            !self.marks.is_empty()
        }
    }

    impl Drop for Layer {
        fn drop(&mut self) {
            // SAFETY: solta o que `open` criou, na ordem certa.
            unsafe {
                let _ = DestroyWindow(self.hwnd);
                if !self.old_bitmap.is_invalid() {
                    SelectObject(self.dc, self.old_bitmap);
                }
                if !self.bitmap.is_invalid() {
                    let _ = DeleteObject(self.bitmap.into());
                }
                let _ = DeleteDC(self.dc);
                let _ = DeleteObject(self.font.into());
            }
        }
    }

    /// Um pixel a menos na altura: uma janela por cima do monitor inteiro
    /// faz o Windows achar que é um jogo em tela cheia.
    fn shrink(area: Area) -> Area {
        Area {
            width: area.width.max(1),
            height: (area.height - 1).max(1),
            ..area
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn fill(buf: &mut [u32], w: i32, h: i32, x: i32, y: i32, rw: i32, rh: i32, color: u32) {
        let (x0, y0) = (x.max(0), y.max(0));
        let (x1, y1) = ((x + rw).min(w), (y + rh).min(h));
        for row in y0..y1 {
            let start = (row * w) as usize;
            for col in x0..x1 {
                buf[start + col as usize] = color;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn frame(
        buf: &mut [u32],
        w: i32,
        h: i32,
        x: i32,
        y: i32,
        rw: i32,
        rh: i32,
        t: i32,
        color: u32,
    ) {
        fill(buf, w, h, x, y, rw, t, color);
        fill(buf, w, h, x, y + rh - t, rw, t, color);
        fill(buf, w, h, x, y, t, rh, color);
        fill(buf, w, h, x + rw - t, y, t, rh, color);
    }

    fn opaque(buf: &mut [u32], w: i32, h: i32, x: i32, y: i32, rw: i32, rh: i32) {
        let (x0, y0) = (x.max(0), y.max(0));
        let (x1, y1) = ((x + rw).min(w), (y + rh).min(h));
        for row in y0..y1 {
            let start = (row * w) as usize;
            for col in x0..x1 {
                buf[start + col as usize] |= 0xff00_0000;
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

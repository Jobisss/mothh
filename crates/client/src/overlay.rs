//! Modo overlay: o app minimiza e fica só uma janelinha por cima de tudo
//! (`OverlayWindow` em `app.slint`), com o nome da conversa da chamada, quem
//! está nela e uma mensagem rápida.
//!
//! A janelinha não tem estado próprio: um timer copia da janela principal o
//! que ela já mostra (quem está na chamada, o microfone, as últimas
//! mensagens da conversa aberta, que o overlay troca para a da chamada).
//! Voltar para o app (o botão, ou abrir o app pela barra de tarefas) fecha a
//! janelinha.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use slint::winit_030::WinitWindowAccessor;
use slint::{ComponentHandle, Model, ModelRc, PhysicalPosition, VecModel};
use tokio::sync::mpsc::UnboundedSender;

use crate::{AppWindow, Command, MessageRow, OverlayWindow};

/// Quantas mensagens o overlay mostra.
const MESSAGES: usize = 5;
/// Largura da janelinha (a altura muda com a mensagem rápida aberta).
const WIDTH: f32 = 300.0;
/// Distância da borda da tela onde ela abre na primeira vez.
const MARGIN: i32 = 24;
const TICK: Duration = Duration::from_millis(100);

struct State {
    window: Option<OverlayWindow>,
    timer: slint::Timer,
    /// A janela principal já foi vista minimizada desde que o overlay abriu:
    /// a partir daí, ela voltar quer dizer que a pessoa abriu o app.
    saw_minimized: Cell<bool>,
    /// As mensagens copiadas por último: só refaz a lista quando mudam.
    shown: RefCell<Vec<(String, String, String)>>,
    /// Onde a janelinha estava, para abrir no mesmo lugar.
    position: Cell<Option<PhysicalPosition>>,
}

pub fn connect(ui: &AppWindow, commands: UnboundedSender<Command>) {
    let state = Rc::new(RefCell::new(State {
        window: None,
        timer: slint::Timer::default(),
        saw_minimized: Cell::new(false),
        shown: RefCell::new(Vec::new()),
        position: Cell::new(None),
    }));
    let main = ui.as_weak();
    ui.on_open_overlay(move || {
        let Some(ui) = main.upgrade() else {
            return;
        };
        if let Err(e) = open(&ui, &state, &commands) {
            eprintln!("overlay: não abriu: {e}");
        }
    });
}

fn open(
    ui: &AppWindow,
    state: &Rc<RefCell<State>>,
    commands: &UnboundedSender<Command>,
) -> Result<(), slint::PlatformError> {
    // As mensagens do overlay são as da conversa aberta: abre a da chamada.
    let _ = commands.send(Command::OpenCallChat);
    // Sai da tela cheia antes de minimizar (senão ela volta em tela cheia).
    ui.set_full_tile("".into());

    let mut st = state.borrow_mut();
    if st.window.is_none() {
        st.window = Some(create(ui, state, commands)?);
    }
    let overlay = st.window.as_ref().expect("acabou de criar").clone_strong();
    st.saw_minimized.set(false);
    st.shown.borrow_mut().clear();
    sync(ui, &overlay, &st);
    let position = st.position.get().or_else(|| top_right(ui));
    drop(st);

    overlay
        .window()
        .set_size(slint::LogicalSize::new(WIDTH, overlay.get_wanted_height()));
    if let Some(position) = position {
        overlay.window().set_position(position);
    }
    overlay.show()?;
    hide_from_taskbar(&overlay);
    ui.window().set_minimized(true);

    let main = ui.as_weak();
    let weak_state = Rc::downgrade(state);
    state
        .borrow()
        .timer
        .start(slint::TimerMode::Repeated, TICK, move || {
            let (Some(ui), Some(state)) = (main.upgrade(), weak_state.upgrade()) else {
                return;
            };
            tick(&ui, &state);
        });
    Ok(())
}

fn create(
    ui: &AppWindow,
    state: &Rc<RefCell<State>>,
    commands: &UnboundedSender<Command>,
) -> Result<OverlayWindow, slint::PlatformError> {
    let overlay = OverlayWindow::new()?;

    let weak = overlay.as_weak();
    overlay.on_drag(move || {
        if let Some(overlay) = weak.upgrade() {
            overlay.window().with_winit_window(|w| {
                let _ = w.drag_window();
            });
        }
    });

    let main = ui.as_weak();
    let weak_state = Rc::downgrade(state);
    overlay.on_open_app(move || {
        if let (Some(ui), Some(state)) = (main.upgrade(), weak_state.upgrade()) {
            close(&ui, &state);
        }
    });

    let sender = commands.clone();
    overlay.on_toggle_mute(move || {
        let _ = sender.send(Command::ToggleMute);
    });

    let sender = commands.clone();
    overlay.on_send(move |text| {
        let text = text.trim().to_string();
        if !text.is_empty() {
            let _ = sender.send(Command::SendCall(text));
        }
    });

    // O "X" do sistema (Alt+F4) também volta para o app.
    let main = ui.as_weak();
    let weak_state = Rc::downgrade(state);
    overlay.window().on_close_requested(move || {
        if let (Some(ui), Some(state)) = (main.upgrade(), weak_state.upgrade()) {
            close(&ui, &state);
        }
        slint::CloseRequestResponse::KeepWindowShown
    });
    Ok(overlay)
}

/// Fecha a janelinha e traz o app de volta.
fn close(ui: &AppWindow, state: &Rc<RefCell<State>>) {
    let st = state.borrow();
    st.timer.stop();
    if let Some(overlay) = &st.window {
        st.position.set(Some(overlay.window().position()));
        let _ = overlay.hide();
    }
    drop(st);
    ui.window().set_minimized(false);
    let _ = ui.show();
    ui.window().with_winit_window(|w| w.focus_window());
}

fn tick(ui: &AppWindow, state: &Rc<RefCell<State>>) {
    let st = state.borrow();
    let Some(overlay) = st.window.as_ref().map(|w| w.clone_strong()) else {
        return;
    };
    // Abriram o app pela barra de tarefas: a janelinha sai.
    let minimized = ui.window().is_minimized();
    if minimized {
        st.saw_minimized.set(true);
    } else if st.saw_minimized.get() {
        drop(st);
        close(ui, state);
        return;
    }
    sync(ui, &overlay, &st);
    let height = overlay.get_wanted_height();
    let size = overlay
        .window()
        .size()
        .to_logical(overlay.window().scale_factor());
    if (size.height - height).abs() > 0.5 {
        overlay
            .window()
            .set_size(slint::LogicalSize::new(WIDTH, height));
    }
}

/// Copia da janela principal o que o overlay mostra.
fn sync(ui: &AppWindow, overlay: &OverlayWindow, st: &State) {
    overlay.set_call_name(ui.get_call_name());
    overlay.set_connected(ui.get_call_connected());
    overlay.set_muted(ui.get_call_muted());
    // O mesmo modelo da janela principal: a borda da voz anda junto.
    overlay.set_people(ui.get_call_people());
    overlay.set_my_photo(ui.get_profile_photo());

    // As mensagens só valem se a conversa aberta é a da chamada.
    let rows: Vec<MessageRow> = if ui.get_call_is_open() {
        let messages = ui.get_messages();
        let count = messages.row_count();
        (count.saturating_sub(MESSAGES)..count)
            .filter_map(|i| messages.row_data(i))
            .collect()
    } else {
        Vec::new()
    };
    let keys: Vec<(String, String, String)> = rows
        .iter()
        .map(|row| {
            (
                row.author.to_string(),
                row.time.to_string(),
                format!("{}{}{}", row.text, row.audio_id, row.file_id),
            )
        })
        .collect();
    if *st.shown.borrow() != keys {
        *st.shown.borrow_mut() = keys;
        overlay.set_messages(ModelRc::new(VecModel::from(rows)));
    }
}

/// O canto de cima à direita da tela onde está o app.
fn top_right(ui: &AppWindow) -> Option<PhysicalPosition> {
    ui.window()
        .with_winit_window(|w| {
            let monitor = w.current_monitor()?;
            let origin = monitor.position();
            let size = monitor.size();
            let width = (WIDTH as f64 * monitor.scale_factor()) as i32;
            Some(PhysicalPosition::new(
                origin.x + size.width as i32 - width - MARGIN,
                origin.y + MARGIN * 3,
            ))
        })
        .flatten()
}

/// No Windows, a janelinha não ganha botão próprio na barra de tarefas.
fn hide_from_taskbar(overlay: &OverlayWindow) {
    #[cfg(windows)]
    {
        use slint::winit_030::winit::platform::windows::WindowExtWindows;
        overlay
            .window()
            .with_winit_window(|w| w.set_skip_taskbar(true));
    }
    #[cfg(not(windows))]
    let _ = overlay;
}

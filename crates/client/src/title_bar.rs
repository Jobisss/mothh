//! Barra da janela do próprio app (a janela não tem a moldura do sistema).
//!
//! O Slint desenha a barra (`TitleBar` em `app.slint`); aqui ficam as ações
//! que só a janela do winit sabe fazer: arrastar (o Windows continua
//! encaixando a janela nas bordas da tela), mudar o tamanho pelas bordas,
//! minimizar, maximizar e fechar.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::window::ResizeDirection;

use crate::AppWindow;

/// Dois cliques na barra dentro deste tempo maximizam (ou restauram).
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

pub fn connect(ui: &AppWindow) {
    // Sombra em volta da janela sem moldura, como as janelas normais.
    #[cfg(windows)]
    ui.window().with_winit_window(|w| {
        use slint::winit_030::winit::platform::windows::WindowExtWindows;
        w.set_undecorated_shadow(true);
    });

    // O arrastar do sistema segura o mouse até soltar, e o Slint não vê o
    // segundo clique como clique duplo; então o clique duplo é contado aqui.
    let last_press: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
    let ui_handle = ui.as_weak();
    ui.on_window_drag(move || {
        let Some(ui) = ui_handle.upgrade() else {
            return;
        };
        let now = Instant::now();
        if last_press
            .get()
            .is_some_and(|t| now.duration_since(t) < DOUBLE_CLICK)
        {
            last_press.set(None);
            toggle_maximized(&ui);
            return;
        }
        last_press.set(Some(now));
        ui.window().with_winit_window(|w| {
            let _ = w.drag_window();
        });
    });

    let ui_handle = ui.as_weak();
    ui.on_window_resize(move |dir| {
        let Some(ui) = ui_handle.upgrade() else {
            return;
        };
        let dir = match dir.as_str() {
            "n" => ResizeDirection::North,
            "s" => ResizeDirection::South,
            "e" => ResizeDirection::East,
            "w" => ResizeDirection::West,
            "ne" => ResizeDirection::NorthEast,
            "nw" => ResizeDirection::NorthWest,
            "se" => ResizeDirection::SouthEast,
            _ => ResizeDirection::SouthWest,
        };
        ui.window().with_winit_window(|w| {
            let _ = w.drag_resize_window(dir);
        });
    });

    let ui_handle = ui.as_weak();
    ui.on_window_minimize(move || {
        if let Some(ui) = ui_handle.upgrade() {
            ui.window().set_minimized(true);
        }
    });

    let ui_handle = ui.as_weak();
    ui.on_window_toggle_maximize(move || {
        if let Some(ui) = ui_handle.upgrade() {
            toggle_maximized(&ui);
        }
    });

    let ui_handle = ui.as_weak();
    ui.on_window_close(move || {
        if let Some(ui) = ui_handle.upgrade() {
            // O mesmo caminho do "X" do sistema.
            ui.window()
                .dispatch_event(slint::platform::WindowEvent::CloseRequested);
        }
    });

    let ui_handle = ui.as_weak();
    ui.on_window_changed(move || {
        if let Some(ui) = ui_handle.upgrade() {
            let maximized = ui.window().is_maximized();
            if ui.get_window_maximized() != maximized {
                ui.set_window_maximized(maximized);
            }
        }
    });
}

fn toggle_maximized(ui: &AppWindow) {
    let maximized = !ui.window().is_maximized();
    ui.window().set_maximized(maximized);
    ui.set_window_maximized(maximized);
}

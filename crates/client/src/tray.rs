//! Ícone da mariposa na bandeja do Windows (perto do relógio).
//!
//! Fechar a janela no "X" não sai do app: ela some e o mothh continua
//! rodando (mensagens, chamada), como o Discord. Clicar no ícone abre a
//! janela de novo; o menu do botão direito tem "abrir o mothh" e "sair".
//! O módulo "ficar na bandeja ao fechar" desligado volta ao jeito antigo:
//! o "X" fecha o app. Fora do Windows não há bandeja e o "X" sempre fecha.

use slint::ComponentHandle;
#[cfg(windows)]
use slint::winit_030::WinitWindowAccessor;

use crate::AppWindow;
use crate::modules::{Module, Modules};

/// Guarda o ícone vivo: se for solto, some da bandeja.
pub struct Tray {
    #[cfg(windows)]
    _icon: Option<tray_icon::TrayIcon>,
}

pub fn connect(ui: &AppWindow, modules: &Modules) -> Tray {
    #[cfg(windows)]
    let icon = windows_only::create(ui);
    #[cfg(windows)]
    let has_tray = icon.is_some();
    #[cfg(not(windows))]
    let has_tray = false;

    let modules = modules.clone();
    ui.window().on_close_requested(move || {
        if has_tray && modules.is_on(Module::Tray) {
            eprintln!("bandeja: janela escondida, o mothh continua rodando");
            return slint::CloseRequestResponse::HideWindow;
        }
        // O laço roda até alguém pedir para sair (`run_event_loop_until_quit`
        // em main.rs), então fechar a última janela não basta.
        let _ = slint::quit_event_loop();
        slint::CloseRequestResponse::HideWindow
    });

    Tray {
        #[cfg(windows)]
        _icon: icon,
    }
}

/// Mostra a janela de novo, na frente das outras.
#[cfg(windows)]
fn bring_back(ui: &AppWindow) {
    let _ = ui.show();
    ui.window().set_minimized(false);
    ui.window().with_winit_window(|w| w.focus_window());
}

#[cfg(windows)]
mod windows_only {
    use std::cell::RefCell;

    use slint::ComponentHandle;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

    use crate::AppWindow;

    const OPEN: &str = "abrir";
    const QUIT: &str = "sair";

    thread_local! {
        // Os eventos do ícone chegam numa função que precisa ser `Send`; ela
        // só pede para rodar na thread da janela, que pega a janela daqui.
        static WINDOW: RefCell<Option<slint::Weak<AppWindow>>> = const { RefCell::new(None) };
    }

    pub fn create(ui: &AppWindow) -> Option<TrayIcon> {
        WINDOW.with(|w| *w.borrow_mut() = Some(ui.as_weak()));

        let menu = Menu::new();
        let open = MenuItem::with_id(OPEN, "abrir o mothh", true, None);
        let quit = MenuItem::with_id(QUIT, "sair", true, None);
        let built = menu
            .append(&open)
            .and_then(|_| menu.append(&PredefinedMenuItem::separator()))
            .and_then(|_| menu.append(&quit));
        if let Err(err) = built {
            eprintln!("bandeja: não deu para montar o menu: {err}");
            return None;
        }

        let icon = match icon() {
            Some(icon) => icon,
            None => {
                eprintln!("bandeja: não deu para ler o ícone");
                return None;
            }
        };
        let tray = TrayIconBuilder::new()
            .with_icon(icon)
            .with_tooltip("mothh")
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build();
        let tray = match tray {
            Ok(tray) => tray,
            Err(err) => {
                eprintln!("bandeja: não deu para criar o ícone: {err}");
                return None;
            }
        };

        MenuEvent::set_event_handler(Some(|event: MenuEvent| {
            let id = event.id().0.clone();
            let _ = slint::invoke_from_event_loop(move || match id.as_str() {
                OPEN => with_window(super::bring_back),
                QUIT => {
                    let _ = slint::quit_event_loop();
                }
                _ => {}
            });
        }));
        TrayIconEvent::set_event_handler(Some(|event: TrayIconEvent| {
            let open = matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            );
            if open {
                let _ = slint::invoke_from_event_loop(|| with_window(super::bring_back));
            }
        }));
        Some(tray)
    }

    fn with_window(f: impl FnOnce(&AppWindow)) {
        let ui = WINDOW.with(|w| w.borrow().as_ref().and_then(|weak| weak.upgrade()));
        if let Some(ui) = ui {
            f(&ui);
        }
    }

    /// A mariposa do app, em 32×32 (o tamanho da bandeja com folga).
    fn icon() -> Option<tray_icon::Icon> {
        let png = include_bytes!("../ui/icons/mothh.png");
        let image = image::load_from_memory(png).ok()?.into_rgba8();
        let image = image::imageops::resize(&image, 32, 32, image::imageops::FilterType::Triangle);
        let (w, h) = image.dimensions();
        tray_icon::Icon::from_rgba(image.into_raw(), w, h).ok()
    }
}

//! Preview visual explícito de desenvolvimento: sem login, rede ou credenciais.
use crate::{AppWindow, ConversationRow, MessageRow, MusicQueueRow, MusicViewData};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{path::PathBuf, time::Duration};

pub fn run(path: PathBuf) -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    let _turntable = crate::turntable::connect(&ui);
    let compact = std::env::var("MOTHH_MUSIC_PREVIEW_SIZE").as_deref() == Ok("compact");
    ui.window().set_size(slint::LogicalSize::new(
        if compact { 760.0 } else { 1440.0 },
        if compact { 600.0 } else { 900.0 },
    ));
    ui.set_splash_open(false);
    ui.set_logged_in(true);
    ui.set_connecting(false);
    ui.set_user_name("jobiss".into());
    ui.set_user_initial("J".into());
    ui.set_status("online".into());
    ui.set_open_title("Os Amigos".into());
    ui.set_open_is_group(true);
    ui.set_can_call(true);
    ui.set_conversations(ModelRc::new(VecModel::from(vec![
        ConversationRow {
            id: "preview".into(),
            name: "Os Amigos".into(),
            initial: "O".into(),
            preview: "som da sala".into(),
            time: "19:49".into(),
            is_group: true,
            selected: true,
            ..Default::default()
        },
        ConversationRow {
            name: "Sala do Grupo".into(),
            initial: "S".into(),
            preview: "a conversa continua por aqui".into(),
            time: "18:30".into(),
            is_group: true,
            ..Default::default()
        },
        ConversationRow {
            name: "Conversas".into(),
            initial: "C".into(),
            preview: "nos vemos mais tarde".into(),
            time: "17:12".into(),
            ..Default::default()
        },
    ])));
    ui.set_messages(ModelRc::new(VecModel::from(vec![
        MessageRow {
            author: "jobiss".into(),
            time: "19:48".into(),
            text: "adicionei uma música à fila".into(),
            ..Default::default()
        },
        MessageRow {
            author: "amigo".into(),
            time: "19:49".into(),
            text: "ouvindo aqui".into(),
            ..Default::default()
        },
    ])));
    let error = std::env::var("MOTHH_MUSIC_PREVIEW_STATE").as_deref() == Ok("error");
    ui.set_music_data(MusicViewData { video_id: if error { "" } else { "preview" }.into(), conversation_id: "preview".into(), panel_open: true, has_track: !error,
        title: if error { "Som da sala" } else { "Nome da Música" }.into(), subtitle: if error { "Uma fila, todo mundo junto." } else { "Artista · canal de origem" }.into(),
        status: "ouvindo junto".into(), detail: "adicionada por jobiss".into(), playing: !error, can_control: true, online: true, listening: !error,
        error: if error { "Música não está habilitada neste servidor. Atualize a configuração do backend para começar." } else { "" }.into(),
        volume: 58, position_ms: 84_000.0, duration_ms: 246_000.0, queued: 3, ..Default::default() });
    ui.set_music_bands(ModelRc::new(VecModel::from(
        (0..32)
            .map(|i| {
                if error {
                    0.0
                } else {
                    (1.0 - ((i as f32 - 17.0) / 17.0).abs()).max(0.0)
                        * (0.5 + (i as f32 * 1.9).sin().abs() * 0.5)
                }
            })
            .collect::<Vec<_>>(),
    )));
    ui.set_music_queue(ModelRc::new(VecModel::from(vec![
        MusicQueueRow {
            id: "preview-1".into(),
            title: "Próxima música".into(),
            detail: "amigo · 3:42".into(),
            can_remove: true,
            ..Default::default()
        },
        MusicQueueRow {
            id: "preview-2".into(),
            title: "Outra música da fila".into(),
            detail: "jobiss · 4:10".into(),
            can_remove: true,
            ..Default::default()
        },
        MusicQueueRow {
            id: "preview-3".into(),
            title: "Pedido indisponível".into(),
            detail: "a fonte não está disponível".into(),
            failed: true,
            can_remove: true,
        },
    ])));
    let weak = ui.as_weak();
    if std::env::var("MOTHH_MUSIC_PREVIEW_STATE").as_deref() == Ok("queue") {
        let weak = ui.as_weak();
        slint::Timer::single_shot(Duration::from_millis(150), move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_music_queue_preview(true);
            }
        });
    }
    slint::Timer::single_shot(Duration::from_millis(700), move || {
        let ui = weak.upgrade().expect("preview fechado antes da captura");
        let frame = ui
            .window()
            .take_snapshot()
            .expect("não foi possível capturar o preview");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("pasta do preview");
        }
        image::save_buffer(
            &path,
            frame.as_bytes(),
            frame.width(),
            frame.height(),
            image::ColorType::Rgba8,
        )
        .expect("não foi possível salvar o preview");
        let _ = slint::quit_event_loop();
    });
    ui.run()
}

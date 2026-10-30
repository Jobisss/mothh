//! Servidor do mothh.

pub mod attachments;
pub mod auth;
pub mod calls;
pub mod groups;
pub mod login_limit;
pub mod state;
mod ws;

use axum::{
    Router,
    extract::{DefaultBodyLimit, State, ws::WebSocketUpgrade},
    response::Response,
    routing::{get, post},
};
use protocol::{ATTACHMENTS_PATH, MAX_FILE_BYTES};

pub use state::AppState;

/// Monta as rotas. Fica separado do `main` para os testes subirem o servidor.
pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/ws", get(ws_upgrade))
        .route(
            ATTACHMENTS_PATH,
            // Passou do limite, o axum responde 413 sem ler o resto.
            post(attachments::upload).layer(DefaultBodyLimit::max(MAX_FILE_BYTES)),
        )
        .route(
            &format!("{ATTACHMENTS_PATH}/{{id}}"),
            get(attachments::download),
        )
        .with_state(state)
}

async fn ws_upgrade(upgrade: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    upgrade.on_upgrade(move |socket| ws::handle_socket(socket, state))
}

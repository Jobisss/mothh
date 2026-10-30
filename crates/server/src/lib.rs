//! Servidor do mothh.

pub mod auth;
pub mod login_limit;
pub mod state;
mod ws;

use axum::{
    Router, extract::State, extract::ws::WebSocketUpgrade, response::Response, routing::get,
};

pub use state::AppState;

/// Monta as rotas. Fica separado do `main` para os testes subirem o servidor.
pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/ws", get(ws_upgrade))
        .with_state(state)
}

async fn ws_upgrade(upgrade: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    upgrade.on_upgrade(move |socket| ws::handle_socket(socket, state))
}

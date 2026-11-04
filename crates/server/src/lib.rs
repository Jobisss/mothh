//! Servidor do mothh.

pub mod attachments;
pub mod auth;
pub mod calls;
pub mod groups;
mod listening;
pub mod login_limit;
pub mod mail;
pub mod mls;
pub mod photos;
pub mod profiles;
pub mod state;
mod ws;

use axum::{
    Router,
    extract::{DefaultBodyLimit, State, ws::WebSocketUpgrade},
    response::Response,
    routing::{get, post},
};
use protocol::{ATTACHMENTS_PATH, MAX_FILE_BYTES, PROFILE_PATH, ProfileImage};

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
        .route(
            &format!("{PROFILE_PATH}/imagens/{{tipo}}"),
            post(profiles::upload_image)
                .delete(profiles::delete_image)
                .layer(DefaultBodyLimit::max(largest_profile_image())),
        )
        .route(
            &format!("{PROFILE_PATH}/{{id}}/imagens/{{tipo}}"),
            get(profiles::download_image),
        )
        .with_state(state)
}

async fn ws_upgrade(upgrade: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    upgrade.on_upgrade(move |socket| ws::handle_socket(socket, state))
}

/// O maior arquivo que alguma imagem do perfil pode ter.
fn largest_profile_image() -> usize {
    ProfileImage::ALL
        .into_iter()
        .map(ProfileImage::max_bytes)
        .max()
        .unwrap_or(0)
}

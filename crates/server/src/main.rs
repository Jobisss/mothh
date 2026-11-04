use std::net::SocketAddr;

use server::{AppState, app, calls::LiveKitConfig, state};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    // Endereço configurável por variável de ambiente; padrão só na máquina local.
    let addr: SocketAddr = std::env::var("MOTHH_ENDERECO")
        .unwrap_or_else(|_| "127.0.0.1:3000".into())
        .parse()
        .expect("MOTHH_ENDERECO deve ser algo como 127.0.0.1:3000");

    // Localmente vem de `.cargo/config.toml`; em produção, do ambiente.
    let database_url = std::env::var("DATABASE_URL")
        .expect("defina DATABASE_URL, por exemplo postgres://mothh:mothh@127.0.0.1:5440/mothh");
    let db = state::connect(&database_url)
        .await
        .expect("não deu para conectar no PostgreSQL (rodou `docker compose up -d`?)");

    // Pasta dos anexos (áudios). Relativa a onde o servidor foi aberto.
    let attachments_dir = std::env::var("MOTHH_ANEXOS").unwrap_or_else(|_| "anexos".into());

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("porta ocupada?");
    tracing::info!("servidor ouvindo em ws://{addr}/ws");
    let mut state = AppState::new(db).with_attachments_dir(attachments_dir);
    match LiveKitConfig::from_env() {
        Some(livekit) => {
            tracing::info!(url = %livekit.url, "chamadas pelo LiveKit");
            state = state.with_livekit(livekit);
        }
        None => tracing::warn!(
            "sem MOTHH_LIVEKIT_URL, MOTHH_LIVEKIT_KEY e MOTHH_LIVEKIT_SECRET: chamadas desligadas"
        ),
    }
    axum::serve(listener, app(state))
        .await
        .expect("o servidor parou com erro");
}

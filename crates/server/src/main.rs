use std::net::SocketAddr;

use server::{AppState, app};
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

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("porta ocupada?");
    tracing::info!("servidor ouvindo em ws://{addr}/ws");
    axum::serve(listener, app(AppState::with_default_group()))
        .await
        .expect("o servidor parou com erro");
}

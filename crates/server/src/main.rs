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

    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://mothh:mothh@127.0.0.1:5432/mothh".into());
    let state = AppState::connect(&database_url)
        .await
        .expect("não deu para abrir o banco; o Postgres está rodando? (docker compose up -d)");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("porta ocupada?");
    tracing::info!("servidor ouvindo em ws://{addr}/ws");
    axum::serve(listener, app(state))
        .await
        .expect("o servidor parou com erro");
}

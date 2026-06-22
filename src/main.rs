use std::sync::Arc;

use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use password_manager_server::config::Config;
use password_manager_server::db;
use password_manager_server::routes;

fn init_logging() {
    tracing_subscriber::registry()
        .with(fmt::layer().json().flatten_event(true))
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    init_logging();

    let config = Config::from_env()?;
    let pool = db::init_pool(&config.database_url, config.db_pool_size)?;
    db::run_migrations(&pool)?;

    let state = Arc::new(routes::AppState {
        pool: Arc::new(std::sync::RwLock::new(pool)),
        config,
    });
    let addr = state.config.bind_addr.clone();
    let app = routes::build_router(state);
    tracing::info!("listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("shutdown signal received, starting graceful shutdown");
}

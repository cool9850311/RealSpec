//! `paygate-demo-merchant` — NOT paygate. The demo shop's own server: it
//! creates orders through paygate's merchant API and receives paygate's
//! result callbacks (`spec/openapi/demo-merchant.yaml`).

mod amount;
mod config;
mod dto;
mod error;
mod gateway;
mod handlers;
mod routes;
mod signature;
mod state;
mod test_support;

use std::sync::Arc;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use config::Config;
use state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::from_env().unwrap_or_else(|err| {
        // Startup fails loudly, naming the variable, before any log
        // formatting machinery is even set up (`spec.md`, "Credentials in
        // this repository": every secret is read from the environment at
        // startup, with no default and no fallback).
        eprintln!("{{\"level\":\"error\",\"message\":\"{err}\"}}");
        std::process::exit(1);
    });

    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            EnvFilter::try_new(&config.log_level).unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let port = config.port;
    let shutdown_grace = config.shutdown_grace;
    let instance_id = config.instance_id.clone();

    tracing::info!(instance_id = %instance_id, port, "paygate-demo-merchant starting");

    let state = Arc::new(AppState::new(config));
    let app = routes::build_router(state).layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(shutdown_grace))
        .await?;

    Ok(())
}

/// Waits for Ctrl+C or SIGTERM, then gives in-flight requests up to
/// `SHUTDOWN_GRACE_SECONDS` before `axum::serve` finishes tearing down —
/// `with_graceful_shutdown` itself is what stops accepting new connections
/// and waits for the in-flight ones; this future only decides WHEN that
/// starts.
async fn shutdown_signal(_grace: std::time::Duration) {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("paygate-demo-merchant shutting down");
}

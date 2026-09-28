//! Graceful shutdown, shared by every long-running role: a `watch` channel
//! flips to `true` the moment SIGTERM or Ctrl+C arrives, and both the admin
//! HTTP server (via `axum::serve`'s own `with_graceful_shutdown`) and each
//! role's poll loop watch the same receiver, so one signal stops both without
//! either one guessing at the other's state.
//!
//! `SHUTDOWN_GRACE_SECONDS` (CommonConfig) is enforced by wrapping the wait
//! for the admin server's own shutdown in [`tokio::time::timeout`] in
//! `main.rs`, since `axum::serve`'s graceful shutdown has no deadline of its
//! own — it waits for in-flight requests to finish, however long that takes.

use tokio::sync::watch;

/// Spawns the task that waits for a termination signal, and returns a
/// receiver that flips from `false` to `true` exactly once, the moment one
/// arrives.
pub fn signal() -> watch::Receiver<bool> {
    let (tx, rx) = watch::channel(false);
    tokio::spawn(async move {
        wait_for_signal().await;
        // The only way `send` fails is if every receiver was already
        // dropped, which only happens once the process is already exiting —
        // nothing left to notify.
        let _ = tx.send(true);
    });
    rx
}

async fn wait_for_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
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
}

/// True once the signal has fired. Read at the top of every loop iteration so
/// a role never starts one more unit of work after shutdown was requested.
pub fn requested(rx: &watch::Receiver<bool>) -> bool {
    *rx.borrow()
}

/// Waits either for the poll interval to elapse or for the shutdown signal to
/// fire, whichever comes first — an idle role sleeps between polls without
/// ever taking longer than necessary to notice shutdown.
pub async fn sleep_or_shutdown(duration: std::time::Duration, rx: &mut watch::Receiver<bool>) {
    if duration.is_zero() {
        return;
    }
    tokio::select! {
        _ = tokio::time::sleep(duration) => {}
        _ = rx.changed() => {}
    }
}

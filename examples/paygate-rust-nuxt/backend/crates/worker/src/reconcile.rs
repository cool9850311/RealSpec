//! The reconciler role: `crates/worker/src/admin.rs`'s `POST /run` runs
//! [`paygate_infra::reconciler::run_pass`] (the query job) and then
//! [`paygate_infra::reconciler::resend_pending_refunds`] (the refund job) —
//! `spec.md`, "Reconciling what never came back": "It has a second job for the
//! same reason" — and answers the combined [`paygate_infra::reconciler::PassSummary`]
//! shape `run_reconcile_pass` builds.
//!
//! When `RECONCILE_POLL_INTERVAL_MS` is not zero, [`poll_loop`] also runs the
//! same pair on a timer; the suites pin it to zero (`spec.md`, "Environment
//! variables") precisely so that `the reconciler runs` (an operator/harness
//! call to `POST /run`) is the only thing that ever makes this role ask a
//! provider.

use std::time::Duration;

use tokio::sync::watch;

use paygate_infra::db::Db;
use paygate_infra::reconciler::{self, PassSummary};
use paygate_infra::Result;

/// Everything one reconcile pass needs, held for the process lifetime and
/// shared between the admin server's `/run` handler and the optional
/// background poller — a plain `Clone` bag rather than a trait object, since
/// there is exactly one implementation.
#[derive(Clone)]
pub struct ReconcileDeps {
    pub db: Db,
    pub http: reqwest::Client,
    pub provider_base_url: String,
    pub after: Duration,
    pub retry: Duration,
    pub batch_size: i64,
    /// Not one of `ReconcileConfig`'s own fields (`spec.md`'s "Environment
    /// variables" names no timeout for the reconciler's own outbound calls,
    /// only `PSP_TIMEOUT_MS` for `api`'s synchronous refund path) — reported
    /// as a probable gap in the final report. `main.rs` reads its own
    /// optional `RECONCILE_HTTP_TIMEOUT_MS` (default 3000, `api`'s own
    /// `PSP_TIMEOUT_MS` default) so the query and resend jobs never hang
    /// forever on a provider that stops answering.
    pub http_timeout: Duration,
}

/// Runs the query job, then the refund-resend job, and folds the second job's
/// count into the first job's summary — `crates/worker/src/admin.rs`'s
/// `POST /run` shape,
/// `{"queried","settled","abandoned","refunds_resent","duplicate_refunds_queued"}`,
/// in one call. The last field is additive: a duplicate the query job itself
/// found and queued a refund for (`paygate_infra::reconciler::PassSummary`'s
/// own doc comment) — without it, the summary would report every OTHER kind
/// of work this pass did except that one.
pub async fn run_pass_and_resend(deps: &ReconcileDeps) -> Result<PassSummary> {
    let mut summary = reconciler::run_pass(
        &deps.db,
        &deps.http,
        &deps.provider_base_url,
        deps.after,
        deps.retry,
        deps.batch_size,
        deps.http_timeout,
    )
    .await?;

    let refunds_resent = reconciler::resend_pending_refunds(
        &deps.db,
        &deps.http,
        &deps.provider_base_url,
        deps.batch_size,
        deps.http_timeout,
    )
    .await?;
    summary.refunds_resent = refunds_resent;

    Ok(summary)
}

/// The background poller used only when `RECONCILE_POLL_INTERVAL_MS != 0`
/// (production's default is 1000; every suite pins it to 0, per `spec.md`'s
/// "Environment variables", and never spawns this loop at all — see
/// `main.rs`).
pub async fn poll_loop(
    deps: ReconcileDeps,
    interval: Duration,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        if crate::shutdown::requested(&shutdown) {
            return;
        }
        if let Err(err) = run_pass_and_resend(&deps).await {
            tracing::error!(error = %err, "reconcile pass failed");
        }
        crate::shutdown::sleep_or_shutdown(interval, &mut shutdown).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pass_summary_serialises_into_the_documented_run_response_shape() {
        let summary = PassSummary {
            queried: 2,
            settled: 1,
            abandoned: 1,
            refunds_resent: 3,
            duplicate_refunds_queued: 1,
        };
        let json = serde_json::json!({
            "queried": summary.queried,
            "settled": summary.settled,
            "abandoned": summary.abandoned,
            "refunds_resent": summary.refunds_resent,
            "duplicate_refunds_queued": summary.duplicate_refunds_queued,
        });
        assert_eq!(json["queried"], 2);
        assert_eq!(json["settled"], 1);
        assert_eq!(json["abandoned"], 1);
        assert_eq!(json["refunds_resent"], 3);
        assert_eq!(json["duplicate_refunds_queued"], 1);
    }
}

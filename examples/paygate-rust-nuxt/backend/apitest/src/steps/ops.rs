//! API: Asking the provider instead of guessing (the reconciler's own run),
//! and API: Infrastructure Failure.

use cucumber::{given, then, when};

use crate::world::World;

/// `the reconciler runs` — runs one pass to completion on every reconciler
/// instance at once; `SKIP LOCKED` means only one of them ever claims a given
/// row (`scaling.feature`, "Two reconcilers racing").
#[given(regex = r"^the reconciler runs$")]
#[when(regex = r"^the reconciler runs$")]
#[then(regex = r"^the reconciler runs$")]
async fn reconciler_runs(world: &mut World) {
    world
        .stack()
        .run_reconciler()
        .await
        .unwrap_or_else(|e| panic!("the reconciler run failed: {e}"));
}

/// `service "<name>" is (stopped|started)`.
#[given(
    regex = r#"^service "(api|relay|ingester|notifier|redis|clickhouse)" is (stopped|started)$"#
)]
#[when(
    regex = r#"^service "(api|relay|ingester|notifier|redis|clickhouse)" is (stopped|started)$"#
)]
#[then(
    regex = r#"^service "(api|relay|ingester|notifier|redis|clickhouse)" is (stopped|started)$"#
)]
async fn service_state(world: &mut World, service: String, state: String) {
    let want_running = state == "started";
    world
        .stack_mut()
        .set_service_state(&service, want_running)
        .await
        .unwrap_or_else(|e| panic!("service {service:?} is {state}: {e}"));
}

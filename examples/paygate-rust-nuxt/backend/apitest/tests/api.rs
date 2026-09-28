//! The cucumber entry point for `spec/bdd/api/*.feature` (`harness = false`
//! in `Cargo.toml`, so this file owns `main`).
//!
//! What happens once, for the whole run:
//!   - the service image is built (`docker build`, the one direct Docker call
//!     this harness makes);
//!   - a report log is opened at
//!     `$REPO_ROOT/reports/paygate-api-<timestamp>.txt` and teed with stdout.
//!
//! What happens once per scenario, in the `before`/`after` hooks:
//!   - the process-wide serial lock is acquired (write half for `@serial`,
//!     read half otherwise — every `@stack:scaled` scenario carries
//!     `@serial`, per `spec.md`, "Isolation");
//!   - a full stack of containers is started (`@stack:scaled` selects the
//!     scaled profile) and torn down.

use std::{fs, io, path::PathBuf, sync::Arc};

use cucumber::{
    gherkin,
    writer::{self, Coloring, Verbosity},
    World as _, WriterExt as _,
};
use futures::FutureExt;
use paygate_apitest::{stack, world::SerialGuard, World};
use tokio::sync::RwLock;

/// The process-wide lock `@serial` scenarios take for writing and everything
/// else takes for reading (`spec.md`, "Isolation": "`@serial` runs alone;
/// every `@stack:scaled` scenario carries it").
static SERIAL_LOCK: std::sync::OnceLock<Arc<RwLock<()>>> = std::sync::OnceLock::new();

fn serial_lock() -> Arc<RwLock<()>> {
    SERIAL_LOCK
        .get_or_init(|| Arc::new(RwLock::new(())))
        .clone()
}

fn has_tag(
    feature: &gherkin::Feature,
    rule: Option<&gherkin::Rule>,
    scenario: &gherkin::Scenario,
    tag: &str,
) -> bool {
    feature.tags.iter().any(|t| t == tag)
        || rule.is_some_and(|r| r.tags.iter().any(|t| t == tag))
        || scenario.tags.iter().any(|t| t == tag)
}

fn repo_root() -> PathBuf {
    // backend/apitest -> backend -> paygate-rust-nuxt (spec_root) -> examples -> RealSpec
    paygate_apitest::registry::spec_root().join("../..")
}

/// Every byte the writer produces, to both the terminal and the run log.
///
/// A failed run has to be diagnosable from the log alone — the step, the
/// message, the body — so the file gets exactly what stdout got rather than a
/// summary of it.
struct TeeStream {
    stdout: io::Stdout,
    file: fs::File,
}

impl io::Write for TeeStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // Both halves get the whole buffer; the count returned is the contract
        // `write` has with its caller, not the sum of two writes.
        self.stdout.write_all(buf)?;
        self.file.write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stdout.flush()?;
        self.file.flush()
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    // Set before any Docker call, because testcontainers reads its configuration
    // once and caches it.
    //
    // `remove` is the library's default and this harness needs it, for one
    // narrow but expensive reason: the runner wraps "create, start, wait for
    // ready" in `tokio::time::timeout`
    // (`testcontainers/src/runners/async_runner.rs`), and a timeout CANCELS
    // that future after `start_container` has already run. The half-built
    // `ContainerAsync` is dropped mid-flight, and its handle never reaches this
    // crate — so the only thing that can still remove that container is its own
    // `Drop`. Under `keep` that `Drop` is `Command::Keep => {}` and the
    // container is left running forever. It compounds: one leaked stack eats
    // the Docker VM's memory, so the next scenario is likelier to time out and
    // leak too. One run turned a single Kafka `SIGSEGV` into eight failed
    // scenarios and nine abandoned containers, with not one "could not remove"
    // printed, because nothing was ever asked to remove them.
    //
    // What `keep` was protecting against is real as well: this suite hung twice
    // in `Drop`, once in `ContainerAsync::drop` and once in `Network::drop`,
    // because testcontainers-rs 0.25 has no Ryuk — no reaper in the crate at
    // all — and `Drop` reaching an async Docker call goes through
    // `async_drop`'s `block_in_place`. Both hangs are addressed by their own
    // causes rather than by disarming `Drop` wholesale:
    //
    //   * the container hang, by `Stack::shutdown` removing every container
    //     explicitly with an awaited `rm()`, which marks each one dropped so
    //     its `Drop` has nothing left to do. `Drop` now fires only for a
    //     container this crate never got a handle to — the leak above — and it
    //     fires INSIDE the runtime, where it is just another await;
    //   * the network hang, by `stack::ensure_run_network`, below: the run's
    //     shared network is created before any container, so
    //     `Network::new` finds it already there and returns `None` — the
    //     library never owns it and its `Drop` never runs for it. Left to
    //     create the network itself, the library owns it and removes it the
    //     moment the last container of a scenario goes, re-creating it for the
    //     next one: 174 create/destroy cycles per run, each one a Docker
    //     address-pool allocation this suite has already been bitten by.
    std::env::set_var("TESTCONTAINERS_COMMAND", "remove");

    // A self-check that needs neither Docker nor the containers it builds:
    // `cargo test --test api -- --help` must at least link and run, without
    // kicking off a `docker build` or a scenario.
    if std::env::args().any(|a| a == "--help" || a == "-h") {
        println!(
            "paygate-apitest: runs spec/bdd/api/*.feature with cucumber-rs.\n\n\
             Env vars:\n  \
             PAYGATE_TEST_PATHS        feature file or directory to run (default: spec/bdd/api)\n  \
             PAYGATE_TEST_CONCURRENCY  concurrent scenarios (default: num_cpus/2)\n  \
             PAYGATE_FAIL_FAST         stop after the first failing scenario when set to 1\n\n\
             Requires Docker: builds paygate:apitest from backend/Dockerfile once, then starts\n\
             one stack of containers per scenario via testcontainers."
        );
        return;
    }

    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    stack::ensure_image_built().await;
    stack::ensure_run_network().await;

    let paths: Vec<String> = match std::env::var("PAYGATE_TEST_PATHS") {
        Ok(p) => vec![p],
        Err(_) => vec![paygate_apitest::registry::api_features_dir()
            .display()
            .to_string()],
    };

    let concurrency: usize = std::env::var("PAYGATE_TEST_CONCURRENCY")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|n: &usize| *n >= 1)
        .unwrap_or_else(|| (num_cpus::get() / 2).max(1));

    let fail_fast = std::env::var("PAYGATE_FAIL_FAST")
        .map(|v| v == "1")
        .unwrap_or(false);

    let reports_dir = repo_root().join("reports");
    fs::create_dir_all(&reports_dir).expect("creating the reports directory");
    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let report_path = reports_dir.join(format!("paygate-api-{timestamp}.txt"));
    let report_file = fs::File::create(&report_path).expect("creating the report log file");
    println!("[apitest] full run log: {}", report_path.display());

    // One writer over a tee'd STREAM, not two writers tee'd together.
    //
    // `writer::Basic::tee(writer::Basic)` looks like the obvious way to get the
    // run onto stdout and into a file at once, and it panics before the first
    // scenario: each `Basic` contributes a `--verbose` flag to the composed
    // cucumber CLI, and clap refuses two arguments with one name
    // ("Argument names must be unique, but 'verbose' is in use by more than one
    // argument or group"). Teeing the bytes instead gives one writer, one CLI,
    // and a log file that is byte-for-byte what a person watching the terminal
    // saw — which is the point of keeping one.
    //
    // Colouring is off for both halves rather than `Auto`: the file is an
    // artefact somebody reads later, and ANSI escapes in it are noise.
    let writer = writer::Basic::new(
        TeeStream {
            stdout: io::stdout(),
            file: report_file,
        },
        Coloring::Never,
        Verbosity::ShowWorldAndDocString,
    )
    .summarized()
    .fail_on_skipped();

    let mut runner = World::cucumber()
        .with_writer(writer)
        .max_concurrent_scenarios(concurrency)
        .before(move |feature, rule, scenario, world: &mut World| {
            async move {
                let serial = has_tag(feature, rule, scenario, "serial")
                    || has_tag(feature, rule, scenario, "stack:scaled");
                world.serial_guard = if serial {
                    SerialGuard::Write(serial_lock().write_owned().await)
                } else {
                    SerialGuard::Read(serial_lock().read_owned().await)
                };

                let scaled = has_tag(feature, rule, scenario, "stack:scaled");
                // No outer `tokio::time::timeout` here on purpose. `Stack::start`
                // already enforces `stack::STACK_TOTAL_TIMEOUT` internally — see
                // that constant's doc comment — precisely so that a scenario whose
                // stack cannot be built in time fails through an ordinary `Err`
                // that has already cleaned up after itself, rather than through an
                // outer cancellation that would drop (and leak) whatever `start`
                // had already built. Wrapping this call in a timeout again would
                // reintroduce exactly that leak: a run that hit it twice left two
                // whole stacks — 22 containers — behind.
                match stack::Stack::start(scaled).await {
                    Ok(stack) => world.stack = Some(stack),
                    Err(e) => panic!("starting the scenario's stack failed: {e:#}"),
                }
            }
            .boxed_local()
        })
        .after(
            |_feature, _rule, _scenario, _finished, world: Option<&mut World>| {
                async move {
                    if let Some(world) = world {
                        // `shutdown().await`, never `drop`: see Stack::shutdown.
                        // Dropping it here parks a worker thread for ever and the
                        // run stops without reporting anything.
                        if let Some(stack) = world.stack.take() {
                            stack.shutdown().await;
                        }
                        world.serial_guard = SerialGuard::None;
                    }
                }
                .boxed_local()
            },
        );

    if fail_fast {
        runner = runner.fail_fast();
    }

    runner
        .run_and_exit(paths.into_iter().next().expect("at least one feature path"))
        .await;
}

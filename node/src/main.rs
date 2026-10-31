#![forbid(unsafe_code)]
//! The node entry point (port of `Main.scala` + `NodeMain.startNode`).

// Heap profiling, for the memory investigations (#117). `dhat::Alloc` implements `GlobalAlloc` inside
// the dhat crate, so installing it needs no `unsafe` here and the crate-level `forbid` still holds.
// It is registered in this *binary* only, never in the library, so a `--all-features` test build
// compiles the dependency without putting a profiling allocator under the test harness.
#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

// jemalloc on Linux, with the background purge thread on. **This is the bound on the defect** (#117):
// glibc gives each thread an arena, keeps that arena's high-water mark, and returns free pages only when
// they happen to sit at the heap top — so a node under fork load reaches its cgroup ceiling while its
// live heap stays small. Measured: the ceiling is glibc's own heap (its accounting explains the cgroup's
// `anon` at 92–164 %, mean 101 %, over 760 samples), with roughly half of that free-but-retained.
// jemalloc purges dirty pages on a decay timer *independent of allocation activity*, so the footprint
// tracks live data instead. The profiling allocator wins when both are selected, because a heap profile
// of the real allocator is the point of that feature.
#[cfg(all(target_os = "linux", not(feature = "dhat-heap")))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

// The purge settings are **not** here: they are `JEMALLOC_SYS_WITH_MALLOC_CONF` in `.cargo/config.toml`,
// which `tikv-jemalloc-sys` reads at build time and compiles into jemalloc's defaults. The usual in-code
// route — a `#[export_name = "_rjem_malloc_conf"]` static — is unavailable in this crate graph, because
// *declaring a static with `export_name`* is itself what `#![forbid(unsafe_code)]` refuses. Baking it in
// is also the stronger form: it cannot be forgotten by a deployment that runs the binary outside this
// repository's image, and the settings are that `background_thread:true` (the purge must not depend on
// allocation activity — an idle worker never allocates again) with 5 s decay on dirty and muzzy pages.

use rchain_shared::chan;
use std::sync::Arc;

use clap::Parser;

use rchain_node::configuration::commandline::options::{Commands, Options};
use rchain_node::configuration::configuration::Configuration;
use rchain_node::runtime::{node_environment, node_runtime, run_cli};
use rchain_shared::log::{Log, LogSource, StderrLog};

fn main() {
    // The profiler writes its dump when it drops, so it has to outlive the whole of `main` — and the
    // node has to *return* from `main` for the file to exist at all, which is what AUDIT C144's
    // SIGTERM handler buys: before it, `docker stop` ended at exit 137 and the dump was never
    // written. `--profile docker` puts the data dir at /var/lib/rnode, where the dump is retrieved
    // from after a clean stop.
    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::builder()
        .file_name("/var/lib/rnode/dhat-heap.json")
        .build();

    // Parse options before building the tokio runtime: the thread-pool size must be known up
    // front (the worker count is fixed at runtime construction).
    let options = Options::parse();

    // The number of tokio worker threads (the scheduler's CPU parallelism): the CLI value, else
    // the machine's hardware parallelism. Validated ≥ 1 so a mistyped flag fails fast with a
    // legible error instead of a tokio panic.
    let worker_threads = match &options.subcommand {
        Commands::Run(run) => match run.thread_pool_size {
            Some(n) if n < 1 => {
                eprintln!("Invalid --thread-pool-size {n}: must be at least 1");
                std::process::exit(1);
            }
            Some(n) => n as usize,
            None => default_worker_threads(),
        },
        _ => default_worker_threads(),
    };

    // The log level (AUDIT C145) is resolved here for the same reason the worker count is: it is a
    // property of the *process*, and a typo has to fail before the node starts rather than leave the
    // operator with logs quieter than they asked for.
    let log_level = match &options.subcommand {
        Commands::Run(run) => match &run.log_level {
            Some(value) => match rchain_shared::log::Level::parse(value) {
                Ok(level) => level,
                Err(e) => {
                    eprintln!("Invalid --log-level: {e}");
                    std::process::exit(1);
                }
            },
            None => rchain_shared::log::Level::Info,
        },
        _ => rchain_shared::log::Level::Info,
    };

    // The rholang parser + reducer recurse through deeply-nested contract sources (the genesis
    // blessed terms in particular); a 2 MiB worker stack overflows. Give the runtime a larger
    // per-worker stack (the JVM node runs these paths on a much larger native stack).
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .thread_stack_size(32 * 1024 * 1024)
        .worker_threads(worker_threads)
        .max_blocking_threads(blocking_thread_cap(worker_threads))
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("Failed to build the tokio runtime: {e}");
            std::process::exit(1);
        }
    };
    runtime.block_on(async_main(options, log_level));
}

/// A conservative bound on the blocking pool. Each thread reserves a 32 MiB stack; tokio's default
/// is 512, which is a large worst case on a small host. This is a bound, not a measured optimum:
/// LMDB work is offloaded through `spawn_blocking`, so the pool must not be tiny.
fn blocking_thread_cap(workers: usize) -> usize {
    workers.saturating_mul(2).clamp(16, 64)
}

/// The machine's hardware parallelism (the tokio default), falling back to 1.
fn default_worker_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The worker count defaults to the machine's parallelism, and **never zero**: a tokio runtime
    /// built with zero worker threads cannot make progress, and the fallback exists for the
    /// platforms where `available_parallelism` fails. The other half of the rule — an explicit
    /// `--thread-pool-size 0` is refused with a message and `exit(1)` — lives in `main` itself and
    /// cannot be asserted from inside the process; it is what the flag's parse-time validation
    /// covers.
    #[test]
    fn the_blocking_thread_cap_is_bounded() {
        assert!(blocking_thread_cap(0) >= 16);
        assert!(blocking_thread_cap(1) >= 16);
        assert_eq!(blocking_thread_cap(24), 48);
        assert!(blocking_thread_cap(1_024) <= 64);
    }

    #[test]
    fn the_default_worker_count_is_at_least_one() {
        let threads = default_worker_threads();
        assert!(
            threads >= 1,
            "a runtime needs at least one worker: {threads}"
        );
        assert!(
            threads <= 1024,
            "a sane machine has fewer than 1024 cores: {threads}"
        );

        // It is the hardware parallelism when the platform reports it…
        if let Ok(available) = std::thread::available_parallelism() {
            assert_eq!(threads, available.get());
        }
        // …and stable across calls, so two builds of the runtime agree.
        assert_eq!(threads, default_worker_threads());
    }
}

async fn async_main(options: Options, log_level: rchain_shared::log::Level) {
    // The one logger the node installs, at the operator's level (AUDIT C145). `StderrLog` writes the
    // wall clock on every line, so this is also what makes a cross-node timeline reconstructable.
    let log: Arc<StderrLog> = Arc::new(StderrLog::new(log_level));

    // Execute a thin-client CLI command (port of `Main.main`'s `runCLI` branch).
    if !matches!(options.subcommand, Commands::Run(_)) {
        if let Err(errors) = run_cli(&options).await {
            for error in &errors {
                println!("{error}");
            }
            std::process::exit(1);
        }
        return;
    }

    let (node_conf, _profile, _config_file) = match Configuration::build(&options) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Configuration error: {e}");
            std::process::exit(1);
        }
    };

    let id = match node_environment::create(&node_conf) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("Initialization error: {e}");
            std::process::exit(1);
        }
    };

    let program = match node_runtime::setup_node_program(&node_conf, &id, log.clone()).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Setup error: {e}");
            std::process::exit(1);
        }
    };

    // The stop path (AUDIT C144). Before this, nothing installed a signal handler: the only stop was
    // a `SIGKILL` after the orchestrator's grace period, and **inside the container the node is PID 1,
    // where the kernel does not apply the default terminate disposition** — so `docker stop`,
    // systemd's `ExecStop` and a pod eviction all had their `SIGTERM` delivered and discarded.
    //
    // The listener handles are the sixth thing that can end the wait, and they must keep being
    // reported (AUDIT C142), so the serve future is pinned and raced rather than moved into a
    // `select!` arm: on a signal it still has to be *awaited* to completion, or dropping it would
    // detach the listeners and turn a drain back into a truncation.
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let serving = program.serve(stop_rx);
    tokio::pin!(serving);
    let stopping = tokio::select! {
        result = &mut serving => Stopping::Served(result),
        signal = rchain_node::runtime::shutdown::shutdown_signal() => Stopping::Signal(signal),
        reason = rchain_node::runtime::shutdown::fatal_requested() => Stopping::Fatal(reason),
    };
    match stopping {
        Stopping::Served(Err(e)) => {
            eprintln!("Server error: {e}");
            std::process::exit(1);
        }
        Stopping::Served(Ok(())) => {}
        Stopping::Fatal(reason) => {
            // **A concluded stop stops the process** (§106's owed D3): **no drain**. The fault means a
            // store has moved under a view that was not rebuilt — the state the reset's own comment
            // says the node "must not keep serving" — and draining the listeners would serve requests
            // out of exactly that state. `stderr` is unbuffered, so the reason reaches the supervisor
            // (which will restart the node, and the boot guard repairs the interrupted work) before
            // the exit.
            eprintln!("Fatal: {reason}");
            std::process::exit(1);
        }
        Stopping::Signal(signal) => {
            log.info(
                LogSource::new("coop.rchain.node.Main"),
                &format!(
                    "Received {signal}: stopping the listeners and draining (AUDIT C144, bounded at \
                     {}s)",
                    rchain_node::runtime::shutdown::SHUTDOWN_DRAIN_TIMEOUT.as_secs()
                ),
            );
            chan::watch_send(&stop_tx, true);
            if let Err(e) = serving.await {
                eprintln!("Server error while shutting down: {e}");
                std::process::exit(1);
            }
            log.info(
                LogSource::new("coop.rchain.node.Main"),
                "Shutdown complete.",
            );
        }
    }
}

/// Why the wait for the listeners ended.
enum Stopping {
    /// `NodeProgram::serve` returned: a listener stopped, or failed. Its result is the error to
    /// report — a clean `Ok` here means an accept loop returned, which is not a normal stop.
    Served(Result<(), String>),
    /// The operator asked the node to stop, naming the signal so the log can say which one arrived.
    Signal(&'static str),
    /// A task concluded the node cannot serve on — a failed reset, or the LFS sync terminal (both say
    /// so in their own words today, and neither stopped anything until this arm existed). The process
    /// stops **without draining**: the reason is printed and the exit code is non-zero.
    Fatal(String),
}

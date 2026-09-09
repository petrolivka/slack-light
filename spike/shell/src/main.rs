//! M0-GUI spike A: a relm4 shell over the real engine, against the mock.
//!
//! Throwaway by design — this is the binary that answers the questions in
//! `docs/M0-GUI-SPIKE-PLAN.md` §1, not the start of `slk-ui`. What it proves
//! is copied out; what it is, is deleted.
//!
//!   spike-shell                      # open the window
//!   spike-shell --rows 5000          # how many synthetic messages the mock holds
//!   spike-shell --bench              # scroll the whole list, print frame times and RSS, exit
//!   spike-shell --bench --idle 60    # …then sit idle that long, print CPU and RSS, exit
//!
//! The GTK main thread never touches the engine directly: a tokio runtime on
//! its own thread hosts it, `Command`s go down an mpsc, `Event`s come up into
//! the component's input channel. That is rule 2 of the architecture, and the
//! spike keeps it because the point is to find out whether relm4 makes it
//! natural or awkward.

mod app;
mod bench;
mod blockkit;
mod markup;
mod row;

use std::sync::Arc;
use std::time::Instant;

fn main() {
    bench::mark_start(Instant::now());

    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let value = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .and_then(|v| v.parse::<u64>().ok())
    };
    let rows = value("--rows").unwrap_or(5000) as usize;
    let options = bench::Options {
        bench: flag("--bench"),
        idle_secs: value("--idle").unwrap_or(0),
        send: args
            .iter()
            .position(|a| a == "--send")
            .and_then(|i| args.get(i + 1))
            .cloned(),
        jump: value("--jump").map(|v| v as u32),
    };

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    // The picture the demo posts, so there is something to draw.
    let png = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/spike.png");
    let backend: Arc<dyn slk_api::SlackBackend> = Arc::new(
        slk_api::MockBackend::new()
            .with_synthetic(rows)
            .with_image(&png),
    );

    // The engine lives here, on its own threads, for the life of the process.
    // `Engine::spawn` calls `tokio::spawn`, so it has to run inside the
    // runtime's context; the handle is what the interface holds.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime");
    let (commands, events) = {
        let _guard = runtime.enter();
        let store = slk_store::Store::open(None).expect("in-memory store");
        slk_sync::Engine::spawn(backend, store, Vec::new())
    };

    // Where fetched pictures go. A temp dir, because this is a spike; the
    // real client has a media cache with a retention rule.
    let media_dir = std::env::temp_dir().join(format!("spike-shell-{}", std::process::id()));

    let init = app::Init {
        commands,
        events: Some(events),
        runtime: runtime.handle().clone(),
        media_dir: media_dir.clone(),
        options,
    };

    // GTK would otherwise try to parse `--bench` itself and refuse it.
    let app = relm4::RelmApp::new("dev.olivka.slack_light.spike").with_args(Vec::new());
    app.run::<app::App>(init);

    // Nothing to flush: the store was in memory and the pictures were a
    // cache. The runtime goes down with whatever is still on it.
    runtime.shutdown_background();
    let _ = std::fs::remove_dir_all(&media_dir);
}

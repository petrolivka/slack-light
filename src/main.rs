//! Wiring: parse the arguments, build the pieces, hand them to each other.
//!
//! Order matters here and it is the one place it does: the tokio runtime and
//! every engine exist before GTK is initialised, the window holds channel
//! ends and nothing else, and the runtime outlives the window.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use slack_light::doctor;
use slk_api::{Credentials, MockBackend, SlackBackend, WebBackend};
use slk_auth as auth;
use slk_config::Config;
use slk_store::Store;
use slk_sync::Engine;
use std::sync::Arc;

#[derive(Parser)]
#[command(
    name = "slack-light",
    version,
    about = "A native, lightweight Slack client for Linux desktops"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,

    /// Run against a demo workspace with no network and no credentials.
    /// **Every automated test must pass this**, so a stray event cannot reach
    /// a real account.
    #[arg(long)]
    anonymous: bool,

    /// With `--anonymous`, have the demo workspace keep talking: a scripted
    /// message every few seconds.
    #[arg(long, value_name = "SECONDS", num_args = 0..=1, default_missing_value = "5")]
    demo: Option<u64>,

    /// With `--anonymous`, this many synthetic messages in `#engineering`,
    /// for a conversation the size of a real one.
    #[arg(long, value_name = "N", default_value = "0")]
    demo_rows: usize,

    /// Make no writes of any kind: no posts, no read marks.
    #[arg(long)]
    read_only: bool,

    /// Keep the message cache in memory only.
    #[arg(long)]
    no_cache: bool,

    /// A `colors.toml` to use instead of the configured theme.
    #[arg(long, value_name = "PATH")]
    theme: Option<std::path::PathBuf>,

    /// Report what this machine and this desktop can do.
    #[arg(long)]
    doctor: bool,

    /// Write a commented default configuration file.
    #[arg(long)]
    write_config: bool,

    /// Print every bindable action name.
    #[arg(long)]
    list_actions: bool,

    /// Print timing and memory lines to stdout as things happen.
    #[arg(long)]
    metrics: bool,

    /// Scroll the open conversation, report frame times and memory, exit.
    #[arg(long)]
    bench: bool,

    /// With `--bench`, sit idle this long afterwards and report CPU.
    #[arg(long, value_name = "SECONDS", default_value = "0")]
    idle: u64,

    /// Send this text once the conversation has loaded, through the same
    /// path the composer uses. For tests without a keyboard.
    #[arg(long, value_name = "TEXT", hide = true)]
    send: Option<String>,

    #[arg(long, value_name = "LEVEL")]
    log_level: Option<String>,

    #[arg(long, value_name = "PATH")]
    log_file: Option<String>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Manage the stored Slack session.
    Auth {
        #[command(subcommand)]
        what: AuthCmd,
    },
    /// Inspect or delete the local message cache.
    Cache {
        #[command(subcommand)]
        what: CacheCmd,
    },
    /// How much is unread, for a status bar. Answers from the running client
    /// when there is one and from the cache when there is not, and says
    /// which — a module that prints nothing looks broken, and one that
    /// prints a stale number without saying so is worse.
    Unread {
        /// Waybar's shape: text, tooltip, class.
        #[arg(long)]
        json: bool,
    },
    /// One line about the running client.
    Status,
    /// Send a message through the running client.
    Send {
        /// `#channel` or `@person`.
        target: String,
        /// The message. Everything after the target.
        text: Vec<String>,
    },
}

#[derive(Subcommand)]
enum CacheCmd {
    /// What the cache holds, and what it costs on disk.
    Stats,
    /// Apply the retention limits now, and give the space back.
    Trim,
    /// Delete the cache and the media alongside it.
    Purge,
}

#[derive(Subcommand)]
enum AuthCmd {
    /// Sign in with your browser: a throwaway Chromium profile opens at
    /// Slack's sign-in page; nothing is copied by hand.
    Add {
        /// Paste the token and cookie yourself instead.
        #[arg(long)]
        paste: bool,
        /// Install a Slack app of your own instead of reading a browser
        /// session. Documented, defensible, and less capable — see
        /// `docs/SLACK-ACCESS-STRATEGY.md`.
        #[arg(long, conflicts_with = "paste")]
        oauth: bool,
        /// An `xapp-…` app-level token, which is the only realtime an
        /// installed app has. Without one the client polls.
        #[arg(long, value_name = "TOKEN", requires = "oauth")]
        app_token: Option<String>,
        /// A specific browser binary to drive.
        #[arg(long, value_name = "PATH")]
        browser: Option<std::path::PathBuf>,
        /// Give up after this many seconds.
        #[arg(long, value_name = "SECONDS", default_value = "300")]
        timeout: u64,
    },
    /// Show which workspaces are configured. Never prints the credentials.
    List,
    /// Forget one workspace, or all of them.
    Remove {
        /// The workspace subdomain. Omit to forget every one.
        team: Option<String>,
    },
}

/// Swap an expired OAuth access token for a fresh one, and write it back.
///
/// Only when the workspace's app has token rotation turned on: without it
/// Slack sends no refresh token and the access token does not expire, so
/// there is nothing to do. Sixty seconds of slack on the deadline, because a
/// token that expires while the request is in flight fails in the least
/// helpful way there is.
/// `slack-light auth add --oauth`: install the app, keep the token.
fn oauth_sign_in(
    browser: Option<std::path::PathBuf>,
    timeout: u64,
    app_token: Option<String>,
) -> Result<()> {
    let got = auth::oauth::install(browser, std::time::Duration::from_secs(timeout))?;
    let mut all = auth::load_all().unwrap_or_default();
    all.retain(|a| a.team != got.team);
    all.push(auth::Account {
        team: got.team.clone(),
        token: got.token,
        // Empty, and that is the signal: no cookie means the OAuth route.
        cookie: String::new(),
        refresh: got.refresh,
        expires_at: got.expires_at,
        app_token: app_token.unwrap_or_default(),
    });
    let p = auth::save_all(&all)?;
    println!("stored {} in {} (0600)", got.team, p.display());
    println!(
        "\nThis route cannot read the unread counts in one call, has no typing\n\
         indicators and no drafts, and its realtime needs an app-level token.\n\
         The client disables what it cannot do rather than failing at it."
    );
    Ok(())
}

fn refresh_if_stale(a: &mut auth::Account) -> Result<()> {
    if a.refresh.is_empty() || a.expires_at == 0 {
        return Ok(());
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;
    if a.expires_at > now + 60 {
        return Ok(());
    }
    let fresh = auth::oauth::refresh(&a.refresh)?;
    a.token = fresh.token;
    if !fresh.refresh.is_empty() {
        a.refresh = fresh.refresh;
    }
    a.expires_at = fresh.expires_at;

    // Written back one account at a time rather than all at once: a rewrite
    // of the whole file here would drop any workspace that failed to load.
    let mut all = auth::load_all().unwrap_or_default();
    if let Some(slot) = all.iter_mut().find(|x| x.team == a.team) {
        *slot = a.clone();
        auth::save_all(&all)?;
    }
    Ok(())
}

fn main() -> Result<()> {
    let t0 = std::time::Instant::now();
    slk_ui::bench::mark_start(t0);
    let cli = Cli::parse();

    if cli.write_config {
        let p = Config::write_default()?;
        println!("wrote {}", p.display());
        return Ok(());
    }
    if cli.list_actions {
        // Written, not printed: `--list-actions | head` closes the pipe early,
        // and `println!` answers a closed pipe with a panic.
        use std::io::Write;
        let out = std::io::stdout();
        let mut out = out.lock();
        for a in slk_config::Action::ALL {
            if writeln!(out, "{:<20} {:<10} {}", a.name(), a.group(), a.help()).is_err() {
                break;
            }
        }
        return Ok(());
    }
    match &cli.command {
        Some(Cmd::Cache { what }) => return cache_cmd(what),
        Some(Cmd::Unread { json }) => return slack_light::ipc::unread(*json, cli.anonymous),
        Some(Cmd::Status) => return slack_light::ipc::status(cli.anonymous),
        Some(Cmd::Send { target, text }) => {
            if text.is_empty() {
                anyhow::bail!("nothing to send");
            }
            return slack_light::ipc::send(target, &text.join(" "), cli.anonymous);
        }
        Some(Cmd::Auth { what }) => {
            return match what {
                AuthCmd::Add {
                    paste,
                    oauth,
                    app_token,
                    browser,
                    timeout,
                } => {
                    if *oauth {
                        oauth_sign_in(browser.clone(), *timeout, app_token.clone())
                    } else if *paste {
                        auth::add()
                    } else {
                        browser_sign_in(browser.clone(), *timeout)
                    }
                }
                AuthCmd::List => {
                    match auth::load_all() {
                        Ok(all) => {
                            println!("{}", auth::path().display());
                            for a in all {
                                println!("  {}", a.team);
                            }
                        }
                        Err(_) => println!("no session stored. Run `slack-light auth add`."),
                    }
                    Ok(())
                }
                AuthCmd::Remove { team } => {
                    let mut all = auth::load_all().unwrap_or_default();
                    let before = all.len();
                    match team {
                        Some(t) => all.retain(|a| a.team != *t),
                        None => all.clear(),
                    }
                    if all.len() == before {
                        println!("nothing matched");
                        return Ok(());
                    }
                    if all.is_empty() {
                        let p = auth::path();
                        let _ = std::fs::remove_file(&p);
                        println!("removed {}", p.display());
                    } else {
                        auth::save_all(&all)?;
                        println!("{} workspace(s) left", all.len());
                    }
                    Ok(())
                }
            };
        }
        None => {}
    }
    if cli.doctor {
        return doctor::report(cli.anonymous);
    }

    let (config, problems) = Config::load();
    init_logging(&cli, &config)?;
    for p in &problems {
        eprintln!("config: {p}");
    }
    slk_ui::bench::set_enabled(cli.metrics || cli.bench);
    install_panic_hook();

    // The engines' home. Built before GTK, kept until after the window closes.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;

    // One backend per configured workspace. A workspace that will not connect
    // does not stop the others: losing one company's Slack because another's
    // session expired would be absurd.
    let mut backends: Vec<(String, Arc<dyn SlackBackend>)> = Vec::new();
    if cli.anonymous {
        let mut demo = MockBackend::new().with_demo_image();
        if cli.demo_rows > 0 {
            demo = demo.with_synthetic(cli.demo_rows);
        }
        match cli.demo {
            Some(secs) => {
                backends.push(("demo".into(), Arc::new(demo.with_live_stream(secs.max(1)))));
                backends.push(("other-corp".into(), Arc::new(MockBackend::second())));
            }
            None => backends.push(("demo".into(), Arc::new(demo))),
        }
    } else {
        let accounts = auth::load_all()?;
        for mut a in accounts {
            let team = a.team.clone();
            // An expired OAuth token is refreshed before anything is asked of
            // it. Doing it here, once, beats discovering it on the first
            // request and having to unwind a half-built client.
            if let Err(e) = refresh_if_stale(&mut a) {
                eprintln!("{team}: could not refresh the token — {e}");
            }
            let r = runtime.block_on(WebBackend::connect(Credentials {
                domain: a.team,
                token: a.token,
                cookie: a.cookie,
                app_token: a.app_token,
            }));
            match r {
                Ok(b) => backends.push((team, Arc::new(b))),
                Err(e) => eprintln!("{team}: not connected — {}", e.user_message()),
            }
        }
        if backends.is_empty() {
            anyhow::bail!("no workspace could be reached. Try `slack-light auth add`.");
        }
    }

    let store_path = if cli.no_cache || !config.store.enabled {
        None
    } else {
        Some(slk_config::data_dir().join("store.sqlite"))
    };

    // Retention runs once at start-up rather than on a timer: it is the only
    // moment nothing else is touching the store.
    if let Some(p) = &store_path {
        if let Ok(mut store) = Store::open(Some(p)) {
            match store.trim(
                config.store.max_messages_per_channel,
                config.store.max_age_days,
            ) {
                Ok(n) if n > 0 => tracing::info!("retention removed {n} messages"),
                Err(e) => tracing::warn!("retention: {e}"),
                _ => {}
            }
        }
        let media = slk_config::cache_dir().join("media");
        let freed = trim_media(&media, config.images.cache_mb);
        if freed > 0 {
            tracing::info!("media cache trimmed by {}", human_bytes(freed));
        }
    }

    // Each engine owns its own connection to the store; every workspace's
    // events are folded into one stream, tagged with which.
    let (ev_tx, ev_rx) = tokio::sync::mpsc::channel(512);
    let mut workspaces = Vec::new();
    {
        let _guard = runtime.enter();
        for (name, backend) in backends {
            let team = backend.team().clone();
            let store = Store::open(store_path.as_deref()).context("opening the message cache")?;
            let (cmd_tx, mut rx) = Engine::spawn(
                backend,
                store,
                config.notify.keywords.clone(),
                config.message.history_page,
            );
            workspaces.push(slk_ui::Workspace {
                team: team.clone(),
                name,
                commands: cmd_tx,
            });
            let ev_tx = ev_tx.clone();
            runtime.spawn(async move {
                while let Some(ev) = rx.recv().await {
                    if ev_tx.send((team.clone(), ev)).await.is_err() {
                        return;
                    }
                }
            });
        }
    }
    drop(ev_tx);

    let media_dir = if cli.anonymous {
        std::env::temp_dir().join(format!("slack-light-demo-{}", std::process::id()))
    } else {
        slk_config::cache_dir().join("media")
    };

    let theme = slk_theme::Choice {
        source: if cli.theme.is_some() {
            "file".into()
        } else {
            config.theme.source.clone()
        },
        builtin: config.theme.builtin.clone(),
        file: cli.theme.clone().or_else(|| {
            Some(config.theme.file.clone())
                .filter(|f| !f.is_empty())
                .map(std::path::PathBuf::from)
        }),
    };
    let user_css = config
        .theme
        .user_css
        .then(|| slk_config::config_dir().join("user.css"));

    // The control socket. Started before the window so a status bar polling
    // every two seconds gets an answer from the first frame rather than a
    // "not running" until the sidebar has loaded. `--anonymous` serves too,
    // on a socket of its own — the path has to be exercised, and the demo's
    // counts must never reach a status bar somebody reads as their real one.
    let snapshot = std::sync::Arc::new(std::sync::Mutex::new(slk_sync::Snapshot::default()));
    slack_light::ipc::serve(
        snapshot.clone(),
        workspaces.iter().map(|w| w.commands.clone()).collect(),
        cli.anonymous,
    );

    slk_ui::run(slk_ui::Init {
        workspaces,
        snapshot,
        events: Some(ev_rx),
        runtime: runtime.handle().clone(),
        media_dir: media_dir.clone(),
        config,
        theme,
        user_css,
        read_only: cli.read_only,
        options: slk_ui::bench::Options {
            bench: cli.bench,
            idle_secs: cli.idle,
            send: cli.send.clone(),
            ..Default::default()
        },
    });

    // The window is gone. Nothing needs flushing — the store is written as
    // it goes — so the runtime goes down with whatever is still on it.
    runtime.shutdown_background();
    if cli.anonymous {
        let _ = std::fs::remove_dir_all(&media_dir);
    }
    Ok(())
}

/// `slack-light auth add`: the browser flow, then a choice.
///
/// The `d` cookie the browser hands back reaches every workspace on the
/// account, and `localConfig_v2` lists every one the user is signed in to —
/// including the ones they would rather this client never touched. So the
/// teams are listed and the user picks; nothing is stored unasked.
fn browser_sign_in(browser: Option<std::path::PathBuf>, timeout: u64) -> Result<()> {
    use std::io::{self, Write};
    println!(
        "A browser window will open at Slack's sign-in page, in a throwaway profile.\n\
         Sign in as you normally would. The window closes by itself afterwards."
    );
    let out = match auth::browser::sign_in(browser, std::time::Duration::from_secs(timeout)) {
        Ok(o) => o,
        Err(f) => {
            eprintln!("{}", f.advice());
            eprintln!("(`slack-light auth add --paste` is the other way in.)");
            std::process::exit(1);
        }
    };
    if out.teams.is_empty() {
        anyhow::bail!("signed in, but no workspace with a session token was found");
    }
    println!("\nSigned in. Workspaces on this account:");
    for (i, t) in out.teams.iter().enumerate() {
        println!("  {}. {}  ({})", i + 1, t.domain, t.team);
    }
    print!("\nStore which? Numbers separated by spaces, `all`, or nothing for none: ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let line = line.trim();
    let chosen: Vec<usize> = if line.eq_ignore_ascii_case("all") {
        (0..out.teams.len()).collect()
    } else {
        line.split_whitespace()
            .filter_map(|s| s.parse::<usize>().ok())
            .filter(|n| *n >= 1 && *n <= out.teams.len())
            .map(|n| n - 1)
            .collect()
    };
    if chosen.is_empty() {
        println!("nothing stored.");
        return Ok(());
    }
    let mut all = auth::load_all().unwrap_or_default();
    for i in chosen {
        let t = &out.teams[i];
        all.retain(|a| a.team != t.domain);
        all.push(auth::Account {
            team: t.domain.clone(),
            token: t.token.clone(),
            cookie: out.cookie.clone(),
            ..Default::default()
        });
        println!("  stored {}", t.domain);
    }
    let p = auth::save_all(&all)?;
    println!(
        "saved to {} (0600). Run `slack-light` to start.",
        p.display()
    );
    Ok(())
}

/// `slack-light cache stats | trim | purge`.
///
/// The cache is the user's employer's messages sitting on their disk, so being
/// able to see how much of it there is and remove it is not a convenience.
fn cache_cmd(what: &CacheCmd) -> Result<()> {
    let path = slk_config::data_dir().join("store.sqlite");
    let media = slk_config::cache_dir().join("media");
    let (config, _) = Config::load();

    match what {
        CacheCmd::Stats => {
            if !path.exists() {
                println!("no cache at {}", path.display());
                return Ok(());
            }
            let store = Store::open(Some(&path)).context("opening the message cache")?;
            let s = store.stats()?;
            println!("{}", path.display());
            println!("  messages        {}", s.messages);
            println!("  conversations   {}", s.conversations);
            println!("  people          {}", s.users);
            println!("  workspaces      {}", s.workspaces);
            if let Some(o) = &s.oldest {
                println!("  oldest          {}", human_ts(o));
            }
            println!("  size            {}", human_bytes(s.bytes));
            println!(
                "  retention       {} days, {} messages a channel",
                config.store.max_age_days, config.store.max_messages_per_channel
            );
            println!("\n{}", media.display());
            println!("  media           {}", human_bytes(dir_size(&media)));
        }
        CacheCmd::Trim => {
            let mut store = Store::open(Some(&path)).context("opening the message cache")?;
            let gone = store.trim(
                config.store.max_messages_per_channel,
                config.store.max_age_days,
            )?;
            store.vacuum()?;
            let freed = trim_media(&media, config.images.cache_mb);
            println!(
                "removed {gone} messages and {} of media",
                human_bytes(freed)
            );
        }
        CacheCmd::Purge => {
            // Deliberately not asking: someone who typed `cache purge` has
            // said what they want, and the cache is rebuildable from Slack.
            for p in [
                &path,
                &path.with_extension("sqlite-wal"),
                &path.with_extension("sqlite-shm"),
            ] {
                if p.exists() {
                    std::fs::remove_file(p).with_context(|| format!("removing {}", p.display()))?;
                }
            }
            if media.exists() {
                std::fs::remove_dir_all(&media)
                    .with_context(|| format!("removing {}", media.display()))?;
            }
            println!("deleted {} and {}", path.display(), media.display());
        }
    }
    Ok(())
}

pub fn human_bytes(n: i64) -> String {
    const U: [&str; 4] = ["B", "kB", "MB", "GB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

fn human_ts(ts: &str) -> String {
    ts.split('.')
        .next()
        .and_then(|s| s.parse::<i64>().ok())
        .and_then(|s| jiff::Timestamp::from_second(s).ok())
        .map(|t| t.strftime("%Y-%m-%d").to_string())
        .unwrap_or_else(|| ts.to_string())
}

fn dir_size(dir: &std::path::Path) -> i64 {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len() as i64)
        .sum()
}

/// Hold the media directory under its limit, oldest-used first.
fn trim_media(dir: &std::path::Path, limit_mb: u64) -> i64 {
    let limit = limit_mb as i64 * 1024 * 1024;
    let mut files: Vec<(std::time::SystemTime, i64, std::path::PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            if !m.is_file() {
                return None;
            }
            let when = m.accessed().or_else(|_| m.modified()).ok()?;
            Some((when, m.len() as i64, e.path()))
        })
        .collect();
    let mut total: i64 = files.iter().map(|(_, n, _)| n).sum();
    if total <= limit {
        return 0;
    }
    files.sort_by_key(|(when, _, _)| *when);
    let mut freed = 0i64;
    for (_, size, path) in files {
        if total <= limit {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= size;
            freed += size;
        }
    }
    freed
}

/// Leave a report worth reading. Nothing about credentials or message
/// content is included: a crash report is something people paste into
/// issues.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let dir = slk_config::state_dir();
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!(
            "crash-{}.txt",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        ));
        let body = format!(
            "slack-light {} panicked\n\n{info}\n\nbacktrace:\n{}\n",
            env!("CARGO_PKG_VERSION"),
            std::backtrace::Backtrace::force_capture()
        );
        let _ = std::fs::write(&path, &body);
        eprintln!("slack-light crashed. Report written to {}", path.display());
        previous(info);
    }));
}

fn init_logging(cli: &Cli, config: &Config) -> Result<()> {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    let level = cli
        .log_level
        .clone()
        .unwrap_or_else(|| config.log.level.clone());
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&level));

    let path = cli.log_file.clone().or_else(|| {
        config.log.file.then(|| {
            slk_config::state_dir()
                .join("slack-light.log")
                .display()
                .to_string()
        })
    });
    match path {
        Some(p) => {
            if let Some(d) = std::path::Path::new(&p).parent() {
                std::fs::create_dir_all(d)?;
            }
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&p)?;
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer().with_writer(file).with_ansi(false))
                .init();
        }
        None => {
            // stderr, never stdout: stdout is where `--metrics` writes.
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer().with_writer(std::io::stderr))
                .init();
        }
    }
    Ok(())
}

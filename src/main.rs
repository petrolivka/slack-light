//! Wiring: parse the arguments, build the pieces, hand them to each other.
//!
//! Until M0-GUI lands there is no window here. What there is, is everything
//! that does not need one: credentials, the cache, the action list — the
//! parts that were carried over intact and are worth being able to run.

use slack_light::auth;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use slk_config::Config;
use slk_store::Store;

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

    /// Make no writes of any kind: no posts, no read marks.
    #[arg(long)]
    read_only: bool,

    /// Keep the message cache in memory only.
    #[arg(long)]
    no_cache: bool,

    /// Write a commented default configuration file.
    #[arg(long)]
    write_config: bool,

    /// Print every bindable action name.
    #[arg(long)]
    list_actions: bool,

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
    /// Sign in. Browser sign-in arrives with M0-GUI; until then this is the
    /// guided paste of a token and cookie.
    Add,
    /// Show which workspaces are configured. Never prints the credentials.
    List,
    /// Forget one workspace, or all of them.
    Remove {
        /// The workspace subdomain. Omit to forget every one.
        team: Option<String>,
    },
}

fn main() -> Result<()> {
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
        Some(Cmd::Auth { what }) => {
            return match what {
                AuthCmd::Add => auth::add(),
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

    let (config, problems) = Config::load();
    init_logging(&cli, &config)?;
    for p in &problems {
        eprintln!("config: {p}");
    }

    // Honest rather than a stub window: the interface is the subject of the
    // M0-GUI spike, and a binary that opened an empty frame would be a demo
    // of nothing.
    eprintln!(
        "slack-light {}: the window is not built yet.\n\
         The Slack layer, the cache and the credentials are; see docs/M0-GUI-SPIKE-PLAN.md.\n\
         `slack-light auth …`, `slack-light cache …` and `--list-actions` work today.",
        env!("CARGO_PKG_VERSION")
    );
    std::process::exit(2);
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

fn human_bytes(n: i64) -> String {
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
///
/// Least-recently-used by access time where the filesystem keeps one and by
/// modification time where it does not, which on a cache that is only ever
/// written once and read many times is the same ordering for anything that has
/// actually been looked at.
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
            tracing_subscriber::registry().with(filter).init();
        }
    }
    Ok(())
}

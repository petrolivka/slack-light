//! A socket so a status bar can ask, and a shell can send.
//!
//! FR-X5. On a Hyprland desktop there is no tray, so the way a person sees an
//! unread count is a waybar module that runs a command every few seconds.
//! That command must be cheap, must not open a window, and must answer
//! something sensible when the client is not running at all.
//!
//! So: a unix socket in the runtime directory, one line of JSON per request,
//! and a fallback that reads the cache directly when nothing is listening.
//! The fallback is the interesting half — a module that prints nothing when
//! the client is closed looks broken, and one that prints a stale count
//! without saying so is worse.

use anyhow::{Context, Result};
use slk_sync::{Command, Snapshot};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};

/// Where the socket lives. `$XDG_RUNTIME_DIR` is per-user, on tmpfs, and
/// cleaned up at logout, which is exactly the lifetime this wants.
pub fn socket_path(anonymous: bool) -> std::path::PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    // The demo workspace gets a socket of its own. It has to have one — a
    // path with no test is a path that works until the day it matters — but
    // it must never be the one a status bar is polling, or somebody reads the
    // demo's three unread messages as their own.
    if anonymous {
        dir.join("slack-light-demo.sock")
    } else {
        dir.join("slack-light.sock")
    }
}

/// Serve requests until the process ends.
///
/// On its own thread, with blocking I/O: this is one connection every few
/// seconds from a status bar, and putting it on the tokio runtime would mean
/// a second reason for that runtime to exist.
pub fn serve(
    state: Arc<Mutex<Snapshot>>,
    senders: Vec<tokio::sync::mpsc::Sender<Command>>,
    anonymous: bool,
) {
    let path = socket_path(anonymous);
    // A socket left behind by a client that was killed. Removing one that is
    // still being served would steal the name from a running instance, so
    // this only removes a socket nothing answers on.
    if path.exists() && UnixStream::connect(&path).is_err() {
        let _ = std::fs::remove_file(&path);
    }
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            tracing::debug!("no control socket ({e}); `slack-light unread` will read the cache");
            return;
        }
    };
    std::thread::Builder::new()
        .name("slk-ipc".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let state = state.clone();
                let senders = senders.clone();
                if let Err(e) = answer(stream, &state, &senders) {
                    tracing::debug!("control socket: {e}");
                }
            }
        })
        .ok();
}

fn answer(
    stream: UnixStream,
    state: &Arc<Mutex<Snapshot>>,
    senders: &[tokio::sync::mpsc::Sender<Command>],
) -> Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut out = stream;
    let (verb, rest) = match line.trim().split_once(' ') {
        Some((v, r)) => (v, r),
        None => (line.trim(), ""),
    };
    match verb {
        "unread" => {
            let snap = state.lock().unwrap().clone();
            writeln!(out, "{}", serde_json::to_string(&snap)?)?;
        }
        "status" => {
            let snap = state.lock().unwrap().clone();
            writeln!(
                out,
                "{} unread, {} mentions{}{}",
                snap.unread,
                snap.mentions,
                if snap.open.is_empty() {
                    String::new()
                } else {
                    format!(" · in {}", snap.open)
                },
                if snap.connected { "" } else { " · offline" }
            )?;
        }
        "send" => {
            let Some((target, text)) = rest.split_once(' ') else {
                writeln!(out, "usage: send <#channel|@person> <text>")?;
                return Ok(());
            };
            // Answer truthfully before handing it over. Without this the
            // shell is told "sent" and the mistake surfaces in a status line
            // nobody is looking at, which is the worst place for it.
            let want = target.trim_start_matches(['#', '@']);
            let known = {
                let snap = state.lock().unwrap();
                snap.names
                    .iter()
                    .any(|n| n.trim_start_matches(['#', '@']).eq_ignore_ascii_case(want))
            };
            if !known {
                writeln!(out, "no conversation called {target}")?;
                return Ok(());
            }
            // Every workspace is asked, and the one that has the conversation
            // answers. Guessing from the name would send to whichever
            // `#general` happened to be first, which across an employer's
            // workspace and a personal one is the worst kind of mistake.
            for tx in senders {
                let _ = tx.blocking_send(Command::SendTo {
                    target: target.to_string(),
                    text: text.to_string(),
                });
            }
            writeln!(out, "sent to {target}")?;
        }
        other => writeln!(out, "unknown request {other:?}; try unread, status, send")?,
    }
    Ok(())
}

/// Ask the running instance. `None` when there is not one.
pub fn ask(request: &str, anonymous: bool) -> Option<String> {
    let mut stream = UnixStream::connect(socket_path(anonymous)).ok()?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .ok()?;
    writeln!(stream, "{request}").ok()?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply).ok()?;
    Some(reply.trim().to_string())
}

/// `slack-light unread --json`, and the plain form beside it.
///
/// Falls back to the cache, and says which it read. A waybar module that
/// prints nothing when the client is closed looks broken; one that prints a
/// stale number without saying so is worse.
pub fn unread(json: bool, anonymous: bool) -> Result<()> {
    if let Some(reply) = ask("unread", anonymous) {
        if json {
            println!("{reply}");
        } else {
            let v: serde_json::Value = serde_json::from_str(&reply)?;
            println!(
                "{} unread, {} mentions",
                v.get("unread").and_then(|v| v.as_u64()).unwrap_or(0),
                v.get("mentions").and_then(|v| v.as_u64()).unwrap_or(0)
            );
        }
        return Ok(());
    }

    let store = slk_store::Store::open(Some(&slk_config::data_dir().join("store.sqlite")))
        .context("no running client, and no cache to read either")?;
    let (mut unread, mut mentions) = (0u32, 0u32);
    for team in store.teams().unwrap_or_default() {
        for c in store.conversations(&team).unwrap_or_default() {
            if !c.is_muted {
                unread += c.unread;
                mentions += c.mentions;
            }
        }
    }
    if json {
        // Waybar's own shape: `text`, `tooltip`, `class`. The class is what a
        // style sheet colours, and "stale" is a state worth colouring.
        println!(
            "{}",
            serde_json::json!({
                "text": if mentions > 0 { format!("{mentions}") } else if unread > 0 { "•".to_string() } else { String::new() },
                "tooltip": format!("{unread} unread, {mentions} mentions (from the cache — slack-light is not running)"),
                "class": "stale",
                "unread": unread,
                "mentions": mentions,
                "connected": false,
                "stale": true,
            })
        );
    } else {
        println!("{unread} unread, {mentions} mentions (from the cache — not running)");
    }
    Ok(())
}

pub fn status(anonymous: bool) -> Result<()> {
    match ask("status", anonymous) {
        Some(reply) => println!("{reply}"),
        None => println!("not running"),
    }
    Ok(())
}

pub fn send(target: &str, text: &str, anonymous: bool) -> Result<()> {
    match ask(&format!("send {target} {text}"), anonymous) {
        // A refusal is an error status, so a script can tell. Printing it on
        // stdout and exiting zero is how a cron job silently stops working.
        Some(reply) if reply.starts_with("no conversation") => anyhow::bail!(reply),
        Some(reply) => println!("{reply}"),
        None => anyhow::bail!("slack-light is not running; start it first"),
    }
    Ok(())
}

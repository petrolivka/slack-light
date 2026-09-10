//! Session restore: where the person left a workspace is where it opens.
//!
//! This needs a store that outlives an engine — the in-memory one every other
//! test uses cannot tell a restart from a first start — so it is on disk, in
//! a directory of its own that the test removes afterwards.

use slk_api::mock::MockBackend;
use slk_store::Store;
use slk_sync::{Command, Engine, Event};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

type Rx = tokio::sync::mpsc::Receiver<Event>;

async fn wait_for<T>(rx: &mut Rx, mut want: impl FnMut(&Event) -> Option<T>) -> Option<T> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return None;
        }
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some(ev)) => {
                if let Some(v) = want(&ev) {
                    return Some(v);
                }
            }
            _ => return None,
        }
    }
}

fn scratch() -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir()
        .join(format!(
            "slack-light-restore-{}-{nanos}",
            std::process::id()
        ))
        .join("store.sqlite")
}

/// The window lands somewhere the moment it has a sidebar, so the restore has
/// to be offered before the first one. It was offered at the end of boot —
/// a network round trip after the cached sidebar — from M3 until the first
/// run against a real workspace showed that it never took effect. Nothing
/// automated had noticed, because the demo keeps no store between runs.
#[tokio::test(flavor = "multi_thread")]
async fn a_restart_offers_the_restore_before_the_first_sidebar() {
    let path = scratch();

    // First run: find #engineering, and leave it as the place to come back to.
    let (cmd, mut rx) = Engine::spawn(
        Arc::new(MockBackend::new()),
        Store::open(Some(&path)).expect("a store on disk"),
        Vec::new(),
        50,
    );
    let eng = wait_for(&mut rx, |ev| match ev {
        Event::Conversations(cs) => cs
            .iter()
            .find(|c| c.name == "engineering")
            .map(|c| c.id.clone()),
        _ => None,
    })
    .await
    .expect("the demo has #engineering");
    cmd.send(Command::Remember(eng.as_str().to_string()))
        .await
        .unwrap();
    cmd.send(Command::Shutdown).await.unwrap();
    // The engine has let go of the store once it stops talking.
    while let Ok(Some(_)) = tokio::time::timeout(Duration::from_secs(10), rx.recv()).await {}

    // Second run, the same store: the restore comes before any sidebar with
    // something in it for the window to land on.
    let (_cmd, mut rx) = Engine::spawn(
        Arc::new(MockBackend::new()),
        Store::open(Some(&path)).expect("the same store"),
        Vec::new(),
        50,
    );
    let first = wait_for(&mut rx, |ev| match ev {
        Event::Restore(c) => Some(Some(c.clone())),
        Event::Conversations(cs) if !cs.is_empty() => Some(None),
        _ => None,
    })
    .await
    .expect("the second run says something");
    if let Some(dir) = path.parent() {
        let _ = std::fs::remove_dir_all(dir);
    }
    assert_eq!(
        first,
        Some(eng),
        "the restore has to arrive before the sidebar the window lands on"
    );
}

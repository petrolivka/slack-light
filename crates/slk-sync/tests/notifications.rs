//! When the engine decides a message is worth interrupting somebody for.
//!
//! This is the rule people judge a chat client by. Too eager and they turn
//! notifications off, at which point the client has none; too shy and they
//! miss the message that mattered. It is a scenario test rather than a unit
//! test because the decision reads the store, the directory and the
//! conversation's own muting, and mocking those apart would be testing the
//! mocks.

use slk_api::mock::MockBackend;
use slk_store::Store;
use slk_sync::{Command, Engine, Event};
use std::sync::Arc;
use std::time::Duration;

/// Drive the engine until it says something we are waiting for, or give up.
async fn wait_for<T>(
    rx: &mut tokio::sync::mpsc::Receiver<Event>,
    mut want: impl FnMut(&Event) -> Option<T>,
) -> Option<T> {
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

fn engine(
    live_every: u64,
) -> (
    tokio::sync::mpsc::Sender<Command>,
    tokio::sync::mpsc::Receiver<Event>,
) {
    let store = Store::open(None).expect("an in-memory store");
    let backend = Arc::new(MockBackend::new().with_live_stream(live_every));
    Engine::spawn(backend, store, Vec::new(), 50)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_mention_in_a_channel_is_worth_interrupting_for() {
    let (_cmd, mut rx) = engine(1);
    let got = wait_for(&mut rx, |ev| match ev {
        Event::Notify {
            who, text, mention, ..
        } if *mention => Some((who.clone(), text.clone())),
        _ => None,
    })
    .await;
    let (who, text) = got.expect("the demo stream mentions the signed-in user");
    assert!(!who.is_empty(), "a notification says who it is from");
    assert!(
        text.contains("rollback"),
        "and carries the message: {text:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ordinary_channel_message_is_not() {
    let (_cmd, mut rx) = engine(1);
    // The demo stream sends two ordinary lines before the mention. Neither
    // may raise anything: a client that interrupts on every message in a
    // busy channel is one people turn off by the second afternoon.
    let first = wait_for(&mut rx, |ev| match ev {
        Event::Notify { text, .. } => Some(text.clone()),
        _ => None,
    })
    .await;
    match first {
        None => panic!("the stream should have reached the mention"),
        Some(text) => assert!(
            text.contains("rollback"),
            "the first notification is the mention, not the chatter before it: {text:?}"
        ),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_keyword_counts_as_a_mention() {
    let store = Store::open(None).expect("an in-memory store");
    let backend = Arc::new(MockBackend::new().with_live_stream(1));
    // "dashboards" appears in the first line of the demo stream, which is
    // otherwise ordinary chatter.
    let (_cmd, mut rx) = Engine::spawn(backend, store, vec!["dashboards".into()], 50);
    let text = wait_for(&mut rx, |ev| match ev {
        Event::Notify { text, .. } => Some(text.clone()),
        _ => None,
    })
    .await
    .expect("a highlight word interrupts like a mention");
    assert!(
        text.contains("dashboards"),
        "the keyword line comes first: {text:?}"
    );
}

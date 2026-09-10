//! `--read-only` means no writes of any kind (FR-Z5).
//!
//! It did not. The mode was implemented in the window — the composer refused
//! to send, reactions refused, uploads refused — and marking a conversation
//! read walked straight past all of it, because nothing in the interface
//! calls that a write. It just happens, on focus, quietly, and it is a
//! `conversations.mark` against a real account that moves the unread state on
//! every device the person owns.
//!
//! Found by running the client against a live workspace under `--read-only`
//! and watching its unread counts go to zero.
//!
//! The guarantee now lives in `slk_api::ReadOnly`, which wraps the backend,
//! so this drives the engine — the layer that decides to mark — through that
//! wrapper and asserts nothing reached Slack.

use slk_api::mock::MockBackend;
use slk_store::Store;
use slk_sync::{Command, Engine, Event};
use std::sync::Arc;
use std::time::Duration;

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

#[tokio::test(flavor = "multi_thread")]
async fn read_only_writes_nothing_at_all() {
    let mock = Arc::new(MockBackend::new());
    let guarded = slk_api::ReadOnly::wrap(mock.clone());
    let (cmd, mut rx) = Engine::spawn(guarded, Store::open(None).unwrap(), Vec::new(), 50);

    let ch = wait_for(&mut rx, |ev| match ev {
        Event::Conversations(cs) => cs
            .iter()
            .find(|c| c.name == "engineering")
            .map(|c| c.id.clone()),
        _ => None,
    })
    .await
    .expect("#engineering");

    // Every shape of write the engine knows, including the quiet one.
    let ts = slk_core::Ts::new("1725701900.000100");
    for cmd_ in [
        Command::MarkRead(ch.clone(), ts.clone()),
        Command::Send {
            channel: ch.clone(),
            thread: None,
            text: "this must not go".into(),
            local_id: "ro-1".into(),
            broadcast: false,
        },
        Command::React {
            channel: ch.clone(),
            ts: ts.clone(),
            name: "tada".into(),
            on: true,
        },
        // #engineering starts starred in the demo, so the write to refuse is
        // taking it *off*.
        Command::Star {
            channel: ch.clone(),
            on: false,
        },
        Command::Typing(ch.clone()),
        Command::Presence(false),
    ] {
        cmd.send(cmd_).await.unwrap();
    }
    // Let the engine work through them.
    tokio::time::sleep(Duration::from_millis(600)).await;

    assert!(
        mock.marks().is_empty(),
        "a read mark reached Slack under --read-only: {:?}",
        mock.marks()
    );
    assert!(
        mock.messages_in(&ch)
            .iter()
            .all(|m| m.text != "this must not go"),
        "a message was posted under --read-only"
    );
    assert!(
        mock.conversation(&ch).is_some_and(|c| c.is_starred),
        "the star was removed under --read-only"
    );
    assert!(
        mock.typed_in().is_empty(),
        "a typing indicator was sent under --read-only"
    );
}

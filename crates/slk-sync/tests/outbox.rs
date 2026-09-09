//! What happens to a message typed while the connection is down.
//!
//! FR-H9 makes two promises — nothing is lost, and nothing is reordered —
//! and both of them are only ever exercised by a failure that a test has to
//! arrange, which is why the mock can be told to be unreachable. A queue
//! nobody has drained is a queue nobody has tested.

use slk_api::mock::MockBackend;
use slk_core::Delivery;
use slk_store::Store;
use slk_sync::{Command, Engine, Event};
use std::sync::Arc;
use std::time::Duration;

async fn wait_for<T>(
    rx: &mut tokio::sync::mpsc::Receiver<Event>,
    mut want: impl FnMut(&Event) -> Option<T>,
) -> Option<T> {
    // Longer than the engine's outbox heartbeat, so a test that is waiting
    // for a retry is waiting for the retry rather than racing it.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
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
async fn a_message_typed_offline_waits_and_then_goes_in_order() {
    let store = Store::open(None).expect("an in-memory store");
    let mock = Arc::new(MockBackend::new());
    let (cmd, mut rx) = Engine::spawn(mock.clone(), store, Vec::new(), 50);

    let ch = wait_for(&mut rx, |ev| match ev {
        Event::Conversations(cs) => cs
            .iter()
            .find(|c| c.name == "engineering")
            .map(|c| c.id.clone()),
        _ => None,
    })
    .await
    .expect("#engineering");

    mock.set_unreachable(true);
    for (i, text) in ["first", "second", "third"].iter().enumerate() {
        cmd.send(Command::Send {
            channel: ch.clone(),
            thread: None,
            text: (*text).into(),
            local_id: format!("q{i}"),
            broadcast: false,
        })
        .await
        .unwrap();
        // The engine answers a send with the optimistic row, then the row it
        // settles on, then the notice — so the notice is the end of the
        // exchange, and waiting for it per message is what puts the three in
        // a known order rather than a hopeful one. The settled row goes past
        // on the way, and it is the interesting one: while a message is
        // queued it still reads as *pending*. A row marked failed that then
        // arrives is worse than one that says it is waiting.
        let mut ever_failed = false;
        let mut said_pending = false;
        let queued = wait_for(&mut rx, |ev| match ev {
            Event::Upserted {
                message, replaces, ..
            } if message.text == *text && replaces.is_some() => {
                match message.delivery {
                    Delivery::Failed(_) => ever_failed = true,
                    Delivery::Pending(_) => said_pending = true,
                    Delivery::Confirmed => {}
                }
                None
            }
            Event::Notice(n) if n.contains("queued") => Some(n.clone()),
            _ => None,
        })
        .await;
        assert!(queued.is_some(), "{text} was not queued");
        assert!(!ever_failed, "{text} was shown as failed while it waited");
        assert!(said_pending, "{text} never settled as pending");
    }

    // The connection comes back. The engine flushes on reconnect, and the
    // demo's first socket drops itself a few seconds in, so this is the real
    // path rather than a special one.
    // Nothing disconnects here: the socket stayed up the whole time and only
    // the sends failed. The queue has to drain anyway — on the heartbeat, not
    // on a reconnect that never comes.
    mock.set_unreachable(false);

    // The confirmations arrive before the notice that closes the flush, so
    // they are collected on the way past rather than looked for afterwards.
    let mut order = Vec::new();
    let sent = wait_for(&mut rx, |ev| match ev {
        Event::Upserted {
            message, replaces, ..
        } if replaces.is_some() && matches!(message.delivery, Delivery::Confirmed) => {
            if ["first", "second", "third"].contains(&message.text.as_str()) {
                order.push(message.text.clone());
            }
            None
        }
        Event::Notice(n) if n.contains("queued message") => Some(n.clone()),
        _ => None,
    })
    .await;
    assert!(
        sent.is_some_and(|n| n.contains('3')),
        "all three should go when the connection returns"
    );
    assert_eq!(
        order,
        vec!["first", "second", "third"],
        "out of order is worse than late"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_is_not_queued() {
    // Only a transport failure waits. Slack saying no — no permission, an
    // archived channel — will say no again in ten minutes, and a message
    // stuck at the head of the queue blocks every one behind it.
    let store = Store::open(None).expect("an in-memory store");
    let mock = Arc::new(MockBackend::new());
    let (cmd, mut rx) = Engine::spawn(mock.clone(), store, Vec::new(), 50);
    wait_for(&mut rx, |ev| matches!(ev, Event::Connected).then_some(())).await;

    cmd.send(Command::Send {
        channel: slk_core::ChannelId::new("CNOSUCH"),
        thread: None,
        text: "into the void".into(),
        local_id: "v1".into(),
        broadcast: false,
    })
    .await
    .unwrap();

    // The settled row, not the optimistic one it replaces.
    let failed = wait_for(&mut rx, |ev| match ev {
        Event::Upserted {
            message, replaces, ..
        } if message.text == "into the void" && replaces.is_some() => {
            Some(matches!(message.delivery, Delivery::Failed(_)))
        }
        _ => None,
    })
    .await;
    assert_eq!(
        failed,
        Some(true),
        "a channel that does not exist is a refusal, not a network problem"
    );
}

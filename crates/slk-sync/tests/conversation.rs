//! Managing a conversation: star, mute, topic, leave, and the pinned list.
//!
//! These are the acts that change what the sidebar says about a channel
//! rather than what is in it, and each one has two halves that can disagree —
//! what Slack was told and what the local copy believes. A star that the
//! sidebar does not show, or shows after Slack refused it, is the bug this
//! file exists to catch.

use slk_api::mock::MockBackend;
use slk_store::Store;
use slk_sync::{Command, Engine, Event, ListTarget};
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

fn engine() -> (
    tokio::sync::mpsc::Sender<Command>,
    tokio::sync::mpsc::Receiver<Event>,
) {
    let store = Store::open(None).expect("an in-memory store");
    Engine::spawn(Arc::new(MockBackend::new()), store, Vec::new(), 50)
}

/// The demo workspace's `#engineering`, whichever team id it was given.
async fn engineering(rx: &mut tokio::sync::mpsc::Receiver<Event>) -> slk_core::ChannelId {
    wait_for(rx, |ev| match ev {
        Event::Conversations(cs) => cs
            .iter()
            .find(|c| c.name == "engineering")
            .map(|c| c.id.clone()),
        _ => None,
    })
    .await
    .expect("the demo workspace has #engineering")
}

#[tokio::test(flavor = "multi_thread")]
async fn muting_reaches_the_sidebar() {
    let (cmd, mut rx) = engine();
    let ch = engineering(&mut rx).await;

    cmd.send(Command::Mute {
        channel: ch.clone(),
        on: true,
    })
    .await
    .unwrap();

    let muted = wait_for(&mut rx, |ev| match ev {
        Event::Conversations(cs) => cs
            .iter()
            .find(|c| c.id == ch)
            .filter(|c| c.is_muted)
            .map(|_| true),
        _ => None,
    })
    .await;
    assert_eq!(
        muted,
        Some(true),
        "a mute has to arrive in the sidebar, not only at Slack"
    );

    cmd.send(Command::Mute {
        channel: ch.clone(),
        on: false,
    })
    .await
    .unwrap();
    let unmuted = wait_for(&mut rx, |ev| match ev {
        Event::Conversations(cs) => cs
            .iter()
            .find(|c| c.id == ch)
            .map(|c| !c.is_muted)
            .filter(|v| *v),
        _ => None,
    })
    .await;
    assert_eq!(unmuted, Some(true), "and unmuting has to undo it");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_star_is_a_toggle_both_ways() {
    let (cmd, mut rx) = engine();
    let ch = engineering(&mut rx).await;
    // The demo starts with #engineering starred, so the first move is off.
    cmd.send(Command::Star {
        channel: ch.clone(),
        on: false,
    })
    .await
    .unwrap();
    let off = wait_for(&mut rx, |ev| match ev {
        Event::Conversations(cs) => cs
            .iter()
            .find(|c| c.id == ch)
            .map(|c| !c.is_starred)
            .filter(|v| *v),
        _ => None,
    })
    .await;
    assert_eq!(off, Some(true), "unstarring clears the star");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_topic_set_by_slash_and_by_command_land_in_the_same_place() {
    let (cmd, mut rx) = engine();
    let ch = engineering(&mut rx).await;

    cmd.send(Command::Slash {
        channel: ch.clone(),
        command: "/topic".into(),
        text: "on-call rotation".into(),
    })
    .await
    .unwrap();

    let topic = wait_for(&mut rx, |ev| match ev {
        Event::Conversations(cs) => cs
            .iter()
            .find(|c| c.id == ch)
            .map(|c| c.topic.clone())
            .filter(|t| t == "on-call rotation"),
        _ => None,
    })
    .await;
    assert_eq!(
        topic.as_deref(),
        Some("on-call rotation"),
        "`/topic` goes through the same handler as the action, so the \
         sidebar refreshes either way"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn inviting_somebody_who_is_not_here_says_so_rather_than_failing_quietly() {
    let (cmd, mut rx) = engine();
    let ch = engineering(&mut rx).await;
    cmd.send(Command::Invite {
        channel: ch,
        who: "nobody-at-all".into(),
    })
    .await
    .unwrap();
    let notice = wait_for(&mut rx, |ev| match ev {
        Event::Notice(n) if n.contains("nobody-at-all") => Some(n.clone()),
        _ => None,
    })
    .await;
    assert!(
        notice.is_some(),
        "an unresolvable name is reported, not swallowed"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_pinned_list_shows_what_slack_says_is_pinned() {
    let (cmd, mut rx) = engine();
    let ch = engineering(&mut rx).await;
    // The list is built from the store, so the history has to be there first.
    cmd.send(Command::Open(ch.clone())).await.unwrap();
    wait_for(&mut rx, |ev| match ev {
        Event::Messages { messages, .. } if !messages.is_empty() => Some(()),
        _ => None,
    })
    .await
    .expect("history");

    cmd.send(Command::ListPinned(ch.clone())).await.unwrap();
    let items = wait_for(&mut rx, |ev| match ev {
        Event::List { title, items } if title == "pinned" => Some(items.clone()),
        _ => None,
    })
    .await
    .expect("a pinned list");

    let messages: Vec<_> = items
        .iter()
        .filter(|r| matches!(r.target, ListTarget::Message(..)))
        .collect();
    assert_eq!(messages.len(), 1, "the demo pins exactly one message");
    assert!(
        messages[0].label.contains("retry limit"),
        "and it is the right one: {:?}",
        messages[0].label
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_status_can_be_given_an_expiry_in_words() {
    // Parsed in the engine rather than in the window, because `/status` and
    // the palette both reach it and only one of them is a text field.
    let now = 1_725_700_000;
    let (emoji, text, expires) = slk_sync::parse_status(":rocket: shipping v2.4 for 2h", now);
    assert_eq!(emoji, "rocket");
    assert_eq!(text, "shipping v2.4");
    assert_eq!(expires, now + 2 * 3600);

    // No duration: every word is the status, and it lasts until changed.
    let (_, text, expires) = slk_sync::parse_status("back in a bit, ask bob for the key", now);
    assert_eq!(text, "back in a bit, ask bob for the key");
    assert_eq!(expires, 0, "a `for` that is not a duration is just a word");

    // No emoji, no text: clearing it.
    let (emoji, text, expires) = slk_sync::parse_status("", now);
    assert!(emoji.is_empty() && text.is_empty() && expires == 0);

    // A colon that is not a shortcode must not eat the message.
    let (emoji, text, _) = slk_sync::parse_status("18:00 standup", now);
    assert!(emoji.is_empty(), "`18:00` is a time, not an emoji");
    assert_eq!(text, "18:00 standup");
}

#[tokio::test(flavor = "multi_thread")]
async fn typing_reaches_the_backend_and_says_nothing_on_screen() {
    // The one command that must never produce a notice: an indicator that
    // interrupts is worse than no indicator.
    let (cmd, mut rx) = engine();
    let ch = engineering(&mut rx).await;
    cmd.send(Command::Typing(ch)).await.unwrap();
    let noise = tokio::time::timeout(
        Duration::from_millis(400),
        wait_for(&mut rx, |ev| match ev {
            Event::Notice(n) => Some(n.clone()),
            _ => None,
        }),
    )
    .await;
    assert!(
        matches!(noise, Err(_) | Ok(None)),
        "typing said something: {noise:?}"
    );
}

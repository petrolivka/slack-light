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

/// The bar arrives on an open, and only for a conversation that has one.
///
/// Both halves matter. The demo gives `#engineering` two bookmarks and
/// `#general` none, and a client that showed the previous conversation's bar
/// on the next conversation would pass a test that only ever opened one.
#[tokio::test(flavor = "multi_thread")]
async fn the_bookmark_bar_belongs_to_its_conversation() {
    let (cmd, mut rx) = engine();
    // Both ids from the one sidebar: `Conversations` arrives once, and a
    // second wait for it would sit until the timeout.
    let (ch, general) = wait_for(&mut rx, |ev| match ev {
        Event::Conversations(cs) => {
            let find = |n: &str| cs.iter().find(|c| c.name == n).map(|c| c.id.clone());
            Some((find("engineering")?, find("general")?))
        }
        _ => None,
    })
    .await
    .expect("the demo workspace has #engineering and #general");

    cmd.send(Command::Open(ch.clone())).await.unwrap();
    let items = wait_for(&mut rx, |ev| match ev {
        Event::Bookmarks { channel, items } if *channel == ch => Some(items.clone()),
        _ => None,
    })
    .await
    .expect("a conversation with bookmarks reports them");
    assert_eq!(items.len(), 2, "both of them, not just the first");
    assert_eq!(items[0].title, "Runbook");
    assert_eq!(items[0].emoji, ":books:");

    // A conversation with no bookmarks says nothing at all rather than
    // repeating the last answer. The refresh runs after the history lands, so
    // waiting for the history is not enough — wait for the *next* open's
    // history, by which time a bar for `#general` would long since have been
    // sent.
    cmd.send(Command::Open(general.clone())).await.unwrap();
    let mut bar_for_general = false;
    let opened = wait_for(&mut rx, |ev| match ev {
        Event::Bookmarks { channel, .. } if *channel == general => {
            bar_for_general = true;
            None
        }
        Event::Messages { channel, .. } if *channel == general => Some(()),
        _ => None,
    })
    .await;
    assert!(opened.is_some(), "the second conversation did open");
    cmd.send(Command::Open(ch.clone())).await.unwrap();
    wait_for(&mut rx, |ev| match ev {
        Event::Bookmarks { channel, .. } if *channel == general => {
            bar_for_general = true;
            None
        }
        Event::Messages { channel, .. } if *channel == ch => Some(()),
        _ => None,
    })
    .await
    .expect("and the first one comes back");
    assert!(
        !bar_for_general,
        "a conversation with no bookmarks must not be given somebody else's"
    );
}

/// Re-opening the same conversation does not ask Slack again.
///
/// The bar changes about as often as a channel's purpose does, and the client
/// that asks on every open is the traffic shape FR-Z1 exists to avoid. The
/// cache answers instead; Slack is asked at most once every ten minutes.
#[tokio::test(flavor = "multi_thread")]
async fn the_bar_is_cached_rather_than_re_fetched() {
    let store = Store::open(None).expect("an in-memory store");
    let backend = Arc::new(MockBackend::new());
    let (cmd, mut rx) = Engine::spawn(backend.clone(), store, Vec::new(), 50);
    let ch = engineering(&mut rx).await;

    for _ in 0..2 {
        cmd.send(Command::Open(ch.clone())).await.unwrap();
        wait_for(&mut rx, |ev| match ev {
            Event::Messages { channel, .. } if *channel == ch => Some(()),
            _ => None,
        })
        .await
        .expect("the conversation opens");
    }
    assert_eq!(
        backend.bookmark_calls(),
        1,
        "the second open answers from the cache"
    );
}

// ---- FR-C2: making, renaming and ending conversations ---------------------

/// What `/create Release Week` means, before any request is made.
#[test]
fn a_channel_name_is_normalised_the_way_slack_would() {
    use slk_api::backend::channel_name;
    assert_eq!(channel_name("Release Week").as_deref(), Ok("release-week"));
    assert_eq!(channel_name("  #ops  ").as_deref(), Ok("ops"));
    assert_eq!(channel_name("čeština").as_deref(), Ok("čeština"));
    // Refused here, with the character named, rather than by Slack a round
    // trip later with the rule named.
    let why = channel_name("q3/plans").unwrap_err();
    assert!(why.contains('/'), "the refusal says which character: {why}");
    assert!(channel_name("   ").is_err());
    assert!(channel_name(&"x".repeat(81)).is_err());
    assert!(channel_name(&"x".repeat(80)).is_ok());
}

/// A new channel lands in the sidebar and is opened — once.
#[tokio::test(flavor = "multi_thread")]
async fn a_created_channel_is_in_the_sidebar_and_on_screen() {
    let (cmd, mut rx) = engine();
    let _ = engineering(&mut rx).await;

    cmd.send(Command::CreateChannel {
        name: "Release Week".into(),
        private: false,
    })
    .await
    .unwrap();

    let id = wait_for(&mut rx, |ev| match ev {
        Event::Conversations(cs) => cs
            .iter()
            .find(|c| c.name == "release-week")
            .map(|c| c.id.clone()),
        _ => None,
    })
    .await
    .expect("the normalised name reaches the sidebar");
    let opened = wait_for(&mut rx, |ev| match ev {
        Event::OpenChannel(c) if *c == id => Some(()),
        _ => None,
    })
    .await;
    assert!(opened.is_some(), "and the window is told to open it");

    // A second one by the same name is Slack's `name_taken`, said in words.
    cmd.send(Command::CreateChannel {
        name: "release-week".into(),
        private: true,
    })
    .await
    .unwrap();
    let said = wait_for(&mut rx, |ev| match ev {
        Event::Notice(n) if n.contains("cannot create") => Some(n.clone()),
        _ => None,
    })
    .await
    .expect("a clash is reported");
    assert!(said.contains("taken"), "in words, not as a code: {said}");
}

/// A private channel is private: the kind is what decides the 🔒.
#[tokio::test(flavor = "multi_thread")]
async fn create_private_makes_a_private_channel() {
    let (cmd, mut rx) = engine();
    let ch = engineering(&mut rx).await;
    cmd.send(Command::Slash {
        channel: ch,
        command: "/create-private".into(),
        text: "war room".into(),
    })
    .await
    .unwrap();
    let private = wait_for(&mut rx, |ev| match ev {
        Event::Conversations(cs) => cs
            .iter()
            .find(|c| c.name == "war-room")
            .map(|c| c.is_private()),
        _ => None,
    })
    .await;
    assert_eq!(private, Some(true));
}

/// Renamed in the sidebar, not only at Slack.
#[tokio::test(flavor = "multi_thread")]
async fn a_rename_reaches_the_sidebar() {
    let (cmd, mut rx) = engine();
    let ch = engineering(&mut rx).await;
    cmd.send(Command::Slash {
        channel: ch.clone(),
        command: "/rename".into(),
        text: "Platform".into(),
    })
    .await
    .unwrap();
    let name = wait_for(&mut rx, |ev| match ev {
        Event::Conversations(cs) => cs
            .iter()
            .find(|c| c.id == ch)
            .map(|c| c.name.clone())
            .filter(|n| n != "engineering"),
        _ => None,
    })
    .await;
    assert_eq!(name.as_deref(), Some("platform"));
}

/// Archived channels leave the sidebar; #general refuses, as at Slack.
#[tokio::test(flavor = "multi_thread")]
async fn archiving_removes_the_row_and_general_refuses() {
    let (cmd, mut rx) = engine();
    let (design, general) = wait_for(&mut rx, |ev| match ev {
        Event::Conversations(cs) => {
            let find = |n: &str| cs.iter().find(|c| c.name == n).map(|c| c.id.clone());
            Some((find("design")?, find("general")?))
        }
        _ => None,
    })
    .await
    .expect("the demo has #design and #general");

    cmd.send(Command::Archive(design.clone())).await.unwrap();
    let gone = wait_for(&mut rx, |ev| match ev {
        Event::Conversations(cs) if !cs.iter().any(|c| c.id == design) => Some(()),
        _ => None,
    })
    .await;
    assert!(gone.is_some(), "an archived channel is not in the sidebar");

    cmd.send(Command::Archive(general.clone())).await.unwrap();
    let said = wait_for(&mut rx, |ev| match ev {
        Event::Notice(n) if n.contains("general") || n.contains("cannot") => Some(n.clone()),
        _ => None,
    })
    .await
    .expect("the refusal is reported");
    assert!(said.contains("#general"), "and says why: {said}");
}

/// A group is everybody named or nobody, and the same people make the same
/// group twice.
#[tokio::test(flavor = "multi_thread")]
async fn a_group_is_everybody_named_or_nobody() {
    let (cmd, mut rx) = engine();
    let _ = engineering(&mut rx).await;
    // The directory arrives after the sidebar; resolving names needs it.
    wait_for(&mut rx, |ev| match ev {
        Event::Users(us) if !us.is_empty() => Some(()),
        _ => None,
    })
    .await
    .expect("the demo has people");

    cmd.send(Command::CreateGroup {
        who: "@alice @nobody-by-this-name".into(),
    })
    .await
    .unwrap();
    let said = wait_for(&mut rx, |ev| match ev {
        Event::Notice(n) if n.contains("nobody here") => Some(n.clone()),
        Event::OpenChannel(_) => Some("OPENED".into()),
        _ => None,
    })
    .await
    .expect("an unknown name is reported");
    assert_ne!(
        said, "OPENED",
        "a group with one of two people must not open"
    );

    let mut opened = Vec::new();
    for _ in 0..2 {
        cmd.send(Command::CreateGroup {
            who: "@alice, @bob".into(),
        })
        .await
        .unwrap();
        let id = wait_for(&mut rx, |ev| match ev {
            Event::OpenChannel(c) => Some(c.clone()),
            _ => None,
        })
        .await
        .expect("the group opens");
        opened.push(id);
    }
    assert_eq!(opened[0], opened[1], "the same people, the same group");
}

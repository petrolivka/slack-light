//! FR-M7's synced half: drafts that follow a person between devices.
//!
//! Each direction has its own way to go wrong. Down: a draft typed on the
//! phone lands in the wrong composer. Up: a draft typed here becomes a second
//! draft at Slack instead of replacing its own, or outlives the message it
//! became. Across: a draft finished on the phone comes back on the laptop —
//! or, worse, words typed here are lost to a race with the phone.

use slk_api::mock::MockBackend;
use slk_api::{ReadOnly, SlackBackend};
use slk_core::ChannelId;
use slk_store::Store;
use slk_sync::{Command, Engine, Event};
use std::sync::Arc;
use std::time::Duration;

type Tx = tokio::sync::mpsc::Sender<Command>;
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

fn engine_over(backend: Arc<dyn SlackBackend>) -> (Tx, Rx) {
    Engine::spawn(backend, Store::open(None).expect("a store"), Vec::new(), 50)
}

/// `#engineering` and `#general`, from the one sidebar event.
async fn ids(rx: &mut Rx) -> (ChannelId, ChannelId) {
    wait_for(rx, |ev| match ev {
        Event::Conversations(cs) => {
            let find = |n: &str| cs.iter().find(|c| c.name == n).map(|c| c.id.clone());
            Some((find("engineering")?, find("general")?))
        }
        _ => None,
    })
    .await
    .expect("the demo has #engineering and #general")
}

/// Every command sent before this one has been handled when it returns: the
/// engine takes commands one at a time, and an open is answered with the
/// conversation's messages.
async fn settle(cmd: &Tx, rx: &mut Rx, ch: &ChannelId) {
    cmd.send(Command::Open(ch.clone())).await.unwrap();
    wait_for(rx, |ev| match ev {
        Event::Messages { channel, .. } if channel == ch => Some(()),
        _ => None,
    })
    .await
    .expect("the engine is still answering");
}

async fn set_draft(cmd: &Tx, ch: &ChannelId, text: &str) {
    cmd.send(Command::SetDraft {
        channel: ch.clone(),
        thread: None,
        text: text.into(),
    })
    .await
    .unwrap();
}

/// Slack's draft for a conversation, as (id, words).
fn at_slack(mock: &MockBackend, ch: &ChannelId) -> Vec<(String, String)> {
    mock.remote_drafts()
        .into_iter()
        .filter(|d| &d.channel == ch && d.thread.is_none())
        .map(|d| (d.id, d.doc.plain()))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_draft_typed_on_the_phone_is_in_the_composer_here() {
    let mock = Arc::new(MockBackend::new());
    let (cmd, mut rx) = engine_over(mock.clone());
    let (_, general) = ids(&mut rx).await;
    cmd.send(Command::Open(general.clone())).await.unwrap();
    let text = wait_for(&mut rx, |ev| match ev {
        Event::Draft {
            channel,
            thread: None,
            text,
        } if *channel == general => Some(text.clone()),
        _ => None,
    })
    .await
    .expect("the phone's draft reaches the composer");
    assert_eq!(text, "half a thought from the phone");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_draft_typed_here_reaches_slack_and_is_edited_in_place() {
    let mock = Arc::new(MockBackend::new());
    let (cmd, mut rx) = engine_over(mock.clone());
    let (eng, general) = ids(&mut rx).await;

    set_draft(&cmd, &eng, "first").await;
    settle(&cmd, &mut rx, &general).await;
    let first = at_slack(&mock, &eng);
    assert_eq!(first.len(), 1, "one draft at Slack");
    assert_eq!(first[0].1, "first");

    set_draft(&cmd, &eng, "second").await;
    settle(&cmd, &mut rx, &general).await;
    let second = at_slack(&mock, &eng);
    // Counted: "the new words are there" passes while there are two drafts.
    assert_eq!(second.len(), 1, "still one, not a second beside it");
    assert_eq!(second[0].0, first[0].0, "the same draft, updated");
    assert_eq!(second[0].1, "second");

    set_draft(&cmd, &eng, "").await;
    settle(&cmd, &mut rx, &general).await;
    assert!(
        at_slack(&mock, &eng).is_empty(),
        "an emptied composer lets it go"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_draft_nobody_touched_costs_no_request() {
    let mock = Arc::new(MockBackend::new());
    let (cmd, mut rx) = engine_over(mock.clone());
    let (eng, general) = ids(&mut rx).await;
    // The window sends this on every conversation switch.
    for _ in 0..3 {
        set_draft(&cmd, &eng, "the same words").await;
    }
    set_draft(&cmd, &general, "").await;
    settle(&cmd, &mut rx, &general).await;
    assert_eq!(mock.draft_saves(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sent_message_takes_its_draft_with_it() {
    let mock = Arc::new(MockBackend::new());
    let (cmd, mut rx) = engine_over(mock.clone());
    let (eng, general) = ids(&mut rx).await;
    set_draft(&cmd, &eng, "about to go").await;
    settle(&cmd, &mut rx, &general).await;
    assert_eq!(at_slack(&mock, &eng).len(), 1);

    cmd.send(Command::Send {
        channel: eng.clone(),
        thread: None,
        text: "about to go".into(),
        local_id: "t-draft-1".into(),
        broadcast: false,
    })
    .await
    .unwrap();
    wait_for(&mut rx, |ev| match ev {
        Event::Upserted {
            channel,
            replaces: Some(_),
            ..
        } if *channel == eng => Some(()),
        _ => None,
    })
    .await
    .expect("the message is confirmed");
    settle(&cmd, &mut rx, &general).await;
    assert!(
        at_slack(&mock, &eng).is_empty(),
        "a draft must not outlive the message it became"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_draft_finished_on_the_phone_does_not_come_back() {
    let mock = Arc::new(MockBackend::new());
    let (cmd, mut rx) = engine_over(mock.clone());
    let (_, general) = ids(&mut rx).await;
    settle(&cmd, &mut rx, &general).await;

    // The phone sends it: Slack's copy goes.
    mock.delete_draft("Dr0PHONE", None).await.unwrap();
    cmd.send(Command::Refresh).await.unwrap();

    cmd.send(Command::Open(general.clone())).await.unwrap();
    let mut came_back = false;
    wait_for(&mut rx, |ev| match ev {
        Event::Draft { channel, .. } if *channel == general => {
            came_back = true;
            None
        }
        Event::Messages { channel, .. } if *channel == general => Some(()),
        _ => None,
    })
    .await
    .expect("the conversation opens");
    assert!(
        !came_back,
        "words already sent on the phone, back in the composer"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn words_typed_here_are_not_lost_to_a_race_with_the_phone() {
    let mock = Arc::new(MockBackend::new());
    let (cmd, mut rx) = engine_over(mock.clone());
    let (eng, general) = ids(&mut rx).await;
    settle(&cmd, &mut rx, &eng).await;

    // The phone lets go of #general's draft while it is being edited here.
    mock.delete_draft("Dr0PHONE", None).await.unwrap();
    set_draft(&cmd, &general, "mine now").await;
    settle(&cmd, &mut rx, &eng).await;

    let there = at_slack(&mock, &general);
    assert_eq!(there.len(), 1, "saved again rather than dropped");
    assert_eq!(there[0].1, "mine now");
    assert_ne!(
        there[0].0, "Dr0PHONE",
        "as a new draft: the old one is gone"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn read_only_keeps_a_draft_here_and_writes_nothing_there() {
    let mock = Arc::new(MockBackend::new());
    let (cmd, mut rx) = engine_over(ReadOnly::wrap(mock.clone() as Arc<dyn SlackBackend>));
    let (eng, general) = ids(&mut rx).await;
    set_draft(&cmd, &eng, "not for Slack").await;
    settle(&cmd, &mut rx, &general).await;
    assert!(at_slack(&mock, &eng).is_empty());
    assert_eq!(mock.draft_saves(), 0);

    // Reading is still reading: the phone's draft comes down.
    cmd.send(Command::Open(general.clone())).await.unwrap();
    let text = wait_for(&mut rx, |ev| match ev {
        Event::Draft { channel, text, .. } if *channel == general => Some(text.clone()),
        _ => None,
    })
    .await;
    assert_eq!(text.as_deref(), Some("half a thought from the phone"));
}

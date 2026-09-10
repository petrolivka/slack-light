//! Retention: what goes, what stays, and what must never go.
//!
//! The cache is someone's employer's messages on their disk. Deleting too much
//! costs a re-fetch; deleting the wrong thing loses what they said mattered.

use slk_core::{Author, ChannelId, Delivery, Message, TeamId, Ts};
use slk_store::Store;

fn msg(ch: &str, secs: i64, pinned: bool, saved: bool) -> (Message, String) {
    let m = Message {
        team: TeamId::from("T1"),
        channel: ChannelId::from(ch),
        ts: Ts::new(format!("{secs}.000100")),
        thread_ts: None,
        author: Author::User(slk_core::UserId::from("U1")),
        author_name: None,
        subtype: None,
        text: format!("message at {secs}"),
        body: Default::default(),
        blocks: Vec::new(),
        attachments: Vec::new(),
        files: Vec::new(),
        reactions: Vec::new(),
        reply_count: 0,
        reply_users: Vec::new(),
        latest_reply: None,
        subscribed: false,
        edited: false,
        pinned,
        saved,
        deleted: false,
        delivery: Delivery::Confirmed,
        raw: "{}".into(),
    };
    (m, "{}".into())
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn count(store: &Store) -> i64 {
    store.message_count().unwrap()
}

#[test]
fn age_removes_the_old_and_keeps_the_recent() {
    let mut s = Store::open(None).unwrap();
    let n = now();
    s.upsert_messages(&[
        msg("C1", n - 86_400 * 200, false, false),
        msg("C1", n - 86_400 * 100, false, false),
        msg("C1", n - 86_400 * 3, false, false),
    ])
    .unwrap();
    assert_eq!(count(&s), 3);
    s.trim(0, 90).unwrap();
    assert_eq!(
        count(&s),
        1,
        "only the three-day-old message should survive"
    );
}

#[test]
fn starred_and_pinned_survive_any_age() {
    let mut s = Store::open(None).unwrap();
    let n = now();
    s.upsert_messages(&[
        msg("C1", n - 86_400 * 500, true, false),
        msg("C1", n - 86_400 * 500 + 1, false, true),
        msg("C1", n - 86_400 * 500 + 2, false, false),
    ])
    .unwrap();
    s.trim(0, 90).unwrap();
    assert_eq!(
        count(&s),
        2,
        "a pin and a save outlive the retention window"
    );
}

#[test]
fn the_count_limit_is_per_channel() {
    let mut s = Store::open(None).unwrap();
    let n = now();
    let mut all = Vec::new();
    for i in 0..10 {
        all.push(msg("C1", n - i, false, false));
    }
    // A quiet channel must not be evicted by a noisy one.
    all.push(msg("C2", n - 50, false, false));
    s.upsert_messages(&all).unwrap();
    s.trim(4, 0).unwrap();
    assert_eq!(
        count(&s),
        5,
        "four from the busy channel, one from the quiet"
    );
}

#[test]
fn the_count_limit_keeps_the_newest() {
    let mut s = Store::open(None).unwrap();
    let n = now();
    let all: Vec<_> = (0..6)
        .map(|i| msg("C1", n - i * 10, false, false))
        .collect();
    s.upsert_messages(&all).unwrap();
    s.trim(2, 0).unwrap();
    // Asserted through `stats`, not by reading the messages back: these rows
    // carry a stub `raw_json`, and the read path rebuilds a message from it.
    let st = s.stats().unwrap();
    assert_eq!(st.messages, 2);
    assert_eq!(
        st.oldest.as_deref(),
        Some(format!("{}.000100", n - 10).as_str()),
        "the two kept must be the newest two"
    );
}

#[test]
fn trimming_an_empty_store_is_not_an_error() {
    let mut s = Store::open(None).unwrap();
    assert_eq!(s.trim(100, 30).unwrap(), 0);
    s.vacuum().unwrap();
}

#[test]
fn stats_counts_what_is_there() {
    let mut s = Store::open(None).unwrap();
    let n = now();
    s.upsert_messages(&[msg("C1", n, false, false), msg("C2", n, false, false)])
        .unwrap();
    let st = s.stats().unwrap();
    assert_eq!(st.messages, 2);
    assert!(st.oldest.is_some());
}

/// A draft in a thread is not the same draft as one in its channel.
///
/// Restoring the wrong one into the wrong composer is how a reply meant for
/// three people lands in `#general`, so the key is (channel, thread) and the
/// empty thread is a real value rather than a missing one.
#[test]
fn drafts_are_kept_per_thread_as_well_as_per_conversation() {
    let store = slk_store::Store::open(None).expect("in-memory");
    let team = slk_core::TeamId::new("T1");
    let ch = slk_core::ChannelId::new("C1");
    let parent = slk_core::Ts::new("1725701900.000100");

    store
        .set_draft(&team, &ch, None, "for the channel")
        .unwrap();
    store
        .set_draft(&team, &ch, Some(&parent), "for the thread")
        .unwrap();

    assert_eq!(
        store.draft(&team, &ch, None).unwrap().as_deref(),
        Some("for the channel")
    );
    assert_eq!(
        store.draft(&team, &ch, Some(&parent)).unwrap().as_deref(),
        Some("for the thread")
    );

    // Sending clears one and leaves the other.
    store.set_draft(&team, &ch, None, "   ").unwrap();
    assert_eq!(store.draft(&team, &ch, None).unwrap(), None);
    assert_eq!(
        store.draft(&team, &ch, Some(&parent)).unwrap().as_deref(),
        Some("for the thread")
    );
}

/// The demo workspace must leave nothing in a real cache.
///
/// `--anonymous` shared the store until this was found: a client that had
/// ever been run against the demo carried 174 fabricated messages in its
/// real cache, where `cache stats` counted them and `slack-light unread`
/// would have summed their badges. The leak is fixed at the source; this is
/// the mop, and it has to be safe to run on every start.
#[test]
fn demo_rows_are_swept_out_and_real_ones_are_not() {
    let mut store = slk_store::Store::open(None).expect("in-memory");
    let real = slk_core::TeamId::new("T0C099XGRMF");
    let demo = slk_core::TeamId::new("T0MOCK");
    let ch = slk_core::ChannelId::new("C1");

    let msg = |team: &slk_core::TeamId, ts: &str, text: &str| {
        let raw = serde_json::json!({
            "type": "message", "user": "U1", "ts": ts, "text": text
        });
        let m = slk_core::Message::parse(team, &ch, &slk_core::UserId::new("U2"), &raw)
            .expect("a message");
        (m, raw.to_string())
    };
    store
        .upsert_messages(&[
            msg(&real, "1788854344.000100", "a real one"),
            msg(&demo, "1725701100.000100", "a demo one"),
        ])
        .unwrap();

    let gone = store.forget_demo().unwrap();
    assert!(gone >= 1, "the demo's message should have been removed");

    let left = store
        .latest_messages(&real, &ch, &slk_core::UserId::new("U2"), 50)
        .unwrap();
    assert_eq!(left.len(), 1, "the real workspace keeps its messages");
    assert_eq!(left[0].text, "a real one");
    assert!(
        store
            .latest_messages(&demo, &ch, &slk_core::UserId::new("U2"), 50)
            .unwrap()
            .is_empty(),
        "and the demo keeps none"
    );
    // Search goes through the FTS index, which is content-backed by `message`:
    // a delete that went around it would leave the index pointing at rows
    // that are no longer there.
    assert!(store.search("demo", 10).unwrap().is_empty());
    // Idempotent, because it runs on every start.
    assert_eq!(store.forget_demo().unwrap(), 0);
}

/// A table nobody writes is a table whose readers are wrong.
///
/// `workspace` was in the schema from M1 and nothing ever inserted into it,
/// so `teams()` answered "none" and `slack-light unread` reading the cache
/// with no client running always reported zero — however much was unread.
#[test]
fn a_workspace_is_recorded_so_the_cache_can_be_read_without_a_client() {
    let store = slk_store::Store::open(None).expect("in-memory");
    assert!(store.teams().unwrap().is_empty());

    let team = slk_core::TeamId::new("T0C099XGRMF");
    store
        .remember_workspace(
            &team,
            "slk-dev",
            "slk-dev",
            &slk_core::UserId::new("U0BV61H04S3"),
            "session",
        )
        .unwrap();
    assert_eq!(store.teams().unwrap(), vec![team.clone()]);

    // Booting again updates rather than duplicates.
    store
        .remember_workspace(
            &team,
            "slk-dev renamed",
            "slk-dev",
            &slk_core::UserId::new("U0BV61H04S3"),
            "session",
        )
        .unwrap();
    assert_eq!(store.teams().unwrap().len(), 1);
}

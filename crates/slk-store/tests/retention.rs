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

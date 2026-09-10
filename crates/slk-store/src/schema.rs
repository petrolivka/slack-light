//! The schema, and the rule that makes it survivable.
//!
//! Every message, conversation and user keeps its `raw_json`. That looks
//! wasteful — M0 measured about 830 bytes a message — and it buys the one
//! thing this project needs most: when a parser turns out to be wrong about an
//! undocumented shape, the fix is a migration over data we already hold rather
//! than a re-fetch of everyone's history.

pub const VERSION: i64 = 5;

/// Steps from an older store to the current one, applied in order.
///
/// The version check exists so a schema change cannot silently meet a database
/// that predates it. During M1 a column was added without bumping the version,
/// and the result was a client that started, connected, and showed "no
/// conversations" with the reason only in a log nobody had enabled. Adding the
/// step is cheaper than diagnosing that twice.
pub const MIGRATIONS: &[(i64, &str)] = &[
    (2, "ALTER TABLE conversation ADD COLUMN peer TEXT;"),
    (
        3,
        "CREATE TABLE IF NOT EXISTS outbox (team TEXT, local_id TEXT, channel TEXT,
                                            thread_ts TEXT NOT NULL DEFAULT '',
                                            text TEXT, broadcast INTEGER, queued_at INTEGER,
                                            PRIMARY KEY (team, local_id));",
    ),
    (
        4,
        "CREATE TABLE IF NOT EXISTS bookmark (team TEXT, channel TEXT, id TEXT,
                                              title TEXT, link TEXT, emoji TEXT, pos INTEGER,
                                              PRIMARY KEY (team, channel, id));",
    ),
    (
        5,
        "CREATE TABLE IF NOT EXISTS section (team TEXT, id TEXT, name TEXT, emoji TEXT,
                                             kind TEXT, pos INTEGER, channels TEXT,
                                             PRIMARY KEY (team, id));",
    ),
];

pub const SCHEMA: &str = r#"
CREATE TABLE workspace (team TEXT PRIMARY KEY, name TEXT, domain TEXT, self_id TEXT,
                        backend TEXT, booted_at INTEGER);

CREATE TABLE conversation (team TEXT, id TEXT, kind TEXT, name TEXT, topic TEXT, purpose TEXT,
                           is_member INTEGER, is_archived INTEGER, is_starred INTEGER, is_muted INTEGER,
                           is_shared INTEGER, member_count INTEGER, peer TEXT,
                           last_read TEXT, latest TEXT, unread INTEGER, mentions INTEGER,
                           notify TEXT, raw_json TEXT, updated_at INTEGER,
                           PRIMARY KEY (team, id));

CREATE TABLE user (team TEXT, id TEXT, name TEXT, display_name TEXT, real_name TEXT, title TEXT,
                   tz TEXT, is_bot INTEGER, is_deleted INTEGER, is_external INTEGER,
                   status_text TEXT, status_emoji TEXT, presence TEXT,
                   raw_json TEXT, updated_at INTEGER,
                   PRIMARY KEY (team, id));

CREATE TABLE message (team TEXT, channel TEXT, ts TEXT, thread_ts TEXT,
                      author_kind TEXT, author_id TEXT, subtype TEXT, text TEXT,
                      raw_json TEXT NOT NULL,
                      reply_count INTEGER, latest_reply TEXT, edited INTEGER,
                      pinned INTEGER, saved INTEGER, deleted INTEGER DEFAULT 0,
                      local_id TEXT, delivery TEXT,
                      PRIMARY KEY (team, channel, ts));
CREATE INDEX message_thread ON message (team, channel, thread_ts, ts);
CREATE INDEX message_author ON message (team, author_id, ts);

-- Which stretches of a channel's history we hold contiguously. The gaps are the
-- complement, and knowing where they are is what makes a reconnect honest
-- rather than hopeful.
CREATE TABLE history_span (team TEXT, channel TEXT, oldest TEXT, newest TEXT,
                           PRIMARY KEY (team, channel, oldest));

CREATE TABLE draft (team TEXT, channel TEXT, thread_ts TEXT NOT NULL DEFAULT '',
                    text TEXT, updated_at INTEGER,
                    PRIMARY KEY (team, channel, thread_ts));

-- A conversation's bookmark bar, cached so it is on screen with the first
-- frame rather than a round trip later. `pos` is Slack's own order: a bar
-- whose rows move between openings is a bar people stop aiming at.
CREATE TABLE bookmark (team TEXT, channel TEXT, id TEXT,
                       title TEXT, link TEXT, emoji TEXT, pos INTEGER,
                       PRIMARY KEY (team, channel, id));

-- The person's own Slack sidebar sections, cached so the first frame is
-- already arranged the way they arranged it. `channels` is comma-joined:
-- a channel id never contains a comma, and a join table for a list that is
-- only ever read whole would be ceremony.
CREATE TABLE section (team TEXT, id TEXT, name TEXT, emoji TEXT,
                      kind TEXT, pos INTEGER, channels TEXT,
                      PRIMARY KEY (team, id));

CREATE TABLE kv (key TEXT PRIMARY KEY, value TEXT);

-- Messages typed while the connection was down. Held on disk rather than in
-- memory: the point of queuing is that the message is not lost, and a client
-- that loses it to a crash has kept the promise only in the easy case.
-- `queued_at` is the whole order guarantee — FR-H9 says "in order", and out
-- of order is worse than late in a conversation.
CREATE TABLE outbox (team TEXT, local_id TEXT, channel TEXT,
                     thread_ts TEXT NOT NULL DEFAULT '',
                     text TEXT, broadcast INTEGER, queued_at INTEGER,
                     PRIMARY KEY (team, local_id));

CREATE VIRTUAL TABLE message_fts USING fts5(text, content='message', content_rowid='rowid',
                                            tokenize='unicode61 remove_diacritics 2');

CREATE TRIGGER message_ai AFTER INSERT ON message BEGIN
  INSERT INTO message_fts(rowid, text) VALUES (new.rowid, new.text);
END;
CREATE TRIGGER message_ad AFTER DELETE ON message BEGIN
  INSERT INTO message_fts(message_fts, rowid, text) VALUES ('delete', old.rowid, old.text);
END;
CREATE TRIGGER message_au AFTER UPDATE ON message BEGIN
  INSERT INTO message_fts(message_fts, rowid, text) VALUES ('delete', old.rowid, old.text);
  INSERT INTO message_fts(rowid, text) VALUES (new.rowid, new.text);
END;
"#;

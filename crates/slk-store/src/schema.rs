//! The schema, and the rule that makes it survivable.
//!
//! Every message, conversation and user keeps its `raw_json`. That looks
//! wasteful — M0 measured about 830 bytes a message — and it buys the one
//! thing this project needs most: when a parser turns out to be wrong about an
//! undocumented shape, the fix is a migration over data we already hold rather
//! than a re-fetch of everyone's history.

pub const VERSION: i64 = 2;

/// Steps from an older store to the current one, applied in order.
///
/// The version check exists so a schema change cannot silently meet a database
/// that predates it. During M1 a column was added without bumping the version,
/// and the result was a client that started, connected, and showed "no
/// conversations" with the reason only in a log nobody had enabled. Adding the
/// step is cheaper than diagnosing that twice.
pub const MIGRATIONS: &[(i64, &str)] = &[(2, "ALTER TABLE conversation ADD COLUMN peer TEXT;")];

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

CREATE TABLE kv (key TEXT PRIMARY KEY, value TEXT);

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

//! The local message cache.
//!
//! Its job is to make the client usable in the first hundred milliseconds and
//! to keep it usable when the network is not. Everything the sidebar and the
//! open conversation need is read from here first and verified from Slack
//! afterwards, which is also what keeps the request pattern looking like one
//! person reading rather than a scraper.
//!
//! One connection, one writer, WAL mode. Callers are expected to be off the UI
//! thread; nothing here is async, because SQLite is not, and pretending
//! otherwise only hides where the blocking happens.

mod schema;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use slk_core::{ChannelId, Conversation, Delivery, Message, TeamId, Ts, User, UserId};
use std::path::Path;

pub struct Store {
    db: Connection,
}

impl Store {
    /// Open or create the store. `None` is an in-memory database, which is what
    /// `--no-cache` and every test use.
    pub fn open(path: Option<&Path>) -> Result<Self> {
        let db = match path {
            Some(p) => {
                if let Some(dir) = p.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                let db = Connection::open(p).with_context(|| format!("opening {}", p.display()))?;
                // The cache holds the user's employer's messages. Nobody else
                // on the machine has any business reading it.
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600));
                }
                db
            }
            None => Connection::open_in_memory()?,
        };
        db.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;

        let version: i64 = db
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap_or(0);
        if version == 0 {
            db.execute_batch(schema::SCHEMA)?;
            db.pragma_update(None, "user_version", schema::VERSION)?;
        } else if version < schema::VERSION {
            for (to, sql) in schema::MIGRATIONS {
                if *to > version {
                    db.execute_batch(sql)
                        .with_context(|| format!("migrating the store to version {to}"))?;
                }
            }
            db.pragma_update(None, "user_version", schema::VERSION)?;
            tracing::info!(
                "migrated the store from version {version} to {}",
                schema::VERSION
            );
        } else if version > schema::VERSION {
            // A newer build wrote this. Reading it with older code would
            // misinterpret columns rather than fail, which is worse.
            anyhow::bail!(
                "the message cache was written by a newer slack-light (version {version}, this \
                 build understands {}). Use the newer build, or delete the cache.",
                schema::VERSION
            );
        }
        Ok(Store { db })
    }

    // ---- conversations -------------------------------------------------

    pub fn upsert_conversations(&mut self, convs: &[Conversation]) -> Result<()> {
        let tx = self.db.transaction()?;
        {
            let mut st = tx.prepare(
                "INSERT INTO conversation
                   (team, id, kind, name, topic, purpose, is_member, is_archived, is_starred,
                    is_muted, is_shared, member_count, peer, last_read, latest, unread, mentions,
                    notify, raw_json, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)
                 ON CONFLICT(team, id) DO UPDATE SET
                   kind=excluded.kind, name=excluded.name, topic=excluded.topic,
                   purpose=excluded.purpose, is_member=excluded.is_member,
                   is_archived=excluded.is_archived, is_starred=excluded.is_starred,
                   is_muted=excluded.is_muted, is_shared=excluded.is_shared,
                   member_count=excluded.member_count, peer=excluded.peer,
                   -- last_read and the counters only ever move forward from a
                   -- fresher source; a stale boot response must not undo a mark
                   -- that already arrived over the websocket.
                   last_read=coalesce(excluded.last_read, conversation.last_read),
                   latest=coalesce(excluded.latest, conversation.latest),
                   unread=excluded.unread, mentions=excluded.mentions,
                   raw_json=excluded.raw_json, updated_at=excluded.updated_at",
            )?;
            for c in convs {
                st.execute(params![
                    c.team.as_str(),
                    c.id.as_str(),
                    format!("{:?}", c.kind),
                    c.name,
                    c.topic,
                    c.purpose,
                    c.is_member,
                    c.is_archived,
                    c.is_starred,
                    c.is_muted,
                    c.is_shared,
                    c.member_count,
                    match &c.kind {
                        slk_core::ConversationKind::Dm { peer } => Some(peer.as_str()),
                        _ => None,
                    },
                    c.last_read.as_ref().map(Ts::as_str),
                    c.latest.as_ref().map(Ts::as_str),
                    c.unread,
                    c.mentions,
                    format!("{:?}", c.notify),
                    "",
                    now(),
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Every conversation we know about, newest activity first, for the sidebar.
    pub fn conversations(&self, team: &TeamId) -> Result<Vec<StoredConversation>> {
        let mut st = self.db.prepare(
            "SELECT id, kind, name, topic, is_member, is_muted, last_read, latest, unread,
                    mentions, peer, is_starred, team, coalesce(purpose, ''), member_count
             FROM conversation WHERE team = ?1 AND is_archived = 0
             ORDER BY (latest IS NULL), latest DESC, name",
        )?;
        let rows = st
            .query_map(params![team.as_str()], |r| {
                Ok(StoredConversation {
                    id: ChannelId::new(r.get::<_, String>(0)?),
                    kind: r.get::<_, String>(1)?,
                    name: r.get::<_, String>(2)?,
                    topic: r.get::<_, String>(3)?,
                    is_member: r.get(4)?,
                    is_muted: r.get(5)?,
                    last_read: r.get::<_, Option<String>>(6)?.map(Ts::new),
                    latest: r.get::<_, Option<String>>(7)?.map(Ts::new),
                    unread: r.get(8)?,
                    mentions: r.get(9)?,
                    peer: r.get::<_, Option<String>>(10)?.map(UserId::new),
                    is_starred: r.get(11)?,
                    team: TeamId::new(r.get::<_, String>(12)?),
                    purpose: r.get::<_, String>(13)?,
                    member_count: r.get::<_, Option<u32>>(14)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Apply a counts response.
    ///
    /// Only the counting fields move. An earlier version rebuilt whole
    /// `Conversation`s from counts and wrote them back, which silently reset
    /// every direct message to a nameless public channel — counts say nothing
    /// about what a conversation *is*.
    pub fn apply_counts(&mut self, team: &TeamId, entries: &[CountUpdate]) -> Result<()> {
        let tx = self.db.transaction()?;
        {
            let mut st = tx.prepare(
                "UPDATE conversation
                 SET last_read = coalesce(?3, last_read),
                     latest    = coalesce(?4, latest),
                     unread    = ?5,
                     mentions  = ?6
                 WHERE team = ?1 AND id = ?2",
            )?;
            for e in entries {
                st.execute(params![
                    team.as_str(),
                    e.id.as_str(),
                    e.last_read.as_ref().map(Ts::as_str),
                    e.latest.as_ref().map(Ts::as_str),
                    e.unread,
                    e.mentions,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn set_last_read(&self, team: &TeamId, ch: &ChannelId, ts: &Ts) -> Result<()> {
        self.db.execute(
            "UPDATE conversation SET last_read = ?3, unread = 0, mentions = 0
             WHERE team = ?1 AND id = ?2",
            params![team.as_str(), ch.as_str(), ts.as_str()],
        )?;
        Ok(())
    }

    /// Recount unread from what we actually hold, which is only meaningful when
    /// the span from `last_read` to the newest message is complete. The caller
    /// decides that; this just does the arithmetic.
    pub fn recount_unread(&self, team: &TeamId, ch: &ChannelId, self_id: &UserId) -> Result<u32> {
        let n: i64 = self.db.query_row(
            "SELECT count(*) FROM message m
             JOIN conversation c ON c.team = m.team AND c.id = m.channel
             WHERE m.team = ?1 AND m.channel = ?2 AND m.deleted = 0
               AND m.author_id != ?3
               AND (c.last_read IS NULL OR m.ts > c.last_read)",
            params![team.as_str(), ch.as_str(), self_id.as_str()],
            |r| r.get(0),
        )?;
        Ok(n.max(0) as u32)
    }

    // ---- users ---------------------------------------------------------

    pub fn upsert_users(&mut self, users: &[User]) -> Result<()> {
        let tx = self.db.transaction()?;
        {
            let mut st = tx.prepare(
                "INSERT INTO user (team, id, name, display_name, real_name, title, tz, is_bot,
                                   is_deleted, is_external, status_text, status_emoji, presence,
                                   raw_json, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
                 ON CONFLICT(team, id) DO UPDATE SET
                   name=excluded.name, display_name=excluded.display_name,
                   real_name=excluded.real_name, title=excluded.title, tz=excluded.tz,
                   is_bot=excluded.is_bot, is_deleted=excluded.is_deleted,
                   is_external=excluded.is_external, status_text=excluded.status_text,
                   status_emoji=excluded.status_emoji, updated_at=excluded.updated_at",
            )?;
            for u in users {
                st.execute(params![
                    u.team.as_str(),
                    u.id.as_str(),
                    u.name,
                    u.display_name,
                    u.real_name,
                    u.title,
                    u.tz,
                    u.is_bot,
                    u.is_deleted,
                    u.is_external,
                    u.status.text,
                    u.status.emoji,
                    format!("{:?}", u.presence),
                    "",
                    now(),
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Record a presence change. Presence is per-session rather than durable,
    /// but keeping it here means the sidebar is not blank for a second after
    /// every restart.
    pub fn set_presence(&self, team: &TeamId, ids: &[UserId], presence: &str) -> Result<()> {
        for id in ids {
            self.db.execute(
                "UPDATE user SET presence = ?3 WHERE team = ?1 AND id = ?2",
                params![team.as_str(), id.as_str(), presence],
            )?;
        }
        Ok(())
    }

    /// The id-to-label map the renderer needs. Small enough to hold whole.
    /// Every name in the directory, with the two facts a reader needs about
    /// the person as well as their name: whether they are from outside this
    /// workspace, and whether their account is gone.
    pub fn user_labels(&self, team: &TeamId) -> Result<Vec<UserLabel>> {
        let mut st = self.db.prepare(
            "SELECT id, CASE WHEN display_name != '' THEN display_name
                             WHEN real_name != '' THEN real_name
                             ELSE name END,
                    coalesce(is_external, 0), coalesce(is_deleted, 0)
             FROM user WHERE team = ?1",
        )?;
        let rows = st
            .query_map(params![team.as_str()], |r| {
                Ok(UserLabel {
                    id: UserId::new(r.get::<_, String>(0)?),
                    label: r.get(1)?,
                    external: r.get(2)?,
                    deactivated: r.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---- messages ------------------------------------------------------

    pub fn upsert_messages(&mut self, msgs: &[(Message, String)]) -> Result<()> {
        let tx = self.db.transaction()?;
        {
            let mut st = tx.prepare(
                "INSERT INTO message (team, channel, ts, thread_ts, author_kind, author_id,
                                      subtype, text, raw_json, reply_count, latest_reply,
                                      edited, pinned, saved, deleted, local_id, delivery)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)
                 ON CONFLICT(team, channel, ts) DO UPDATE SET
                   thread_ts=excluded.thread_ts, text=excluded.text, raw_json=excluded.raw_json,
                   reply_count=max(excluded.reply_count, message.reply_count),
                   latest_reply=coalesce(excluded.latest_reply, message.latest_reply),
                   edited=excluded.edited, pinned=excluded.pinned, saved=excluded.saved,
                   deleted=excluded.deleted, delivery=excluded.delivery",
            )?;
            for (m, raw) in msgs {
                let (kind, id) = match &m.author {
                    slk_core::Author::User(u) => ("user", u.as_str().to_string()),
                    slk_core::Author::Bot { id, .. } => ("bot", id.as_str().to_string()),
                    slk_core::Author::System => ("system", String::new()),
                };
                let (delivery, local) = match &m.delivery {
                    Delivery::Confirmed => ("confirmed", None),
                    Delivery::Pending(id) => ("pending", Some(id.clone())),
                    Delivery::Failed(e) => ("failed", Some(e.clone())),
                };
                st.execute(params![
                    m.team.as_str(),
                    m.channel.as_str(),
                    m.ts.as_str(),
                    m.thread_ts.as_ref().map(Ts::as_str),
                    kind,
                    id,
                    m.subtype,
                    m.text,
                    raw,
                    m.reply_count,
                    m.latest_reply.as_ref().map(Ts::as_str),
                    m.edited,
                    m.pinned,
                    m.saved,
                    m.deleted,
                    local,
                    delivery,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The newest `limit` messages in a channel, oldest first so the caller can
    /// append them straight into a view.
    pub fn latest_messages(
        &self,
        team: &TeamId,
        ch: &ChannelId,
        self_id: &UserId,
        limit: usize,
    ) -> Result<Vec<Message>> {
        let mut st = self.db.prepare(
            "SELECT raw_json FROM message
             WHERE team = ?1 AND channel = ?2 AND deleted = 0 AND thread_ts IS NULL OR
                   (team = ?1 AND channel = ?2 AND deleted = 0 AND thread_ts = ts)
             ORDER BY ts DESC LIMIT ?3",
        )?;
        let mut out = self.rows_to_messages(
            &mut st,
            team,
            ch,
            self_id,
            params![team.as_str(), ch.as_str(), limit as i64],
        )?;
        out.reverse();
        Ok(out)
    }

    /// The page of messages immediately older than `before`, for scrollback.
    pub fn messages_before(
        &self,
        team: &TeamId,
        ch: &ChannelId,
        self_id: &UserId,
        before: &Ts,
        limit: usize,
    ) -> Result<Vec<Message>> {
        let mut st = self.db.prepare(
            "SELECT raw_json FROM message
             WHERE team = ?1 AND channel = ?2 AND deleted = 0 AND ts < ?3
               AND (thread_ts IS NULL OR thread_ts = ts)
             ORDER BY ts DESC LIMIT ?4",
        )?;
        let mut out = self.rows_to_messages(
            &mut st,
            team,
            ch,
            self_id,
            params![team.as_str(), ch.as_str(), before.as_str(), limit as i64],
        )?;
        out.reverse();
        Ok(out)
    }

    /// A thread: its parent and every reply, oldest first.
    pub fn thread(
        &self,
        team: &TeamId,
        ch: &ChannelId,
        self_id: &UserId,
        parent: &Ts,
    ) -> Result<Vec<Message>> {
        let mut st = self.db.prepare(
            "SELECT raw_json FROM message
             WHERE team = ?1 AND channel = ?2 AND deleted = 0
               AND (ts = ?3 OR thread_ts = ?3)
             ORDER BY ts",
        )?;
        self.rows_to_messages(
            &mut st,
            team,
            ch,
            self_id,
            params![team.as_str(), ch.as_str(), parent.as_str()],
        )
    }

    fn rows_to_messages(
        &self,
        st: &mut rusqlite::Statement<'_>,
        team: &TeamId,
        ch: &ChannelId,
        self_id: &UserId,
        p: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<Message>> {
        let raws: Vec<String> = st
            .query_map(p, |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(raws
            .iter()
            .filter_map(|raw| {
                let v: serde_json::Value = serde_json::from_str(raw).ok()?;
                Message::parse(team, ch, self_id, &v)
            })
            .collect())
    }

    /// Threads with replies, newest activity first.
    ///
    /// From what is held rather than from an endpoint, so it works offline and
    /// needs no undocumented method. The cost is that it only knows the
    /// threads whose parents have been loaded, which is what the view says.
    pub fn threads(
        &self,
        team: &TeamId,
        limit: usize,
    ) -> Result<Vec<(ChannelId, Ts, String, u32)>> {
        let mut st = self.db.prepare(
            "SELECT channel, ts, text, reply_count FROM message
             WHERE team = ?1 AND deleted = 0 AND reply_count > 0
             ORDER BY coalesce(latest_reply, ts) DESC LIMIT ?2",
        )?;
        let rows = st
            .query_map(params![team.as_str(), limit as i64], |r| {
                Ok((
                    ChannelId::new(r.get::<_, String>(0)?),
                    Ts::new(r.get::<_, String>(1)?),
                    r.get(2)?,
                    r.get::<_, i64>(3)? as u32,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Messages kept for later, newest first.
    pub fn saved(&self, team: &TeamId, limit: usize) -> Result<Vec<(ChannelId, Ts, String)>> {
        let mut st = self.db.prepare(
            "SELECT channel, ts, text FROM message
             WHERE team = ?1 AND deleted = 0 AND saved = 1
             ORDER BY ts DESC LIMIT ?2",
        )?;
        let rows = st
            .query_map(params![team.as_str(), limit as i64], |r| {
                Ok((
                    ChannelId::new(r.get::<_, String>(0)?),
                    Ts::new(r.get::<_, String>(1)?),
                    r.get(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// What is pinned in one conversation, newest first.
    ///
    /// Scoped to a conversation because that is what a pin is: Slack has no
    /// workspace-wide pinned list, and a flat one across channels would read
    /// as a second Saved list rather than as this channel's notice board.
    pub fn pinned(&self, team: &TeamId, ch: &ChannelId, limit: usize) -> Result<Vec<(Ts, String)>> {
        let mut st = self.db.prepare(
            "SELECT ts, text FROM message
             WHERE team = ?1 AND channel = ?2 AND deleted = 0 AND pinned = 1
             ORDER BY ts DESC LIMIT ?3",
        )?;
        let rows = st
            .query_map(params![team.as_str(), ch.as_str(), limit as i64], |r| {
                Ok((Ts::new(r.get::<_, String>(0)?), r.get(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Reconcile the pin flags in one conversation against what Slack says.
    ///
    /// A whole-channel reconcile rather than a set of individual flips, so a
    /// pin somebody else removed while we were away disappears here too.
    /// Returns how many of the given timestamps we do not actually hold —
    /// pins older than the cache, which the interface reports rather than
    /// silently omitting.
    pub fn set_pinned(&self, team: &TeamId, ch: &ChannelId, all: &[Ts]) -> Result<usize> {
        self.db.execute(
            "UPDATE message SET pinned = 0 WHERE team = ?1 AND channel = ?2 AND pinned = 1",
            params![team.as_str(), ch.as_str()],
        )?;
        let mut missing = 0;
        for ts in all {
            let n = self.db.execute(
                "UPDATE message SET pinned = 1 WHERE team = ?1 AND channel = ?2 AND ts = ?3",
                params![team.as_str(), ch.as_str(), ts.as_str()],
            )?;
            if n == 0 {
                missing += 1;
            }
        }
        Ok(missing)
    }

    // ---- the outbox ----------------------------------------------------

    /// Hold a message that could not be sent. Idempotent on `local_id`, so a
    /// retry that fails again does not queue it twice.
    pub fn enqueue(
        &self,
        team: &TeamId,
        local_id: &str,
        ch: &ChannelId,
        thread: Option<&Ts>,
        text: &str,
        broadcast: bool,
    ) -> Result<()> {
        self.db.execute(
            "INSERT INTO outbox (team, local_id, channel, thread_ts, text, broadcast, queued_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(team, local_id) DO NOTHING",
            params![
                team.as_str(),
                local_id,
                ch.as_str(),
                thread.map(Ts::as_str).unwrap_or(""),
                text,
                broadcast,
                now(),
            ],
        )?;
        Ok(())
    }

    /// Everything waiting, oldest first. Order is the promise.
    pub fn outbox(&self, team: &TeamId) -> Result<Vec<Queued>> {
        let mut st = self.db.prepare(
            "SELECT local_id, channel, thread_ts, text, broadcast FROM outbox
             WHERE team = ?1 ORDER BY queued_at, rowid",
        )?;
        let rows = st
            .query_map(params![team.as_str()], |r| {
                let thread: String = r.get(2)?;
                Ok(Queued {
                    local_id: r.get(0)?,
                    channel: ChannelId::new(r.get::<_, String>(1)?),
                    thread: (!thread.is_empty()).then(|| Ts::new(thread)),
                    text: r.get(3)?,
                    broadcast: r.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn dequeue(&self, team: &TeamId, local_id: &str) -> Result<()> {
        self.db.execute(
            "DELETE FROM outbox WHERE team = ?1 AND local_id = ?2",
            params![team.as_str(), local_id],
        )?;
        Ok(())
    }

    // ---- conversation flags --------------------------------------------

    pub fn set_starred(&self, team: &TeamId, ch: &ChannelId, on: bool) -> Result<()> {
        self.db.execute(
            "UPDATE conversation SET is_starred = ?3 WHERE team = ?1 AND id = ?2",
            params![team.as_str(), ch.as_str(), on],
        )?;
        Ok(())
    }

    pub fn set_muted(&self, team: &TeamId, ch: &ChannelId, on: bool) -> Result<()> {
        self.db.execute(
            "UPDATE conversation SET is_muted = ?3 WHERE team = ?1 AND id = ?2",
            params![team.as_str(), ch.as_str(), on],
        )?;
        Ok(())
    }

    pub fn set_membership(&self, team: &TeamId, ch: &ChannelId, member: bool) -> Result<()> {
        self.db.execute(
            "UPDATE conversation SET is_member = ?3 WHERE team = ?1 AND id = ?2",
            params![team.as_str(), ch.as_str(), member],
        )?;
        Ok(())
    }

    pub fn set_topic(&self, team: &TeamId, ch: &ChannelId, topic: &str) -> Result<()> {
        self.db.execute(
            "UPDATE conversation SET topic = ?3 WHERE team = ?1 AND id = ?2",
            params![team.as_str(), ch.as_str(), topic],
        )?;
        Ok(())
    }

    pub fn set_purpose(&self, team: &TeamId, ch: &ChannelId, purpose: &str) -> Result<()> {
        self.db.execute(
            "UPDATE conversation SET purpose = ?3 WHERE team = ?1 AND id = ?2",
            params![team.as_str(), ch.as_str(), purpose],
        )?;
        Ok(())
    }

    /// Every muted conversation, which is what `muted_channels` wants written.
    pub fn muted(&self, team: &TeamId) -> Result<Vec<ChannelId>> {
        let mut st = self
            .db
            .prepare("SELECT id FROM conversation WHERE team = ?1 AND is_muted = 1 ORDER BY id")?;
        let rows = st
            .query_map(params![team.as_str()], |r| {
                Ok(ChannelId::new(r.get::<_, String>(0)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Messages that name this user, newest first.
    ///
    /// A `LIKE` over the stored text: mentions are stored as `<@Uxxxx>`, which
    /// is an exact string and cannot appear by accident.
    pub fn mentions(
        &self,
        team: &TeamId,
        self_id: &UserId,
        limit: usize,
    ) -> Result<Vec<(ChannelId, Ts, String, String)>> {
        let needle = format!("%<@{}>%", self_id.as_str());
        let mut st = self.db.prepare(
            "SELECT channel, ts, text, author_id FROM message
             WHERE team = ?1 AND deleted = 0 AND author_id != ?2
               AND (text LIKE ?3 OR text LIKE '%<!here>%' OR text LIKE '%<!channel>%')
             ORDER BY ts DESC LIMIT ?4",
        )?;
        let rows = st
            .query_map(
                params![team.as_str(), self_id.as_str(), needle, limit as i64],
                |r| {
                    Ok((
                        ChannelId::new(r.get::<_, String>(0)?),
                        Ts::new(r.get::<_, String>(1)?),
                        r.get(2)?,
                        r.get(3)?,
                    ))
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// One user, for a profile.
    pub fn user(&self, team: &TeamId, id: &UserId) -> Result<Option<StoredUser>> {
        let mut st = self.db.prepare(
            "SELECT id, display_name, real_name, title, tz, is_bot, is_deleted,
                    status_text, status_emoji, presence
             FROM user WHERE team = ?1 AND id = ?2",
        )?;
        let mut rows = st.query_map(params![team.as_str(), id.as_str()], |r| {
            Ok(StoredUser {
                id: UserId::new(r.get::<_, String>(0)?),
                display_name: r.get(1)?,
                real_name: r.get(2)?,
                title: r.get(3)?,
                tz: r.get(4)?,
                is_bot: r.get(5)?,
                is_deleted: r.get(6)?,
                status_text: r.get(7)?,
                status_emoji: r.get(8)?,
                presence: r.get(9)?,
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    /// Local full-text search, which is the half of search that works offline
    /// and across every workspace at once.
    pub fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(TeamId, ChannelId, Ts, String)>> {
        let mut st = self.db.prepare(
            "SELECT m.team, m.channel, m.ts, m.text FROM message_fts f
             JOIN message m ON m.rowid = f.rowid
             WHERE message_fts MATCH ?1 AND m.deleted = 0
             ORDER BY rank LIMIT ?2",
        )?;
        let rows = st
            .query_map(params![query, limit as i64], |r| {
                Ok((
                    TeamId::new(r.get::<_, String>(0)?),
                    ChannelId::new(r.get::<_, String>(1)?),
                    Ts::new(r.get::<_, String>(2)?),
                    r.get(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---- drafts and session state --------------------------------------

    /// Keep what somebody typed and did not send.
    ///
    /// Per conversation *and* per thread: a reply half-written in a thread is
    /// not the same draft as one half-written in the channel, and restoring
    /// the wrong one into the wrong composer is how a thread reply ends up in
    /// `#general`.
    pub fn set_draft(
        &self,
        team: &TeamId,
        ch: &ChannelId,
        thread: Option<&Ts>,
        text: &str,
    ) -> Result<()> {
        let thread = thread.map(Ts::as_str).unwrap_or("");
        if text.trim().is_empty() {
            self.db.execute(
                "DELETE FROM draft WHERE team = ?1 AND channel = ?2 AND thread_ts = ?3",
                params![team.as_str(), ch.as_str(), thread],
            )?;
            return Ok(());
        }
        self.db.execute(
            "INSERT INTO draft (team, channel, thread_ts, text, updated_at)
             VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(team, channel, thread_ts) DO UPDATE SET
               text = excluded.text, updated_at = excluded.updated_at",
            params![team.as_str(), ch.as_str(), thread, text, now()],
        )?;
        Ok(())
    }

    pub fn draft(
        &self,
        team: &TeamId,
        ch: &ChannelId,
        thread: Option<&Ts>,
    ) -> Result<Option<String>> {
        let thread = thread.map(Ts::as_str).unwrap_or("");
        Ok(self
            .db
            .query_row(
                "SELECT text FROM draft WHERE team = ?1 AND channel = ?2 AND thread_ts = ?3",
                params![team.as_str(), ch.as_str(), thread],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn set_kv(&self, key: &str, value: &str) -> Result<()> {
        self.db.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn kv(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .db
            .query_row("SELECT value FROM kv WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn message_count(&self) -> Result<i64> {
        Ok(self
            .db
            .query_row("SELECT count(*) FROM message", [], |r| r.get(0))?)
    }

    /// What the cache holds, for `slack-light cache stats`.
    pub fn stats(&self) -> Result<Stats> {
        let one = |sql: &str| -> Result<i64> { Ok(self.db.query_row(sql, [], |r| r.get(0))?) };
        Ok(Stats {
            messages: one("SELECT count(*) FROM message")?,
            conversations: one("SELECT count(*) FROM conversation")?,
            users: one("SELECT count(*) FROM user")?,
            workspaces: one("SELECT count(*) FROM workspace")?,
            oldest: self
                .db
                .query_row("SELECT min(ts) FROM message", [], |r| {
                    r.get::<_, Option<String>>(0)
                })
                .optional()?
                .flatten(),
            bytes: one(
                "SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()",
            )
            .unwrap_or(0),
        })
    }

    /// Drop what is past the retention limits, and say how much went.
    ///
    /// Two limits, because either alone leaves a hole: age alone lets one busy
    /// channel grow without bound inside the window, and count alone keeps a
    /// dead channel's messages for ever. Anything starred or pinned survives
    /// both — someone said those mattered, which is the whole signal there is.
    ///
    /// The count limit is applied per channel rather than per store, so a
    /// quiet channel is not evicted by a noisy one.
    pub fn trim(&mut self, max_per_channel: u32, max_age_days: u32) -> Result<u64> {
        let tx = self.db.transaction()?;
        let mut gone = 0u64;

        if max_age_days > 0 {
            // Slack timestamps are seconds-since-epoch with a suffix, and they
            // compare correctly as text only because they are fixed-width. The
            // cutoff is built the same way for the same reason.
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let cutoff = now - (max_age_days as i64) * 86_400;
            gone += tx.execute(
                "DELETE FROM message
                  WHERE CAST(ts AS REAL) < ?1
                    AND coalesce(pinned, 0) = 0 AND coalesce(saved, 0) = 0",
                [cutoff],
            )? as u64;
        }

        if max_per_channel > 0 {
            gone += tx.execute(
                "DELETE FROM message WHERE rowid IN (
                     SELECT rowid FROM (
                         SELECT rowid,
                                row_number() OVER (PARTITION BY team, channel
                                                   ORDER BY ts DESC) AS n
                           FROM message
                          WHERE coalesce(pinned, 0) = 0 AND coalesce(saved, 0) = 0
                     ) WHERE n > ?1)",
                [max_per_channel],
            )? as u64;
        }

        // A span that no longer describes anything held would make the next
        // reconnect think it has history it has just deleted.
        tx.execute(
            "DELETE FROM history_span
              WHERE NOT EXISTS (SELECT 1 FROM message m
                                 WHERE m.team = history_span.team
                                   AND m.channel = history_span.channel
                                   AND m.ts >= history_span.oldest
                                   AND m.ts <= history_span.newest)",
            [],
        )?;
        tx.commit()?;
        Ok(gone)
    }

    /// Give the file back to the filesystem after a trim.
    ///
    /// Separate from `trim` because it rewrites the database and a client
    /// starting up should not wait for that.
    pub fn vacuum(&self) -> Result<()> {
        self.db.execute_batch("VACUUM")?;
        Ok(())
    }
}

/// A name from the directory, and what to say about the person beside it.
#[derive(Debug, Clone)]
pub struct UserLabel {
    pub id: UserId,
    pub label: String,
    /// A Slack Connect guest, from another workspace entirely.
    pub external: bool,
    pub deactivated: bool,
}

/// What `slack-light cache stats` prints.
#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub messages: i64,
    pub conversations: i64,
    pub users: i64,
    pub workspaces: i64,
    /// The oldest message timestamp held, as Slack's `ts`.
    pub oldest: Option<String>,
    pub bytes: i64,
}

/// A person, as a profile shows them.
#[derive(Debug, Clone)]
pub struct StoredUser {
    pub id: UserId,
    pub display_name: String,
    pub real_name: String,
    pub title: String,
    pub tz: Option<String>,
    pub is_bot: bool,
    pub is_deleted: bool,
    pub status_text: String,
    pub status_emoji: String,
    pub presence: String,
}

/// The counting fields of one conversation, and nothing else.
#[derive(Debug, Clone)]
pub struct CountUpdate {
    pub id: ChannelId,
    pub last_read: Option<Ts>,
    pub latest: Option<Ts>,
    pub unread: u32,
    pub mentions: u32,
}

/// One message waiting for the connection to come back.
#[derive(Debug, Clone)]
pub struct Queued {
    pub local_id: String,
    pub channel: ChannelId,
    pub thread: Option<Ts>,
    pub text: String,
    pub broadcast: bool,
}

/// What the sidebar needs, without paying to rebuild whole `Conversation`s.
#[derive(Debug, Clone)]
pub struct StoredConversation {
    pub id: ChannelId,
    pub kind: String,
    pub name: String,
    pub topic: String,
    pub is_member: bool,
    pub is_muted: bool,
    pub last_read: Option<Ts>,
    pub latest: Option<Ts>,
    pub unread: u32,
    pub mentions: u32,
    /// For a direct message, the other person. A DM carries no name of its
    /// own, so without this the sidebar shows a bare `#`.
    pub peer: Option<UserId>,
    pub is_starred: bool,
    /// Which workspace this belongs to. Several are connected at once, and a
    /// command sent to the wrong one is a message in the wrong company.
    pub team: TeamId,
    pub purpose: String,
    pub member_count: Option<u32>,
}

impl StoredConversation {
    pub fn is_dm(&self) -> bool {
        self.kind.starts_with("Dm") || self.kind.starts_with("Mpim")
    }
    pub fn is_private(&self) -> bool {
        self.kind.starts_with("Private")
    }
    pub fn has_unread(&self) -> bool {
        self.unread > 0 || self.mentions > 0
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

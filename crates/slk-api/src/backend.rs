//! The one interface every backend implements.
//!
//! Two real backends exist — a browser session and an official OAuth app — and
//! they differ in what they *can* do, not only in how. `Capabilities` is how
//! the UI finds that out, so it can disable an action visibly instead of
//! offering something that will fail.

use crate::error::Result;
use async_trait::async_trait;
use slk_core::{
    Bookmark, ChannelId, Conversation, FileId, Message, TeamId, Ts, User, UserId, Workspace,
};

/// What this backend can actually do. A missing capability disables the action
/// in the interface rather than hiding it, so both backends look the same and
/// behave honestly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Capabilities {
    /// `client.counts`: every unread badge in one call. Without it the client
    /// must verify conversations one at a time.
    pub counts: bool,
    pub realtime: bool,
    pub typing: bool,
    pub presence: bool,
    pub drafts: bool,
    pub saved: bool,
    pub search: bool,
}

impl Capabilities {
    /// What an installed app can do.
    ///
    /// Measured against the documented API rather than guessed: `client.counts`
    /// and `drafts.*` are not public methods at all, `chat.command` is not
    /// either, and typing is a frame on a websocket this route does not have.
    /// Realtime depends on an app-level token the person has to paste, so the
    /// caller passes what it found rather than this deciding.
    pub fn oauth(realtime: bool) -> Self {
        Capabilities {
            counts: false,
            realtime,
            typing: false,
            // `users.getPresence` exists but there is no subscription, so it
            // would have to be polled — FR-P2 grades that S for this route.
            presence: false,
            drafts: false,
            // `stars.*` still answers for a user token; `saved.*` does not.
            saved: true,
            search: true,
        }
    }

    /// Everything the browser session route provides.
    pub fn session() -> Self {
        Capabilities {
            counts: true,
            realtime: true,
            typing: true,
            presence: true,
            drafts: true,
            saved: true,
            search: true,
        }
    }
}

/// Something done to a conversation.
#[derive(Debug, Clone)]
pub enum ChannelOp {
    Join(ChannelId),
    Leave(ChannelId),
    SetTopic(ChannelId, String),
    SetPurpose(ChannelId, String),
    Invite(ChannelId, UserId),
    /// Star, or unstar. Slack calls it a favourite now and the endpoint still
    /// calls it a star.
    Star(ChannelId, bool),
    /// The whole muted set, not one channel.
    ///
    /// `muted_channels` is a single preference holding a comma-separated list,
    /// so muting one conversation is a write of every muted conversation. The
    /// caller owns the list because the caller owns the conversations; a
    /// backend that tried to keep its own copy would drift from the sidebar
    /// the first time another client muted something.
    SetMuted(Vec<ChannelId>),
}

/// One search result, from Slack or from the local index.
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub channel: ChannelId,
    pub channel_name: String,
    pub ts: Ts,
    pub user: Option<UserId>,
    pub text: String,
}

/// One file `search.files` found.
///
/// The channel and timestamp are optional because Slack's file search
/// answers with the file, not with the message it was shared in — a file can
/// be in several conversations, or in none any more. A hit that cannot say
/// where it came from is still worth showing; pretending it can is not.
#[derive(Debug, Clone)]
pub struct FileHit {
    pub id: FileId,
    pub name: String,
    pub mimetype: String,
    pub size: u64,
    pub url_private: Option<String>,
    pub channel: Option<ChannelId>,
    pub channel_name: String,
    pub user: Option<UserId>,
}

/// One page of history, with the cursor to continue from.
#[derive(Debug, Clone, Default)]
pub struct Page {
    pub messages: Vec<Message>,
    pub has_more: bool,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HistoryQuery {
    /// Return messages older than this.
    pub latest: Option<Ts>,
    /// Return messages newer than this. Used by gap fill after a reconnect.
    pub oldest: Option<Ts>,
    pub limit: u16,
    pub cursor: Option<String>,
    /// Include the messages at `latest` and `oldest` themselves. Slack excludes
    /// them by default, so re-reading one message by its own timestamp returns
    /// every message except the one asked for.
    pub inclusive: bool,
}

impl Default for HistoryQuery {
    fn default() -> Self {
        HistoryQuery {
            latest: None,
            oldest: None,
            limit: 50,
            cursor: None,
            inclusive: false,
        }
    }
}

/// What `boot` learns in one round trip.
#[derive(Debug, Clone, Default)]
pub struct Boot {
    pub conversations: Vec<Conversation>,
    pub users: Vec<User>,
    /// Channel ids the user has muted. Not on the channel object; it lives in
    /// prefs, and muted conversations must never raise a badge.
    pub muted: Vec<ChannelId>,
    /// Per-conversation notification preferences, where the backend can read
    /// them. Also prefs, also not on the channel object. FR-U4: a channel the
    /// person set to "everything" should interrupt for everything, and one set
    /// to "never" should not interrupt at all — following our own rule
    /// instead is a client that ignores what they told Slack.
    pub notify: Vec<(ChannelId, slk_core::NotifyPref)>,
}

/// Per-conversation unread state, cheap enough to refresh often.
#[derive(Debug, Clone, Default)]
pub struct Counts {
    pub conversations: Vec<CountEntry>,
}

#[derive(Debug, Clone)]
pub struct CountEntry {
    pub id: ChannelId,
    pub last_read: Option<Ts>,
    pub latest: Option<Ts>,
    pub unread: u32,
    pub mentions: u32,
    /// Slack's own flag that our cached history for this conversation can no
    /// longer be trusted. Honoured, not second-guessed.
    pub history_invalid: bool,
}

#[async_trait]
pub trait SlackBackend: Send + Sync {
    fn team(&self) -> &TeamId;
    fn self_id(&self) -> &UserId;
    fn capabilities(&self) -> Capabilities;

    /// How much longer this backend is holding requests off, if Slack has
    /// asked it to. Not every backend has a rate limit worth showing, so the
    /// default is "not waiting".
    async fn paused_for(&self) -> Option<std::time::Duration> {
        None
    }

    async fn whoami(&self) -> Result<Workspace>;
    async fn boot(&self) -> Result<Boot>;
    async fn counts(&self) -> Result<Counts>;

    async fn history(&self, ch: &ChannelId, q: HistoryQuery) -> Result<Page>;
    async fn replies(&self, ch: &ChannelId, thread: &Ts, q: HistoryQuery) -> Result<Page>;
    async fn users(&self) -> Result<Vec<User>>;

    /// Search the workspace. Slack's own modifiers (`from:`, `in:`, `before:`)
    /// are passed through untouched, because they are the reason to use this
    /// rather than the local index.
    async fn search(&self, query: &str, count: u16) -> Result<Vec<SearchHit>>;

    /// The workspace's own emoji: name to image URL.
    ///
    /// Aliases are resolved here rather than by the caller — Slack answers
    /// `alias:other` and chains them, and every consumer would otherwise have
    /// to know that. A backend that cannot ask has none.
    async fn custom_emoji(&self) -> Result<Vec<(String, String)>> {
        Ok(Vec::new())
    }

    /// `search.files`. A backend that cannot search files finds none rather
    /// than failing: the search box is one box, and an error where an empty
    /// list belongs reads as a broken client.
    async fn search_files(&self, _query: &str, _count: u16) -> Result<Vec<FileHit>> {
        Ok(Vec::new())
    }

    /// The workspace's user groups, so `@design-team` can be completed and
    /// resolved. Cheap and small; fetched once at boot.
    async fn usergroups(&self) -> Result<Vec<(String, String)>> {
        Ok(Vec::new())
    }

    /// Public channels the user is *not* in, for browsing and joining.
    ///
    /// A separate call from `boot`, which returns only what you are already a
    /// member of. Capped rather than paged to exhaustion: the point is to find
    /// a channel by name, and fetching every channel in a large workspace to
    /// populate a picker is exactly the bulk traffic this client does not do.
    async fn public_channels(&self, limit: u16) -> Result<Vec<Conversation>>;

    /// Who is in a conversation.
    async fn members(&self, ch: &ChannelId) -> Result<Vec<UserId>>;

    /// One user by id.
    ///
    /// `users.list` is paged and, in a real workspace, enormous; it also omits
    /// Slack's own system users. So the directory is filled lazily: whenever a
    /// name is needed and missing, it is fetched.
    async fn user_info(&self, id: &UserId) -> Result<User>;

    /// Post, returning the timestamp Slack assigned. `client_msg_id` is echoed
    /// back on the websocket, which is how the optimistic row is matched to its
    /// confirmation without guessing.
    ///
    /// `broadcast` only means anything with a `thread`: it is Slack's "also
    /// send to channel", which puts the reply in the conversation as well as
    /// in the thread.
    async fn post(
        &self,
        ch: &ChannelId,
        thread: Option<&Ts>,
        text: &str,
        client_msg_id: &str,
        broadcast: bool,
    ) -> Result<Ts>;

    /// Follow or stop following a thread, so its replies do or do not count as
    /// unread and notify.
    async fn follow_thread(&self, ch: &ChannelId, thread: &Ts, on: bool) -> Result<()>;

    async fn mark(&self, ch: &ChannelId, ts: &Ts) -> Result<()>;

    /// Add or remove one reaction. `name` is a Slack shortcode without colons.
    async fn react(&self, ch: &ChannelId, ts: &Ts, name: &str, on: bool) -> Result<()>;

    /// Edit one of our own messages.
    async fn edit(&self, ch: &ChannelId, ts: &Ts, text: &str) -> Result<()>;

    /// Delete one of our own messages.
    async fn delete(&self, ch: &ChannelId, ts: &Ts) -> Result<()>;

    /// A link to a message that will still work tomorrow, for pasting
    /// somewhere else.
    async fn permalink(&self, ch: &ChannelId, ts: &Ts) -> Result<String>;

    /// Post as `/me`.
    async fn me_message(&self, ch: &ChannelId, text: &str) -> Result<()>;

    /// Say that we are typing here.
    ///
    /// There is no REST endpoint for this: the web client sends a frame down
    /// the websocket it already has. A backend with no socket cannot do it,
    /// and says so by doing nothing rather than by failing — a typing
    /// indicator is the least important thing in the client and must never be
    /// the reason a message does not go.
    async fn typing(&self, _ch: &ChannelId) -> Result<()> {
        Ok(())
    }

    /// Set our own presence: `true` for active, `false` for away.
    async fn set_presence(&self, active: bool) -> Result<()>;

    /// Set or clear the status; empty text and emoji clear it.
    ///
    /// `expires` is a Unix time, or zero for "until I change it". Slack calls
    /// it `status_expiration` and treats zero the same way.
    async fn set_status(&self, text: &str, emoji: &str, expires: i64) -> Result<()>;

    /// Snooze notifications for so many minutes, or end a snooze with zero.
    async fn snooze(&self, minutes: u32) -> Result<()>;

    /// Join, leave, or change a conversation.
    async fn channel_op(&self, op: ChannelOp) -> Result<()>;

    /// Hand an unrecognised slash command to the workspace, the way the web
    /// client does, so `/giphy` and the rest still work.
    async fn slash(&self, ch: &ChannelId, command: &str, text: &str) -> Result<()>;

    /// Save for later, or unsave.
    async fn save(&self, ch: &ChannelId, ts: &Ts, on: bool) -> Result<()>;

    /// Pin to the conversation, or unpin.
    async fn pin(&self, ch: &ChannelId, ts: &Ts, on: bool) -> Result<()>;

    /// What is pinned here, authoritatively.
    ///
    /// The store knows about a pin only for history it has actually read, and
    /// somebody else's pin on a message from last year is exactly the one
    /// worth showing. A default of "nothing" is the honest answer for a
    /// backend that cannot ask.
    async fn pins(&self, _ch: &ChannelId) -> Result<Vec<Ts>> {
        Ok(Vec::new())
    }

    /// What is bookmarked on this conversation's bar.
    ///
    /// A default of "none" rather than an error: the bar is decoration on a
    /// conversation that works without it, and a backend whose token lacks
    /// `bookmarks:read` should cost the bar, not the conversation.
    async fn bookmarks(&self, _ch: &ChannelId) -> Result<Vec<Bookmark>> {
        Ok(Vec::new())
    }

    /// Fetch a file to disk.
    ///
    /// Slack's file URLs are not public: every one of them, thumbnails
    /// included, needs the same credentials as an API call. Handing such a URL
    /// to a browser or to `curl` gets an HTML sign-in page, which is why this
    /// goes through the backend rather than through the system opener.
    async fn download(&self, url: &str, to: &std::path::Path) -> Result<u64>;

    /// Share a local file.
    async fn upload(
        &self,
        ch: &ChannelId,
        thread: Option<&Ts>,
        path: &std::path::Path,
        comment: Option<&str>,
    ) -> Result<()>;

    /// Open the realtime stream. Returns `None` when this backend has none, in
    /// which case the caller polls.
    ///
    /// `presence` is the set of people whose presence should be reported.
    /// Slack sends none unless asked, and asking for the whole directory would
    /// be a great deal of traffic for dots nobody is looking at.
    async fn connect(&self, presence: &[UserId]) -> Result<Option<crate::events::EventStream>>;
}

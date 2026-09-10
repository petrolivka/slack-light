//! `--read-only`, enforced where the writes actually happen.
//!
//! FR-Z5 says this mode "performs no writes of any kind", and until this
//! existed it was implemented in the window: the composer refused to send,
//! reactions refused, uploads refused. Marking a conversation read did not,
//! because nothing in the window is *called* a write — `maybe_mark` just
//! happens, on focus, quietly, and it is a `conversations.mark` on somebody's
//! real account. Found by running the client against a live workspace under
//! `--read-only` and watching the unread counts go to zero.
//!
//! A guarantee implemented in the interface is a guarantee that every other
//! caller bypasses, and here the interface bypassed it itself. So it lives at
//! the boundary instead: a backend that wraps another, passes every read
//! through, and refuses every write. Code added later is covered by
//! construction rather than by remembering.

use crate::backend::*;
use crate::error::{ErrorKind, Result, SlackError};
use async_trait::async_trait;
use slk_core::{Bookmark, ChannelId, Conversation, TeamId, Ts, User, UserId, Workspace};
use std::sync::Arc;

pub struct ReadOnly {
    inner: Arc<dyn SlackBackend>,
}

impl ReadOnly {
    pub fn wrap(inner: Arc<dyn SlackBackend>) -> Arc<dyn SlackBackend> {
        Arc::new(ReadOnly { inner })
    }

    fn refuse<T>(what: &'static str) -> Result<T> {
        Err(SlackError::new(
            what,
            ErrorKind::Permission,
            "read-only: nothing is written",
        ))
    }
}

#[async_trait]
impl SlackBackend for ReadOnly {
    // ---- reads: straight through ---------------------------------------

    fn team(&self) -> &TeamId {
        self.inner.team()
    }
    fn self_id(&self) -> &UserId {
        self.inner.self_id()
    }
    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }
    async fn paused_for(&self) -> Option<std::time::Duration> {
        self.inner.paused_for().await
    }
    async fn whoami(&self) -> Result<Workspace> {
        self.inner.whoami().await
    }
    async fn boot(&self) -> Result<Boot> {
        self.inner.boot().await
    }
    async fn counts(&self) -> Result<Counts> {
        self.inner.counts().await
    }
    async fn history(&self, ch: &ChannelId, q: HistoryQuery) -> Result<Page> {
        self.inner.history(ch, q).await
    }
    async fn replies(&self, ch: &ChannelId, thread: &Ts, q: HistoryQuery) -> Result<Page> {
        self.inner.replies(ch, thread, q).await
    }
    async fn users(&self) -> Result<Vec<User>> {
        self.inner.users().await
    }
    async fn search(&self, query: &str, count: u16) -> Result<Vec<SearchHit>> {
        self.inner.search(query, count).await
    }
    async fn search_files(&self, query: &str, count: u16) -> Result<Vec<FileHit>> {
        self.inner.search_files(query, count).await
    }
    async fn custom_emoji(&self) -> Result<Vec<(String, String)>> {
        self.inner.custom_emoji().await
    }
    async fn usergroups(&self) -> Result<Vec<(String, String)>> {
        self.inner.usergroups().await
    }
    async fn public_channels(&self, limit: u16) -> Result<Vec<Conversation>> {
        self.inner.public_channels(limit).await
    }
    async fn members(&self, ch: &ChannelId) -> Result<Vec<UserId>> {
        self.inner.members(ch).await
    }
    async fn user_info(&self, id: &UserId) -> Result<User> {
        self.inner.user_info(id).await
    }
    async fn permalink(&self, ch: &ChannelId, ts: &Ts) -> Result<String> {
        self.inner.permalink(ch, ts).await
    }
    async fn pins(&self, ch: &ChannelId) -> Result<Vec<Ts>> {
        self.inner.pins(ch).await
    }
    async fn bookmarks(&self, ch: &ChannelId) -> Result<Vec<Bookmark>> {
        self.inner.bookmarks(ch).await
    }
    /// To local disk, not to the account: this is how an image gets drawn.
    async fn download(&self, url: &str, to: &std::path::Path) -> Result<u64> {
        self.inner.download(url, to).await
    }
    /// Listening is not writing.
    async fn connect(&self, presence: &[UserId]) -> Result<Option<crate::events::EventStream>> {
        self.inner.connect(presence).await
    }

    // ---- writes: refused ------------------------------------------------

    async fn post(
        &self,
        _ch: &ChannelId,
        _thread: Option<&Ts>,
        _text: &str,
        _local_id: &str,
        _broadcast: bool,
    ) -> Result<Ts> {
        Self::refuse("chat.postMessage")
    }
    async fn follow_thread(&self, _ch: &ChannelId, _thread: &Ts, _on: bool) -> Result<()> {
        Self::refuse("subscriptions.thread")
    }
    /// The one that got through. A read mark is a write: it moves the unread
    /// state on every device the person owns.
    async fn mark(&self, _ch: &ChannelId, _ts: &Ts) -> Result<()> {
        Self::refuse("conversations.mark")
    }
    async fn react(&self, _ch: &ChannelId, _ts: &Ts, _name: &str, _on: bool) -> Result<()> {
        Self::refuse("reactions")
    }
    async fn edit(&self, _ch: &ChannelId, _ts: &Ts, _text: &str) -> Result<()> {
        Self::refuse("chat.update")
    }
    async fn delete(&self, _ch: &ChannelId, _ts: &Ts) -> Result<()> {
        Self::refuse("chat.delete")
    }
    async fn me_message(&self, _ch: &ChannelId, _text: &str) -> Result<()> {
        Self::refuse("chat.meMessage")
    }
    async fn set_presence(&self, _active: bool) -> Result<()> {
        Self::refuse("users.setPresence")
    }
    async fn set_status(&self, _text: &str, _emoji: &str, _expires: i64) -> Result<()> {
        Self::refuse("users.profile.set")
    }
    async fn snooze(&self, _minutes: u32) -> Result<()> {
        Self::refuse("dnd.setSnooze")
    }
    async fn channel_op(&self, _op: ChannelOp) -> Result<()> {
        Self::refuse("conversations")
    }
    async fn slash(&self, _ch: &ChannelId, _command: &str, _text: &str) -> Result<()> {
        Self::refuse("chat.command")
    }
    async fn save(&self, _ch: &ChannelId, _ts: &Ts, _on: bool) -> Result<()> {
        Self::refuse("saved")
    }
    async fn pin(&self, _ch: &ChannelId, _ts: &Ts, _on: bool) -> Result<()> {
        Self::refuse("pins")
    }
    /// Telling a room you are typing is telling the room something.
    async fn typing(&self, _ch: &ChannelId) -> Result<()> {
        Self::refuse("typing")
    }
    async fn upload(
        &self,
        _ch: &ChannelId,
        _thread: Option<&Ts>,
        _path: &std::path::Path,
        _comment: Option<&str>,
    ) -> Result<()> {
        Self::refuse("files.upload")
    }
}

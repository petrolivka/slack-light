# slack-light — Architecture

**How the client is built internally: crates, data model, storage, the sync engine, the GTK runtime, theming, configuration and testing.**

| | |
|---|---|
| Document status | v0.2 — amended for the native-GUI pivot |
| Date | 2026-09-09 (v0.1: 2026-09-07) |
| Owner | petr |
| Companion to | [ANALYSIS-AND-REQUIREMENTS.md](./ANALYSIS-AND-REQUIREMENTS.md) (requirements) · [ADR-001-native-gui.md](./ADR-001-native-gui.md) (why native) · [SLACK-ACCESS-STRATEGY.md](./SLACK-ACCESS-STRATEGY.md) (which API route and why) |
| Decided | Rust, GTK4 via relm4 (no libadwaita), tokio; session-token backend primary with OAuth behind the same trait; browser sign-in; multiple workspaces at once; SQLite + FTS5 message cache; themed from Omarchy |
| Lineage | §3–§6 (domain model, backend, storage, sync engine) were built and verified by the terminal predecessor and are unchanged. §1, §2, §7, §8, §10 and §12 were rewritten for the window |

---

## 1. Shape of the system

```
                       ┌──────────────────────────────────────────────────┐
                       │              GTK main thread                      │
   Wayland ◄─────────► │  relm4 components · widgets · shortcuts · CSS     │
   (GDK)               │  hold channel ends; send Commands, apply Events   │
                       └───────────┬──────────────────────▲───────────────┘
                                   │ Command               │ Event
                                   ▼                       │
                       ┌──────────────────────────────────────────────────┐
                       │      tokio runtime, on its own thread             │
                       │  routes Commands to workspaces, fans Events into  │
                       │  the GLib main context; notification policy       │
                       └──┬───────────────┬───────────────┬───────────────┘
                          │               │               │
              ┌───────────▼───┐   ┌───────▼───────┐   ┌───▼────────────┐
              │ Workspace A   │   │ Workspace B   │   │ Workspace C    │
              │ SyncEngine    │   │ SyncEngine    │   │ SyncEngine     │
              │  ├ websocket  │   │  ├ websocket  │   │  ├ websocket   │
              │  ├ Web API    │   │  ├ Web API    │   │  ├ Web API     │
              │  └ rate gate  │   │  └ rate gate  │   │  └ rate gate   │
              └───────┬───────┘   └───────┬───────┘   └───────┬────────┘
                      └───────────────────┼───────────────────┘
                                          ▼
                              ┌────────────────────────┐
                              │  Store task (SQLite)   │  single writer,
                              │  messages · users ·    │  FTS5 index,
                              │  channels · drafts     │  media cache index
                              └────────────────────────┘
```

Three rules, inherited from the predecessor projects and kept:

1. **Message passing, one owner per piece of state.** The main thread owns the component models; each workspace task owns its connection state and its store connection. No `Arc<Mutex<Everything>>`.
2. **The GTK main thread never does I/O.** It sends a `Command`, gets an `Event` later, and renders whatever it has meanwhile. A slow network shows a spinner, never a frozen window. This is the one rule the pivot changed by a single word.
3. **Every external shape is parsed defensively.** Slack's responses — especially the undocumented ones — are treated as untrusted input. Unknown fields are ignored, missing ones degrade, and nothing panics.

---

## 2. Crate layout

```
slack-light/
├── Cargo.toml                 # workspace; the `slack-light` binary lives at the root
├── src/
│   ├── main.rs                # GApplication, CLI (clap), config load, panic hook, wiring, --doctor
│   ├── doctor.rs              # GTK, Wayland, portal, theme, browser, credentials, cache
│   └── bin/
│       ├── probe.rs           # dev-tools: read-only tour of every endpoint, dumps fixtures
│       ├── seed.rs            # dev-tools: fill the test workspace with the fixture corpus
│       └── wscheck.rs         # dev-tools: listen to the live event stream
├── crates/
│   ├── slk-core/              # ids, Ts, domain types, rich-text AST, mrkdwn ⇄ AST, emoji, permalinks
│   ├── slk-api/               # SlackBackend trait; session + oauth + mock backends; websocket; rate gate
│   ├── slk-store/             # SQLite schema, migrations, queries, FTS, retention
│   ├── slk-sync/              # engine per workspace: boot, counts, history, gap fill, outbox, notifications
│   ├── slk-config/            # TOML config + action names + keymap
│   ├── slk-notify/            # desktop notifications over D-Bus
│   ├── slk-auth/              # browser sign-in over the DevTools protocol; the guided paste; the 0600 file
│   ├── slk-theme/             # Omarchy colors.toml → GTK CSS; built-in dark/light; live reload
│   └── slk-ui/                # relm4 components; AST → Pango markup; Block Kit → widgets; shortcuts
└── fuzz/                      # cargo-fuzz targets: mrkdwn, rich_text, event JSON, keymap
```

Dependency direction is strictly downward: `slk-ui → slk-sync → slk-api, slk-store → slk-core`; `slk-ui → slk-theme, slk-config, slk-core`; `slk-auth → slk-api`. `slk-core` and `slk-config` have **no I/O and no async**, so the parsers are unit-testable and fuzzable in isolation. `slk-ui` is the only crate that links GTK; `slk-theme` links only GTK's CSS provider. The six unchanged crates are the ones the predecessor carried through two milestones.

---

## 3. Domain model (`slk-core`)

### 3.1 Identifiers

```rust
pub struct TeamId(String);        // "T0123ABC" — a workspace; Enterprise Grid orgs prefix "E"
pub struct UserId(String);        // "U…" or "W…" (Grid)
pub struct ChannelId(String);     // "C…" public/private, "D…" DM, "G…" legacy group/mpim
pub struct BotId(String);         // "B…"
pub struct FileId(String);        // "F…"

/// Slack message timestamp: "1725700000.123456". Unique per channel, lexically
/// ordered when both halves are zero-padded, which Slack guarantees.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ts(String);
impl Ts { pub fn secs(&self) -> i64; pub fn as_str(&self) -> &str; }

/// Every message-scoped key is qualified by workspace; nothing is global.
pub struct MsgRef { pub team: TeamId, pub channel: ChannelId, pub ts: Ts }
```

### 3.2 Entities

```rust
pub struct Workspace {
    pub id: TeamId, pub name: String, pub domain: String,
    pub self_id: UserId, pub enterprise: Option<EnterpriseId>,
    pub icon: Option<Url>,
}

pub enum ConversationKind { Public, Private, Dm { peer: UserId }, Mpim { members: Vec<UserId> } }

pub struct Conversation {
    pub team: TeamId, pub id: ChannelId, pub kind: ConversationKind,
    pub name: String, pub topic: String, pub purpose: String,
    pub is_member: bool, pub is_archived: bool, pub is_starred: bool, pub is_muted: bool,
    pub is_shared: bool,                     // Slack Connect
    pub member_count: Option<u32>,
    pub created: i64, pub creator: Option<UserId>,
    pub last_read: Option<Ts>, pub latest: Option<Ts>,
    pub unread: u32, pub mentions: u32,      // from counts, maintained by events
    pub notify: NotifyPref,                  // All | Mentions | Nothing | Default
    pub priority: f32,                       // Slack's own ordering hint when present
}

pub struct User {
    pub team: TeamId, pub id: UserId,
    pub name: String, pub display_name: String, pub real_name: String,
    pub title: String, pub tz: Option<String>, pub tz_offset: i32,
    pub is_bot: bool, pub is_app_user: bool, pub is_deleted: bool, pub is_restricted: bool,
    pub is_external: bool,                   // different team than the conversation's
    pub color: Option<String>,               // Slack's per-user hex, used for the name colour
    pub status: Option<UserStatus>, pub presence: Presence,
    pub avatar: Option<Url>, pub email: Option<String>,
}

pub struct Message {
    pub team: TeamId, pub channel: ChannelId, pub ts: Ts,
    pub thread_ts: Option<Ts>,               // Some(parent) on replies; Some(self.ts) on parents
    pub author: Author,                      // User(UserId) | Bot { id, name, icon } | System
    pub subtype: Option<Subtype>,            // ChannelJoin, MeMessage, BotMessage, Tombstone, …
    pub text: String,                        // raw mrkdwn as sent; the fallback
    pub body: Doc,                           // parsed rich text (from `blocks` or from `text`)
    pub blocks: Vec<Block>,                  // Block Kit blocks that are not rich_text
    pub attachments: Vec<Attachment>,        // legacy attachments / unfurls
    pub files: Vec<FileMeta>,
    pub reactions: Vec<Reaction>,            // { name, count, users, by_me }
    pub edited: Option<Edited>, pub is_starred: bool, pub pinned: bool,
    pub reply_count: u32, pub reply_users: Vec<UserId>, pub latest_reply: Option<Ts>,
    pub subscribed: bool, pub thread_last_read: Option<Ts>,
    pub reply_broadcast: bool,
    pub delivery: Delivery,                  // Confirmed | Pending(local_id) | Failed(String)
}
```

### 3.3 Rich text AST

Slack sends user messages as `blocks: [{type: "rich_text", …}]` plus a `text` fallback in mrkdwn. The AST is the common target of both:

```rust
pub struct Doc(pub Vec<BlockNode>);
pub enum BlockNode {
    Section(Vec<Inline>),
    List { style: ListStyle, indent: u8, items: Vec<Vec<Inline>> },
    Preformatted(String),
    Quote(Vec<Inline>),
}
pub enum Inline {
    Text { text: String, style: Style },                 // bold, italic, strike, code
    Link { url: String, text: Option<String>, style: Style },
    User(UserId), UserGroup(SubteamId), Channel(ChannelId),
    Broadcast(Broadcast),                                 // Here | Channel | Everyone
    Emoji { name: String, skin: Option<u8>, unicode: Option<String> },
    Date { ts: i64, format: String, link: Option<String>, fallback: String },
    Color(String),
}
```

Two parsers produce it, both in `slk-core` and both fuzzed:

- `rich_text::parse(&serde_json::Value) -> Doc` — walks `rich_text_section` / `rich_text_list` / `rich_text_preformatted` / `rich_text_quote` and their element types. Unknown element → its `text` if any, else skipped.
- `mrkdwn::parse(&str) -> Doc` — the fallback for bots, legacy messages and `text`-only fields inside Block Kit sections. Handles `*`, `_`, `~`, `` ` ``, ```` ``` ````, `>`, `<url|text>`, `<@U>`, `<#C|name>`, `<!subteam^S|@h>`, `<!here>`, `<!date^…>`, `:emoji:` and HTML entities.

And one serializer for the outbound path: `mrkdwn::emit(&Doc) -> String`, used after the composer's `@name` / `#name` tokens have been resolved to ids. Round-trip property: `parse(emit(doc))` preserves every inline.

### 3.4 Emoji

`slk-core::emoji` bundles a shortcode → Unicode table generated from the same dataset Slack's web client uses (`emoji-datasource`, iamcal), so `:+1:`, `:simple_smile:`, `:skin-tone-3:` and Slack's aliases resolve identically. Custom emoji come from `emoji.list` per workspace (`alias:` entries resolved), stored in the store, rendered as `:name:` until M4 adds inline images. A width table (`emoji_width(&str) -> u8`) is derived per terminal at startup by the `doctor` probe (print, query cursor) with a bundled default of 2 for every emoji; this is what keeps columns aligned.

---

## 4. Backend abstraction (`slk-api`)

### 4.1 The trait

```rust
#[async_trait]
pub trait SlackBackend: Send + Sync {
    // identity & boot
    async fn whoami(&self) -> Result<Workspace>;
    async fn boot(&self) -> Result<Boot>;                       // conversations, self, prefs, (counts)
    async fn counts(&self) -> Result<Counts>;                   // unread/mention per conversation + threads
    // reads
    async fn history(&self, ch: &ChannelId, q: HistoryQuery) -> Result<Page<Message>>;
    async fn replies(&self, ch: &ChannelId, thread: &Ts, q: HistoryQuery) -> Result<Page<Message>>;
    async fn conversations(&self, q: ConvQuery) -> Result<Page<Conversation>>;
    async fn conversation_info(&self, ch: &ChannelId) -> Result<Conversation>;
    async fn members(&self, ch: &ChannelId) -> Result<Page<UserId>>;
    async fn users(&self, q: UserQuery) -> Result<Page<User>>;
    async fn user_info(&self, id: &UserId) -> Result<User>;
    async fn search(&self, q: SearchQuery) -> Result<SearchResults>;
    async fn emoji(&self) -> Result<Vec<CustomEmoji>>;
    async fn permalink(&self, m: &MsgRef) -> Result<Url>;
    // writes
    async fn post(&self, ch: &ChannelId, thread: Option<&Ts>, text: &str, opts: PostOpts) -> Result<Ts>;
    async fn edit(&self, m: &MsgRef, text: &str) -> Result<()>;
    async fn delete(&self, m: &MsgRef) -> Result<()>;
    async fn react(&self, m: &MsgRef, name: &str, on: bool) -> Result<()>;
    async fn mark(&self, ch: &ChannelId, ts: &Ts) -> Result<()>;
    async fn mark_thread(&self, m: &MsgRef, ts: &Ts) -> Result<()>;
    async fn typing(&self, ch: &ChannelId) -> Result<()>;
    async fn save(&self, m: &MsgRef, on: bool) -> Result<()>;   // Later: saved.* on session, stars.* on oauth
    async fn pin(&self, m: &MsgRef, on: bool) -> Result<()>;
    async fn upload(&self, ch: &ChannelId, thread: Option<&Ts>, file: UploadSpec) -> Result<FileId>;
    async fn download(&self, url: &Url, to: &Path, progress: ProgressTx) -> Result<()>;
    async fn set_presence(&self, p: PresenceSetting) -> Result<()>;
    async fn set_status(&self, s: Option<UserStatus>) -> Result<()>;
    async fn dnd(&self, op: DndOp) -> Result<DndState>;
    async fn channel_op(&self, op: ChannelOp) -> Result<()>;   // join, leave, create, archive, invite, topic, purpose, rename
    async fn slash(&self, ch: &ChannelId, cmd: &str, text: &str) -> Result<SlashResult>;
    // realtime
    async fn connect(&self) -> Result<Box<dyn EventStream>>;     // websocket; yields RtEvent
    fn capabilities(&self) -> Capabilities;                     // what this backend can do (§4.4)
}
```

`HistoryQuery { latest: Option<Ts>, oldest: Option<Ts>, limit: u16, inclusive: bool, cursor: Option<String> }` maps onto `conversations.history` / `conversations.replies`. Every `Result` error is a `SlackError` with a stable `kind()` — `Auth`, `RateLimited { retry_after }`, `NotFound`, `Permission`, `Transport`, `Shape(String)` — so callers can react without matching strings.

### 4.2 Backends

| Backend | Token | Transport | Notes |
|---|---|---|---|
| **`SessionBackend`** (primary) | `xoxc-…` + cookie `d=xoxd-…` | `POST https://<domain>.slack.com/api/<method>` with the token as a form field and the cookie as a header; the web client's own websocket at `wss-primary.slack.com` | Same requests the web client makes. Also implements the internal methods (`client.userBoot`, `client.counts`, `drafts.*`, `saved.list`, `subscriptions.thread.mark`, `chat.command`, `users.prefs.*`) that give parity |
| **`OAuthBackend`** | `xoxp-…` user token from a Slack app (+ `xapp-…`) | `POST https://slack.com/api/<method>` with `Authorization: Bearer`; realtime via Socket Mode where it delivers user events (M0 verifies), else polling of the focused conversation | Official and stable; per-app rate limits (possibly 1/min × 15 on history outside the home workspace); **no presence, no typing, no counts, no drafts, no saved** → `capabilities()` says so. Never `rtm.connect`: modern app tokens are rejected and classic apps end 2026-11-16 |
| **`MockBackend`** | none | scripted | Replays a JSON script of responses and events; used by every UI test and by `--demo` |
| **`FixtureBackend`** | none | recorded | Serves captured responses from `tests/fixtures/`; used by parser and sync tests |

Exactly one backend per workspace; different workspaces may use different backends.

### 4.3 Session backend details

- **Credentials**: `xoxc` token and `d` cookie, obtained once by `slack-light auth add` (§7.7, browser sign-in; `--paste` for the manual flow), stored in the OS keyring under `slktui/<team_id>`, fallback `~/.config/slktui/auth.json` mode 0600. Never logged; every `tracing` field that could carry them is redacted by a layer, not by discipline.
- **Requests**: form-encoded body with `token`, `Cookie: d=…` header, a browser-like `User-Agent`, `_x_reason`/`_x_mode` fields omitted unless a method needs them. JSON responses are checked for `ok:false` and mapped to `SlackError::kind`.
- **Rate gate**: a per-workspace token bucket per Slack tier plus a global concurrency cap (4 in flight). `429` responses honour `Retry-After` and set the `⏸` status for that workspace; nothing retries in a loop.
- **Realtime**: the web client's websocket, `wss://wss-primary.slack.com/?token=<xoxc>&gateway_server=<TEAM>-1&slack_client=desktop&batch_presence_aware=1`, with the `d` cookie on the handshake (the form wee-slack uses; M0 captures the exact handshake and `hello`). `tokio-tungstenite` over rustls. Ping every 10 s with a 20 s pong timeout (emacs-slack's numbers; M0 measured a 21 ms round trip), reconnect with jittered exponential backoff (1 s → 60 s cap), and every reconnect triggers the gap-fill routine (§6.4). **Reconnect with the most recent `reconnect_url` the server pushed** — it sends one every few minutes — and fall back to a fresh handshake only when none has been seen. M0 measured that a socket URL is *not* single-use, contrary to the legacy RTM documentation this project first took the rule from. Outgoing frames: `ping`, `typing`, `presence_sub`. The websocket module is the single most likely thing to break when Slack changes something, so it is one file with one fixture-driven test and nothing else depends on its frame shapes.
- **Files**: `url_private` downloads carry the token as `Authorization: Bearer` plus the cookie; uploads use `files.getUploadURLExternal` → `POST` bytes → `files.completeUploadExternal`.

### 4.4 Capabilities

`Capabilities` is a bit-set the UI consults before offering an action: `Counts`, `Drafts`, `Saved`, `Slash`, `Realtime`, `Typing`, `Presence`, `Search`, `Reminders`, `ChannelAdmin`, `Canvas`. A missing capability disables the action visibly (dim menu entry, toast on key) rather than hiding it, so the two backends look the same and behave honestly.

---

## 5. Storage (`slk-store`)

### 5.1 Why SQLite

Instant startup from the last known state, offline reading, local full-text search across workspaces, and resilience to rate limits (history already fetched is never fetched again). `rusqlite` with the bundled SQLite and FTS5; one writer connection owned by the store task, read connections opened per query task in WAL mode.

Location: `$XDG_DATA_HOME/slack-light/store.sqlite` (0600). `--no-cache` runs fully in memory. Optional at-rest encryption via `bundled-sqlcipher` is a build feature (`--features sqlcipher`), keyed from the keyring.

### 5.2 Schema

```sql
CREATE TABLE workspace (team TEXT PRIMARY KEY, name TEXT, domain TEXT, self_id TEXT,
                        backend TEXT, boot_json TEXT, booted_at INTEGER);

CREATE TABLE conversation (team TEXT, id TEXT, kind TEXT, name TEXT, topic TEXT, purpose TEXT,
                           is_member INTEGER, is_archived INTEGER, is_starred INTEGER, is_muted INTEGER,
                           is_shared INTEGER, member_count INTEGER, created INTEGER, creator TEXT,
                           last_read TEXT, latest TEXT, unread INTEGER, mentions INTEGER,
                           notify TEXT, priority REAL, raw_json TEXT, updated_at INTEGER,
                           PRIMARY KEY (team, id));

CREATE TABLE user (team TEXT, id TEXT, name TEXT, display_name TEXT, real_name TEXT, title TEXT,
                   tz TEXT, tz_offset INTEGER, is_bot INTEGER, is_deleted INTEGER, is_restricted INTEGER,
                   color TEXT, status_text TEXT, status_emoji TEXT, status_expires INTEGER,
                   avatar TEXT, email TEXT, raw_json TEXT, updated_at INTEGER,
                   PRIMARY KEY (team, id));

CREATE TABLE message (team TEXT, channel TEXT, ts TEXT, thread_ts TEXT, author_kind TEXT, author_id TEXT,
                      subtype TEXT, text TEXT, raw_json TEXT NOT NULL,        -- the source of truth
                      reply_count INTEGER, latest_reply TEXT, edited_ts TEXT, is_starred INTEGER,
                      pinned INTEGER, deleted INTEGER DEFAULT 0, local_id TEXT, delivery TEXT,
                      PRIMARY KEY (team, channel, ts));
CREATE INDEX message_thread ON message (team, channel, thread_ts, ts);
CREATE INDEX message_author ON message (team, author_id, ts);

-- what ranges of a channel's history we hold contiguously; gaps are the complement
CREATE TABLE history_span (team TEXT, channel TEXT, oldest TEXT, newest TEXT, complete_start INTEGER,
                           PRIMARY KEY (team, channel, oldest));

CREATE TABLE reaction (team TEXT, channel TEXT, ts TEXT, name TEXT, user_id TEXT,
                       PRIMARY KEY (team, channel, ts, name, user_id));

CREATE TABLE file (team TEXT, id TEXT PRIMARY KEY, name TEXT, mimetype TEXT, size INTEGER,
                   url_private TEXT, thumb_url TEXT, width INTEGER, height INTEGER,
                   local_path TEXT, raw_json TEXT);
CREATE TABLE message_file (team TEXT, channel TEXT, ts TEXT, file_id TEXT,
                           PRIMARY KEY (team, channel, ts, file_id));

CREATE TABLE emoji (team TEXT, name TEXT, url TEXT, alias_of TEXT, local_path TEXT, PRIMARY KEY (team, name));
CREATE TABLE draft (team TEXT, channel TEXT, thread_ts TEXT, text TEXT, updated_at INTEGER,
                    PRIMARY KEY (team, channel, thread_ts));
CREATE TABLE outbox (local_id TEXT PRIMARY KEY, team TEXT, channel TEXT, thread_ts TEXT, text TEXT,
                     attempts INTEGER, last_error TEXT, created_at INTEGER);
CREATE TABLE kv (key TEXT PRIMARY KEY, value TEXT);   -- session: last view, cursor per channel, schema version

CREATE VIRTUAL TABLE message_fts USING fts5(text, content='message', content_rowid='rowid',
                                            tokenize='unicode61 remove_diacritics 2');
-- triggers keep message_fts in sync on insert/update/delete
```

`raw_json` is kept for every message, conversation and user so a parser fix can re-derive columns with a migration instead of a re-fetch, and so `v` (view source) works offline. Retention: `store.max_messages_per_channel` (default 5 000) and `store.max_age_days` (default 90) trim old spans nightly; starred/pinned messages are exempt. **M0 measured ~830 bytes per message with `raw_json` retained**, so those defaults imply roughly 200 MiB across 50 channels — retention is therefore also expressed as `store.max_size_mb`, and `slktui cache stats` reports the real figure.

### 5.3 Store task API

The store task owns the writer and exposes typed async methods (`upsert_messages`, `messages_before(channel, ts, n)`, `thread(channel, ts)`, `search_local(q)`, `set_last_read`, …) over a request channel. Reads that the UI needs at frame rate — the visible window of a conversation — are served from an in-memory `ConversationView` maintained by the UI from store results plus live events, so rendering never queries SQLite.

---

## 6. Sync engine (`slk-sync`)

One `SyncEngine` per workspace, a state machine driven by the backend's event stream and by `Command`s from the core task.

### 6.1 States

```
Offline ──connect──▶ Booting ──boot ok──▶ Connecting(ws) ──hello──▶ Live
   ▲                    │                        │                   │
   └──── auth error ────┘◀──── ws closed ────────┴─────── ws closed ─┘
                (Reconnecting: backoff, then Connecting; gap-fill on Live)
```

### 6.2 Boot

1. `whoami` → verify the token, learn the self id. Auth failure → `Event::AuthRequired(team)`; the workspace stays browsable from the store.
2. `boot` → conversations (all types the user is a member of, plus public channels lazily), prefs (muted, notification prefs, starred, sidebar sections when the backend has them), custom emoji.
3. `counts` → unread/mention counts and `last_read` per conversation, thread counts. This is what makes the sidebar correct **before any history is fetched**.
4. Users: the members of DMs and MPIMs and the authors of cached messages first; the full directory paged in the background at low priority with a 24 h TTL.
5. Emit `Event::WorkspaceReady(team)`.

### 6.3 History

- Opening a conversation requests `history(latest=None, limit=50)` unless the store already holds a span that includes `latest`. Scrolling to the top of a loaded span requests the page before its `oldest`. Every page extends or merges a `history_span`.
- Threads are fetched with `replies` on open and kept fresh by events; reply counts on parents come from events.
- **Priority queue**: the focused conversation first, then visible sidebar entries with unread mentions, then everything else; a single worker per workspace drains it through the rate gate. Bulk prefetch of whole workspaces is deliberately **not** done — it is scraper-shaped traffic and it burns the history rate limit.

### 6.4 Gap fill after reconnect

On every transition to `Live`:

1. `counts` again → any conversation whose `latest` moved past the store's newest span edge is a gap.
2. For the focused conversation and every conversation with new mentions: `history(oldest = span.newest, inclusive=false)` until caught up (pages of 200, bounded to 5 pages; beyond that the span is marked broken and the UI shows `── history gap ──` with a `Enter` to load).
3. Other conversations are left to lazy loading; their badges are already correct from `counts`.

### 6.5 Events

Realtime events are normalised into `RtEvent` in `slk-api` and applied by the engine to the store, then re-emitted to the UI as `Event`s:

| Realtime | Effect |
|---|---|
| `message` (no subtype) | upsert; bump conversation `latest`; unread/mention counters if not from self and not marked; notification decision |
| `message` subtypes `message_changed`, `message_deleted`, `message_replied`, `thread_broadcast` | update the referenced row (deleted → `deleted=1` or tombstone for parents) |
| `reaction_added` / `reaction_removed` | reaction table |
| `channel_marked`, `im_marked`, `group_marked`, `mpim_marked`, `thread_marked` | `last_read`, recompute unread from the store |
| `user_typing` | UI only, 3 s expiry |
| `presence_change` | user presence; the engine subscribes (`presence_sub`) to users visible in the sidebar and the open conversation, re-sent when that set changes |
| `user_change`, `team_join`, `user_status_changed` | user table |
| `channel_created/joined/left/archive/unarchive/rename/deleted`, `member_joined_channel/left` | conversation table |
| `pin_added/removed`, `star_added/removed`, `emoji_changed`, `dnd_updated`, `pref_change` | respective tables |
| `desktop_notification` (session backend) | Slack's own decision that this deserves a notification — used when present, with our own policy as fallback |
| `reconnect_url` | the URL to use for the next reconnect; kept, never acted on immediately |
| `badge_counts_updated`, `update_global_thread_state` | Slack's own unread arithmetic, pushed unasked. M0 saw both; **investigate before hand-rolling counters** (§6.5 of the findings) |
| `activity_deleted`, `user_interaction_changed` | the activity feed; undocumented, ignored for now |
| unknown | logged at debug with its `type`, dropped |

### 6.6 Outbox

Sends are optimistic: the message is written to the store with `delivery = Pending(local_id)` and rendered immediately. The outbox task posts it; on success the row is re-keyed to the server `ts` (the echo event carries the same text and arrives before or after — both orders are handled by matching on `local_id` kept in `message.local_id` for 60 s). Failures mark the row `Failed` with the error; retries are manual or on reconnect, never automatic loops. Edits, deletes and reactions are also optimistic with rollback and a toast on failure.

### 6.7 Notification policy

Decided in the core task from: message author ≠ self · conversation not muted · (DM, or mention of self/here/channel, or keyword match, or conversation pref = All, or a followed thread reply) · not in DND · not (conversation focused ∧ terminal focused). The session backend's `desktop_notification` events short-circuit the decision when available. Output goes to `slk-notify` (desktop / bell / OSC), throttled to at most one notification per conversation per 5 s with coalescing (`3 new messages in #eng`).

---

## 7. UI runtime (`slk-ui`, `slk-theme`)

### 7.1 Threads and channels

```
GTK main thread                          tokio runtime thread
────────────────                         ────────────────────
relm4 App                                Engine (workspace A)   ─┐
  ├ Sidebar          ─ Command ─────►    Engine (workspace B)    ├─ one task each,
  ├ Conversation     ◄─ Event ──────     Engine (workspace C)   ─┘  own store connection
  ├ Thread                               │
  ├ Composer                             └ notifier task
  └ modals
```

The application starts a tokio runtime on a dedicated thread before `gtk::init`. Engines are spawned there exactly as before — `Engine::spawn(backend, store, keywords)` — and their `Event` receivers are bridged into the GLib main context with `relm4::Sender` (or `glib::MainContext::channel`), so a component receives events as ordinary input messages. Commands go the other way over the engines' `mpsc` senders. Nothing on the main thread is `async`; nothing on the runtime touches a widget.

### 7.2 Components

relm4's shape: a `Model`, an `Input` enum, an `Output` enum, `update()`, and a declarative `view!`. One component per pane, factories for the lists:

| Component | Holds | Notes |
|---|---|---|
| `App` | workspaces, current, connection state, overlays | routes engine events to panes; owns the `Paned` layout and remembered widths |
| `Sidebar` | `FactoryVecDeque<Section>` → `FactoryVecDeque<Row>` | sections fold; a folded heading shows count and mentions |
| `Conversation` | `gtk::ListView` over a `gio::ListStore` of message ids; a row cache | **virtualised** — the spike proves 5 000 rows at 60 fps; rows are recycled, textures released when a row is unbound |
| `Thread` | the same widget, narrower | stacks over the conversation below a width threshold |
| `Composer` | `gtk::TextView`, completion popover, attachment strip | `Enter`/`ctrl-Enter` configurable; drag-and-drop target |
| modals | jump-to, emoji picker, palette, search, profile, links, confirm | `gtk::Popover` or a transient `gtk::Window`, each a component |

Every `Action` from `slk-config` is a `gio::SimpleAction` on the application, bound through a `gtk::ShortcutController` built from the keymap. That is what keeps remapping and the generated shortcuts window: the map is data, the actions are names, and the window lists whatever is in force.

### 7.3 Rendering a message

```
Message ─► rows::markup(msg, ctx) ─► Pango markup string      (cached by (ts, ctx.version))
           ├─ header: time, author (colour from the theme's user palette), badges, external/deactivated
           ├─ body: ast::to_pango(&Doc, ctx)     bold, italic, strike, code, links, mentions, quotes, lists
           ├─ blocks: blockkit::widgets(&[Block]) ─► gtk::Box of real widgets (header, fields grid, context, button)
           ├─ attachments, files (thumbnail Picture with a sized placeholder)
           ├─ reactions chips (gtk::FlowBox of toggle buttons)
           └─ thread summary (a link row)
```

`ctx` carries the name lookups, the self id, the theme's semantic colours, and the settings. It no longer carries width metrics: Pango lays out text, and the whole width apparatus of the predecessor (`width::Metrics`, the probe, the correction tables, FR-K6, D8) is gone. Whether a row is one `gtk::Label` with markup or a `gtk::TextView` with tags is spike decision 2; the difference is selection across paragraphs versus cost per row.

### 7.4 Images

`backend.download` fetches into the media cache as before (files need the auth header). A row shows a placeholder sized from the file's `original_w/h` immediately, and swaps in a `gdk::Texture` when the bytes arrive and the row is realised. Full-size viewing is an in-window overlay with zoom and a save button through the portal. Textures are dropped when the row is recycled, which is what keeps NFR-4 honest.

### 7.5 Theming (`slk-theme`)

```
~/.local/state/omarchy/current/theme ─┐ (symlink, FileMonitor)
                                      ├─► colors.toml ─► semantic palette ─► CSS text ─► gtk::CssProvider (APPLICATION priority)
built-in dark.toml / light.toml ──────┘                                          ▲
settings portal color-scheme ─────────────────────────────────────────────────────┘ selects built-in when no Omarchy
~/.config/slack-light/user.css ────────────────────────────────────────────────── layered last
```

Omarchy does not theme GTK, so the application does it the Omarchy way: read the palette, write the CSS, reapply on change. The semantic layer (background, surface, text, dim, accent, mention, link, code, selection, the twelve author colours) is the contract; both Omarchy's keys and the built-in themes map onto it, and the CSS is generated from it once. Which mechanism notices a theme change — the monitor, a `theme-set.d` hook, or Omarchy's `themed/*.tpl` — is spike decision 3.

### 7.6 External editor

`ctrl-x`: write the draft to a temp file, spawn `$TERMINAL -e $EDITOR file` (Omarchy's `xdg-terminals.list` names the terminal; `$TERMINAL` and a short list of known ones otherwise), wait without blocking the main thread, re-read on exit. Any failure keeps the draft; stderr is drained and surfaced.

### 7.7 Sign-in (`slk-auth`)

Chromium (or another Chromium-family browser on the path) is launched with `--user-data-dir=<tmp>` and `--remote-debugging-pipe`, pointed at Slack's sign-in page. Over the DevTools protocol the flow watches for the `d` cookie, reads it and the `xoxc` tokens the web client keeps in local storage, and then wipes the profile — on success and on every failure path. It never opens the user's own profile and never reads the Slack desktop application's data. The result lands in the same `auth::add` storage the manual flow uses.

---

## 8. Configuration and CLI (`slk-config`, `main.rs`)

### 8.1 Files

| Path | Purpose |
|---|---|
| `$XDG_CONFIG_HOME/slack-light/config.toml` | settings, keymap, theme choice; hot-reloaded on save |
| `$XDG_CONFIG_HOME/slack-light/themes/*.toml` | user themes (the built-in format, for people off Omarchy) |
| `$XDG_CONFIG_HOME/slack-light/user.css` | CSS layered over the generated theme |
| `$XDG_CONFIG_HOME/slack-light/auth.json` | credentials fallback when no keyring, 0600 |
| `$XDG_DATA_HOME/slack-light/store.sqlite` | message cache |
| `$XDG_CACHE_HOME/slack-light/media/` | downloaded thumbnails and emoji |
| `$XDG_STATE_HOME/slack-light/slack-light.log` | log file when `--log-file` or `log.file = true` |
| `~/.local/state/omarchy/current/theme/colors.toml` | read, never written: Omarchy's current palette |

### 8.2 Config schema (defaults shown)

```toml
[general]
default_workspace = ""          # team id or domain; empty = last used
confirm_quit = false            # a window close is not an accident the way a stray `q` was

[window]
remember = true                 # size, pane widths, last conversation, open thread
sidebar_width = 240             # pixels; dragging the divider updates this
thread_width = 380
decorations = "auto"            # auto | server | client — auto asks the compositor; Hyprland says server

[sidebar]
unread_first = false
hide_read = false
collapse_other_workspaces = true

[message]
timestamp = "absolute"          # absolute | relative
group = true                    # merge consecutive messages from one author within 5 min
hide_joins = false
mark_read = "on_focus"          # on_focus | on_view | manual
history_page = 50

[composer]
send = "enter"                  # enter | ctrl-enter
editor = ""                     # overrides $VISUAL/$EDITOR
terminal = ""                   # for $EDITOR; empty = $TERMINAL, then Omarchy's xdg-terminals.list
paste_threshold = 8             # lines pasted before the code-fence question

[emoji]
skin_tone = 0                   # 0 = default, 2..6 = Slack's tones; applied to the picker and to reactions

[images]
enabled = true
thumbnail_width = 480           # pixels, at scale 1
cache_mb = 256

[files]
download_dir = "~/Downloads"

[notify]
desktop = true
keywords = []                   # whole words that count as a mention
quiet_when_focused = true       # no notification for the conversation on screen while the window has focus

[store]
enabled = true
max_messages_per_channel = 5000
max_age_days = 90

[network]
concurrency = 4
user_agent = ""                 # empty = built-in browser-like string

[theme]
source = "auto"                 # auto | omarchy | builtin — auto uses Omarchy when its symlink exists
builtin = "auto"                # auto | dark | light | high-contrast — auto follows the desktop's color-scheme
user_css = true                 # layer ~/.config/slack-light/user.css on top

[ui]
reduced_motion = "auto"         # auto follows gtk-enable-animations
mouse = true

[keymap]
preset = "default"              # default | vim — vim adds j/k/g/G in lists and a modal composer

[keys]
# "<Control>k" = "jump_to"      # GTK accelerator syntax, one per action; `--list-actions` names them

[log]
level = "info"
file = false

[debug]
enabled = false                 # enables view-source and the raw event log window
```

Workspaces are **not** in the config file; they live in the keyring/auth store and are listed with `slack-light auth list`.

### 8.3 CLI

```
slack-light                          # run (or raise the running instance)
slack-light auth add                 # browser sign-in in a throwaway profile; --paste for the manual flow; validates with auth.test
slack-light auth add --oauth         # official app flow: opens the browser, local redirect listener
slack-light auth list | remove <team>
slack-light --anonymous              # demo mode with the mock backend; no credentials touched (for tests and screenshots)
slack-light --read-only              # no writes of any kind: no posts, marks, reactions, presence
slack-light --no-cache               # in-memory store
slack-light --doctor                 # GTK/Wayland, portal, theme source, browser, keyring, workspaces, cache
slack-light cache stats | trim | purge
slack-light unread --json            # for a waybar module, against the running instance
slack-light --write-config | --list-actions
slack-light --log-level debug --log-file path
```

`--read-only` and `--anonymous` are the safety valves: **any test that drives the application must pass `--anonymous`**, so a stray event can never post to a real workspace.

---

## 9. Error handling, logging, safety

- `thiserror` types per crate, `anyhow` at the binary boundary. Every `SlackError` carries the method name and `kind()`; the UI maps kinds to toasts (`Auth` → re-auth prompt; `RateLimited` → `⏸ 42 s`; `Permission` → "you cannot do that here"; `Shape` → "unexpected response, logged").
- `tracing` everywhere; a redaction layer scrubs `token`, `xox[a-z]-…`, `Cookie`, `Authorization`, emails and message text at `info` and above (message text is visible only at `trace`, off by default). Logs go to a file only, never to the terminal.
- Panic hook: write the panic and the last 200 log lines to `$XDG_STATE_HOME/slack-light/crash-<ts>.txt`, show one dialog pointing at it if GTK is still alive, exit 1. Nothing about credentials or message content is included.
- No telemetry, ever. TLS verification is never disabled. Subprocess stderr (`$EDITOR`, opener) is drained and surfaced, per the predecessor projects's NFR-13.

---

## 10. Testing strategy

| Layer | Method |
|---|---|
| `slk-core` parsers | unit tests on real captured shapes; property tests for `parse(emit(doc)) == doc`; **fuzz** targets for mrkdwn, rich_text and event JSON |
| `slk-ui` markup | `ast::to_pango` over the fixture corpus: valid markup (Pango's parser accepts it), every construct produces its tag; Block Kit widget trees asserted structurally |
| `slk-api` | `FixtureBackend` replays captured responses; robustness suite mutates fixtures (drop keys, null values, wrong types) and asserts no panic; `wiremock` for the rate gate and `429` handling |
| `slk-sync` | deterministic scenario tests: boot, event application, reconnect gap-fill, outbox re-keying in both echo orders |
| `slk-store` | migrations up from every past schema; FTS queries; retention |
| `slk-ui` components | `update()` driven with a test sender, no widgets — the fast layer |
| the application | started under a headless Wayland compositor (`weston --backend=headless`) against `MockBackend`; the **AT-SPI accessibility tree** walked and asserted on — the predecessor reconstructed the screen from the escape stream, this reads the tree; a control missing from it is a defect for the test and for a screen reader alike |
| CI | fmt, clippy `-D warnings`, build + test on Linux, MSRV job, the headless a11y suite, and the **offline-guarantee** job running parser suites in a network-less namespace |

**Fixtures come from a dedicated free test workspace** (`slk-dev`), created in M0, seeded with a known corpus of messages (every mrkdwn feature, a bot posting Block Kit, files, threads, reactions, custom emoji). Unlike the predecessor projects, this project can have a real account that is safe to write to — and no test ever touches any other.

---

## 11. Sequence sketches

### 11.1 Startup to first frame

```
main ─► load config ─► open store ─► read last session (workspace, conversation, cursor)
     ─► first frame from store (< 150 ms, sidebar badges from stored counts, marked "stale")
     ─► spawn core task ─► for each workspace: SyncEngine::start()
          boot ─► counts ─► Event::WorkspaceReady ─► sidebar badges go live
          connect ws ─► Live ─► gap-fill focused conversation ─► rows update in place
```

### 11.2 Sending a message

```
INSERT Enter ─► Command::Send{team, channel, thread, text}
  core: resolve @/# tokens ─► mrkdwn::emit ─► store.insert(Pending(local_id)) ─► Event::MessageUpserted
  outbox: backend.post ─► Ok(ts) ─► store.rekey(local_id → ts) ─► Event::MessageUpserted
  ws echo `message` ─► engine: matches local_id (or ts already present) ─► no duplicate
  Err(e) ─► store.mark_failed ─► Event::MessageUpserted (Failed) ─► toast
```

### 11.3 Opening a thread

```
NORMAL Enter on parent ─► Command::OpenThread(ref)
  store.thread(ref) ─► immediate render of cached replies
  backend.replies(ref) ─► store.upsert ─► Event::ThreadUpdated ─► re-render
  engine.mark_thread on view (policy) ─► Command::MarkThread ─► backend.mark_thread
```

---

## 12. Decisions recorded here

| # | Decision | Why |
|---|---|---|
| D1 | `raw_json` stored for every entity | parser fixes become migrations, not re-fetches; view-source works offline. Costs ~830 bytes per message, measured in M0 |
| D2 | One writer task for SQLite | WAL + single writer avoids `SQLITE_BUSY`; reads are cheap and parallel |
| D3 | Rendering never queries the store | frame-rate reads come from `ConversationView` in memory |
| D4 | Counts before history | correct badges with one request per workspace instead of one per conversation |
| D5 | No bulk prefetch | player-shaped, not scraper-shaped traffic — the account-safety stance carried over from the predecessor projects |
| D6 | Optimistic writes with local ids | the UI is instant; echo-order ambiguity is handled explicitly |
| D7 | Capabilities bit-set | two backends, one UI, honest disabled states |
| ~~D8~~ | ~~Emoji width table per terminal~~ | **Retired by ADR-001.** It was right, and it was solving a problem the medium had. Pango does not have it |
| D9 | Capability probes time out rather than block, and report "no answer" distinctly from "answered no" | A blocking read on something that ignores a query is a hang; the predecessor hit it on terminals and again in its own `--doctor`. Now applies to the DevTools handshake, the portal and the theme symlink |
| D10 | GTK4 through relm4, no libadwaita | The only native option on Wayland; libadwaita ignores themes by design and the requirement is to follow Omarchy's. [ADR-001](./ADR-001-native-gui.md) |
| D11 | Theme from Omarchy's `colors.toml`, generated to CSS, reapplied live | Omarchy does not theme GTK, so the application does it the way Omarchy's other tools do |
| D12 | Browser sign-in in a throwaway profile | The same credentials as manual paste with none of the copying; never the user's real profile; msga proved the flow |
| D13 | Engine on a tokio thread, UI on the GTK main thread, a channel between | Rule 2 unchanged; the interface can be rewritten again without touching Slack, which is exactly what just happened |
| D14 | The conversation is a virtualised `ListView`, never a `ListBox` | 5 000 realised rows of Pango and textures is the difference between 60 fps and Electron |

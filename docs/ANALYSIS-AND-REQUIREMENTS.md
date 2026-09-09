# slack-light — Analysis & Requirements

**A full-featured, lightweight Slack client for the Linux desktop, in Rust + GTK4: every workspace at once, threads, reactions, files with inline images, Block Kit, search — the official client's shape, as a native window that takes its colours from the desktop.**

| | |
|---|---|
| Document status | v0.2 — amended for the native-GUI pivot |
| Date | 2026-09-09 (v0.1: 2026-09-07) |
| Owner | petr |
| Scope | Product analysis, domain analysis of Slack's APIs, functional & non-functional requirements, architecture summary, interface summary, risks, roadmap |
| Out of scope | Implementation, exhaustive API schemas, test plans |
| Lineage | This document was written for `slktui`, a terminal client that reached M2 (parity core) before [ADR-001](./ADR-001-native-gui.md) replaced its interface with a native one. Everything about Slack — §4, §5, §7, the backend and storage requirements — was verified by that client and stands. The interface sections were rewritten; where a requirement was retired the row says so rather than vanishing |
| Slack-side results | [M0-FINDINGS.md](./M0-FINDINGS.md) — the session route, websocket, counts, drafts, storage, all measured live. §3 of it (terminal widths) is history now |
| GUI spike | [M0-GUI-SPIKE-PLAN.md](./M0-GUI-SPIKE-PLAN.md) — what the GTK client has to prove before M1 |
| Companion docs | [ADR-001-native-gui.md](./ADR-001-native-gui.md) — why native, why GTK4/relm4 · [SLACK-ACCESS-STRATEGY.md](./SLACK-ACCESS-STRATEGY.md) — which API route and why · [ARCHITECTURE.md](./ARCHITECTURE.md) — crates, data model, storage, sync engine, GTK runtime |
| Decided so far | Rust; **GTK4 via relm4, no libadwaita**; session-token backend primary with the official OAuth backend behind the same trait; **browser sign-in in a throwaway profile**; multiple workspaces at once; SQLite + FTS5 cache; Slack-like three-pane window; **themed from Omarchy's `colors.toml`**; files + inline images and Block Kit are v1 musts; desktop notifications, `$EDITOR`, link opening and clipboard in v1; GPL-3.0-or-later; docs in English |

---

## 1. Vision

> A Slack that feels like the desktop app — same sidebar, same threads, same reactions, same unread badges, same search — but starts in well under a second, uses a fifth of the memory, looks like the rest of your desktop, and never makes you reach for the browser during a working day.

Three things define the product:

1. **Parity, not a subset.** Every existing lightweight Slack client is a *lite* client: no threads, or no files, or no multiple workspaces, or no Block Kit, or read-only. The mental model here is the official client: workspaces, channels, DMs, threads, reactions, mentions, edits, files, search, presence, status, saved items. A user should not have to open the browser for routine work.
2. **Native and light.** A GTK4 window, not a webview: system fonts, cursor, scaling, input methods, clipboard and portals as every other application has them. First frame from the local cache in under 300 ms, event-driven with ~0 % idle CPU, under 120 MB resident with three workspaces. Keyboard-first — every action reachable without the mouse, remappable — because the target user tiles windows in Hyprland. Coloured by the desktop's theme (Omarchy's, when present) rather than by its own.
3. **Honest and durable.** Slack's private API is used the way the official web client uses it — one user, one browser-shaped session, no scraping, no export. Every external shape is parsed defensively and pinned by fixtures, so the day Slack changes something the client shows an empty pane and a log line, not a crash.

### Non-goals

- Huddles, calls, screen sharing — no public API; WebRTC against an undocumented signalling protocol is a separate project.
- Canvases and Lists beyond a link to open in the browser — no read API for their content.
- Bulk export, archival or scraping of workspaces. This is a client, not slackdump; that stance is what keeps it defensible (§5).
- Being a Slack *app* platform: no interactive Block Kit (modals, buttons that post back), no workflows.
- Anything but Linux/Wayland as a first-class target in v1. GTK4 builds on X11, macOS and Windows; none is verified before M5, and none is the point.
- A terminal interface. There was one; [ADR-001](./ADR-001-native-gui.md) records why it was the wrong surface for a medium full of images and threads.

---

## 2. Prior art & differentiation

| Project | Language | Auth / realtime | What it does | Gap we exploit |
|---|---|---|---|---|
| [slack-term](https://github.com/jpbruinsslot/slack-term) | Go | user token, legacy RTM | The classic terminal Slack; 6.6k stars | Stale since 2024-04, RTM path dies with classic apps on 2026-11-16; no threads, no files, one workspace |
| [wee-slack](https://github.com/wee-slack/wee-slack) | Python / WeeChat | `xoxc` + cookie, web-client websocket | The long-lived full client: threads, reactions, counts, typing | Requires WeeChat; IRC mental model; Python plugin, no local store, no images |
| [emacs-slack](https://github.com/emacs-slack/emacs-slack) | Elisp | `xoxc` + cookie, RTM-style websocket | Full client inside Emacs, alive (pushed 2026-09-01) | Requires Emacs |
| [slk](https://github.com/gammons/slk) | Go | `xoxc` + cookie, `wss-primary` websocket | Newest serious attempt: SQLite scrollback, kitty/sixel images; 341 stars, pushed 2026-09-06 | **Closest architectural precedent.** Single binary Go; we match its transport and outdo it on parity (threads, Block Kit, multi-workspace, search, notifications) |
| [sclack](https://github.com/haskellcamargo/sclack) | Python / urwid | legacy token, RTM | Pretty, alpha | Dead since 2022 |
| [slackdump](https://github.com/rusq/slackdump) | Go | `xoxc` + cookie, Edge API | Exporter, not a client | Best public reference for the internal endpoints; alive (v4.4.4, 2026-09-03) |
| [slackatui](https://github.com/MasonLiebe/slackatui), [slack_rust](https://github.com/oovets/slack_rust), [kurenn/slack-tui](https://github.com/kurenn/slack-tui) | Rust / Rust / Go | official OAuth + Socket Mode (or a broken `rtm.connect`) | 2026 experiments, 0–25 stars | Official-token clients: no presence, no typing, no counts, admin friction; none has traction |
| [slack-morphism](https://crates.io/crates/slack-morphism) | Rust crate | official tokens | Mature SDK (2.27.0, 2026-09-06): Web API, Socket Mode, Block Kit types | No cookie/session route — its connector cannot inject headers. Model types are a useful reference |
| [make-slack-great-again](https://github.com/punarinta/make-slack-great-again) (msga) | C++ / Qt 6 | `xoxc` + cookie via **browser sign-in in a throwaway profile**, DevTools protocol | The closest thing to this project: a native lightweight desktop client on the session route; 114 stars, 424 commits, maintained; also does Teams and e-mail | **Precedent that the route works in a GUI** and that users accept browser sign-in. Qt, not GTK — no desktop theming on a GTK-shaped desktop; scope sprawl into other services. Its sign-in flow is adopted here, its Slack-desktop-app cookie import is not |
| `slktui` (this project's predecessor) | Rust / Ratatui | `xoxc` + cookie, web-client websocket | Reached parity: multi-workspace, threads, images, Block Kit, search, notifications; 176 acceptance checks | Everything below the interface is reused verbatim. The interface is why this document exists in v0.2 |

**Differentiation statement:** there is no native GTK Slack client on the route the long-lived clients (wee-slack, emacs-slack, msga) survive on. `slack-light` is that client, with the things none of them have together: a local store for instant startup and offline search, several workspaces side by side, the desktop's own theme, and a Slack layer that has already carried a client through parity.

---

## 3. Users & scenarios

- **P1 — Hyprland developer on Omarchy (primary, = you).** Tiling compositor, three Slack workspaces (employer, a client, a hobby community), the Electron client eating 1.5 GB. Wants a real window that tiles like the others, follows the desktop theme when it changes, drives entirely from the keyboard, notifies correctly, and never misses a thread reply.
- **P2 — Any Wayland desktop user who is tired of Electron.** GNOME, Sway, KDE. Wants the same client; gets the built-in dark/light theme following the desktop's `color-scheme` instead of Omarchy's palette.
- **P3 — Privacy- and battery-conscious laptop user.** Wants a client that idles at 0 % CPU, stores its cache locally with a known retention, and can run read-only.

Dropped from v0.1: the SSH/tmux user. A terminal client served them; a native one cannot, and the trade was made deliberately.

**Key scenarios**

- S1: Launch → the sidebar with correct unread badges appears from the cache in < 150 ms → live counts replace them within a second → `Enter` on `# engineering` shows the conversation with the *New messages* line in the right place.
- S2: `ctrl-k` → type `eng` → `Enter` → type `@ali` `Tab` → `Enter` sends → the message appears instantly with a pending mark, then confirms; alice's reply arrives over the websocket a moment later; the desktop notification is suppressed because the window is focused and the conversation is open.
- S3: A bot posts a Block Kit deploy summary with a header, fields and a button → it renders as real widgets — a heading, a two-column field grid, a button that opens the URL.
- S4: A colleague drops a screenshot → the thumbnail renders inline; a click opens it full size in the window; `ctrl-s` saves it through the file portal.
- S5: Laptop suspends for an hour → on resume the websocket reconnects, the gap in the focused channel is filled, badges are refreshed from counts, and nothing was marked read by accident.
- S6: `/` → `from:@bob in:#design before:2026-09-01` → results grouped by conversation → `Enter` jumps to the message in context; `local:migration` searches the offline index across all three workspaces.
- S7: The window is tiled from full width to a third of the screen mid-conversation → the thread pane becomes a stacked page, the sidebar collapses to icons, the focused message stays in view, nothing re-fetches.
- S9: `omarchy-theme-set "Gruvbox"` while the client is open → it recolours within a second: sidebar, selection, links, mentions, code — no restart, no flash.
- S8: The employer's workspace revokes the session → a re-auth prompt appears for that workspace only; the other two keep working; its cached history stays readable.

---

## 4. Domain analysis — how Slack actually works for a third-party client

This section drives most of the hard requirements. Facts are from Slack's documentation and from the source of wee-slack, emacs-slack and slackdump, as researched on 2026-09-07; items marked *unverified* must be confirmed in M0.

### 4.1 Two routes in, one of them undocumented

| Route | Token | How you get it | What it gives you | What it costs |
|---|---|---|---|---|
| **Session (web client)** | `xoxc-…` token + `d=xoxd-…` cookie | Sign in with the browser; copy the token and cookie (DevTools, or `GET https://<ws>.slack.com` with the cookie and extract `api_token`); SSO works because the browser did it | Everything the web client can do: all Web API methods **plus** the internal ones — `client.userBoot`, `client.counts`, `drafts.*`, `saved.list`, `subscriptions.thread.mark`, `chat.command`, `users.prefs.*`, `search.modules.*` — and the web client's own websocket | Undocumented and unsupported; grey zone under the API ToS; has broken twice (2022-09, 2023-09) and been fixed by the community within days; cookie TTL shortened in Dec 2025 (still > 1 year); reportedly volatile on Enterprise Grid (*unverified*) |
| **Official app** | `xoxp-…` user token (+ `xapp-…` for Socket Mode) | Create a Slack app (a manifest we ship), install it into each workspace, OAuth in the browser | Documented Web API with user scopes; realtime via Socket Mode; stable, rotatable tokens | Admin approval in locked-down workspaces; **no presence, no typing, no counts, no drafts, no saved-for-later**; classic RTM is closed to new apps and shuts down for classic ones on 2026-11-16; possible 15-message history cap when the app is distributed outside its home workspace (§4.3) |
| Legacy tokens | `xoxs-`, `xoxa-`, test tokens | — | — | Dead. Not considered |

**Decision (see [SLACK-ACCESS-STRATEGY.md](./SLACK-ACCESS-STRATEGY.md)):** the session route is the primary backend because parity is the product; the official route is a second backend behind the same trait, validated in M0 and completed in M3, so a workspace that forbids one can use the other.

### 4.2 Realtime transport

- **Session route:** a websocket to `wss://wss-primary.slack.com/?token=<xoxc>&gateway_server=<TEAM>-1&slack_client=desktop&batch_presence_aware=1` with the `d` cookie on the handshake (wee-slack). It delivers the RTM-style event stream: `message` and its subtypes, `reaction_*`, `channel_marked`/`im_marked`/`thread_marked`, `user_typing`, `presence_change` (after `presence_sub`), `dnd_updated`, `user_change`, `channel_*`, `pin_*`, `star_*`, `emoji_changed`, `desktop_notification`, `user_huddle_changed`. Outgoing: `ping`, `typing`, `presence_sub`, and messages can be sent as frames. The URL is short-lived; reconnects must fetch a fresh one. emacs-slack pings every 10 s and reconnects on a 20 s timeout.
- **Official route:** Socket Mode (`apps.connections.open` with `xapp`) delivers Events API envelopes that must be acked; expect a `disconnect` every few hours with ~10 s notice. It **cannot deliver presence or typing**. Whether user-scoped events (`message.channels` on behalf of the user) arrive over Socket Mode is *unverified* — M0 tests it; the fallback is polling, which is expensive (§4.3).

### 4.3 Rate limits

- Web API tiers: 1 (1+/min), 2 (20+/min), 3 (50+/min), 4 (100+/min), per method per workspace; `chat.postMessage` ~1/s per channel; `429` with `Retry-After` seconds. Conversations, users and history methods are Tier 3.
- **The May 2025 change:** `conversations.history` and `conversations.replies` dropped to **1 request/minute and 15 objects** for *commercially distributed non-Marketplace apps* created after 2025-05-29 and new installs of such apps. Slack states explicitly that *internal customer-built apps are not impacted*. Dates circulating for existing installs (2025-09, 2026-03) come from third parties only and are *unverified*. Session tokens are not app tokens and no session-token client reports the cap (*inferred*).
- **Implication:** an app the user creates in their own workspace is exempt; the same app installed into a workspace they do not administer may not be. The history layer must therefore work within 15 messages/minute *when it has to* (page size ≤ 15, websocket deltas first, counts before history) and use bigger pages only when the backend reports the higher limit.

### 4.4 Reading state is the hard part

Slack's unread model is: per conversation a `last_read` timestamp, `unread_count`/`mention_count`, and per thread its own `last_read` and subscription. The session route's `client.counts` returns all of it in one call — `{id, last_read, latest, mention_count, has_unreads, history_invalid}` for every channel, DM and MPIM plus thread totals — which is why the official client can show correct badges before loading any history. The official route has no counts method: `conversations.info` returns `unread_count` for DMs only, and everything else needs `history(oldest=last_read)` per conversation.

Marking read is `conversations.mark(channel, ts)` (batch with a ~5 s timeout, as Slack advises), and threads via the undocumented `subscriptions.thread.mark`. Marks are broadcast to the user's other clients as `*_marked` events, so a mark done on the phone updates the terminal, and vice versa. Getting this exactly right — not marking read while the window is unfocused, not double-counting after reconnect, not losing a thread's unread state — is the deep dive in §7.

### 4.5 Content model

- **User messages are `rich_text` blocks.** The composer's output is `blocks: [{type: "rich_text"}]` with sections, lists, preformatted and quote elements and inline `text`/`emoji`/`link`/`user`/`usergroup`/`channel`/`date`/`broadcast`/`color` elements; the `text` field is a mrkdwn fallback. Render blocks first, mrkdwn second.
- **mrkdwn** is not Markdown: `*bold*`, `_italic_`, `~strike~`, `` `code` ``, ```` ``` ````, `>` quotes, no list syntax; tokens `<url|text>`, `<@U…>`, `<#C…|name>`, `<!subteam^S…|@h>`, `<!here>`, `<!channel>`, `<!everyone>`, `<!date^ts^{format}^url|fallback>`; `&`, `<`, `>` are HTML-escaped; `:emoji:` shortcodes.
- **App messages** carry Block Kit: `header`, `section` (with `fields` and `accessory`), `context`, `divider`, `image`, `actions`, `rich_text`; plus legacy `attachments` (also used for link unfurls).
- **Message subtypes** (35): `bot_message`, `me_message`, `channel_join/leave/topic/purpose/name/archive`, `file_share`, `message_changed`, `message_deleted`, `message_replied`, `thread_broadcast`, `pinned_item`, `tombstone`, `huddle_thread`, … Hidden subtypes are excluded from history.
- **Threads:** parents carry `thread_ts == ts`, `reply_count`, `reply_users` (max 5), `reply_users_count`, `latest_reply`, `subscribed`, `last_read`, `unread_count`; replies carry `thread_ts` of the parent; `reply_broadcast` copies a reply into the channel.
- **Files:** `url_private`/`url_private_download` and every `thumb_*` require `Authorization: Bearer <token>` (plus the `d` cookie on the session route). Upload is `files.getUploadURLExternal` → `POST` bytes → `files.completeUploadExternal`; `files.upload` was retired on 2025-03-11.
- **Emoji:** Slack's shortcode set matches the `emoji-data` dataset (`+1`, `hankey`, `simple_smile` quirks included); custom emoji via `emoji.list` with `alias:` entries; skin tones as `name::skin-tone-N`.
- **Saved for later:** `stars.*` still answers but has no user-facing effect; the web client uses the undocumented `saved.list`. **Reminders:** the `/remind` slash command via `chat.command`. **Scheduled messages:** `chat.scheduleMessage` and friends are official. **Drafts:** `drafts.*`, session route only.
- **Slack Connect:** channels flagged `is_shared`/`is_ext_shared`; external users have a foreign `team_id` and reduced profiles. **Enterprise Grid:** ids prefixed `W`/`E`, messages carry `team_id`/`source_team`/`user_team`, org tokens must pass `team_id` to listing methods, and the web client uses an extra `edgeapi.slack.com` for user and permission caches.
- **Canvases, Lists, huddles:** no read API worth using. Links only.

### 4.6 Library landscape (Rust)

| Need | Candidate | Assessment |
|---|---|---|
| Slack Web API + Socket Mode + Block Kit types | [`slack-morphism`](https://crates.io/crates/slack-morphism) 2.27 | Mature and very active. **Cannot do the session route**: no header/cookie injection in its connector, and it lacks `search.*`, `bookmarks.*`, `dnd.*`. Reference for model types only |
| Session route | [`slacko`](https://crates.io/crates/slacko) 0.2 | The one crate with `xoxc`+`xoxd` "stealth" support; 0 stars, abandoned after three releases. Reference for request shape only |
| Everything, our way | **in-house thin client** | ~40 methods, hand-written serde structs, `reqwest` + `tokio-tungstenite`, our own tier-aware limiter. Same choice ytmtui made for InnerTube, for the same reason: the fragile surface must be ours to fix |

**Recommended composition:** an in-house `slk-api` crate with two backends behind one `SlackBackend` trait; model types written by us, informed by slack-morphism's.

---

## 5. Legal & ethical constraints

- This is an **unofficial** client. Slack's API terms forbid exceeding rate limits, "excessive or abusive usage" and circumventing technical measures, and say undocumented parts of the API should not be relied on. The session route relies on undocumented behaviour. The README must say so plainly, before any sign-in instructions.
- Slack's own warning (via slackdump) applies: on some plans and security settings, automated or unusual access may trigger security alerts or notify workspace administrators. A user's *employer* may have policies about unofficial clients that are stricter than Slack's. This is the user's decision to make, informed.
- **Traffic must look like one person using one client.** One websocket per workspace, lazy history, counts before history, no bulk prefetch, no export, no polling loops, cached responses, backoff on `429`, a single honest `User-Agent`.
- **No export or archival feature, ever.** Downloading the file under the cursor is a client action; downloading a channel is not.
- **Credentials never leave the machine.** Keyring or 0600 file; never logged; redaction is enforced by a `tracing` layer, not by discipline; crash reports contain no tokens and no message text.
- **The local cache is the user's company data on the user's disk.** 0600, bounded retention, `--no-cache`, optional at-rest encryption, and a `slktui cache purge` command.
- **Read-only mode** exists and does what it says: no posts, no marks, no reactions, no presence changes.

---

## 6. Functional requirements

Priority: **M** = must (v1.0), **S** = should (v1.1), **C** = could (v1.2+), **W** = won't (this release).

### 6.1 Authentication & workspaces

| ID | Requirement | Pri |
|---|---|---|
| FR-A1 | **Sign in with your browser:** launch a Chromium-family browser in a throwaway temporary profile at Slack's sign-in page; the user signs in normally (SSO, 2FA); the `d` cookie and `xoxc` tokens are read over the DevTools protocol; the profile is wiped. **Never the user's real profile, never the Slack desktop app's data.** Validated by a live `auth.test`. Manual paste of token + cookie remains as the fallback, with clear instructions | M |
| FR-A2 | Credentials in the OS keyring when available (Secret Service), else `~/.config/slack-light/auth.json` mode 0600 | M |
| FR-A3 | **Multiple workspaces connected at once**, each with its own backend, token, websocket and rate gate; unified sidebar and notifications; `ctrl-1..9` switching | M |
| FR-A4 | Detect an expired or revoked session per workspace, prompt for re-auth without disturbing the other workspaces or losing drafts; cached history stays readable | M |
| FR-A5 | Official OAuth backend (`slack-light auth add --oauth`): shipped app manifest, browser flow with a loopback redirect, token rotation, Socket Mode realtime; honest `capabilities()` so the UI disables what it cannot do | S |
| FR-A6 | Enterprise Grid: org-level session works across the org's workspaces; `team_id` passed where required; Edge API used for user lookups when present | S |
| FR-A7 | `--read-only` and `--anonymous` (mock backend, no credentials touched) modes | M |

### 6.2 Sidebar & navigation

| ID | Requirement | Pri |
|---|---|---|
| FR-N1 | Sidebar per workspace with Starred, Channels, Direct messages, Apps sections; collapsible; other workspaces collapsed with their badge; muted conversations dimmed | M |
| FR-N2 | Unread and mention badges correct **before any history is fetched**, from counts (session) or from stored state + lazy verification (official) | M |
| FR-N3 | **Jump to** (`ctrl-k`): fuzzy search over every conversation and person in every workspace | M |
| FR-N4 | Previous/next conversation and previous/next *unread* conversation keys; back/forward history (`ctrl-o` and `[` / `]`; `ctrl-i` too where the terminal distinguishes it from Tab) | M |
| FR-N5 | Sidebar options mirroring the official client: unread first, hide read, recents section, custom section order | S |
| FR-N6 | Slack's own sidebar sections (custom sections from `users.prefs`) when the backend provides them | C |

### 6.3 Conversations & history

| ID | Requirement | Pri |
|---|---|---|
| FR-H1 | Open any conversation the user is a member of: public, private, DM, group DM, Slack Connect shared; browse public channels not joined with a `join` action | M |
| FR-H2 | History from the local store instantly, then verified/extended from the API; infinite scroll upward with a loading indicator; "beginning of conversation" marker | M |
| FR-H3 | Live updates via the websocket: new messages, edits, deletions, reactions, thread reply counts, marks, typing indicators, presence | M |
| FR-H4 | **Gap fill** after reconnect or resume: detect gaps per conversation from counts/latest, fill the focused and mentioned conversations first, mark unfillable spans visibly | M |
| FR-H5 | Day separators, *New messages* line, consecutive-author grouping, edited/deleted markers, system messages as dim single lines | M |
| FR-H6 | Jump to a message in context from search, mentions, threads or a pasted permalink, with history loaded around it and a two-second highlight | M |
| FR-H7 | Follow mode and a `↓ N new` chip when scrolled up | M |
| FR-H8 | Conversation header with topic, purpose, member count, star, mute state; channel info modal with members and presence | M |
| FR-H9 | Offline: every cached conversation readable with an `(offline)` marker; sends are queued and delivered in order on reconnect | S |

### 6.4 Composing

| ID | Requirement | Pri |
|---|---|---|
| FR-M1 | Multi-line composer with emacs-style editing, `Enter` sends, `shift-Enter`/`alt-Enter`/`ctrl-j` newline, bracketed paste with a snippet/code-block prompt above 8 lines | M |
| FR-M2 | Completion popups for `@user` (conversation members first, then workspace, `@here`/`@channel`, user groups), `#channel`, `:emoji:` (Unicode and custom), `/command` | M |
| FR-M3 | Outgoing text converted to Slack's wire format: mentions to `<@U…>`, channels to `<#C…|name>`, entities escaped, shortcodes preserved; formatting markers passed through as mrkdwn | M |
| FR-M4 | Optimistic send: instant `⏳` row, confirmed on ack, `✗ failed — r to retry` on error; echo deduplication in both orders | M |
| FR-M5 | Edit own last message (`↑` on empty composer, `e` on a message), delete own message with confirmation | M |
| FR-M6 | Reply in thread from the thread pane's own composer; "also send to channel" toggle (`reply_broadcast`) | M |
| FR-M7 | Drafts per conversation and per thread, persisted in the store; restored on restart; synced with Slack's drafts when the backend supports it | S (local) / C (synced) |
| FR-M8 | `$EDITOR` escape hatch (`ctrl-x`) with the terminal restored around the child and the draft preserved on any failure | M |
| FR-M9 | Slash commands: local built-ins (`/me`, `/status`, `/dnd`, `/away`, `/mute`, `/join`, `/leave`, `/msg`, `/topic`, `/invite`, `/search`, `/upload`, `/thread`, `/edit`) and pass-through of workspace commands (`/remind`, `/giphy`, …) via `chat.command` where supported | S |
| FR-M10 | Typing indicator sent while composing (throttled to one per 3 s), received indicators shown | S |
| FR-M11 | Schedule a message (`/schedule 09:00 text` or `ctrl-s` in the composer); list and cancel scheduled messages | C |

### 6.5 Threads, reactions & message actions

| ID | Requirement | Pri |
|---|---|---|
| FR-T1 | Thread pane: parent on top, replies below, own composer; open from any message; unread thread replies badge; subscribe/unsubscribe | M |
| FR-T2 | **Threads view**: every followed thread with unread state, newest activity first | M |
| FR-T3 | React with the emoji picker (search, categories, recent, skin tone, custom) or with `1`–`9` for frequent reactions; toggle off; reaction chips show counts and who | M |
| FR-T4 | Copy message text and permalink; open links and files under the cursor; pick among several | M |
| FR-T5 | Save for later (Later list), pin/unpin, pinned items view | S |
| FR-T6 | Quote a message into the composer; forward/share a message to another conversation | S |
| FR-T7 | View message source (raw JSON) behind `debug.enabled` | S |

### 6.6 Search

| ID | Requirement | Pri |
|---|---|---|
| FR-S1 | Workspace search with Slack's modifiers (`from:`, `in:`, `before:`, `after:`, `has:`, `is:`) via `search.messages`; results grouped by conversation; jump in context | M |
| FR-S2 | **Local full-text search** (`local:` prefix or a toggle) over the SQLite FTS5 index across all workspaces, instant and offline | M |
| FR-S3 | File search (`search.files`) | S |
| FR-S4 | Search history recallable with `↑` | S |

### 6.7 People, presence & status

| ID | Requirement | Pri |
|---|---|---|
| FR-P1 | User profiles: name, title, status, timezone with local time, presence; open a DM from a profile | M |
| FR-P2 | Presence glyphs in the sidebar and member lists, subscribed only for visible users (`presence_sub`), refreshed by events | M (session) / S (official, polled) |
| FR-P3 | Set own presence (`/away`, `/active`) and status with emoji and expiry; DND snooze/end | S |
| FR-P4 | Member list of a conversation with filtering; user groups resolved in mentions | M |
| FR-P5 | External (Slack Connect) users marked `⇄`, deactivated users struck through | M |

### 6.8 Files & images

| ID | Requirement | Pri |
|---|---|---|
| FR-F1 | Files shown per message with name, size, type; download to a configured directory with progress; open after download | M |
| FR-F2 | **Inline images** as native textures: thumbnail in the message, click for full size in an in-window viewer with zoom and save; lazy fetch when the row is realised; on-disk media cache with an LRU bound; a placeholder of the final size before the fetch completes | M |
| FR-F3 | Upload from the file portal (`ctrl-u`), by **drag-and-drop** onto the conversation, or by pasting an image from the clipboard; to the conversation or thread, with a comment | M |
| FR-F4 | Snippets/text files: first 12 lines inline with syntax highlighting, expandable | S |
| FR-F5 | Paste an image from the clipboard to upload | **M** — folded into FR-F3; trivial with GDK's clipboard |
| FR-F6 | Custom emoji rendered as small inline images | **S** — was C; a texture in a label is not a hard problem any more |

### 6.9 Block Kit & app messages

| ID | Requirement | Pri |
|---|---|---|
| FR-B1 | Render `header`, `section` (text, fields, accessory placeholder, URL buttons), `context`, `divider`, `image`, `actions` (URL buttons openable; others inert and labelled), `rich_text`; unknown blocks as a labelled placeholder; never an error | M |
| FR-B2 | Legacy `attachments`: title, text, fields, image, colour bar; link unfurls rendered as dim quoted blocks | M |
| FR-B3 | Bot and app identity: name, `⚙` glyph, app colour on the gutter | M |
| FR-B4 | Interactive elements (buttons that post back, selects, modals) | W |

### 6.10 Notifications & unread

| ID | Requirement | Pri |
|---|---|---|
| FR-U1 | Unread/mention counters maintained from events between count refreshes; never double-count; consistent with the phone after a `*_marked` event | M |
| FR-U2 | Mark-read policy: on focus (default), on view, or manual; `M`/`U` explicit marks; threads marked separately; never mark while the terminal is unfocused where focus is detectable | M |
| FR-U3 | Desktop notifications for DMs, mentions, keywords, followed-thread replies and "all messages" conversations; suppressed in DND, for muted conversations, and for the focused conversation; throttled and coalesced | M |
| FR-U4 | Use Slack's own `desktop_notification` events when the backend provides them; use the per-channel notification preference from prefs when available | S |
| FR-U5 | Terminal bell and OSC 9 / OSC 777 notifications as independent toggles for SSH | M |
| FR-U6 | Mentions & reactions activity view (`@`) | M |
| FR-U7 | Global unread summary in the status bar; `alt-shift-↑/↓` walks unread conversations across workspaces | M |

### 6.11 Channel management

| ID | Requirement | Pri |
|---|---|---|
| FR-C1 | Join/leave, star/unstar, mute/unmute, set topic/purpose, invite | S |
| FR-C2 | Create channel (public/private), archive, rename; create group DM | C |
| FR-C3 | Bookmarks list in the conversation header | C |

### 6.12 Interface & interaction

| ID | Requirement | Pri |
|---|---|---|
| FR-I1 | Slack-like three-pane window: sidebar, conversation, thread pane; composer; status line. Panes resize by dragging and collapse when the window is narrow (thread becomes a stacked page, sidebar collapses to icons); usable down to 800×500 | M |
| FR-I2 | **Keyboard-first:** every action reachable without the mouse; `ctrl-k` jump-to, `ctrl-1..9` workspaces, arrow keys in every list, `Esc` always goes back; **fully remappable** through `slk-config`'s action names, bound as GTK shortcuts; an optional `vim` preset adds `j`/`k`/`g`/`G` in lists and a modal composer | M |
| FR-I3 | Shortcuts window (`ctrl-?`) generated from the live keymap; command palette (`ctrl-shift-p`) listing every action with its binding | M |
| FR-I4 | Mouse and touchpad: everything clickable, kinetic scrolling, drag-resize panes, drag-and-drop files, right-click message menu | M |
| FR-I5 | **Theming:** on Omarchy, colours generated from the current theme's `colors.toml` and re-applied live when the theme changes; elsewhere a built-in dark and light theme following the desktop's `color-scheme`; a user CSS file layered on top for the rest | M |
| FR-I6 | Toasts for errors and confirmations; loading indicators — **the main thread never blocks on network I/O** | M |
| FR-I7 | Text is Pango: emoji, CJK, RTL, ligatures, selection and copy, clickable links, hover on mentions — all native. *The per-terminal width probe (FR-K6) and the shortcode fallback are retired* | M |
| FR-I8 | Session restore: window size and pane widths, last workspace, conversation, scroll position, open thread | S |
| FR-I9 | Accessibility through GTK's AT-SPI tree: every control named and reachable, focus order sensible, screen-reader usable; `gtk-enable-animations` honoured for reduced motion; the high-contrast theme. No colour-only information | S |
| FR-I10 | **Wayland-native:** fractional scaling and HiDPI, server-side decorations where the compositor prefers them (Hyprland does), a stable `app_id` for window rules, single instance with activation raising the existing window | M |

### 6.13 System integration

| ID | Requirement | Pri |
|---|---|---|
| FR-X1 | Open links and downloaded files through GIO's default handlers (the portal when sandboxed) | M |
| FR-X2 | Clipboard through GDK: text, permalinks, and images both ways | M |
| FR-X3 | Desktop notifications over D-Bus (`notify-rust`; works with mako, swaync, dunst); clicking one raises the window on that conversation; the unread count as a badge where the notification daemon shows one | M |
| FR-X4 | File chooser and save dialogs through `xdg-desktop-portal` (Hyprland ships its own portal), so they look like the desktop's | M |
| FR-X5 | CLI/IPC against the running instance: `slack-light unread --json` for a **waybar module**, `slack-light send`, `slack-light status` | **S** — was C; a status bar is how a Hyprland user sees an unread count, there is no tray |
| FR-X6 | Idle detection (`ext-idle-notify` on Wayland, logind fallback) to set presence away automatically | S |
| FR-X7 | `$EDITOR` escape hatch (`ctrl-x`): the draft opens in the user's editor **in their terminal** (`$TERMINAL`, or Omarchy's `xdg-terminals.list`); the composer re-reads it when the editor exits. Any failure keeps the draft | M |

### 6.14 Configuration, cache & diagnostics

| ID | Requirement | Pri |
|---|---|---|
| FR-K1 | TOML config at `$XDG_CONFIG_HOME/slack-light/config.toml`, hot-reload on save; `--write-config` writes a commented default; a bad entry is reported and skipped | M |
| FR-K2 | SQLite store with FTS5; retention by count and age applied at start-up; starred/pinned exempt; `slack-light cache stats\|trim\|purge` | M |
| FR-K3 | Media cache (thumbnails, images, custom emoji) bounded by size, LRU | M |
| FR-K4 | `--log-level`, `--log-file`, redaction layer; `slack-light --doctor` reporting GTK and Wayland (compositor, scale, portal, decorations), theme source (Omarchy palette or built-in), a drivable browser for sign-in, keyring, each workspace's token validity, websocket reachability, cache stats | M |
| FR-K5 | `--no-cache` in-memory mode; optional SQLCipher build feature | S |
| ~~FR-K6~~ | ~~Measured grapheme widths~~ — **retired by ADR-001.** Pango measures text; there is nothing to probe. Kept as a row so the id is not reused | — |
| **FR-K7** | **Capability detection distinguishes "answered no" from "did not answer"**, and `--doctor` reports which. A query that goes unanswered must time out, never block. Now applies to the DevTools handshake, the portal, and the theme symlink rather than to terminal queries | **M** |

### 6.15 Account safety

| ID | Requirement | Pri |
|---|---|---|
| FR-Z1 | Traffic resembles one client: one websocket per workspace, lazy history, counts before history, no bulk prefetch, no polling loops | M |
| FR-Z2 | A global gate on requests in flight, with `Retry-After` honoured account-wide rather than per method; never retry-loop a failing request; visible `⏸` state with the seconds remaining | M |
| FR-Z3 | No export, archive or bulk-download feature, ever | M |
| FR-Z4 | README states the ToS grey zone, the possibility of admin alerts, and the employer-policy question plainly, before any auth instructions | M |
| FR-Z5 | `--read-only` mode performs no writes of any kind; `--anonymous` never touches credentials; **every automated test drives `--anonymous`** | M |

---

## 7. Deep dive — reading state and sync

This is the feature that separates a client people trust from one they keep the browser open next to. If badges lie, or a message gets marked read that was never seen, or a reconnect drops a thread reply, the client is worse than useless: it is misleading.

### 7.1 The model

Per conversation: `last_read: Ts`, `latest: Ts`, `unread`, `mentions`, `history_invalid` (Slack's own flag that counts need a refetch). Per followed thread: `last_read`, `unread`, `subscribed`. Per message: whether it mentions self (direct, `@here`/`@channel`, user group, keyword).

Invariants the engine maintains:

1. `unread == count(messages in store with ts > last_read, not from self, not hidden subtype)` whenever the span from `last_read` to `latest` is fully held; otherwise `unread` comes from the last counts response plus events since.
2. A `*_marked` event **replaces** `last_read` and triggers a recount from the store; it never increments or decrements.
3. Events arriving during a counts request are applied after it, in order (a sequence number on the event stream; the counts response carries the sequence at which it was taken, or, when it does not, the engine buffers events for the request's duration).
4. Self-sent messages never count as unread and never trigger notifications; a message from self on another device *does* advance `last_read` implicitly (Slack marks on send).

### 7.2 When to mark

| Policy | Marks read when |
|---|---|
| `on_focus` (default) | the conversation pane has focus **and** follow mode is on **and** the terminal window has focus (where the terminal reports focus events; assumed focused otherwise) — mark to the newest visible message, debounced 1 s |
| `on_view` | same, but without requiring pane focus (reading in the sidebar overlay counts) |
| `manual` | only `M` |

Threads follow the same policy in the thread pane, independently. Leaving a conversation with `U` (mark unread here) sets `last_read` to just before the cursor.

### 7.3 Reconnect and gap fill

```
ws closed ─► Reconnecting(backoff) ─► fresh URL ─► connected ─► Live
   on Live:
     1. counts()                      → per conversation: latest_server vs span.newest
     2. for focused + mentioned conv: history(oldest=span.newest) pages (≤5) → merge span
     3. others: badge from counts; history on open
     4. resubscribe presence for visible users
     5. flush outbox in order; dedupe by local_id
```

`history_invalid` from counts forces step 2 for that conversation regardless. A span that could not be closed in five pages is marked broken and rendered as `── history gap · Enter to load ──`, so the user knows rather than wonders.

### 7.4 Official-backend degradation

Without counts, the engine keeps `last_read` from the store, learns `latest` from Socket Mode events, and verifies conversations lazily on open or when their sidebar row becomes visible, at most one `history` call per conversation per 5 minutes and never more than the tier allows. Presence and typing are simply absent (`Capabilities` says so; the sidebar shows no presence glyphs rather than stale ones).

### 7.5 Verification

Every rule above is a deterministic scenario test in `slk-sync`: a scripted event stream plus scripted API responses, asserting the store and the emitted UI events. Order-dependent cases (echo before ack, mark during counts, delete of a message not yet stored, reply to a parent not yet stored) are explicit tests, not hopes.

---

## 8. Non-functional requirements

| ID | Requirement |
|---|---|
| NFR-1 | **Startup:** first frame from the local store < 300 ms warm (GTK initialisation is most of it); live counts within 1 s of network availability; UI interactive before any network call completes |
| NFR-2 | **Responsiveness:** every keypress acknowledged within one frame (< 16 ms); zero blocking I/O on the main thread; rendering never queries SQLite; scrolling a 5 000-row conversation holds 60 fps on an integrated GPU |
| NFR-3 | **Idle cost:** ~0 % CPU when nothing changes — no periodic redraw; < 1 % while a busy channel streams messages |
| NFR-4 | **Memory:** < 120 MB RSS with three workspaces connected and 5 000 messages loaded in the open conversation; measured in the spike and in CI. The official client is 500 MB and up |
| NFR-5 | **Resilience:** no panics reach the user; the panic hook writes a report and shows one dialog; backend shape failures degrade to a visible empty/error state; a workspace failing never affects another |
| NFR-6 | **Desktop compatibility:** Wayland first — Hyprland (verified, with Omarchy's configuration), Sway, GNOME, KDE; X11 through GTK's backend, unverified. Correct behaviour on tiling, fractional scaling, focus changes, theme changes, suspend/resume |
| NFR-7 | **Portability:** Linux primary; other GTK platforms may build and are not a goal; a single binary depending on the system GTK 4.14+ and nothing else at runtime beyond D-Bus |
| NFR-8 | **Accessibility:** AT-SPI tree complete and named; keyboard-complete; no colour-only information; high-contrast theme; reduced motion via the GTK setting |
| NFR-9 | **Security:** credentials in the keyring or 0600; redacted logs; TLS verification never disabled; no telemetry; cache 0600 with bounded retention |
| NFR-10 | **Maintainability:** backends behind a trait with recorded-fixture tests from a dedicated test workspace, so a Slack shape change is a contained fix |
| NFR-11 | **Observability:** `tracing` spans, `--log-file`, `--doctor`, an in-app raw event log behind `debug.enabled` |
| NFR-12 | **Build:** stable Rust, MSRV set by gtk4-rs/relm4 (verify in the spike; expected ≥ 1.83); the only system dependency is GTK 4.14+ development files (bundled SQLite, rustls); Linux needs D-Bus for notifications and the keyring at runtime |
| NFR-13 | **No subprocess may discard its stderr** (`$EDITOR`, opener, browser) — drain and surface it, per ytmtui's finding |
| NFR-14 | **Offline CI:** parser, render and sync suites run with networking disabled; a red CI means our code broke, not that Slack changed overnight |

---

## 9. Proposed architecture (summary — see [ARCHITECTURE.md](./ARCHITECTURE.md))

### 9.1 Crate layout

```
slack-light/
├── crates/
│   ├── slk-core/      ids, Ts, domain types, rich-text AST, mrkdwn ⇄ AST, emoji, permalinks   (pure, carried over)
│   ├── slk-api/       SlackBackend trait; session + oauth + mock backends; websocket; rate gate  (carried over)
│   ├── slk-store/     SQLite + FTS5, migrations, retention                                     (carried over)
│   ├── slk-sync/      per-workspace engine: boot, counts, history, gap fill, outbox, notifications (carried over)
│   ├── slk-config/    TOML config, action names, keymap                                        (carried over)
│   ├── slk-notify/    desktop notifications over D-Bus                                         (carried over)
│   ├── slk-auth/      browser sign-in (DevTools protocol), keyring, manual paste               (new)
│   ├── slk-theme/     Omarchy colors.toml → GTK CSS; built-in themes; live reload              (new)
│   └── slk-ui/        relm4 components: window, sidebar, conversation, thread, composer, modals;
│                      AST → Pango markup; Block Kit → widgets; keymap runtime as GTK shortcuts  (new)
└── src/main.rs (GApplication, CLI, wiring), src/bin/probe.rs, seed.rs, wscheck.rs (dev-tools)
```

`slk-core` and `slk-config` are I/O-free, so the two parsers — the parts most exposed to Slack's shapes — are developed against fixtures long before any window opens. The three new crates replace `slk-tui`, `slk-render` and `slk-art`.

### 9.2 Concurrency model

Message passing with one owner per state: the GTK main thread owns the relm4 component models; a tokio runtime on its own thread hosts one engine task per workspace, each owning its connection; each engine owns a store connection (WAL, single writer per connection). Commands flow down over a channel, events flow up into the main context, nothing shares a mutex. GTK redraws only what changed.

### 9.3 Key abstractions

```rust
trait SlackBackend {                       // session / oauth / mock / fixture
    async fn boot(&self) -> Result<Boot>;
    async fn counts(&self) -> Result<Counts>;
    async fn history(&self, ch: &ChannelId, q: HistoryQuery) -> Result<Page<Message>>;
    async fn post(&self, ch: &ChannelId, thread: Option<&Ts>, text: &str, opts: PostOpts) -> Result<Ts>;
    async fn mark(&self, ch: &ChannelId, ts: &Ts) -> Result<()>;
    async fn connect(&self) -> Result<Box<dyn EventStream>>;
    fn capabilities(&self) -> Capabilities;
    /* … */
}

struct Doc(Vec<BlockNode>);                // rich text AST, produced by rich_text::parse and mrkdwn::parse
struct SyncEngine { state: SyncState, backend: Arc<dyn SlackBackend>, store: StoreHandle, /* … */ }
```

### 9.4 Tech stack

| Concern | Choice | Why / risk |
|---|---|---|
| GUI | **GTK 4.14+** via **gtk4-rs**, **relm4 0.11** (no libadwaita) | Native on Wayland; Elm-shaped components; relm4 is 0.x and will break once or twice — accepted (R14) |
| Theming | `gtk::CssProvider` at application priority, CSS generated from Omarchy's `colors.toml`; `gio::FileMonitor` on the current-theme symlink | Omarchy does not theme GTK, so the app themes itself the Omarchy way |
| Sign-in | Chromium in a throwaway profile, driven over the DevTools protocol (`--remote-debugging-pipe`), `tokio-tungstenite` or a pipe transport | msga's proven flow; Firefox cannot be driven this way |
| Async | tokio on a dedicated runtime thread; `relm4::Sender` / `glib::MainContext` channel into the UI | websocket, HTTP, store; the main thread never awaits |
| Slack HTTP | **in-house** over reqwest 0.13 (`json`, `form`, `multipart`, `stream`, rustls default) | Fixed `Cookie: d=…` via default headers; no cookie jar |
| Websocket | tokio-tungstenite 0.30 (`rustls-tls-native-roots`) | Auto-pongs; we add ping timers and reconnect |
| Composer | `gtk::TextView` with completion popover | Multi-line, undo, selection, IME — for free |
| Images | `gdk::Texture` from bytes, `gtk::Picture`; an in-window viewer | Nothing to detect, nothing to probe |
| Rich text | AST → Pango markup in `gtk::Label` (spike decides vs `TextView` + tags) | Selection, links, emoji, CJK, RTL native |
| Storage | rusqlite 0.40 (`bundled`, FTS5 is compiled in) + jiff | Single writer thread; WAL |
| Secrets | keyring 4.2 (`v1` stores) + 0600 file fallback | Needs Secret Service on Linux at runtime |
| Notifications | notify-rust 4.18 | MSRV 1.89 |
| Clipboard / open | `gdk::Clipboard` · `gio::AppInfo::launch_default_for_uri` | Native; portals when sandboxed |
| Emoji | vendored `emoji-data` shortcode map (build.rs → phf) + emojis 0.9 fallback + workspace `emoji.list` | **Confirmed by M0 measurement, not just research:** `:thinking_face:` is a Slack/emoji-data name that the gemoji-based `emojis` crate does not resolve. Vendoring is required, not optional |
| Text width | — | Retired with FR-K6. Pango does this |
| Time | jiff 0.2 | Local tz, `Ts` parse; aligned with rusqlite and slack-morphism |
| Config / CLI | toml 1.1 + etcetera · clap 4.6 | XDG layout on every platform |
| Errors / logging | thiserror 2 + anyhow + tracing + tracing-appender | Redaction layer |
| Tests | component `update()` with a test sender · AT-SPI tree under headless weston · wiremock · proptest · cargo-fuzz | Fixtures from the test workspace; the a11y tree is the screen |
| Licence | **GPL-3.0-or-later** | Free choice; no dependency forces it |

---

## 10. Interface design (summary — a `GUI-SPEC.md` follows the spike)

### 10.1 Main window

One window, three panes, the official client's arrangement — because parity is the point and nobody has to learn it:

```
┌───────────────────────────────────────────────────────────────────────────────┐
│ acme ▾ 2/3            ⌕ ctrl-k jump to…                     ● Active · petr   │  header
├──────────────┬────────────────────────────────────────┬───────────────────────┤
│ Starred      │ # engineering  ★  42 members            │ Thread · # engineering│
│  # eng    ●3 │ Topic: v2.4 today                       │ ───────────────────── │
│ Channels     │ ── Today ──                             │ alice  09:12          │
│  # general 2 │ alice 09:12  Deploy of v2.4 finished ✅ │ Deploy of v2.4 …      │
│  # design    │              ↳ 3 replies · last 10:01   │ bob    09:20          │
│  🔒 leads  1 │ bob   09:15  nice 🎉                    │ was the migration     │
│ Direct       │              👍 2  🎉 1                  │ included?             │
│  ● alice  ●1 │ ci-bot 09:31 ┃ build #4412 passed       │                       │
│  ○ bob       │              ┃ main · 3m 12s  [Open]    │                       │
│ other-corp 12│ alice 09:41  [screenshot.png thumbnail] │ ┌───────────────────┐ │
│ hobby     ●1 │ ── New messages ──                      │ │ Reply…            │ │
│              ├─────────────────────────────────────────┤ └───────────────────┘ │
│              │ Message #engineering                    │                       │
├──────────────┴────────────────────────────────────────┴───────────────────────┤
│ ✓ connected · 3 workspaces · ↑ 2 mentions                                    │  status
└───────────────────────────────────────────────────────────────────────────────┘
```

The sidebar and the thread pane are `gtk::Paned` children with remembered widths; below a width threshold the thread stacks over the conversation and the sidebar collapses to icons with badges. Everything in the sidebar and the conversation is a virtualised `ListView`.

### 10.2 Keyboard (default, abridged)

| Key | Action | | Key | Action |
|---|---|---|---|---|
| `ctrl-k` | Jump to | | `ctrl-1..9` | Workspace N |
| `↑` / `↓` | Move in the focused list | | `Enter` | Open (conversation, thread, link) |
| `Esc` | Back / close | | `ctrl-Enter` or `Enter` | Send (configurable) |
| `ctrl-r` | React | | `ctrl-t` | Reply in thread |
| `ctrl-e` | Edit last message | | `ctrl-shift-c` | Copy permalink |
| `ctrl-f` | Search | | `ctrl-shift-f` | Search offline |
| `ctrl-u` | Attach | | `ctrl-x` | Draft in `$EDITOR` |
| `alt-↑` / `alt-↓` | Previous / next conversation | | `alt-shift-↑/↓` | Previous / next unread |
| `ctrl-shift-p` | Command palette | | `ctrl-?` | Shortcuts |

All remappable through `slk-config`; the `vim` preset adds `j`/`k`/`g`/`G` in lists and a modal composer. The shortcuts window is generated from the active map.

---

## 11. Risks & mitigations

| # | Risk | Impact | Likelihood | Mitigation |
|---|---|---|---|---|
| R1 | **Session route breaks** (Slack changes the websocket, the token form, or internal endpoints) | Realtime or parity features stop until fixed | Medium — happened 2022-09 and 2023-09, fixed by the community within days | Everything behind `SlackBackend`; fixtures pin every shape; websocket envelope and endpoints isolated in one module; the official backend as a working alternative; `--doctor` diagnoses |
| R2 | **Account or workspace action** for using an unofficial client | Loss of access to a workspace; awkward conversation with an admin | Low (no documented ban; wee-slack/emacs-slack have run for years) — higher on Enterprise Grid and locked-down plans | FR-Z1–Z5: client-shaped traffic, no export, honest README; the official backend for workspaces where policy demands it |
| R3 | **Undocumented shapes change** (`client.counts`, `client.userBoot`, `drafts`, websocket events) | Silent wrong badges or empty panes | Medium | Defensive parsing, `raw_json` retained for re-derivation, robustness suite mutating fixtures, `history_invalid` honoured |
| R4 | **History rate limits** (15/min on distributed apps; Tier 3 generally) | Slow backfill, `⏸` states | Medium on the official backend, low on session | Counts before history, websocket deltas, page size adapted per backend, local store means nothing is fetched twice |
| ~~R5~~ | ~~Emoji and CJK width mismatches~~ | — | — | **Closed by ADR-001.** It was measured, solved, and then made irrelevant: Pango lays out text |
| R6 | **Websocket reliability**: silent drops, laptop sleep, NAT timeouts | Missed messages, stale state | High (it *will* happen daily) | Ping/pong timers, jittered backoff, gap fill designed as the normal path (§7.3), `history_invalid` |
| R7 | **Block Kit sprawl** — apps post blocks we do not handle | Unreadable bot messages | Medium | Render the documented subset, `text` fallback, labelled placeholders, a corpus of real bot messages in fixtures |
| ~~R8~~ | ~~Composer UX in a terminal~~ | — | — | **Closed by ADR-001.** `gtk::TextView` distinguishes every modifier and has IME |
| R9 | **Scope creep** — this document is big | Never ships | **High** | Strict M/S/C gating; M0–M2 is a complete usable client; everything else is additive |
| R10 | **Enterprise Grid quirks** — token rotation, Edge API, `W` ids, `team_id` everywhere | Grid users cannot use it | Medium (one report of hourly rotation, *unverified*) | Ids qualified by team from day one; Edge API behind the backend; Grid verification is an explicit M3 item with a Grid volunteer |
| R11 | **Cache holds company data on disk** | Leak if the disk is shared or stolen | Low probability, high impact | 0600, retention limits, `--no-cache`, SQLCipher feature, `cache purge`, README says where it lives |
| R12 | **Official backend cannot deliver user events over Socket Mode** (*unverified*) | Official route degrades to polling | Medium | Tested in M0 (§13); if it fails, the official backend is documented as "no realtime" and polls the focused conversation only |
| R13 | **Classic RTM shutdown 2026-11-16** confuses users of other clients migrating | Support noise, not a defect | — | We never use `rtm.connect` with app tokens; README explains |
| R14 | **relm4 is 0.x** and breaks between minors (0.10 → 0.11 in four months) | A week lost to an upgrade, once or twice | High | Components kept thin and idiomatic; the engine boundary is a channel, so the UI can be rewritten without touching Slack; pin the version, upgrade deliberately |
| R15 | **GTK's memory floor** makes "lightweight" a smaller number than a terminal's | 120 MB is not 10 MB | Certain | Restated honestly in NFR-4; measured in the spike; no webview, no libadwaita, textures released when rows are recycled |
| R16 | **Browser sign-in breaks** when Chromium changes DevTools, flags or the profile layout, or when Slack changes where tokens live in the web client | Sign-in fails for new users | Medium | Manual paste is always there; every failure names itself (FR-K7); msga is upstream evidence of what changed |
| R17 | **`ListView` with rich rows** — thousands of Pango-marked labels, textures, and Block Kit widgets — drops frames or leaks | Scrolling feels like Electron | Medium | Spike A2 measures it before M1; recycled factories, lazy textures, and a row cache are the tools if it does |
| R18 | **No compositor support for something we assume** — server-side decorations, `ext-idle-notify`, the portal | Ugly or missing on one desktop | Low on Hyprland, Medium elsewhere | Every one is probed and reported by `--doctor`; each has a fallback (CSD, no auto-away, GTK's own chooser) |

---

## 12. Open questions — decisions needed

Decided during analysis (recorded so nobody re-asks):

| # | Question | Decision |
|---|---|---|
| Q1 | Auth route | Session token primary; official OAuth backend behind the same trait (M0 validates, M3 completes) |
| Q2 | Multi-workspace in v1 | Yes, connected simultaneously |
| Q3 | Local persistence | SQLite + FTS5, bounded, optional encryption |
| Q4 | Layout | Slack-like three panes |
| Q5 | Must-have extras | Files + inline images; Block Kit rendering |
| Q6 | v1 integrations | Desktop notifications; `$EDITOR`; open links + clipboard. CLI/IPC → S (waybar) |
| Q7 | Licence | GPL-3.0-or-later |
| Q8 | Docs language | English |
| Q17 | Terminal or native? | **Native.** [ADR-001](./ADR-001-native-gui.md); decided 2026-09-09 after the terminal client reached parity |
| Q18 | Toolkit | **GTK4 via relm4.** Only native option on Wayland; what Omarchy's own tools use. iced/egui/Slint (not native), Tauri (webview), Qt (C++) rejected |
| Q19 | libadwaita? | **No.** It ignores themes by design; the requirement is to follow Omarchy's |
| Q20 | How to theme | **From Omarchy's `colors.toml`**, live; built-in dark/light following `color-scheme` elsewhere. Omarchy does not theme GTK, verified in its repository |
| Q21 | Sign-in | **Browser sign-in in a throwaway profile** (msga's flow), manual paste as fallback. Not msga's Slack-desktop-app cookie import |
| Q22 | Keep the terminal interface as a second frontend? | **No.** New repository, clean history, six crates carried over |

Still open, with recommendations:

| # | Question | Recommendation |
|---|---|---|
| Q9 | Default keymap: modal vim or non-modal? | **Non-modal, keyboard-complete** — a GUI composer that swallows `j` is a bug report waiting to happen. `keymap.preset = "vim"` adds `j`/`k`/`g`/`G` in lists and a modal composer for the owner |
| Q10 | Default mark-read policy | `on_focus` with follow mode, debounced 1 s — closest to the official client without the "marked read because a notification flashed" failure |
| Q11 | Image backend | `gdk::Texture` — there is no longer a question here |
| Q12 | Custom emoji as images | C priority; shortcodes until then |
| Q13 | Should `Enter` send by default, or `ctrl-Enter`? | `Enter` sends (Slack default); `composer.send = "ctrl-enter"` for people who write multi-line prose |
| Q14 | Enterprise Grid support level | S: works when a Grid volunteer can test; not a v1.0 blocker |
| Q15 | Name of the test workspace and who owns it | `slk-dev`, a free workspace on the owner's personal account, created in M0, seeded with the fixture corpus |
| Q16 | Where should the OAuth app manifest live? | `contrib/slack-app-manifest.yaml`, with `slack-light auth add --oauth` printing the create-app URL |

---

## 13. Roadmap

The Slack side is done and measured; the milestones below are about the window.

| Milestone | Contents | Acceptance criteria |
|---|---|---|
| **M0-GUI — Spike (1 wk)** | Per [M0-GUI-SPIKE-PLAN.md](./M0-GUI-SPIKE-PLAN.md): a relm4 shell over the real engine against the mock — `ListView` with 5 000 rows, rich text as Pango, an image, a Block Kit button, send; Omarchy theming with live change; browser sign-in against `slk-dev`; the a11y-tree test instrument under headless weston; keyboard feel | `M0-GUI-FINDINGS.md` with **measured** RSS, first frame, idle CPU and scroll frame times; the `colors.toml` schema; GO/NO-GO on R14–R17 |
| **M1 — Skeleton** | One workspace live: sidebar with correct badges from counts; open a conversation from the store then the API; live messages; send with optimistic delivery; browser sign-in in the product; theming in the product; panic report; `--anonymous`; the a11y test suite | "Follow a conversation and reply without opening the browser", in a tiled window on Hyprland, coloured by the current theme |
| **M2 — Parity core (v1.0)** | Port what the predecessor had: multi-workspace; threads and the threads view; reactions and the emoji picker; edit/delete; mentions and completion; DMs; jump-to; search (API + local); profiles, presence, member lists; files download, upload, drag-and-drop, **inline images** with a viewer; **Block Kit as widgets**; notifications with the full policy; mark-read policies; reconnect gap fill; shortcuts window, palette; back/forward, permalinks, keywords, retention, `cache` — every `M` row in §6 | "A working day needs no browser." Every `M` requirement met or listed with a reason; **the requirements table is the checklist, not this roadmap** |
| **M3 — Polish (v1.1)** | Official OAuth backend; session restore; `$EDITOR`; drafts synced; saved/Later, pins, status/DND; slash commands; channel management; waybar module; idle-away; Enterprise Grid verification; a screen reader actually driving it | Every `S` requirement met or listed with a reason |
| **M4 — Delight (v1.2)** | Custom emoji as images, scheduled messages, bookmarks, forward/share, sidebar custom sections, link unfurl previews | `C` items as chosen |
| **M5 — Hardening** | Fixture suite from `slk-dev`, robustness and fuzz targets, CI with the offline guarantee and the headless a11y suite, **AUR package** (the target user runs Arch), release workflow, other-desktop verification | Same bar as before |

**Build-order rationale:** the spike front-loads the four things that could make a GTK client the wrong choice — relm4's churn, GTK's memory floor, driving a browser for sign-in, and a long list of rich rows — before any feature work. The predecessor's lesson about roadmaps is written into M2's acceptance: **the requirements table is checked, not the roadmap**, because the two drift and the wrong one was being ticked.

---

## 14. Glossary

- **Session token (`xoxc`)** — the token the Slack web client uses, paired with the `d` cookie (`xoxd`).
- **Web API** — Slack's documented HTTPS methods (`conversations.history`, `chat.postMessage`, …).
- **Internal / Edge API** — undocumented methods the web client uses (`client.counts`, `client.userBoot`, `drafts.*`) and the `edgeapi.slack.com` caches on Enterprise Grid.
- **RTM** — Slack's legacy Real Time Messaging websocket; closed to new apps, shutting down for classic apps on 2026-11-16. The web client's own websocket speaks the same event dialect.
- **Socket Mode** — the official websocket delivery of Events API payloads to apps, using an `xapp` token.
- **rich_text** — the Block Kit block that the Slack composer emits for every user message.
- **mrkdwn** — Slack's non-Markdown text markup, used in `text` fallbacks and app messages.
- **Block Kit** — Slack's block-based layout model for app messages.
- **`Ts`** — Slack's message timestamp string, unique per channel and lexically sortable.
- **Counts** — `client.counts`: per-conversation unread/mention state in one call.
- **Gap fill** — fetching the history between the newest stored message and the server's newest after a disconnect.
- **Slack Connect** — channels shared between organisations.
- **Enterprise Grid** — Slack's multi-workspace organisation product.

---

## 15. Sources

- Slack docs: [tokens](https://docs.slack.dev/authentication/tokens/) · [scopes](https://docs.slack.dev/reference/scopes/) · [token rotation](https://docs.slack.dev/authentication/using-token-rotation/) · [Events API](https://docs.slack.dev/apis/events-api/) · [Socket Mode](https://docs.slack.dev/apis/events-api/using-socket-mode/) · [legacy RTM](https://docs.slack.dev/legacy/legacy-rtm-api/) · [rate limits](https://docs.slack.dev/apis/web-api/rate-limits/) · [2025-05-29 rate-limit change](https://docs.slack.dev/changelog/2025/05/29/rate-limit-changes-for-non-marketplace-apps/) · [2025-06-03 clarification](https://docs.slack.dev/changelog/2025/06/03/rate-limits-clarity/) · [conversations.mark](https://docs.slack.dev/reference/methods/conversations.mark/) · [retrieving messages](https://docs.slack.dev/messaging/retrieving-messages/) · [formatting](https://docs.slack.dev/messaging/formatting-message-text/) · [rich text block](https://docs.slack.dev/reference/block-kit/blocks/rich-text-block/) · [message event](https://docs.slack.dev/reference/events/message/) · [file object](https://docs.slack.dev/reference/objects/file-object/) · [working with files](https://docs.slack.dev/messaging/working-with-files/) · [presence and status](https://docs.slack.dev/apis/web-api/user-presence-and-status/) · [stars/reminders retirement](https://docs.slack.dev/changelog/2023-07-its-later-already-for-stars-and-reminders/) · [classic apps deprecation](https://docs.slack.dev/changelog/2024-09-legacy-custom-bots-classic-apps-deprecation/) · [Slack Connect](https://docs.slack.dev/apis/slack-connect/) · [Enterprise](https://docs.slack.dev/enterprise/developing-for-enterprise-orgs/) · [API ToS](https://slack.com/terms-of-service/api)
- Session route references: [wee-slack `slack_api.py`](https://github.com/wee-slack/wee-slack/blob/master/slack/slack_api.py) and [`slack_workspace.py`](https://github.com/wee-slack/wee-slack/blob/master/slack/slack_workspace.py) · [emacs-slack `slack-team-ws.el`](https://github.com/emacs-slack/emacs-slack/blob/master/slack-team-ws.el) · [slackdump edge client](https://github.com/rusq/slackdump/tree/master/internal/edge) · [slk](https://github.com/gammons/slk) · [retrieving Slack cookies](https://www.papermtn.co.uk/retrieving-and-using-slack-cookies-for-authentication/)
- Crates: [gtk4-rs](https://gtk-rs.org) · [relm4](https://relm4.org) · [reqwest](https://crates.io/crates/reqwest) · [tokio-tungstenite](https://crates.io/crates/tokio-tungstenite) · [rusqlite](https://crates.io/crates/rusqlite) · [keyring](https://crates.io/crates/keyring) · [notify-rust](https://crates.io/crates/notify-rust) · [emojis](https://crates.io/crates/emojis)
- Terminal width problem (historical, why the predecessor could not win): [ratatui #1271](https://github.com/ratatui/ratatui/issues/1271) · [ratatui #2357](https://github.com/ratatui/ratatui/issues/2357) · [ucs-detect results](https://www.jeffquast.com/post/ucs-detect-test-results/)

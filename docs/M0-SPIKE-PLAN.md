# M0 Spike — Plan

> **Historical record.** This is the spike plan of `slktui`, the terminal predecessor. Spikes A, B, C and E — the session route, the websocket, the official app, and storage — proved the Slack layer this project carries over unchanged. Spike D (rendering and terminal width) proved something too: that the medium was the problem. See [ADR-001](./ADR-001-native-gui.md), and [M0-GUI-SPIKE-PLAN.md](./M0-GUI-SPIKE-PLAN.md) for what the window has to prove.


**Purpose: prove, separately and cheaply, the four things that could kill the project before any UI is built — and produce the fixture corpus everything else is tested against.**

| | |
|---|---|
| Duration | 1 week |
| Output | `spike/` — throwaway binaries, deleted when promoted to `crates/` in M1 (ytmtui's pattern); `tests/fixtures/` — permanent |
| Verdict | GO / NO-GO on R1 (session route), R5 (terminal emoji width), R12 (official realtime) — recorded in `M0-FINDINGS.md` |
| Precondition | A free test workspace `slktui-dev` on the owner's personal account, created on day 1, seeded per §5. **No spike binary ever points at any other workspace.** |

---

## 1. Spike A — session authentication and the internal endpoints

**Binary:** `authcheck` — read-only by default.

| Step | Call | Assert |
|---|---|---|
| A1 | Load `xoxc` + `d` from `~/.config/slktui/auth.json` (0600) or `$SLKTUI_TOKEN`/`$SLKTUI_COOKIE` | tightened to 0600 on load |
| A2 | `auth.test` (token as form field, `Cookie: d=…`, browser-like User-Agent) | `ok`, `user_id`, `team_id` |
| A3 | Same call with `Authorization: Bearer xoxc-…` and **no** form token | record whether it works — decides the request shape |
| A4 | `client.userBoot` | channels, ims, mpims, self, prefs present; dump raw JSON to the fixture dir (stripped of tokens) |
| A5 | `client.counts` with `thread_counts_by_channel=true` | per-conversation `last_read`, `latest`, `mention_count`, `has_unreads`; `threads` totals |
| A6 | `conversations.history` on `#fixtures`, `limit=15` then `limit=200` | both succeed; note any `ratelimited` response and its `Retry-After` |
| A7 | `conversations.replies` on the seeded thread | replies with `thread_ts` |
| A8 | `users.list` (one page), `users.info` on self, `emoji.list` | shapes captured |
| A9 | `search.messages` for a seeded phrase | at least one hit |
| A10 | `drafts.list`, `saved.list`, `users.prefs.get`, `bookmarks.list`, `dnd.info` | shapes captured; a `not_allowed_token_type` or `unknown_method` here is a finding, not a failure |
| A11 | `--write`: `chat.postMessage` to `#fixtures`, then `chat.update`, `reactions.add`, `reactions.remove`, `chat.delete`, `conversations.mark` | every write echoes back in a subsequent `history`; the channel is left as it was found |
| A12 | Download a seeded image with `Authorization: Bearer` + cookie; upload a small file with `files.getUploadURLExternal` → `POST` → `files.completeUploadExternal` | bytes match; the upload appears in history |

**Go criterion:** A2–A9 green. A10 tells us which parity features route A really has today.

## 2. Spike B — the web-client websocket

**Binary:** `wscheck`.

| Step | Do | Assert |
|---|---|---|
| B1 | Connect `wss://wss-primary.slack.com/?token=<xoxc>&gateway_server=<TEAM>-1&slack_client=desktop&batch_presence_aware=1` with the `d` cookie on the handshake | `hello` frame; capture it |
| B2 | Send `{"type":"ping","id":1}` every 10 s | `pong` with `reply_to` |
| B3 | Post a message from the phone/browser to `#fixtures` | `message` event arrives within 1 s; capture |
| B4 | Send `{"type":"message","channel":…,"text":…,"id":2}` on the socket | `{ok, reply_to:2, ts}`; then the same message arrives as an event (or not — record which) |
| B5 | Post via `chat.postMessage` with a `client_msg_id` | the echo event carries the same `client_msg_id` — this is the outbox dedupe key |
| B6 | From the browser: edit, delete, react, mark read, start typing, open a thread reply, star, pin | `message_changed`, `message_deleted`, `reaction_added`, `channel_marked`, `user_typing`, `message_replied`/`thread_marked`, `star_added`, `pin_added` captured |
| B7 | `presence_sub` for three users; toggle one to away in the browser | `presence_change` |
| B8 | Kill the network for 90 s, restore | detect the drop within 30 s via missed pong; reconnect with a **fresh** URL (the old one is single-use); `client.counts` afterwards shows `history_invalid` or a moved `latest` for `#fixtures` |
| B9 | Leave connected for 8 hours overnight | log every non-message frame type seen (`reconnect_url`, `desktop_notification`, `pref_change`, …) |

**Go criterion:** B1–B5 and B8 green. B6/B7/B9 build the event fixture corpus.

## 3. Spike C — the official route

**Binary:** `oauthcheck`.

| Step | Do | Assert |
|---|---|---|
| C1 | Create an app from `contrib/slack-app-manifest.yaml` in `slktui-dev` (user scopes from §4.1 of the requirements doc; Socket Mode on; user events subscribed) | manifest accepted as written |
| C2 | OAuth with a loopback redirect on `127.0.0.1:<port>`; store `xoxp` and `xapp` | `auth.test` ok |
| C3 | `conversations.list`, `conversations.history` (limit 15 and 200), `users.list`, `search.messages`, `chat.postMessage`, `reactions.add`, `conversations.mark`, `conversations.info` on a DM | shapes captured; note the effective history limit |
| C4 | `apps.connections.open` → Socket Mode; post a message from the phone to `#fixtures` | **does a `message` event for the user's channels arrive?** Record the envelope. This is R12 |
| C5 | Token rotation enabled: refresh once | new token works |

**Go criterion:** C1–C3 green proves the trait boundary. C4 decides whether route B has realtime; a NO downgrades FR-A5 to "poll the focused conversation" and is written down, not argued about.

## 4. Spike D — rendering and terminal width

**Binary:** `renderprobe` — ratatui, no network; input is the fixture corpus.

| Step | Do | Assert |
|---|---|---|
| D1 | Parse 30 captured messages: every mrkdwn feature, a `rich_text` with nested lists and a quote, a bot Block Kit message (header, section with fields and a URL button, context, divider, image), a legacy attachment unfurl, a `file_share` with an image, a `me_message`, a `channel_join`, a tombstoned thread parent, an edited message, a message with 12 reactions | AST round-trips: `parse(emit(doc)) == doc` for user messages |
| D2 | Render them at 200, 120, 80 and 60 columns | no panic; snapshot each |
| D3 | The alignment corpus: `👍`, `👍🏽`, `👨‍👩‍👧‍👦`, `❤️` (VS16), `☺` (text presentation), `🇨🇿`, `日本語テキスト`, `Ａ`, a 300-char URL, a 40-line code block | render a table whose right border must be straight; print it in **kitty, alacritty, foot, ghostty, wezterm, xterm, and tmux inside kitty**; photograph/inspect; record per-terminal deviations |
| D4 | Width probe: print each emoji, query the cursor column (`CSI 6 n`), compare with `unicode-width` | a per-terminal table for `--doctor`; decide whether the default must be "every emoji cluster is width 2" |
| D5 | Inline image: render the seeded PNG with `ratatui-image` in kitty (graphics protocol), foot (sixel), alacritty (half-block), and tmux with and without passthrough | image appears in the right cell box; scrolling it out removes it; no leftover artefacts |
| D6 | `ratatui-textarea` composer: multi-line, `shift+Enter` vs `alt+Enter` vs `ctrl+j` in each terminal above, bracketed paste of 20 lines | table of what each terminal delivers; hint text decided |

**Go criterion:** D1–D3 green. D4 decides the emoji strategy (R5). D5/D6 pick defaults.

## 5. Spike E — the store

**Binary:** `storeprobe`.

| Step | Do | Assert |
|---|---|---|
| E1 | Create the schema from ARCHITECTURE §5.2 with `rusqlite` bundled, WAL | FTS5 available |
| E2 | Insert 100 000 synthetic messages across 50 channels in a transaction per 1 000 | total time; < 10 s |
| E3 | `messages_before(channel, ts, 50)` | < 1 ms |
| E4 | FTS query for a word in 0.1 % of messages, ranked | < 20 ms |
| E5 | Startup path: open the DB, read the sidebar state and the last 50 messages of one channel | < 50 ms cold |
| E6 | Retention: trim to 5 000 per channel and 90 days, keep starred | correct counts afterwards |

**Go criterion:** all of it; the numbers go into NFR-1.

## 6. Seeding the test workspace

`slktui-dev`, free plan, owner's personal account. Channels: `#fixtures` (public), `#private-fixtures` (private), a DM with a second test user, a group DM with three. Seeded content, scripted so it can be re-created:

- one message per mrkdwn feature, one `rich_text` message with every element type, ten reactions on one message, a thread with five replies and one broadcast reply, an edited message, a deleted message, a pinned message, a starred message
- a bot (the manifest app itself, or an incoming webhook) posting: a Block Kit deploy summary, a `context` + `divider` message, an `image` block, a legacy attachment with colour and fields, an unknown block type
- files: a PNG, a JPEG, a 2 MB image, a text snippet, a PDF
- custom emoji: two, one an alias
- users: three humans (one with a status and timezone, one deactivated), one bot user

Every fixture is captured from this workspace, has tokens and cookies stripped, and is committed. **Nothing is ever captured from any other workspace** — Slack responses are full of personal and company data.

## 7. What M0 writes down

`M0-FINDINGS.md`, in the ytmtui format: results per acceptance criterion, the request shapes that work, the event corpus, the per-terminal width table, measured store numbers, defects found, and the list of requirement changes the findings force.

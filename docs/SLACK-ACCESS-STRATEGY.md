# slack-light — Slack Access Strategy: Risk Analysis

**Question:** which way into Slack gives a full-featured lightweight client the best chance of shipping and surviving — the web client's session token, an official OAuth app, or both?

**Short answer:** Both, with the session route primary. Parity — the whole point of the product — is only reachable through the session route, because presence, typing, unread counts, drafts and saved items have no official equivalent for a user client. The official route is cleaner and safer but structurally cannot be full-featured, and is worth having as the second backend for workspaces whose policy forbids the first. Neither choice is language-dependent; every risk below is Slack-side.

| | |
|---|---|
| Status | Research complete (2026-09-07) — recommendation in §6 |
| Companion to | [ANALYSIS-AND-REQUIREMENTS.md](./ANALYSIS-AND-REQUIREMENTS.md) §4, §11 |
| Verdict | **Session route primary. Official route as a second backend behind the same trait. Validate both in M0.** |

---

## 1. The three routes

| | A — Session (web client) | B — Official user-token app | C — Bot-token app |
|---|---|---|---|
| Token | `xoxc-…` + cookie `d=xoxd-…` | `xoxp-…` (+ `xapp-…` for Socket Mode) | `xoxb-…` (+ `xapp-…`) |
| Identity | The user, exactly as in the browser | The user, through an app they authorised | A bot — **not the user**; posts as the bot, sees only channels it was invited to |
| Setup | Sign in with the browser, copy two strings | Create an app from our manifest, install it into each workspace, OAuth in the browser | Same, plus invite the bot everywhere |
| Admin gate | None — it is the user's own session | "Require App Approval" is a one-click admin setting; many companies have it on | Same |
| Documented? | No. Web API methods yes; `client.*`, `drafts.*`, `saved.*`, the websocket URL — no | Yes | Yes |
| Verdict | **Primary** | **Secondary backend** | **Rejected** — a Slack *client* must act as the person |

---

## 2. Feature coverage — what each route can actually do

Legend: ✅ works · ⚠ works with a cost · ❌ not possible

| Feature | A — Session | B — Official user token |
|---|---|---|
| Read channels, DMs, threads, history | ✅ | ✅ (Tier 3, or 1/min ×15 when distributed outside the home workspace — §4) |
| Post, edit, delete, react, upload | ✅ | ✅ |
| Search messages and files | ✅ (`search.messages`, plus `search.modules.*`) | ✅ (`search:read`, user token only) |
| **Unread counts and mention badges in one call** | ✅ `client.counts` | ❌ — `conversations.info` has `unread_count` for DMs only; everything else needs `history(oldest=last_read)` per conversation |
| Mark read (channels) | ✅ `conversations.mark` | ✅ |
| Mark read (threads) | ✅ `subscriptions.thread.mark` (undocumented) | ❌ |
| Realtime messages, reactions, marks | ✅ web-client websocket (RTM dialect) | ⚠ Socket Mode — user-scoped events *probably* delivered; **unverified**, M0 tests it |
| **Presence** (`presence_change`) | ✅ `presence_sub` on the websocket | ❌ "cannot be tracked using the Events API"; polling `users.getPresence` only |
| **Typing indicators** | ✅ `user_typing` + sending `typing` | ❌ RTM-only event |
| Drafts synced with other devices | ✅ `drafts.*` (rejects app tokens) | ❌ |
| Saved for later | ✅ `saved.list` | ⚠ `stars.*` answers but "has no user-facing impact" since 2023 |
| Reminders | ✅ `/remind` via `chat.command` | ❌ `reminders.*` retired alongside stars; slash commands are not callable by apps |
| Workspace slash commands (`/giphy`, `/zoom`) | ✅ `chat.command` | ❌ |
| Notification preferences per channel, sidebar sections, muted list | ✅ `users.prefs.get`, `client.userBoot` | ⚠ partial (`conversations.info` has no mute flag; prefs not exposed) |
| Slack's own notification decisions | ✅ `desktop_notification` events | ❌ |
| Status, DND, presence set | ✅ | ✅ |
| Custom emoji list | ✅ | ✅ |
| Scheduled messages | ✅ | ✅ |
| Bookmarks, pins | ✅ | ✅ |
| Enterprise Grid | ✅ with Edge API; token behaviour reportedly volatile (*unverified*) | ✅ org-level install; `team_id` everywhere |
| Huddles, canvases content, Lists | ❌ | ❌ |

The table is the argument. Route B alone cannot show who is online, who is typing, or — without a request per conversation — how many unread messages a channel has. Those are not polish; they are what makes a chat client feel alive.

---

## 3. Stability and breakage history

### Route A

| When | What broke | How long |
|---|---|---|
| 2022-09-20 | Slack removed `rtm.start`; wee-slack had moved to `rtm.connect`/the web socket the day before (v2.9.0) | 0 days for wee-slack users |
| 2023-09 | Websocket connections with session tokens failed after a Slack change; wee-slack v2.10.1 fixed it | days |
| 2025-12 | Cookie TTL shortened from ~10 years to > 1 year | re-login once a year |
| 2026-01 (*unverified, single report*) | On Enterprise Grid the token stopped being persisted in `localStorage`; extraction via DevTools/Network only, and tokens rotate frequently | Grid users only |

Three projects have run on this route for years and are all alive in September 2026: wee-slack (pushed 2026-08-17), emacs-slack (2026-09-01), slackdump (v4.4.4, 2026-09-03). A newer Go client, `slk`, chose the same route in 2026.

### Route B

| When | What | Effect |
|---|---|---|
| 2024-06-04 | Classic app creation closed | RTM unavailable to any new app |
| 2025-03-11 | `files.upload` retired | new upload flow (we use it) |
| 2025-05-29 | History rate limits cut to 1/min ×15 for commercially distributed non-Marketplace apps; internal apps exempt | §4 |
| 2026-11-16 | Classic apps and RTM shut down | irrelevant to us, we never used RTM with app tokens |

Route B breaks on a schedule with changelogs; route A breaks without notice but gets fixed by a community that includes us.

---

## 4. Rate limits — the thing everyone gets wrong about the 2025 change

The May 2025 change reads as if it kills history access for everyone. It does not:

- It applies to `conversations.history` and `conversations.replies`.
- For **"commercially distributed apps created after May 29, 2025, and net new installations of existing apps"** — i.e. apps distributed outside their home workspace and not in the Marketplace.
- Slack's FAQ, verbatim: *"Will my internal app be impacted? No, internal customer-built apps are not impacted and retain their current, higher rate limits."* Re-affirmed on 2025-06-03.
- Session tokens are not app tokens. No session-token client reports the cap. (*Inferred*, not stated by Slack.)

What this means for us:

| Situation | History budget |
|---|---|
| Route A, any workspace | Tier 3: 50+/min, up to 1 000 objects |
| Route B, app created in the user's own workspace | Tier 3 |
| Route B, our manifest installed into a workspace the user does not administer | Possibly 1/min × 15 — **design for it**: page size ≤ 15, counts and websocket deltas first, backoff on 429, never a background backfill |

The rest of the API is Tier 2–4 and `chat.postMessage` is ~1/s per channel; a token-bucket per tier per workspace, honouring `Retry-After`, covers it.

---

## 5. Risk of using route A — honestly

- **Terms.** Slack's API terms forbid exceeding rate limits, abusive usage and circumventing technical measures, and warn that undocumented parts should not be relied on. Nothing names session tokens. The Acceptable Use Policy is about load and scraping. A single user's client that behaves like the web client is, at worst, a grey zone; there is **no documented case** of an account ban for wee-slack or emacs-slack use in their combined decade.
- **Admin visibility.** slackdump's README warns that on some plans and security settings, unusual access may trigger security alerts or admin notifications. That is Slack telling admins about a new session, which the browser login already created. A client that looks like one person on one device is unremarkable; a client that pulls whole workspaces is not.
- **Employer policy.** Independent of Slack, an employer may forbid unofficial clients. That is a rule about the user's job, not about the software. The README must say it, and route B exists for the case where an admin would rather approve a manifest than see a session token in use.
- **Enterprise Grid.** The only concrete pain reports are from Grid: token rotation and extraction. Grid is S priority and gets verified by a Grid volunteer in M3, not assumed.

Mitigations are behavioural and already in the requirements: FR-Z1–Z5 — one websocket per workspace, lazy history, counts before history, no bulk prefetch, no export, no polling loops, backoff, an honest User-Agent, a read-only mode, and a README that states all of the above before any auth instructions.

---

## 6. Recommendation

1. **Route A is the primary backend.** It is the only route that delivers the product in §1 of the requirements doc, and it has a decade of precedent in living projects.
2. **Route B is a real second backend, not a stub.** M0 proves its trait boundary (reads, a post, and whether Socket Mode delivers user events); M3 completes it with honest `capabilities()` so the UI disables presence, typing, drafts and counts rather than faking them. Shipped with an app manifest and a one-command OAuth flow.
3. **Route C is rejected** for a client; a bot is not the user.
4. **Everything Slack-specific lives behind `SlackBackend`** with fixtures from a dedicated test workspace, so whichever route breaks, the fix is contained and the other keeps the client usable.
5. **The client never does anything a browser session would not**, and the README says so plainly. That — not any technology — is what manages R2.

## 7. What M0 must verify

| # | Claim | Test |
|---|---|---|
| V1 | Session token + cookie authenticate `auth.test`, `client.userBoot`, `client.counts`, `conversations.history` | `authcheck` binary, read-only |
| V2 | The web-client websocket URL form and its handshake with the `d` cookie work; `hello` arrives; a message posted from the phone arrives as an event; a message sent from the socket echoes | `wscheck` binary |
| V3 | A post via the Web API with the session token echoes on the socket, and `local_id`/`client_msg_id` round-trips | same |
| V4 | An app from our manifest, installed into the test workspace, reads history and posts with `xoxp` | `oauthcheck` binary |
| V5 | Socket Mode delivers a user-scoped `message.channels` event to that app | same; **go/no-go on route B's realtime** |
| V6 | Whether `Authorization: Bearer xoxc-…` works in addition to the form `token` field, per endpoint | `probe` |
| V7 | `client.counts` `history_invalid` semantics after a forced disconnect | `wscheck` |

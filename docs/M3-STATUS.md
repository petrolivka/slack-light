# M3 — Polish (v1.1): what was built, and what was measured

M3's acceptance is one line in the roadmap: **every `S` requirement met, or
listed with a reason.** This is that list. The rule M2 was accepted under
applies unchanged — *the requirements table in §6 is the checklist, not the
roadmap* — so the audit below walks §6, not the milestone description.

Nine commits, `a0630ab` … `6fe476a`. 91 unit and scenario tests (from 71),
67 accessibility checks in `tests/a11y/e2e.sh` (from 45), clippy clean,
`cargo fmt` applied, every automated run against `--anonymous`.

---

## 1. The `S` audit

| Req | What it asks | State |
|---|---|---|
| FR-A5 | Official OAuth backend, loopback redirect, rotation, Socket Mode, honest `capabilities()` | **Built.** Not exercised against Slack — see §5 |
| FR-A6 | Enterprise Grid: org session, `team_id` where required | **Built, unverified.** See §5 |
| FR-H9 | Offline marker; sends queued and delivered in order | **Met** |
| FR-M7 | Drafts per conversation and per thread, persisted, restored | **Met** (the `C` half, synced with Slack's own drafts, is M4) |
| FR-M9 | Slash commands: the built-ins, and pass-through | **Met** |
| FR-M10 | Typing indicator sent (throttled) and received | **Met** on the session route; not possible on OAuth, and it says so |
| FR-N5 | Sidebar options: unread first, hide read, recents, section order | **Met** |
| FR-P2 | Presence, polled, on the official route | **Not met, deliberately.** See §4 |
| FR-P3 | Own presence and status with emoji and expiry; DND | **Met** |
| FR-S3 | File search | **Met** |
| FR-S4 | Search history on `↑` | **Met** |
| FR-T5 | Save for later, pin/unpin, pinned items view | **Met** — the pinned view was the third of this row M2 never claimed |
| FR-T6 | Quote into the composer; forward/share | **Met** |
| FR-T7 | View message source behind `debug.enabled` | **Met** |
| FR-U4 | Slack's own `desktop_notification`; per-channel prefs | **Met** |
| FR-C1 | Join/leave, star/unstar, mute/unmute, topic/purpose, invite | **Met** |
| FR-I8 | Session restore: size, pane widths, workspace, conversation, thread | **Met except scroll position.** See §4 |
| FR-I9 | Accessibility: named controls, focus order, reduced motion, high contrast, no colour-only information | **Built. Not driven by a screen reader.** See §5 |
| FR-K5 | `--no-cache`; optional SQLCipher | **Met** — `--no-cache` since M1, SQLCipher as a build feature |
| FR-X5 | `unread --json`, `send`, `status` for a waybar module | **Met** |
| FR-X6 | Idle detection setting presence away | **Met**, and measured firing on this desktop |

---

## 2. What Slack actually does, that was not written down before

Everything here was measured or read out of a real response, not assumed.

**Muting is not per channel.** `muted_channels` is a single preference
holding a comma-separated list, so muting one conversation is a write of
every muted conversation. `ChannelOp::SetMuted(Vec<ChannelId>)` says so in
its shape, and the caller owns the list because the caller owns the sidebar.
A backend keeping its own copy would drift the first time another client
muted something.

**Starring a channel is still `stars.add`.** Slack renamed the feature to
"favourites" in the interface and left the endpoint alone — the same pair
that saving a message falls back to.

**`pins.list` returns items, not messages.** A pinned *file* has no `ts` of
its own, so it is skipped rather than guessed at.

**`all_notifications_prefs` is a JSON document nested as a string inside
another one.** Every step of reading it is an `Option`; an unreadable value
costs the preference, not the boot.

**`search.files` answers with the file, not with the message that shared
it.** A hit can name the conversation and not the message. That is a real
answer, so the row opens the conversation rather than jumping somewhere
arbitrary, and a file shared nowhere we know says so.

**Typing has no REST endpoint.** The web client sends a frame down the
socket it already has. Socket Mode is inbound only, so the official route
cannot send one at all.

**Socket Mode envelopes must be acknowledged.** An unacknowledged envelope
is redelivered three times and then the connection is dropped, which from
inside the client looks exactly like a flaky network.

**Hyprland 0.56.2 has no dispatcher that takes focus by force.**
`hl.dsp.focus({ window = "class:…" })` answers `ok` and does not move the
keyboard when another window is holding it. It really does find the window —
an unmatched selector answers `window not found` — so `ok` means "asked",
not "done". This is a finding about the *test harness*, and §3 is about what
it was hiding.

---

## 3. Defects found, and what they were hiding

**The accessibility suite could type into somebody else's window, and did.**
A run produced thirty-four failures that said nothing about the client: the
keyboard belonged to another terminal the whole time, and every `wtype` went
there instead. `wtype` talks to the compositor, not to a window. There was
no check that the window under test had the keyboard.

This is the accident CONTRIBUTING rule 1 exists to prevent — on this machine
the window that could have received it is the developer's real Slack. Every
keystroke now goes through a guard that checks the active window's class,
asks once for focus, checks again, and **aborts the run** if it still is not
the client: nothing typed, non-zero exit, and a line naming the window that
would have received it. A suite that needs the desktop to itself should say
so rather than quietly produce a red wall.

**Session restore was dead code that looked alive.** `Command::Remember` and
`Event::Restore` have existed since M1. The engine read the key at boot; the
window never wrote it. Both ends are now connected.

**The outbox only drained on reconnect.** A send can fail on the network
while the websocket stays up — a proxy hiccup, a change of network — and
then nothing reconnects, so nothing retries, and the message waits for ever
in a client that looks perfectly connected. Found by a scenario test that
made sends fail *without* dropping the socket. There is now a five-second
heartbeat as well.

**The mock accepted a post to a channel that does not exist**, which made
the demo workspace more forgiving than the real one — the wrong direction
for a mock to be wrong in. It answers `channel_not_found` now, which is what
let the "a refusal is not queued" case be tested at all.

**`slack-light send` reported success for a conversation that does not
exist.** The socket replied before the engine had looked. The window now
publishes the conversation names into the snapshot so the socket can answer
truthfully, and a refusal exits non-zero — printing an error on stdout and
exiting zero is how a cron job silently stops working.

**`pinned` claimed a chord this same table already gives the palette.** The
first-claimant rule makes that survivable rather than silent, but which one
wins then depends on the preset, so within one table it is a mistake. A unit
test now refuses two defaults asking for one chord.

**The event match had a catch-all.** It is now exhaustive, and the compiler
proved it: in M2 `Event::Notify` fell through that catch-all and
notifications never happened at all. Adding an event should break the match.
It did, twice, during this milestone — which is the point.

---

## 4. Decided against, with the reason

**Custom emoji in message bodies (FR-F6).** They render as images in
reaction chips, which is where they mostly appear. The message *body* still
shows `:name:`. The body is a single Pango label, which is what makes
selection and copy work across a whole message (FR-I7), and a label cannot
hold an image without giving that up — the alternative is a flow of label
and picture widgets per message, at a cost NFR-4 would notice and with
selection lost.

The requirement was re-graded from C to S on the note that "a texture in a
label is not a hard problem any more". In GTK 4 it is exactly as hard as it
was. That is a finding, not an omission.

**Syntax highlighting for snippets (FR-F4).** Slack sends
`preview_highlight` as a block of HTML from its own editor. Rendering it
means shipping an HTML parser; the alternative is writing a highlighter, and
one that is wrong about a language asserts things about code that are not
true. Monospace, and the filetype named in the header.

**Scroll position in session restore (FR-I8).** A chat client that opens
anywhere but the newest message is answering a question nobody asked, and
paging makes a stored offset meaningless. Everything else in that row is
restored.

**Polled presence on the official route (FR-P2's `S` half).** `users.getPresence`
exists and there is no subscription, so following twenty people means twenty
calls on a timer, for ever. That is precisely the traffic shape FR-Z1 exists
to avoid, and a dot beside a name is not worth it. `capabilities().presence`
is false for that route and the dots are simply absent.

**SQLCipher as a default (FR-K5).** The key has to come from somewhere, and
a cache that cannot be opened after a keyring reset is worse than one that
somebody with your disk could have read. It is a build feature, keyed from
`SLACK_LIGHT_KEY` in the launcher and never a prompt — a client that asks
for a passphrase at start-up is one people run with it in their shell
history. For most people who ask for this, `--no-cache` is the answer.

**A shipped OAuth client secret.** One in a GPL binary is not a secret, and
an app registered by this project would put every user's access under a
single installation somebody else can revoke.
`contrib/slack-app-manifest.yml` is the app; a test asserts it asks for
exactly the scopes the client uses, so neither list can drift.

---

## 5. What is built but not proven

Written down rather than implied by a green suite.

**The OAuth route has never talked to Slack.** Every endpoint is the
documented one and the parsing is the same code the session route uses, but
no `oauth.v2.access` exchange, no Socket Mode connection and no `xoxp` call
has been made against a real workspace. It needs somebody to register an app
from the manifest and run `auth add --oauth`.

**Enterprise Grid is unverified.** `auth.test` reports `enterprise_id`, and
from then on every call carries `team_id` — written to the shape Slack
documents. There is no Grid org here to test against.

**A screen reader has still not driven the client.** The accessibility tree
is asserted on, every control this milestone added is in it with a usable
name — the custom-emoji chip is `":shipit: 2"`, not `"2"` — and high
contrast is tested as a WCAG AAA contrast ratio rather than as a screenshot.
None of that is the same as Orca reading a conversation aloud.

**The accessibility suite has not run against M3's changes.** The session on
this machine was locked (`hyprlock`) for the whole of the second half of the
milestone, so the keyboard could not be taken — and with the new guard the
suite correctly refuses to run rather than typing at a lock screen. The 22
checks added this milestone are written and syntax-checked; they have not
been executed. `tests/a11y/e2e.sh` on an unlocked desktop is the next thing
to run.

**Drag-and-drop is still verified by eye alone**, and Hyprland still has no
click dispatcher, so no pointer path is asserted by the suite. Mitigated as
it was in M2: every pointer action goes through the same `Msg::RowAction`
the keyboard uses.

---

## 6. What M3 did not claim

The remaining rows in §6 are `C` and `W`, which is M4's list: custom sidebar
sections from `users.prefs` (FR-N6), synced drafts (FR-M7's `C` half),
scheduled messages (FR-M11), bookmarks (FR-C3), create/archive/rename
(FR-C2), and Block Kit interactivity (FR-B4), which is `W` and stays `W` —
posting back to an app's action URL as the user is the one thing in Block
Kit that a third-party client should not do.

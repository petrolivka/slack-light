# M0 Spike — Findings

> **Historical record.** These are the findings of `slktui`, the terminal predecessor, kept verbatim. Everything measured about Slack — the session route, `client.counts`, drafts, the websocket, reconnect URLs, storage cost per message — stands and is the basis of this project. §3 (terminal widths, graphics and keyboard protocols) is the measurement that, in the end, argued for [ADR-001](./ADR-001-native-gui.md): a correct solution to a problem a text-rendering engine does not have.


**Verdict: GO, and the access strategy is fully evidenced.** Both halves of the route are now measured: the Web API answers every internal endpoint the product's parity depends on, and the websocket carries every event the reading-state model needs, including the typing and presence that the official route structurally cannot provide. The history rate cap that would have forced a redesign does not apply to a session token. The parsing and layout core works against real Slack responses, and the local store beats every latency budget by more than an order of magnitude. One risk came back materially worse than assumed and forced a design change. Eight real defects were found and fixed, two of which would have shipped: a parser that hung on an ordinary quoted message, and every multi-parameter link arriving broken. Only spike C, the official OAuth route, is unstarted.

| | |
|---|---|
| Date | 2026-09-08 |
| Workspace | `slk-dev`, created for this. No probe has ever pointed at another one |
| Code | `spike/` — throwaway, to be promoted into `crates/` during M1 and deleted |
| Environment | Rust 1.98.1, Arch Linux 7.1.9, Hyprland/Wayland, SQLite 3.53.2 (bundled), ratatui 0.30.2 |
| Companion to | [M0-SPIKE-PLAN.md](./M0-SPIKE-PLAN.md) — what each spike had to prove |

---

## 1. Results against the acceptance criteria

| # | Spike | Result |
|---|---|---|
| **A** | Session auth and the internal endpoints | ✅ **GREEN** — every endpoint answered, including `client.counts` and `drafts.list` |
| **A/D** | The parsers against a real capture | ✅ **GREEN** after three fixes; every block and element type Slack sent has a renderer |
| **D1/D2** | Parse and lay out the message corpus | ✅ **GREEN** — 31 fixtures, round-trip holds, nothing overflows at four widths |
| **D3/D4** | Terminal width behaviour | ⚠️ **GREEN with a design change** — four of five terminals agree exactly, alacritty needs six per-grapheme corrections |
| **D5** | Image protocol availability | ✅ **GREEN** — measured per terminal; alacritty has none, so half-block is a main path, not a rarity |
| **D6** | Composer key distinguishability | ✅ **GREEN** — the kitty keyboard protocol is available in all four terminals, and **not** through tmux |
| **E** | Store: schema, bulk insert, paging, FTS, retention | ✅ **GREEN** — every budget beaten by 10× or more |
| **B** | The web client websocket | ✅ **GREEN** — connects, streams every event the client needs, and corrects one architectural assumption |
| **C** | The official OAuth route | ⏸ **BLOCKED** — needs a workspace to install an app into |

---

## 2. D1/D2 — the parsers and the layout hold

`renderprobe` parses a corpus of 31 messages covering every mrkdwn construct, `rich_text` with nested lists, quotes and preformatted blocks, a bot's Block Kit with header, fields, context, divider and buttons, a legacy attachment unfurl, files with and without images, twelve reactions, a thread parent, a tombstone, `me_message`, `channel_join`, CJK, mixed-width scripts, an external Slack Connect user, an empty message, and a degenerate one with no user and an unknown subtype.

```
D1  parsed 31 messages, no panic
    round-trip parse(emit(parse(t))) == parse(t): 23/23 held
    content assertions: 22/22 held

D2  layout
    200 cols, gutter=true  ->   86 lines
    120 cols, gutter=true  ->   89 lines
     80 cols, gutter=false ->  120 lines
     60 cols, gutter=false ->  124 lines
    no line exceeded its width, no panic (419 lines laid out)
```

The round-trip property is the one that keeps the outbound path honest: what the composer sends must parse back to the same tree. It holds for every construct including the styles that have no single mrkdwn marker, where the emitter has to nest `*_~…~_*` and the parser has to unpick it.

The content assertions matter more than "did not crash", which is the failure mode a test suite falls into when it only checks for panics. They assert what the parsers must *extract*: that tokens resolve to names, that entities are unescaped exactly once and after tokenising, that a code block is preserved verbatim, that an unknown `rich_text` element is skipped while its siblings survive, that a URL button keeps its URL, that three unrenderable block types each become a labelled placeholder.

Twelve tests in `spike/tests/parsers.rs` pin the behaviour that is easy to get subtly wrong — delimiter word-boundary rules, code spans swallowing formatting, unescaping order — and prove the parsers degrade rather than panic on thirteen shapes of malformed JSON and sixteen shapes of malformed mrkdwn.

---

## 3. D3/D4 — the width problem is worse than a policy can fix

R5 was rated High and it deserved it. `termprobe` prints one grapheme cluster, asks the terminal where the cursor ended up with `CSI 6 n`, and compares that against `unicode-width`. Twenty cases, five terminals:

| terminal | agreement | corrections needed | kitty keyboard | kitty graphics | sixel |
|---|---|---|---|---|---|
| kitty | 20/20 | 0 | yes | yes | no |
| ghostty | 20/20 | 0 | yes | yes | no |
| foot | 20/20 | 0 | yes | no | **yes** |
| **alacritty** | **14/20** | **6** | yes | no | no |
| tmux (in kitty) | 20/20 | 0 | **no** | **no** | reports yes, but see §3.2 |

### 3.1 alacritty does not cluster graphemes, and it errs in both directions

| case | unicode-width | kitty/foot/ghostty | alacritty |
|---|---|---|---|
| `👍🏽` skin tone | 2 | 2 | **4** |
| `👨‍👩‍👧‍👦` ZWJ family | 2 | 2 | **8** |
| `🏳️‍🌈` ZWJ flag | 2 | 2 | **3** |
| `❤️` VS16 heart | 2 | 2 | **1** |
| `1️⃣` keycap | 2 | 2 | **1** |
| `⚠️` VS16 warning | 2 | 2 | **1** |

It draws a ZWJ sequence as its component emoji and renders VS16 sequences in text presentation. **This is why no global policy can work**: some cases need widening and others need narrowing, so "emoji are always wide" and "emoji are always narrow" are both wrong for the same terminal. Measured against the twenty cases, the best global policy for alacritty still gets six wrong.

The practical size of the error: the ten-emoji alignment corpus measures 28 cells by `unicode-width` and 34 cells in alacritty. Six cells of drift per line is precisely how a chat client ends up with a ragged right border and a sidebar that overlaps the conversation.

**Design change, now implemented in the spike:** `width::Metrics` carries an optional measured table alongside the fallback policy. `Metrics::from_probe` builds it from a `termprobe` report and stores **only** the entries that differ, so a well-behaved terminal produces an empty table and costs nothing. `Ctx` carries `Metrics` rather than a bare policy, and every measurement in the renderer goes through it. Verified: the same corpus measures 28 cells under kitty's table and 34 under alacritty's, and the layout suite still reports no overflow under either.

### 3.2 tmux changes three answers, and one of them is a trap

Inside tmux the kitty keyboard protocol query goes unanswered and the kitty graphics query goes unanswered — tmux does not pass either through by default. Meanwhile tmux answers the primary device attributes query *itself* with `\e[?1;2;4c`, and attribute 4 means sixel.

So a client that trusts `DA1` under tmux concludes "sixel is available" when the host terminal is kitty, which does not do sixel, and draws garbage. **Capability detection must treat tmux as opaque**: inside tmux, use passthrough if it is enabled and otherwise assume no graphics, regardless of what DA1 says.

### 3.3 D6 is settled, and better than assumed

The composer's `shift+Enter` question has a clean answer: kitty, foot, alacritty and ghostty **all** implement the kitty keyboard protocol, so `shift+Enter` is distinguishable from `Enter` in every terminal likely to be used. Under tmux it is not.

This makes the UI spec's hint line computable rather than a guess: detect the protocol at startup, show `shift+Enter for a newline` when it is there and `alt+Enter for a newline` when it is not.

---

## 4. E — the store is not going to be the bottleneck

```
E1  schema created in 1.8 ms  (sqlite 3.53.2, fts5 present)
E2  100000 messages across 50 channels in 5392 ms  (18545 msg/s, 78.9 MiB on disk)
E3  messages_before(channel, ts, 50)  best of 20: 0.0 ms
E4  FTS 'kafkaesque' (87 matches, top 50)  best of 10: 0.2 ms
    FTS phrase "deploy staging"       best of 10: 7.1 ms
E5  cold open + sidebar + 50 messages: 0.6 ms
E6  retention trim to 500/channel: 100000 -> 25159 in 629 ms; fts index stayed consistent
```

| metric | budget (M0 plan) | measured | |
|---|---|---|---|
| 100 k inserts | < 10 s | 5.4 s | ✅ |
| paging query | < 1 ms | ~0 ms | ✅ |
| FTS query | < 20 ms | 0.2 ms (7.1 ms for a phrase) | ✅ |
| cold start read | < 50 ms | 0.6 ms | ✅ |

**NFR-1 has enormous headroom.** The first frame is budgeted at 150 ms and the store contributes under a millisecond of it, so the budget is really about process start and terminal setup.

Two findings worth carrying forward:

- **`raw_json` costs about 830 bytes per message.** Retaining it is decision D1 in the architecture and it earns its place — a parser fix becomes a migration rather than a re-fetch, and view-source works offline — but the default retention of 5 000 messages per channel across 50 channels implies roughly 200 MiB. The default should be stated in megabytes in the config, not only in message counts, and `slktui cache stats` should show it.
- **The FTS triggers keep the index exactly in step with the table through bulk deletes**, which the retention job does nightly. Asserted, not assumed: the probe fails if the counts diverge.

---

## 5. Defects found and fixed — which is what a spike is for

**(a) The plain-text projection resolved neither names nor emoji.** `Doc::plain()` rendered a mention as `@U01ALICE` and an emoji as `:tada:`. That projection feeds the local search index, desktop notifications and copy-to-clipboard, so every notification would have read "@U01ALICE said…" and the search index would have stored ids nobody types. Fixed by introducing a `Names` lookup trait in the AST — keeping it free of any dependency on the UI's context type — and resolving shortcodes through the emoji table.

**(b) A Block Kit field painted straight through the grid.** Fields arrive as `*Label*\nvalue`. Putting that into a span puts a newline in the middle of a laid-out row, and everything to its right ended up on the wrong line:

```
┃ Duration
3m 12s                       Coverage
87.4% (+0.3)
```

Fixed by flattening each field to `Label: value` for the grid, and by collapsing newlines anywhere a string is placed into a positioned span.

**(c) Twelve reactions overflowed the pane by up to 76 cells.** A popular message easily carries a dozen; they were emitted on one unbroken line. Fixed by wrapping the chips like any other content. This was caught by the layout suite's overflow check, not by eye — which is the argument for having the check.

**(d) A probe that blocks on silence is a hang, not a "no".** The first capability probe read stdin with a blocking read while checking a timeout *after* the read returned. Terminals that answer every query (kitty, ghostty) sailed through; foot and alacritty locked up until killed. Fixed with `poll(2)` before each read.

**(e) Buffered stdin defeats `poll`.** With the poll in place the probe then reported "no reply" from terminals that had answered: `std::io::stdin()` is buffered, its first read swallowed the entire reply into userspace, and `poll` then correctly reported the file descriptor as empty. Fixed by reading the descriptor directly.

**(f) The parser hung on a real message.** Slack escapes the block-quote marker
along with everything else in the `text` fallback, so a quoted message arrives
as `&gt; ` and not `> `. The quote branch matched only the raw form while the
scanner that decides where a block ends matched both, so on a real quote neither
consumed the line and `parse` spun forever. A wrong render is recoverable; a
frozen client is not. Fixed in both places, and the block loop now asserts that
every pass consumes a line, so the whole class cannot come back — in a release
build it takes the line rather than spinning.

**(g) Every link with more than one query parameter was broken.** URLs inside a
`<url|text>` token are escaped like the rest of the text, so `?a=1&amp;b=2`
reached the browser verbatim and requested something other than what the sender
wrote. Found by comparing what Slack stored as `rich_text` against what our
mrkdwn parser made of the same message's `text`.

**(h) A build killed the developer's terminal, three times.** Cargo runs one
`rustc` per core; on 24 cores with rustls, a bundled SQLite and tokio in the
graph the peak was 17.6 GB of RAM and 33 GB of swap. `systemd-oomd` then killed
the cgroup — and a build started from a terminal is *inside that terminal's*
cgroup, so what died was the window and the editing session, not the build. No
coredump, no kernel OOM message, nothing but a journal line. Fixed with a `jobs`
cap and a `build.sh` that runs cargo in a memory-limited scope of its own.

> Both (d) and (e) are the same class as ytmtui's NFR-13: a subsystem that fails *silently* and looks healthy. Worth generalising — **any capability detection must distinguish "answered no" from "did not answer"**, and the `doctor` output must show which it was.

---

## 6. A new finding about emoji names

`:thinking_face:` did not resolve. Slack's shortcode set is `emoji-data`'s (iamcal), and the `emojis` crate indexes gemoji's, where the same emoji is `:thinking:`. The two sets overlap heavily but not completely, and the differences are exactly the everyday names.

This confirms the crate research's warning as a measured fact rather than a caution. **The client must vendor `emoji-data` and generate its own map at build time**, using the `emojis` crate only as a fallback. Until then the renderer degrades correctly — an unresolved name stays `:thinking_face:`, which is also what a custom workspace emoji does — so this is a quality gap, not a defect.

Posting the alignment corpus to a real workspace and reading it back named the
gap precisely. Slack rewrote what was sent into *its own* shortcodes, and four of
them are ones the gemoji-based crate does not know:

| Slack's name | what it is | gemoji's name |
|---|---|---|
| `man-woman-girl-boy` | 👨‍👩‍👧‍👦 | `family_man_woman_girl_boy` |
| `flag-cz` | 🇨🇿 | `czech_republic` |
| `rainbow-flag` | 🏳️‍🌈 | `rainbow_flag` |
| `thinking_face` | 🤔 | `thinking` |

The `rich_text` path is unaffected, because Slack sends the code points in the
element's `unicode` field and we prefer that. It is the `text` fallback that
degrades, which is precisely the path bots and older messages take.

---

## 7. A — the session route answers everything

Run against `slk-dev`, a workspace created for the purpose. Read-only; the write
path is opt-in and undoes itself.

```
A2   auth.test                        ok      229 ms   team=slk-dev user=petr.olivka
A3   auth.test (Bearer, no form token) ok     154 ms   Bearer accepted
A4   client.userBoot                  ok      244 ms   channels=3 ims=3 mpims=0
A5   client.counts                    ok      178 ms   channels=3 ims=3 with_unreads=1
A6   conversations.history            ok      180 ms   limit=15  returned=15 has_more=true
A6   conversations.history            ok      181 ms   limit=200 returned=16 has_more=false
A7   conversations.replies            ok      175 ms   replies=4
A8   users.list / users.info / emoji.list        ok    15 custom emoji
A9   search.messages                  ok      175 ms
A10  drafts.list                      ok      150 ms
A10  saved.list                       ok      153 ms
A10  users.prefs.get                  ok      226 ms
A10  bookmarks.list                   ok      154 ms
A10  dnd.info                         ok      149 ms
     client.counts carries last_read per conversation: true
```

**Nothing failed.** Four results decide things the architecture was only assuming:

| Question | Answer |
|---|---|
| Do the internal endpoints answer for a session token? | Yes — `client.userBoot`, `client.counts`, `drafts.list`, `saved.list`, `users.prefs.get` all return data. These are exactly the parity features the official route cannot offer, so the access strategy's central claim holds |
| Does `client.counts` carry `last_read`? | Yes, for every conversation. One call gives every sidebar badge, which is what §7 of the requirements builds the whole reading-state model on |
| Is the form `token` field required? | No. `Authorization: Bearer` is accepted, so `slk-api` can use a header and keep the body clean |
| Does `conversations.history` cap at 15 objects? | **No.** `limit=15` returned 15 with `has_more=true`; `limit=200` returned all 16 with `has_more=false`. The May 2025 cap that hits distributed apps does not apply here. This was marked *inferred, unverified* in the research and is now measured |

Latency is 150–250 ms per call from Europe, which is the number the loading
states in the UI spec should be designed around.

### The parsers against a real capture

`seed --post` filled a `#fixtures` channel with a known corpus and `seed --dump`
captured fourteen endpoints. `realprobe` then ran the parsers over the real
messages and inventoried every `type` discriminator in the response against the
set we render.

```
20 messages, 2 users, 15 custom emoji
block            actions, context, divider, header, rich_text, section
rich_text block  rich_text_preformatted, rich_text_quote, rich_text_section
rich_text inline broadcast, date, emoji, link, text
block element    button, mrkdwn, plain_text
text object      mrkdwn, plain_text
every block and element type in this capture has a renderer
laid out 208 lines across four widths, 0 overflows
```

Three things this found that the hand-written corpus could not:

1. **Slack rewrites what you post.** Nineteen of twenty messages came back as
   `rich_text` blocks, including ones posted as plain `text`. The rich-text path
   is the common case by a wide margin, and the mrkdwn parser is the fallback for
   bots and history, exactly as the architecture assumed — but the ratio is far
   more lopsided than expected.
2. **Two real bugs**, both in §5 above: the hang on an escaped block quote, and
   every multi-parameter URL arriving broken. Both were found by comparing what
   Slack stored as blocks against what our parser made of the same message's
   `text`, which is a comparison worth keeping as a test.
3. **Slack did not turn a bulleted list into a list.** `• one` stayed literal
   text. Our mrkdwn parser reads those bullets as a list and re-renders them with
   its own markers, so the result looks identical but the plain-text projection
   differs. A deliberate divergence, recorded here so nobody "fixes" it twice.

---

## 8. B — the websocket carries everything, and one assumption was wrong

Ninety seconds on the live socket while a person posted, edited, reacted,
replied in a thread, typed, switched channels and changed presence.

```
B1   connected in 153 ms, HTTP 101 Switching Protocols
B1   hello received - the stream is live
     hello=true  pongs=8  ping rtt avg=21 ms
```

Every event the reading-state model in §7 of the requirements depends on
arrived:

| Event | Count | What it settles |
|---|---|---|
| `message` | 2 | **`blocks=true` on the wire** — live messages carry rich_text, same as history |
| `message/message_changed` | 1 | edits arrive as their own subtype |
| `message/message_replied` | 2 | thread reply counts update without a fetch |
| `reaction_added` | 2 | |
| `channel_marked` | 2 | a read on another device reaches us, which is what keeps badges honest |
| `thread_marked` | 2 | threads have their own read state, and it syncs |
| `user_typing` | 20 | **typing indicators work**, one of the parity features the official route cannot offer |
| `presence_change` | 1 | **presence works**, after an explicit `presence_sub` — the other such feature |
| `reconnect_url` | 2 | see below |
| `badge_counts_updated` | 2 | undocumented; Slack pushing its own unread arithmetic |
| `update_global_thread_state` | 2 | undocumented; the Threads view's counter |
| `activity/activity_deleted` | 2 | undocumented; the activity feed |
| `user_interaction_changed` | 1 | undocumented |

The last four are not in any documentation or in any prior-art client's list.
They cost nothing — unknown events are logged at debug and dropped — but two of
them, `badge_counts_updated` and `update_global_thread_state`, look like Slack
volunteering the arithmetic the sync engine was going to do itself. Worth
investigating in M2 before hand-rolling counters.

### The assumption that was wrong

The architecture said a reconnect must always fetch a fresh socket URL because
they are single-use. **Measured: the same URL was accepted for a second
connection.** The claim came from the legacy RTM documentation, where
`rtm.connect` URLs did expire in thirty seconds, and it does not hold for the
web client's socket.

More useful than the correction: Slack sends `reconnect_url` events during the
session, twice in ninety seconds. That is the intended mechanism — keep the
latest one and reconnect with it, rather than re-authenticating. The reconnect
path in `slk-sync` should use it, and fall back to a fresh `auth.test` only when
no `reconnect_url` has been seen.

### One absence worth recording

No `desktop_notification` event appeared. In a workspace of one, nothing
generates a notification, so this says nothing either way about the event's
availability. The notification policy in §6.7 of the architecture must therefore
keep its own decision logic rather than assume Slack will send that event; it can
be adopted as a short-circuit later if it turns up in a workspace with other
people in it.

---

## 9. Still blocked

**Spike C** (the official OAuth route, and whether Socket Mode delivers
user-scoped events — risk R12) needs an app installed into a workspace and is not
started.

## 8. What M0 changes in the plan

| Doc | Change |
|---|---|
| Requirements §6.12 | **New requirement FR-K6:** measure grapheme widths at first run and cache a per-terminal correction table; `--doctor` refreshes it. A global emoji policy is insufficient — proven, not assumed |
| Requirements §11 | **R5 downgraded to Medium** — it is now a measured, solved problem with a mechanism in place, rather than an open question |
| Requirements §9.4 | Emoji: vendor `emoji-data` and generate the map in `build.rs`; `emojis` is the fallback, not the source of truth |
| UI spec §5.2 | The newline hint is computed from the kitty keyboard protocol probe, not fixed text. All four terminals support it; tmux does not |
| UI spec §7.2 | alacritty offers **no** image protocol, so half-block is a primary path. Under tmux, ignore DA1 entirely and rely on passthrough or nothing |
| Architecture §5.2 | Retention should also be expressed as a size budget; ~830 bytes per message with `raw_json` retained |
| Architecture §7.2 | `Ctx` carries `width::Metrics` (policy plus measured table), not an `EmojiPolicy` |
| Architecture §10 | Add the rule that capability detection must distinguish "answered no" from "did not answer", and surface which in `doctor` |

---

## 9. Recommended next step

Create the `slktui-dev` test workspace and run `authcheck` and `wscheck` against it. That is perhaps an hour of work and it is the last thing standing between the plan and a fully evidenced go/no-go on the access strategy, which is the decision the whole architecture rests on.

Everything else in M0 is done, and M1 can start on the pure crates — `slk-core` and `slk-render` are effectively written and tested in `spike/`, ready to be promoted.

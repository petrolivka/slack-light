# M2 — Parity core: Status

M2 is being done in two halves. **M2a — the look** is complete; **M2b —
the features** is the rest of the `M` requirements, and its first block —
a message you can act on — is complete too (§4). The order was
deliberate: the message row is the thing every later feature is drawn on
top of, so redoing it afterwards would mean redoing reactions, threads and
hover actions with it.

| | |
|---|---|
| Date | 2026-09-09 |
| Follows | [M1-STATUS.md](./M1-STATUS.md) |
| Acceptance | "A working day needs no browser." **The requirements table in §6 is the checklist, not this document** |

---

## 1. M2a — the look

The layout is the official client's, because parity means nobody has to
learn a new shape. The *language* is an editor's — dark, dense, monospace
where it counts, rounded panels, separation by background rather than by
lines — taken from the owner's own VSCode and terminal rather than
invented.

| | Before | Now |
|---|---|---|
| Message row | name, time, body; a rule between every message | rounded-square avatar in the author's colour, name and time, body in mono indented under them; **no rule anywhere** |
| Consecutive messages | name and time repeated every time | grouped: same author within five minutes on the same day is one block |
| Separators | none | day breaks (`TODAY`, `YESTERDAY`, weekday, date) and a `NEW MESSAGES` line at the read mark |
| Reactions | text in a label | bordered chips; the ones you are in filled with the accent |
| Threads | text | `↳ 2 replies · bob, alice · last 11:40` as a link row |
| Sidebar | one flat list | `STARRED` / `CHANNELS` / `DIRECT MESSAGES`, each folding, with a count when folded; unread in bold; mentions as a pill |
| Workspaces | none visible | an activity rail on the far left, one tile each, the current one marked by an accent edge; hidden when there is only one |
| Header | a bold word | name, member count and topic on one dense line |
| Composer | a one-line entry | a panel: several lines, its own border, `enter sends · shift+enter newline` underneath |
| Status bar | the last thing that happened | left: connection and workspace. right: mentions and the two keys worth knowing. A notice appears for six seconds and then the state comes back |

Nine of the palette's colours become named GTK colours (`@sl_bg`,
`@sl_accent`, …) so `~/.config/slack-light/user.css` can refer to them, and
the eight author colours become classes, because a per-row colour cannot
come out of a stylesheet any other way and a CSS provider per row would
cost more than the row.

### Defects found while building it

- **Sections broke keyboard navigation.** `alt-Up` stepped over
  conversation indices, but with headings in the list a row index is no
  longer a conversation index, and a folded section has no rows at all. It
  now steps over what is on screen, in the order it is on screen.
- **`ctrl-k` filtering matched the headings.** A heading is a row; while a
  query is being typed the sidebar is a flat list of what matches, and
  Enter can only land on a conversation.
- **The window grew the sidebar, not the conversation.** `GtkPaned`
  resizes its start child by default, which on a tiling compositor means
  the sidebar grows every time the window does.
- **The inline picture vanished.** `can_shrink` took it to nothing in a
  vertical box; it needs an explicit size, computed from the file's own
  dimensions.
- **Your own name was unreadable** — accent blue on the highlight yellow.
  The palette gained `on_fill`: what to write *on* a filled highlight,
  which is the window's background and the only colour guaranteed to read.

## 2. What the measurements actually say

Re-measured with a clean store, three samples each, and compared against
M1 rather than against a remembered number. Two corrections to the record:

**A frame percentile is the display's refresh interval, not the cost of
the work.** This machine has a 120 Hz panel and a 60 Hz external monitor,
so `scroll_p50` reads 8.3 ms or 16.7 ms depending only on which output the
window opened on. What it means is "no frame was dropped"; the honest
number for hitches is `scroll_over_20ms_pct`, which is 0–2 %.

**An idle window repaints continuously in some positions — and M1 does it
too.** Measured over five seconds with seven messages: M2a 116/489/491
frames across three runs, M1 114/201/256. Idle CPU 0.5–2.5 % in both. It
is position-dependent and survives pinning the scrollbar, replacing
`ListView::scroll_to` with an adjustment move, and `ContentFit::Fill` on
the picture.

> **Answered in §10.** It is GTK's own text caret, which fades rather than
> switches and so drives the frame clock at the display's full rate for as
> long as the composer has focus. Not a defect in this client, and now a
> switch. The "position-dependent" part was the window opening on the
> 120 Hz panel or the 60 Hz monitor.

The earlier "0.06 % idle, 54 MB" from M1 was one lucky sample that I
believed. Three samples of the same command now give 99.0 / 98.8 / 99.1 MB
private at 5 007 rows — stable, and about 6 MB above M1's 93 MB, which is
what the richer row costs. Still inside the restated NFR-4 (private under
80 MB is *not* met at 5 000 rows; see below).

| Measure | M1 | M2a | Budget |
|---|---|---|---|
| Window mapped | 106 ms | 93–96 ms | < 300 ms ✅ |
| 5 007 rows on screen | 254 ms | 258–321 ms | < 300 ms ⚠️ borderline |
| Frames over 20 ms while scrolling | 2.8 % | 0–2 % | 60 fps ✅ |
| Private memory, 5 007 rows | 93 MB | 99 MB | < 80 MB ❌ |
| Resident, 5 007 rows | 218 MB | 224 MB | < 200 MB ❌ |

The two memory rows are over budget **at five thousand messages in one
conversation**, which is the stress case, not the working case: the client
loads a page at a time and the store keeps the rest on disk. The budget was
written for "5 000 messages loaded", so either the budget or the loading
strategy has to change, and that is an M2b decision with a measurement
behind it rather than a number chosen now.

## 3. M2b — what is left

Every `M` row in §6 of the requirements that M1 and M2a have not met — 47
of 67 at M1, fewer now. By area:

- **Threads**: the pane, the threads view, follow/unfollow, reply
- **Message actions**: reactions and the picker, edit, delete, copy text
  and permalink, save, pin, hover affordances
- **Composing**: completion for `@`, `#`, `:emoji:` and `/commands`,
  outgoing conversion to Slack's wire format, drafts
- **History**: scrollback upward with its loading state, jump to a message
  in context, back and forward
- **Files**: download, upload, drag-and-drop, an in-window image viewer
- **People**: profiles, presence, member lists
- **Search**: Slack's own and the offline index
- **Notifications**: the full policy, mark-read policies, gap fill
- **Interface**: the shortcuts window and the command palette, generated
  from the live keymap rather than printed into the status bar
- **The idle repaint** (§2), and the five preset bindings that still fall
  back to defaults because `slack` does not bind them

---

## 4. M2b, first block: a message you can act on

M2a drew reactions, a thread summary and a hover area with nothing behind
any of them. This block puts the engine behind them. The engine needed no
changes at all: every command was already written and tested for the
terminal client, so this is entirely the window learning to ask.

| | Before | Now |
|---|---|---|
| The cursor | there was none; the list was `NoSelection` | a selected message, moved with alt-j/k, alt-Home/End and the page keys, marked by an accent edge rather than a filled row |
| Reactions | chips you could read | chips you can click, a `＋` beside them, three quick reactions on alt-1/2/3, and a picker on alt-r with search and skin tone |
| Threads | `↳ 2 replies` as text | a pane beside the conversation: the parent, its replies, follow, "also send to the conversation", and its own composer. Opened by the link, by alt-t, or by any reply in it |
| Message actions | none | edit, delete (with a confirmation, because it is the one action with no undo), copy text, copy link, open the first link, save, pin, download files, upload, mark read |
| Where they live | — | a hover bar over the row — three quick reactions, the picker, the thread, and a `⋯` menu built only when it is opened |
| Pinned and saved | invisible | a line under the message: `📌 pinned · 🔖 saved` |
| The keyboard | 13 actions bound | 55, with the shortcuts window (F1) generated from the live keymap, including what has no key |
| Escape | closed the jump bar | unwinds one layer at a time: picker, jump bar, edit, thread, cursor |

Everything the pointer can do goes through `Msg::RowAction`, which carries
a timestamp and an action name and then runs the same code the keyboard
runs. Clicking `↳` and pressing alt-t are one call.

### The keymap defect that was worth the afternoon

**Nine bindings were dead, and nothing anywhere said so.** Turning on the
whole action list at once made it visible; a smaller change would have
shipped it.

- `<Alt>T` and `<Alt>t` are the **same accelerator**. `gtk_accelerator_parse`
  lower-cases a capital that has no `<Shift>` beside it, so the preset's
  `threads` (alt-T) and `open_thread` (alt-t) collided silently and alt-t
  answered "threads: not in this build yet".
- `<Alt><Shift>t` matches **nothing**. Measured with a capture-phase key
  logger: alt-shift-m arrives as keyval `m` with `SHIFT|ALT`, and neither
  `<Alt><Shift>m` nor `<Alt><Shift>M` fires on it — while `<Alt>m`,
  `<Alt>slash`, `<Alt>at` and `<Alt><Shift>Down` all do. Shift on a *named*
  key is fine; shift on a *character* is not.

So `logic::bindable` refuses shift-plus-a-character, the affected actions
get control chords instead, and `e2e.sh` asserts that no such binding is
ever installed again. Two more rules came out of the same pass: a chord
with no modifier at all is text, not a shortcut — the preset binds `[` and
`]`, which as accelerators would eat the brackets out of every code
snippet — and the first action to claim an accelerator keeps it, with the
loser reported rather than silently dropped.

The logger that found it stayed, under `--metrics`, and logs **only chords**:
a plain key is the message the user is typing.

### Five more defects, all found by driving it

- **A replaced row was grouped against itself.** An edited or echoed
  message is compared against the end of the list, which for the newest
  message is the message itself — same author, same second — so it grouped
  under itself and lost its avatar, its name and its pin. Present since M1;
  invisible until a pin needed somewhere to render.
- **The cursor did not survive an echo.** Saving a message made the engine
  send it back, the row was replaced, and the next action said "no message
  selected" with the cursor still visibly on the row.
- **Closing the thread moved the cursor into it.** Clearing a list emits a
  selection change; the thread pane's arrived after `close_thread` had
  already pointed the cursor back at the conversation.
- **An expiring notice wiped a newer one.** "you can only edit your own
  messages" lasted a fraction of a second because a "pinned" from six
  seconds earlier chose that moment to clear itself. Notices are numbered
  now, and a timer only clears the one it was raised for.
- **A reaction scrolled its own message off screen.** Removing and
  re-inserting a row can take it out of the viewport; the chip appeared
  somewhere nobody was looking.

And one that was mine, not the code's: `./build.sh test` ran `cargo test`,
which in this workspace tests the root package — a thin binary with no
tests. "0 passed" read as success and covered nothing. It runs the whole
workspace now.

### Measurements

Both builds measured today, on this machine, with the same command —
`--anonymous --no-cache --demo-rows 5000 --metrics --bench --idle 5` —
three samples each. **The command is written down this time**, because §2's
numbers were not: re-measuring commit `da426f5` today gives 60 MB and
178 MB where §2 records 99 MB and 224 MB, so §2's memory row and this one
are not comparable and §2 should be read as "M2a against M1", nothing more.

| Measure | M2a (da426f5) | M2b | Budget |
|---|---|---|---|
| Window mapped | 100–109 ms | 101–117 ms | < 300 ms ✅ |
| 5 007 rows on screen | 242–268 ms | 253–263 ms | < 300 ms ✅ |
| Frames over 20 ms while scrolling | 0.0 % | 0.0 % | 60 fps ✅ |
| Anonymous (private) memory, idle | 60.0 MB | 64.8 MB | < 80 MB ✅ |
| Resident, idle | 178.4 MB | 196.7 MB | < 200 MB ✅ |
| Idle CPU over five seconds | 2.0–2.2 % | 2.1–2.2 % | ~0 % ❌ |

The first version of the hover bar built its six buttons in every pooled
row and cost **15 MB of private memory and 10 MB resident**. Building it on
the first hover instead gives most of that back — the numbers above are
with the lazy version; the eager one measured 75.4 MB private and 207 MB
resident, over the resident budget on its own. Most rows are never pointed
at, so most of those widgets never existed.

The idle repaint of §2 is unchanged and still unfixed, in both builds.

## 5. M2b, second block: composing

| | |
|---|---|
| Completion | `@` people and groups, `#` channels, `:emoji:` and `/commands`, in a popup under the word being typed. Tab and the arrows move, Enter takes, Escape closes and leaves what was typed |
| The wire format | `slk-core::outgoing` turns `@alice` into `<@U0ALICE>`, `#design` into `<#C0DESIGN\|design>`, `@here` into `<!here>`, escapes `&`, `<` and `>`, and **never rewrites inside code** — a client that mentions somebody because their name appeared in a stack trace is a client people turn off |
| Drafts | what was typed and not sent is kept per conversation, and comes back |

The completion rules are deliberately strict, because the alternative is a
popup that opens while somebody types an e-mail address: the sigil must
start a word, whitespace ends the token, `/` only counts at the very start
of a message, and `:` needs a character after it or every clock time opens
the emoji list.

Two defects, both from driving it:

- **`:roc` offered `:rock:` before `:rocket:`.** The picker's own ranking is
  shortest-prefix-first, which is right in general and wrong here; the
  curated common set now wins a tie.
- **Restoring a draft re-opened the completion popup, and the next Enter
  accepted a completion instead of sending.** The flag that says "the client
  is editing the buffer, not the user" was read in the message handler,
  which runs *after* the flag is down again. It is read inside the signal
  handler now, where it is still up.

## 6. M2b, third block: history

| | |
|---|---|
| Scrollback | near the top, the page before this one is asked for, with a line saying so and another saying when there is nothing older. The reading position is held: the new page goes in front and the view is put back where the eye was |
| Page size | `[message] history_page`, now actually read by the engine. 50 by default, clamped to 10–1000, which is Slack's own ceiling |
| Jump to a message | `Event::MessagesAround` fills the conversation around one message and puts the cursor on it. A Slack permalink clicked inside a message jumps there instead of opening a browser |
| The mock | pages the way Slack does — the newest `limit` of what matched, with `has_more`. It used to hand over the whole channel at once, which meant scrollback could not be exercised against it at all |

### Two defects, one of them since M1

**Every conversation longer than the window has been opening at its
*oldest* message.** `scroll_to_end` set the adjustment once, when the
messages arrived — before the rows had been measured, so `upper` was still
zero and "scroll to the bottom" clamped to the top. It was invisible for as
long as the demo fitted on one screen. The bottom is now *followed*: taken
again every time the content grows, until the reader scrolls away from it,
and re-armed when they scroll back down.

**`ListView::scroll_to` does not reach the far end of a long list.**
Measured: alt-Home selected the oldest message and left the view exactly
where it was. It is reliable for a neighbouring row, which is what the
cursor keys need; the two ends move the scrollbar directly now.

### What paging did to the idle repaint

The client no longer puts five thousand rows in a list unless somebody
scrolls back a hundred pages, and that turns out to matter:

| Rows in the list | Idle CPU over five seconds |
|---|---|
| 50 (the default page) | 0.53–0.72 % |
| 1 000 (`history_page = 1000`) | 0.60–0.64 % |
| 5 007 (before paging) | 2.00–2.35 % |

So §2's defect is **not fixed** — 0.6 % is not "~0 %" — but the condition
that made it worst is no longer one the client creates by itself. The jump
between 1 000 and 5 000 rows is the lead worth following.

The rest of the numbers, three samples, `--anonymous --no-cache
--demo-rows 5000 --metrics --bench --idle 5`: window mapped 97–101 ms,
first page on screen 133–135 ms (it was 253–263 ms for all 5 007), frames
over 20 ms while scrolling 2.2–2.9 %, private memory 57.5 MB, resident
189–190 MB.

## 7. M2b, fourth block: the lists

The right-hand pane stopped being the thread pane and became **the** pane:
a thread, a search, a member list and a profile are all "the thing beside
the conversation", and two panes would compete for the same width.

| Key | What the pane shows |
|---|---|
| alt-t / the `↳` link | the thread, with its own composer |
| ctrl-t | threads with replies |
| ctrl-d | saved for later |
| alt-@ | messages that name you |
| alt-m | who is in this conversation |
| alt-i | who wrote the message under the cursor |
| alt-c | public channels to join |
| alt-/ , ctrl-f | search, Slack's own and the offline index |

Every list answers with the same five row shapes — a message, a thread, a
person, a channel to join, and a line that is only there to be read — so
one function knows what choosing a row does and the engine decides
nothing about presentation. An empty answer is a sentence ("nothing here",
"nothing matched", "asking…"), because a blank pane cannot tell the reader
whether it is empty or still loading.

Search puts its query in the pane rather than in a window of its own: a
result you can read while still looking at the conversation it came from
is the whole reason to have a pane.

`editor` — the terminal client's "edit this draft in $EDITOR" — is the one
action that does not survive the pivot, and says so.

## 8. M2b, fifth block: notifications, marking read, the palette

**Notifications were not raised at all** — `Event::Notify` fell through the
window's catch-all arm and nothing happened. The engine had been deciding
correctly since M0 and nobody was listening.

The decision is deliberately split. The engine knows whether a message
*matters* — a direct message, a mention, a highlight word, and never a
muted conversation. The window knows whether the user can already see it:
the window has focus, that conversation is open, and it is scrolled to the
bottom. Both halves are now tested, the engine's by three scenario tests
against the mock's live stream (the first the engine has had), the
window's by pure functions and by the a11y suite driving the real thing.

Only the desktop channel survives the pivot. The bell and the OSC escape
sequences are terminal channels: they write to a stdout nobody is looking
at when the interface is a window.

**Marking read** now follows `[message] mark_read`, which was configurable
and ignored:

| | |
|---|---|
| `manual` | only when asked |
| `on_focus` | when the conversation is opened, if the window has focus |
| `on_view` | when the newest message is actually on screen, and the window has focus |

Every one of them checks that the window has the keyboard, because a
client that clears badges while it is buried behind a browser is a client
that loses messages. The mark is guarded by the timestamp it was last sent
at, or `on_view` would send one on every scroll event that ends at the
bottom, which is most of them.

**Presence** reaches the sidebar: a filled or hollow dot beside a direct
message, which is one glyph and no more.

**The command palette** (ctrl-p) lists every action by name with the key it
is on, filterable, arrows to move while the entry keeps the keyboard. It is
why `keys::install` registers an action whether or not it got an
accelerator: the two that GTK owns are still reachable by name.

## 9. M2b, sixth block: files and slash commands

**A slash command is not a message.** Anything shaped like `/word` at the
start goes to the workspace, which is what Slack does — and what Slack also
does is keep the text in the box when the command turns out not to exist.
The engine now says so with `Event::SlashRejected`, carrying what was
typed, and the window puts it back in the composer. A path is not a
command: `/home/petr/notes.md is where` is a sentence, and so is `and/or`.

**Files dropped on the window** are sent to the conversation, or to the
thread if one is open — which is where the person was looking when they
dropped them. The whole window is the drop target, because aiming at the
composer is a thing people do only once.

**An image opens at its own size**, in a window rather than a browser: the
file is already on disk in the cache, and handing a Slack URL to a browser
means handing it the session cookie too. Clicking the picture works, and so
does alt-v — which is the point, because of what the harness cannot do.

### What the test suite cannot see

Hyprland has a dispatcher to move the pointer and none to click it, and
`wtype` is a keyboard. **No pointer path is asserted by the suite**: the
hover bar's buttons, the reaction chips, the thread link, the image. Two
things keep that from being a hole. Every one of them goes through the same
`Msg::RowAction` the keyboard uses, so the code under the pointer is the
code under test; and where there was no keyboard equivalent — the image —
one was added rather than left unreachable.

Drag-and-drop has neither, and is verified by eye alone. It is written down
here rather than implied by a green suite.

## 10. The idle repaint, answered

Chased since M1, blamed on four different widgets, and finally measured by
taking focus away rather than by reading code. Eight idle seconds, 50 rows,
same binary:

| | Idle CPU | Frames |
|---|---|---|
| Composer focused, caret blinking | 1.97 % | 484 |
| Window not focused | 0.54 % | 114 |
| `--read-only`, so the composer has no caret | 0.47 % | 115 |
| `[ui] reduced_motion = true` | **0.02 %** | **0** |

GTK 4 fades the text caret in and out rather than switching it on and off,
which asks the frame clock for a frame every frame for as long as a text
field has focus. That is the whole of it. The remaining ~14 frames a second
were widget animations — a scrollbar settling, an overshoot — and
`gtk-enable-animations` takes those too.

So **NFR-3 is met**, and the honest statement of it is: an idle window
costs 0.02 % of a core and paints nothing, *if* nothing on it is animating;
a blinking caret costs 2 % of a core, on this machine, at 120 Hz. The
client does not turn the caret off by default, because a text field without
one is a text field people think is broken. `[ui] reduced_motion` — which
already existed and meant "stop anything from changing on its own" — now
does something in the window, and `e2e.sh` asserts that an idle window with
it set paints exactly zero frames.

Two things this cost, worth writing down: the earlier rounds ruled out
`propagate_natural_height`, `ContentFit::Contain`, an automatic scrollbar
policy and `ListView::scroll_to` — all four were plausible, three of them
were genuinely worth fixing anyway, and none of them was the answer.
Reading code found four suspects; changing one variable at a time found the
cause in three measurements.

## 11. The memory budget, decided

NFR-4 was written as "private under 80 MB and resident under 200 MB with
5 000 messages loaded". Paging changed the premise: the client loads a page
of 50 and asks for more as the reader goes back, so five thousand messages
in one list is now something a person has to work at.

Measured, three samples each, `--anonymous --no-cache --demo-rows 5000`:

| Rows in the list | Private | Resident |
|---|---|---|
| 50 (the default page) | 57.5 MB | 190 MB |
| 1 000 (`history_page = 1000`) | 59.5 MB | 191 MB |
| 5 007 (all of them, before paging) | 64.8 MB | 197 MB |

The budget stands as written and is met at every size, including the stress
case it was written for. No number needed changing; what needed changing
was the loading strategy, and that is done.

## 12. One more layout defect, found by looking

Opening the side pane on a half-screen tile **clipped the left edge of
every message**: avatars, names and the start of each line disappeared
behind the sidebar. Three wrong explanations before the right one, all
worth recording because each was plausible:

- *The paned is squeezing the conversation* — it was not; `set_position`
  was giving the conversation the width it asked for.
- *The picture is forcing the row wide* — it was, partly. A `Picture` with
  a width request makes that width the row's minimum, and the list's
  minimum is its widest row. Fixed by requesting only the height and
  letting `Contain` scale the picture.
- *The minimums do not fit* — they did not, but shrinking them only made
  the clipping smaller.

The cause was `hscrollbar_policy: Never`. It does not mean "do not scroll
horizontally"; it means **"demand the child's full minimum width"**. That
minimum travelled up through the panes into the window, the window's
minimum grew past the tile the compositor had given it, and the compositor
clipped what would not fit. `External` gives the child its minimum and
scrolls instead, with no scrollbar drawn — which is what "never scroll
horizontally" was meant to say in the first place.

A window whose minimum size exceeds its tile is a class of bug a tiling
compositor finds and a floating one hides. Worth remembering.

### What is left in M2b

Nothing on the list. The remaining `M` requirements are the ones M2 never
claimed: the workspace-level operations (archive, invite, channel
management), and the parts of Block Kit that need interactivity.

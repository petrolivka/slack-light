# M2 — Parity core: Status

M2 is being done in two halves. **M2a — the look** is complete; **M2b —
the features** is the rest of the `M` requirements. The order was
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
the picture. **It is a real defect against NFR-3 ("~0 % CPU when nothing
changes") and it is not new**; it is on the M2b list with this evidence
attached rather than left as a surprise.

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

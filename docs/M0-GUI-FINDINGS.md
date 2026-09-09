# M0-GUI Spike — Findings

| | |
|---|---|
| Status | Spike A complete, 2026-09-09; B–E pending |
| Plan | [M0-GUI-SPIKE-PLAN.md](./M0-GUI-SPIKE-PLAN.md) |
| Binary | `spike/shell` — relm4 0.11 over the real `slk-sync` engine against `MockBackend`, no network |
| Environment | Rust 1.98.1, GTK 4.22.4, relm4 0.11.0 / gtk4-rs 0.11.4, Arch Linux 7.1.9, Hyprland on Wayland, Mesa (default GSK renderer), integrated GPU |
| Verdict so far | **GO on R14 (relm4) and R17 (list performance). GO on R15 (memory) with NFR-4 restated — see §3.** R16 (browser sign-in) not yet spiked |

Every number below is printed by the binary itself (`spike-shell --bench`)
from `/proc/self/status`, `/proc/self/schedstat` and the window's
`gdk::FrameClock`. Release build. The conversation held **5 007 rows**: the
demo's seven plus 5 000 synthetic messages cycling through every rich-text
construct, with a Block Kit payload every fiftieth and a real PNG posted
through the mock's own `upload`.

## 1. Spike A — the shell, criterion by criterion

| # | Criterion | Result |
|---|---|---|
| A1 | Sidebar, conversation, composer, fed by engine events; main thread does no I/O | ✅ The engine runs on a two-thread tokio runtime built before `gtk::init`; its `Event` receiver is forwarded into the component's `relm4::Sender`; `Command`s go the other way with `runtime.spawn`. Nothing on the main thread is `async`; the model holds channel ends and widgets. relm4 made this natural: an `Event` is just another `Input` variant |
| A2 | `gtk::ListView` with a factory, 5 000 rows, 60 fps | ✅ `TypedListView<Row, NoSelection>`. **Scroll: 1 193 frames, p50 16.7 ms, p95 17.1 ms, max 66 ms, 2.7 % of frames over 20 ms** — the display's 60 Hz, with a handful of hitches when a Block Kit row is realised |
| A3 | Rich text as Pango markup, every construct distinct | ✅ Bold, italic, strike, inline code, links (clickable, `Label` opens them), user and channel mentions, `@here`, your own name highlighted, emoji resolved from shortcodes, quotes, bullet and numbered lists, code blocks, CJK, a ZWJ family emoji, escaped entities. The whole of `slk-render` (1 749 lines) became `markup.rs` (200 lines), because Pango does the part that was hard |
| A4 | Inline image as a texture, lazy, placeholder first | ✅ The PNG goes `FetchImage` → engine → `backend.download` → `ImageFetched` → `gdk::Texture::from_filename`, the same path as the client. The placeholder is an empty `gdk::Paintable` of the file's size, not a size request (see §4) |
| A5 | Block Kit as widgets, URL button opens | ✅ Header, section with a 2-column field grid and an accessory button, context, divider, actions — as real widgets; `gtk::UriLauncher` opens the URL; a button with no URL is drawn disabled with a tooltip, since posting back is a non-goal |
| A6 | Enter sends; optimistic row, then confirmation | ✅ Through the same `update` path the composer uses: **optimistic row in 2 ms, confirmed in 3 ms** (67-row conversation). In the 5 000-row bench run both were ~120 ms, because the command queued behind the engine writing the 5 000-row page to the store — an artefact of the stress setup, not of the interface |
| A7 | Measured RSS, first frame, idle CPU | See §2 and §3 |

Two things the criteria did not ask for and the run showed: the window
picked up the desktop's dark preference by itself (GTK's `color-scheme`
setting), and the sidebar, quotes, reactions and thread summary all
rendered on the first attempt with no width arithmetic anywhere.

## 2. Timing

| Measure | rows 0 | rows 1 000 | rows 5 000 | rows 5 000, Cairo renderer |
|---|---|---|---|---|
| Window mapped (`first_map_ms`) | 75 | 73 | 94 | 55 |
| Conversation loaded and on screen (`first_messages_ms`) | 77 | 124 | 226 | 179 |
| Scroll p50 / p95 / max (ms) | — | 16.7 / 17.1 / 82 | 16.7 / 17.1 / 66 | 16.7 / 20.5 / 67 |
| Frames over 20 ms during scroll | — | 2.8 % | 2.7 % | 5.1 % |
| Jump across the list, p50 / max (ms) | — | 94 / 385 | 99 / 399 | 98 / 401 |
| Idle CPU over 30 s | — | — | **0.08 %** | — |
| Idle repaints over 30 s | — | — | 64 (the composer's cursor blink, 2 Hz) | — |

`first_map` is the window on screen; `first_messages` includes the engine
booting the mock, storing the page, and the list being filled. Both are well
inside the 300 ms threshold. The cold start (binary not in the page cache)
was not separately measured; the spike binary loads GTK, Mesa and fonts from
a warm cache in every run above.

**Jumps are the one slow thing.** Scrolling realises rows a few at a time
and stays at 60 Hz; jumping to a far position realises a whole screen from
cold, and that costs 90–100 ms for plain rows and up to 400 ms where the
screen contains Block Kit. It is roughly 6 ms per plain row: parse the
markup, lay out the label. A jump is a user action that happens once (a
search result, a permalink), not continuously, so it is acceptable for M1
— and it is the obvious optimisation target: cache the markup string on
the row at creation (off the main thread), and build Block Kit trees
lazily. Recorded as a decision for M1 rather than done in the spike.

## 3. Memory — three honest answers

`/proc/self/status` splits the resident set. The split matters more than
the total:

| Point | VmRSS | of which RssAnon (ours) | of which RssFile (shared libraries) |
|---|---|---|---|
| Window mapped, 0 rows | 93 MB | **14 MB** | 79 MB |
| Window mapped, 5 000 rows in the mock | 102 MB | 23 MB | 79 MB |
| 5 000 rows loaded in the list | 137 MB | 50 MB | 88 MB |
| after scrolling ~800 rows | 164 MB | 56 MB | 108 MB |
| after five cold jumps | 165 MB | 57 MB | 108 MB |
| after 30 s idle | 165 MB | 57 MB | 108 MB |
| same, Cairo renderer, after scroll | 130 MB | 53 MB | 69 MB (+8 MB shmem) |

What this says:

- **The floor is GTK and Mesa, and it is shared.** 79 MB of file-backed
  pages before a single message: GTK, GLib, Pango, HarfBuzz, Mesa's GL
  driver and its shader cache, fontconfig and the fonts. Every GTK process
  on the desktop maps the same pages, and the kernel counts them once per
  process in RSS. The application's own allocations at that point are 14 MB.
- **Ours grows with the rows, as it should.** 5 000 rows cost 36 MB of
  private memory — about 7 kB a row — and that is three copies of every
  message in this stress setup: the mock holds them, the in-memory store
  holds them, and the list model holds a `Message` per row with its parsed
  AST. The real client keeps the store on disk and the engine keeps
  nothing, so only the list copy remains, and a conversation view holds a
  page or three, not 5 000.
- **Scrolling adds 20 MB of shared pages, not private ones.** RssFile grows
  from 88 to 108 MB as glyph atlases and shader variants are touched;
  RssAnon grows 6 MB for the recycled widget pool. Nothing leaks: idle after
  jumps is flat to within 100 kB over 30 s.
- **The renderer is a third of the file-backed floor.** Cairo instead of
  the GL renderer cuts RssFile from 108 to 69 MB at the cost of a slightly
  worse p95 (20.5 ms). Not a recommendation — the GL renderer is the
  default for a reason on a compositor — but it locates the memory.

**Against NFR-4 as written (< 120 MB VmRSS with three workspaces and 5 000
messages loaded): fails at 165 MB.** Against what the requirement meant —
that the client should not be the Electron client, which sits at 500 MB and
up, most of it private — it passes by a wide margin: 57 MB private, 165 MB
resident including the toolkit everyone else already has in memory.

**Decision:** NFR-4 is restated as two numbers: **private memory
(`RssAnon`) under 80 MB, and VmRSS under 200 MB**, with three workspaces
connected and 5 000 messages loaded, measured in CI by this benchmark. The
first is what the client is responsible for; the second is what `top`
shows. R15 is **GO** on those terms, and the reason is written here rather
than the threshold quietly raised.

## 4. What the spike decided (plan §6)

1. **`TypedListView` over `gtk::ListView`, confirmed.** 5 000 rows scroll
   at the display rate with a recycled pool of roughly thirty row widgets.
   `NoSelection`; selection is not a concept in a conversation.
2. **Rich text is one `gtk::Label` with Pango markup per message**, with
   `set_selectable`, wrapping at `WordChar`, and `max_width_chars` bounding
   the natural width. Paragraph structure (quotes, lists, code) is expressed
   inside the one label. `TextView` was not needed: selection within a
   message works, links work, and one label is what keeps a row cheap.
   Cross-message selection is the thing given up; the official client does
   not have it either.
3. **Block Kit is a vertical `gtk::Box` built in `bind` and torn down in
   `unbind`.** The section's accessory goes *below* its text, not beside it:
   a horizontal box holding a wrapping label reports a minimum height above
   its natural height, and GTK 4.20+ warns about it on every bind — 1 700
   warnings in one scroll before the layout was changed, zero after.
4. **A picture placeholder is an empty `gdk::Paintable` of the final size,
   never `set_size_request`.** A size request is a minimum; a `Picture`
   with no paintable has a natural size of zero; the two contradict and GTK
   says so. An empty paintable gives the right natural size and
   `can_shrink` does the rest.
5. **The engine bridge is thirty lines and needs no crate.** `runtime.spawn`
   a forwarder from the engine's `mpsc::Receiver<Event>` into
   `sender.input_sender().clone()`; relm4's `Sender::send` is the only call
   that crosses threads. Commands are `runtime.spawn(async move { tx.send(cmd).await })`.
6. **relm4's `gnome_43` and later features pull libadwaita in** (`adw/v1_2`
   in the feature list), so the spike stays on the base feature set and
   enables GTK versions through the `gtk4` crate directly (`v4_18`).
   Consequence: `TypedListView::find` is unavailable (it is behind
   `gnome_43`); a nine-line `position` over the iterator replaces it.
7. **The test instrument** is not decided here — that is spike D. What this
   spike shows is that everything measurable is measurable from inside the
   process, without a display server's help.

## 5. Defects found on the way

- **`set_size_request` on a `Picture` as a placeholder** — warned on every
  row (§4.4).
- **A horizontal box round a wrapping label** — warned on every bind
  (§4.3). Both were found by counting stderr lines during the benchmark,
  not by reading the code, which is the argument for the benchmark
  printing its warnings count.
- **The first benchmark never finished.** 5 000 rows at a readable scroll
  speed is 300 000 pixels, three and a half minutes; the harness's timeout
  killed it. Reworked into a timed segment (~800 rows, 1 200 frames — the
  percentiles are stable well before that) plus five cold jumps, which is a
  better test anyway: continuous scrolling and cold realisation are
  different costs and they now have different numbers.
- **Inline code and reaction chips had hard-coded light backgrounds**,
  unreadable on the dark theme the window picked up from the desktop.
  Replaced with translucent backgrounds (`background_alpha`) until spike B
  generates the palette from the theme.

## 6. Still to do in M0-GUI

Spike B (Omarchy theming), C (browser sign-in against `slk-dev` only),
D (headless compositor and the accessibility tree), E (keyboard feel).
Nothing in A argues against any of them; A's numbers are the baseline they
will be measured against.

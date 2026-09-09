# M0-GUI Spike — Findings

| | |
|---|---|
| Status | Spikes A–E complete, 2026-09-09; C's success path awaits a human sign-in |
| Plan | [M0-GUI-SPIKE-PLAN.md](./M0-GUI-SPIKE-PLAN.md) |
| Binary | `spike/shell` — relm4 0.11 over the real `slk-sync` engine against `MockBackend`, no network |
| Environment | Rust 1.98.1, GTK 4.22.4, relm4 0.11.0 / gtk4-rs 0.11.4, Arch Linux 7.1.9, Hyprland on Wayland, Mesa (default GSK renderer), integrated GPU |
| Verdict | **GO on all four risks.** R14 relm4, R15 memory (NFR-4 restated, §3), R16 browser sign-in (every automatic path proven, §7), R17 list performance. M1 can start |

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

## 6. Spike B — Omarchy theming

| # | Criterion | Result |
|---|---|---|
| B1 | Read `colors.toml`, document the schema | ✅ Schema in §6.1. Read from `~/.local/state/omarchy/current/theme/colors.toml` |
| B2 | Generate GTK CSS, apply at `APPLICATION` priority | ✅ ~30 rules from nine `@define-color` names; the message renderer takes a `Semantic` palette derived from the same file (link, mention, dim, code, eight author colours) |
| B3 | Recolour live on `omarchy-theme-set`, within a second | ✅ **550 ms after the command started** — `omarchy-theme-set gruvbox` itself took 3.3 s (it retints waybar, the browser, VS Code…); the window had already changed. Restoring `tokyo-night` reloaded again |
| B4 | The `themed/*.tpl` mechanism | Investigated, not used — §6.2 |
| B5 | Off Omarchy: built-in dark and light, chosen by `color-scheme` | ✅ With `HOME` pointed at an empty directory the source is `Builtin("dark")`, decided by the settings portal's `color-scheme`; `--theme file.toml` loads any palette in the schema |
| B6 | Decorations and `app_id` | ✅ Hyprland advertises both `zxdg_decoration_manager_v1` and `org_kde_kwin_server_decoration_manager`; GTK 4.22 binds the KDE one, receives `default_mode = Server`, and draws **no client-side titlebar** — the window appears as a bare surface with Hyprland's border, which is exactly what a tiled window should be. `app_id` is `dev.olivka.slack_light.spike` (the application id); the client's will be `dev.olivka.slack_light` |

### 6.1 `colors.toml`, as it is

```toml
mode = "dark"                       # or "light"
accent, selection, muted
background, dark_background, darker_background, lighter_background
foreground, dark_foreground, light_foreground, bright_foreground
red, yellow, orange, green, cyan, blue, magenta, brown
bright_red, bright_yellow, bright_green, bright_cyan, bright_blue, bright_magenta
```

Twenty-eight keys, all `#rrggbb`. `brown` is absent from older themes, so it
has a default. The built-in dark and light palettes are written in the same
schema, which is what makes one generator enough.

### 6.2 How Omarchy actually applies a theme

Read from `/usr/share/omarchy/bin/omarchy-theme-set` rather than assumed:

- **`current/theme` is a directory, not a symlink.** The next theme is
  staged beside it as `next-theme`, then `rm -rf current/theme` and `mv`.
  A file monitor on the directory itself fires once and then watches a
  path that no longer exists. The monitor is on the *parent*
  (`~/.local/state/omarchy/current/`), filtered to the `theme` and
  `theme.name` entries, debounced 250 ms because the swap is several events.
- **Templates** (`~/.config/omarchy/themed/*.tpl`, over
  `/usr/share/omarchy/default/themed/*.tpl`) are rendered by
  `omarchy-theme-set-templates` with `sed` substitutions of `{{ key }}`,
  `{{ key_rgb }}`, `{{ key_strip }}` — and the output lands **inside the
  theme directory** as `current/theme/<name>`. So a `slack-light.css.tpl`
  would give the application a ready CSS file on every theme change. Not
  used, because the application has to work off Omarchy anyway, and one
  generator over one schema is simpler than two paths; the template is
  what a user who wants to override the mapping would write, and the
  `@define-color` names are what it would target.
- **Hooks** (`~/.config/omarchy/hooks/theme-set.d/*`) run after the swap
  with the theme name as `$1`. Not needed: the monitor sees the swap first.
- **`omarchy-theme-set-gnome`** sets gsettings `color-scheme` and
  `gtk-theme` (`Adwaita` / `Adwaita-dark`) from the palette's `mode`. That
  is why spike A's window came up dark without being told — and why a light
  palette over the desktop's dark GTK theme left the buttons dark: the
  application now sets its own `gtk-theme-name` from the palette's `mode`
  rather than trusting the desktop's.

## 7. Spike C — browser sign-in

| # | Criterion | Result |
|---|---|---|
| C1 | Launch a Chromium-family browser in a throwaway profile over `--remote-debugging-pipe` | ✅ Chromium 151 found on PATH (Chrome 152 also present); handshake `Browser.getVersion` answers in well under a second |
| C2 | Read the `d` cookie and the `xoxc` tokens over CDP | Built: `Storage.getCookies` polled each second for `d=xoxd-…` on `slack.com`; then `Target.attachToTarget` on the `app.slack.com` page and `Runtime.evaluate` of `localStorage.localConfig_v2` for the teams and tokens. **Not yet exercised: it needs a person to sign in** — see below |
| C3 | Refuse `slack://` deep links | ✅ `Default/Preferences` seeded with `protocol_handler.excluded_schemes.slack = true` before launch |
| C4 | Wipe the profile on every path | ✅ Verified after the timeout run: no `/tmp/slack-light-signin-*`, no browser process left |
| C5 | Store through `auth::add`, `auth.test` against `slk-dev` only | Built with a hard guard: any team whose domain is not `slk-dev` is skipped with a reason, and nothing is written until `auth.test` says yes. **Awaits C2** |
| C6 | Failure modes named | ✅ `no_browser` (0 ms, with `--browser /nonexistent`), `timeout` (12.5 s with `--timeout 12`), and — found by accident — `no_devtools` |

**The accident is the finding.** The first run reported `no_devtools` in
27 ms. The pipe ends are numbered 3–6 in the parent, so a naive
`dup2(child_write, 4)` in the child landed on top of the parent's own write
end, which the next line then closed — taking the browser's fd 4 with it.
Both ends are now moved above 10 with `F_DUPFD` before being placed on 3
and 4. Worth writing down because every DevTools-pipe implementation has
to get this right and none of the documentation says so.

**What remains is a human.** `spike-signin` opens the browser at Slack's
sign-in page and waits five minutes; C2 and C5 complete the moment
someone signs in to `slk-dev` in that window. Run:

```
target/release/spike-signin --save
```

It prints the team and a five-character token prefix, never the token, and
saves only for `slk-dev`. The `d` cookie it obtains reaches every workspace
on the account, as it always has; the spike stores it exactly where
`auth add` does and nowhere else.

## 8. Spike D — testing without a human

| # | Criterion | Result |
|---|---|---|
| D1 | Start under a headless compositor in CI | ⚠️ **Not on this machine.** No `weston`, `cage`, `sway` or `Xvfb` is installed and the job cannot install packages. `gtk4-broadwayd` *is* installed and the shell runs under it headlessly (window mapped in 33 ms) — but GTK's AT-SPI backend is compiled for X11 and Wayland displays only, so under Broadway nothing appears on the accessibility bus, even with `GTK_A11Y=atspi`. **CI needs `weston --backend=headless` (Debian/Arch package `weston`)**; on the developer's machine the suite runs against the live Hyprland, which is what it did here |
| D2 | The accessibility tree, walked and asserted | ✅ `spike/a11y/tree.py` — PyGObject over the a11y bus (address from `org.a11y.Bus`), no pyatspi, no Rust crate. 87 nodes for a twelve-row conversation: `application → window → list → list item → label`, the composer as a text entry. Names are what GTK gives by default (label text, window title); the sidebar rows have no accessible name beyond their labels — M1 should set `accessible-role`/labels on rows so a reader says "engineering, 1 mention" rather than two labels |
| D3 | Component logic tested without widgets | ✅ Confirmed the shape: relm4's `ComponentSender` cannot be constructed outside a running component, so `update()` cannot be driven from a test. The answer is `logic.rs`: the landing pick, upsert placement, keyboard stepping and chord-to-accelerator conversion are plain functions with six tests, and `update()` calls them. That is the rule for `slk-ui`: **decisions in plain modules, `update()` a dispatcher** |
| D4 | End to end: open, send, confirmed — through the tree | ✅ `spike/a11y/e2e.sh`: 8 checks, all passing, driven by `wtype` and read from the tree (§9) |

**Input injection.** `wtype` (the Wayland virtual-keyboard protocol) types
into the focused window; focus is set with Hyprland's dispatcher, whose
syntax changed in 0.56 to a Lua form — `hl.dsp.focus({ window = "class:…" })`;
the legacy `focuswindow class:…` is refused, including over the IPC socket.
Recorded because it cost an hour.

## 9. Spike E — the keyboard

| # | Criterion | Result |
|---|---|---|
| E1 | `slk-config` actions bound as GTK shortcuts | ✅ `keys.rs`: each `Action` becomes a `gio::SimpleAction` on the application with the preset's chord as its accelerator, converted from the rendered form (`ctrl+k` → `<Control>k`). Eight actions bound; four came from the `slack` preset, four fell back to spike defaults because the preset does not bind them (`next/prev_conversation`, `normal`, `workspace_1`) — a gap in the preset to close in M1 |
| E2 | Focus visible and sensible | ✅ The composer takes focus on map; `ctrl-k` moves it to the jump entry; Enter or Escape there returns it. Before the fix, `wtype` after focusing the window typed into nothing — GTK gives a new window no focus widget, and the first keystroke was lost. E2 is a rule now: **every action ends with focus somewhere named** |
| E3 | Everything without the mouse, tiled | ✅ The e2e run: `ctrl-k`, `des`, Enter opens `#design`; typing sends; `alt-Up` moves to `#leads`; `ctrl-u` clears; Escape returns; F1 lists the bindings. All from `wtype`, the window tiled on Hyprland |

## 10. Still to do in M0-GUI

Only the human half of spike C: sign in to `slk-dev` in the window
`spike-signin --save` opens. Everything else in the plan has a result
above. M1 starts from these decisions:

1. `slk-ui`: relm4, `TypedListView`, one Pango label per message, Block
   Kit as vertical widget trees, decisions in plain modules.
2. `slk-theme`: the generator in `theme.rs`, the parent-directory monitor,
   the palette's `mode` driving `gtk-theme-name`.
3. `slk-auth`: `spike/signin` promoted, with the same guard until the real
   client's own confirmation dialog replaces it.
4. Tests: `logic`-style unit tests; the a11y tree as the instrument;
   `weston --backend=headless` in CI; `wtype` for input.
5. Focus is a stated post-condition of every action.

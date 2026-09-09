# M0-GUI — Spike Plan: what a GTK client has to prove first

| | |
|---|---|
| Status | Plan, 2026-09-09 |
| Follows | [ADR-001](./ADR-001-native-gui.md); the original [M0-SPIKE-PLAN.md](./M0-SPIKE-PLAN.md) proved the Slack side and is not repeated |
| Duration | one week |
| Output | a `spike/` binary and `M0-GUI-FINDINGS.md`, in the format of `M0-FINDINGS.md`: a result per criterion, what was measured, what it changes |
| Verdict | GO / NO-GO on R14 (relm4), R15 (memory floor), R16 (browser sign-in), R17 (list performance) |
| Precondition | the six carried-over crates build in this repository; `slk-dev` exists and holds the fixture corpus |

The Slack layer is not in question — it ran for two milestones. What is in
question is whether GTK4, driven from Rust through relm4, gives a client
that is fast, small, themed by Omarchy, and testable without a human. Each
spike below answers one of those, and each has a number attached so the
answer is a measurement rather than an impression.

## 1. Spike A — the shell

**Binary:** `spike/shell` — relm4 application over the real `slk-sync`
engine against `MockBackend`. No network.

| # | Acceptance criterion |
|---|---|
| A1 | A window with a sidebar (sections, badges), a conversation pane, a composer; the sidebar and conversation are fed by `Event`s from the engine running on a tokio runtime thread; the GTK main thread does no I/O (enforced: the engine handle is `Send`, the UI holds only channel ends) |
| A2 | The conversation is a `gtk::ListView` with a relm4 factory — **not** a `gtk::ListBox` — holding the demo channel plus 5 000 synthetic messages; scrolling is smooth at 60 fps on the integrated GPU (measured with `GTK_DEBUG=frames` or `GDK_DEBUG=…`, report the frame time distribution) |
| A3 | A message row renders the rich-text AST as Pango markup: bold, italic, strike, code, a link that is clickable, a mention, an emoji, a quote, a code block, a list — the whole `slk-core` fixture corpus, no panics, and every construct visibly distinct |
| A4 | An inline image from the fixture PNG as a `gdk::Texture`, sized to fit, lazily loaded when the row is realised, with a placeholder of the right size before |
| A5 | A Block Kit message: header, section with fields, context row, actions with a URL button — as widgets, and the button opens the URL |
| A6 | `Enter` in the composer sends through the engine; the optimistic row appears, then confirms — the same `local_id` path as before |
| A7 | **Measured:** RSS after start with the demo workspace open, after 5 000 rows have been scrolled, and after ten minutes idle. Startup wall-clock to first frame, cold and warm. Idle CPU over one minute |

**Thresholds:** RSS ≤ 120 MB at every point; first frame ≤ 300 ms warm;
idle CPU ≤ 0.5 %. Above 150 MB RSS or 500 ms is a NO-GO on R15 and a
conversation about what to change, not a silent acceptance.

## 2. Spike B — Omarchy theming

**Binary:** the same shell, with `slk-theme`.

| # | Acceptance criterion |
|---|---|
| B1 | Read `~/.local/state/omarchy/current/theme/colors.toml`; document its actual schema (the keys, not the guess) in the findings |
| B2 | Generate GTK CSS from it — window, sidebar, selection, links, mentions, code, borders, the composer — and apply it with `gtk::CssProvider` at `STYLE_PROVIDER_PRIORITY_APPLICATION` |
| B3 | Run `omarchy-theme-set` to another theme while the shell is open: it recolours within a second, without a restart, without a flash of the wrong colours. Decide **how** it is noticed — a `gio::FileMonitor` on the symlink, or a hook installed into `theme-set.d/` — and record why |
| B4 | Investigate `~/.config/omarchy/themed/*.tpl`: if Omarchy renders templates with the palette on theme change, shipping one may replace B2's generator entirely. Record what the mechanism actually does |
| B5 | Off Omarchy (no symlink): a built-in dark theme, a built-in light theme, and `color-scheme` from the settings portal selects between them |
| B6 | Window decorations: Hyprland prefers server-side. Confirm the window asks for SSD via `xdg-decoration` and looks right with Omarchy's Hyprland config; record the `app_id` for window rules |

## 3. Spike C — browser sign-in

**Binary:** `spike/signin` — no UI beyond a dialog.

| # | Acceptance criterion |
|---|---|
| C1 | Find a Chromium-family browser (Omarchy ships Chromium); launch it with `--user-data-dir=<tmp>` and `--remote-debugging-pipe` (preferred over a port: no listening socket, and it works for sandboxed browsers), pointed at `https://slack.com/signin` |
| C2 | Over the DevTools protocol, wait for the `d` cookie to appear, read it, and read the `xoxc` token(s) and team ids the way the web client exposes them (`localConfig_v2` in localStorage, or the boot page — record which works) |
| C3 | Pre-seed the profile so `slack://` deep links are refused, or the flow ends in a "open in app?" prompt instead of a signed-in web client (msga hit this; see its `blockDeepLinks`) |
| C4 | Wipe the profile after, and on every failure path; verify nothing is left under `$TMPDIR` |
| C5 | Feed the result into the existing `auth::add` storage; `auth.test` succeeds against **`slk-dev` only** |
| C6 | Failure modes reported distinctly, each with a sentence a user can act on: no browser found, browser refused to start, DevTools did not answer, user closed the window, timeout. A silent hang is a failing criterion (the D9 rule) |

**This spike touches a real account.** It signs into `slk-dev` and nothing
else. The `d` cookie it obtains reaches every workspace on the account, as
it always has; the spike stores it exactly where `auth add` does today and
nowhere else.

## 4. Spike D — testing without a human

| # | Acceptance criterion |
|---|---|
| D1 | The shell starts under a headless Wayland compositor in CI (`weston --backend=headless-backend.so` or `cage`), with no real display |
| D2 | The accessibility tree (AT-SPI, via the `atspi` crate or `busctl`) exposes the sidebar rows, the message rows and the composer with usable names; assert on it the way the pty harness asserted on the screen. Record what GTK exposes by default and what needs explicit `accessible-role`/labels |
| D3 | A relm4 component's `update()` is driven with a test sender and asserted on without any widget — the fast layer of the test pyramid |
| D4 | One end-to-end check: start against the mock, open a conversation, send a message, see it confirmed — through D2's tree |

## 5. Spike E — what the keyboard feels like

Not a measurement; a judgement, recorded honestly.

| # | Criterion |
|---|---|
| E1 | `ctrl-k` jump-to, `ctrl-1..9` workspaces, `↑`/`↓` in lists, `Enter` opens, `Esc` back, `ctrl-Enter`/`Enter` send — bound through `slk-config`'s action names as `gtk::ShortcutController` entries, so remapping and the generated shortcuts window still work |
| E2 | Focus is always visible and always somewhere sensible after every action (GTK's default focus handling is not) |
| E3 | Everything in E1 works without touching the mouse, in Hyprland, with the window tiled |

## 6. Decisions the findings must record

1. `ListView` + factory confirmed for the conversation, or what replaced it and why.
2. Rich text: Pango markup in `gtk::Label`, or `gtk::TextView` with tags — and the selection/copy behaviour that decided it.
3. How theme changes are noticed (monitor vs hook vs template).
4. The `colors.toml` schema, verbatim.
5. Browser sign-in: pipe vs port, where the tokens were read from, which Chromium flags were required.
6. The test instrument (a11y tree) and what it could not see.
7. Measured RSS, first-frame time, idle CPU, and scroll frame times — with the machine they were measured on.
8. GO / NO-GO per risk.

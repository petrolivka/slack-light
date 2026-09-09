# ADR-001 — A native GTK4 application, not a terminal one

| | |
|---|---|
| Status | **Accepted** 2026-09-09 |
| Supersedes | the `slktui` project (a Ratatui client, M0–M2 complete, abandoned at commit `0c6aa2f`) |
| Decided by | petr |
| Consequence | this repository; a clean history; the six UI-independent crates and the documentation carried over |

## Context

`slktui` reached M2: a working day in Slack needed no browser. It was
verified end to end against a live workspace, had 176 acceptance checks
driving the binary through a pseudo-terminal, and its Slack layer — the
session route, the web-client websocket, the undocumented endpoints, the
SQLite cache, the sync engine — is solid and measured.

Its interface was the problem, and the problems were not bugs. They were the
shape of the medium:

- **Emoji and CJK widths cannot be known, only measured.** No global width
  policy works: alacritty needs widening for ZWJ sequences and narrowing for
  VS16 sequences at the same time. The client grew a per-terminal probe and a
  correction table. It worked, and it was a solution to a problem a
  text-rendering engine does not have.
- **Images need a protocol.** Kitty graphics, Sixel, iTerm2, half-blocks;
  detection that has to distinguish "no" from "no answer"; nothing at all
  inside tmux. A screenshot is a normal Slack message, and it was the hardest
  thing on screen.
- **Block Kit is a widget tree** — buttons, fields, context rows, images —
  drawn into a monospace grid it was never designed for. It rendered; it
  rendered as a caricature.
- **Mouse hit-testing was arithmetic**, including the blank padding that
  bottom-anchors a short conversation; it was wrong by four rows once.
- **Text selection, clickable links, hover, file previews** — every one a
  workaround, none native.

Slack is a medium-rich product: images, threads, reactions, structured bot
messages, file previews. A character grid is the wrong surface for it. The
one thing a terminal client does that nothing else can — running over SSH —
is not what Slack is used for.

## Decision

**Build a native GTK4 application in Rust, `slack-light`, on relm4, without
libadwaita.** Keep everything below the interface. Drop the interface, its
rendering layer and its image layer entirely, and start the repository with
a clean history.

### Why GTK4

It is the only option that is actually native. iced, egui and Slint draw
their own widgets: they ignore the system cursor, fonts, scaling and input
methods, and they do not look or behave like anything else on the desktop.
Tauri is a webview — Electron in a smaller box. Qt is fine and it is a C++
world with weaker Rust bindings. GTK4 runs natively on Wayland, which is
where the target user lives (Hyprland, on Omarchy), and it is what Omarchy's
own launcher (Walker) is built with.

### Why relm4

relm4 is not a different toolkit; it is a way to write GTK4 in Rust without
losing your mind — a model, messages, and a declarative view, in the shape of
Elm. It is at 0.11 (April 2026) and moves: expect a breaking release or two
during the project. That is accepted, because the alternative is raw
`gtk4-rs` with hand-written state plumbing.

### Why not libadwaita

libadwaita is GNOME's look. It carries its own palette, allows only
light/dark and an accent colour, and deliberately does not follow GTK
themes. The user wants the application coloured by Omarchy's current theme,
which is the opposite requirement. Plain GTK4 with application CSS generated
from Omarchy's palette gives full control. It costs the ready-made GNOME
widgets (`AdwHeaderBar`, `AdwToast`, `AdwNavigationSplitView`); the plain
GTK equivalents are less pretty and entirely adequate.

### Theming, the Omarchy way

Omarchy does **not** theme GTK: its `config/` has no `gtk-4.0` and its themes
carry no `gtk.css`. What each theme has is `colors.toml`; the current theme
is the symlink `~/.local/state/omarchy/current/theme`; on a theme change
Omarchy runs hooks in `~/.config/omarchy/hooks/theme-set.d/`. So the
application reads `colors.toml`, generates its CSS, and re-applies it when
the symlink changes — the same thing every other Omarchy-aware application
does. Off Omarchy, it ships a dark and a light theme and follows the
desktop's `color-scheme` portal setting.

### Authentication, the msga way

[make-slack-great-again](https://github.com/punarinta/make-slack-great-again)
(C++/Qt6, the same session route) signs users in by launching a
Chromium-family browser in a **throwaway temporary profile**, letting them
sign in normally (SSO and 2FA included), reading the `d` cookie and the
`xoxc` tokens over the Chromium DevTools protocol, and wiping the profile.
It never touches the user's real browser profile. We adopt exactly that; the
manual-paste flow stays as the fallback. We do **not** adopt msga's other
mode — decrypting the Slack desktop application's cookie database through
the keyring — because it reaches into data that is not ours.

The security profile is unchanged from before: the same two credentials, the
same route, the same account-wide `d` cookie, the same terms-of-service grey
zone. What changes is that nobody copies a token out of DevTools by hand.

### What "lightweight" now means

A terminal client idles around 10 MB. A GTK4 application idles around
60–120 MB, and the official Electron client around 500 MB and up. The goal is
restated honestly: **well under a fifth of the official client, and never
above 120 MB with three workspaces connected.** The spike measures it.

## What carries over

| Crate | Lines | Status |
|---|---|---|
| `slk-core` — ids, domain model, rich-text AST, mrkdwn and `rich_text` parsers, emoji, permalinks | 2 589 | unchanged |
| `slk-api` — `SlackBackend` trait, session backend, websocket, mock, rate gate | 2 277 | unchanged |
| `slk-sync` — the engine: boot, counts, history, gap fill, outbox, notification policy, retention | 1 691 | unchanged |
| `slk-config` — TOML config, actions, keymap | 1 131 | keymap semantics adapted to GTK shortcuts |
| `slk-store` — SQLite + FTS5, migrations, retention | 1 027 | unchanged |
| `slk-notify` — desktop notifications | 152 | unchanged |
| `src/auth.rs`, `src/doctor.rs` | ~900 | auth kept; doctor rewritten for GTK/Wayland |
| Documentation: requirements, access strategy, architecture, M0 findings | | carried and amended, this ADR records how |

**Dropped:** `slk-tui` (5 205 lines), `slk-render` (1 749), `slk-art` (199),
the pty harness and its four suites (1 462 lines of Python), `UI-SPEC.md`,
`M1-STATUS.md`, `M2-STATUS.md`. About 60 % of the code survives, and it is
the 60 % that was hard: the M0 spike, the reconnect and gap-fill logic, the
parsers pinned by fixtures.

The three rules survive verbatim with one word changed: **the GTK main
thread never does I/O.** It sends a `Command`, receives an `Event`, and
renders what it has meanwhile. The `Command`/`Event` channel between engine
and interface is the same one; the interface on the other end is different.

## Consequences

- The M0 findings about *terminals* (§3 of `M0-FINDINGS.md`: width tables,
  graphics protocols, keyboard protocol) are now history rather than
  requirements. FR-K6, D8 and R5 are retired. The findings about *Slack*
  stand.
- `M0-SPIKE-PLAN.md` is kept as the record of what was proved; a new
  `M0-GUI-SPIKE-PLAN.md` covers what a GTK client has to prove before M1.
- Testing changes instrument. The pty harness reconstructed the screen from
  the escape stream and asserted on it; its GUI equivalent is the
  accessibility tree (AT-SPI), walked under a headless Wayland compositor.
  Component update logic is tested without widgets at all.
- Distribution changes shape: an AUR package matters more than
  `cargo binstall`, because the target user runs Arch.
- Nothing about account safety changes. The `d` cookie still reaches every
  workspace on the account, tests still run against the mock only, and the
  employer's workspaces are still never touched by anything automated.

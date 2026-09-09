# M1 — Skeleton: Status

**Complete.** "Follow a conversation and reply without opening the browser",
in a tiled window on Hyprland, coloured by the current theme — and every
piece of it driven from the keyboard and asserted through the accessibility
tree, against the mock.

| | |
|---|---|
| Date | 2026-09-09 |
| Follows | [M0-GUI-FINDINGS.md](./M0-GUI-FINDINGS.md) (the spikes) · [ADR-001](./ADR-001-native-gui.md) (the decision) |
| Verified against | the mock (`--anonymous`) for everything automated; the session backend is the one the terminal predecessor ran for two milestones and is unchanged |

---

## 1. Against the milestone

| Roadmap item | Result |
|---|---|
| One workspace live: sidebar with badges from counts | ✅ `slk-ui` over the unchanged `slk-sync` engine; badges are the engine's counts; several workspaces are wired at once because the loop is the same (`ctrl-1..5`, `ctrl-w`) |
| Open a conversation from the store, then the API | ✅ The engine's `Open` path, unchanged: cached messages first, fresh ones after |
| Live messages | ✅ `Upserted` and `Deleted` applied in place; the demo's scripted stream (`--demo`) shows it |
| Send with optimistic delivery | ✅ Optimistic row, then confirmation, measured through the real binary: 2 ms and 3 ms |
| Browser sign-in in the product | ✅ `slack-light auth add` drives a throwaway Chromium profile over the DevTools pipe, then **lists the workspaces found and asks which to store** — the `d` cookie is account-wide and `localConfig_v2` names every workspace on the account, so nothing is stored unasked. `--paste` is the other way in. The success path still awaits a person signing in to `slk-dev`; every automatic path is proven |
| Theming in the product | ✅ `slk-theme`: Omarchy's `colors.toml` when present, live on change; built-in dark/light by the desktop's `color-scheme` otherwise; `[theme]` in the config; `--theme file.toml`; `~/.config/slack-light/user.css` layered over the generated CSS |
| Panic report | ✅ `~/.local/state/slack-light/crash-<ts>.txt`, one line on stderr pointing at it, nothing about credentials or messages in it |
| `--anonymous` | ✅ The mock, with `--demo` (a live stream and a second workspace), `--demo-rows N` (a conversation the size of a real one), and a demo picture through the real download path |
| The a11y test suite | ✅ `tests/a11y/tree.py` and `tests/a11y/e2e.sh`: eight checks, keyboard in, tree out, 8/8 against the real binary |

Beyond the milestone: `--doctor` rewritten for a desktop (GTK version,
display, compositor, portal, notifications, a11y bus, theme source, a
browser for sign-in, credentials, cache); `--metrics` and `--bench` kept
from the spike so the numbers can be re-measured in CI; `--read-only`
honoured in the composer; retention and the media cache bound at start-up
as before.

## 2. Measured, through `slack-light --anonymous --demo-rows 5000 --bench --idle 10`

| | |
|---|---|
| Window mapped | 111 ms (the runtime and the engines are built before GTK now, and are inside this number) |
| Conversation of 5 007 rows on screen | 254 ms |
| Scroll p50 / p95 | 8.3 ms / 17.0 ms, 2.8 % of frames over 20 ms |
| Cold jump across the list, p50 | 92 ms |
| Idle CPU | 0.06 % |
| Private memory after scrolling | 54 MB (`RssAnon`); 164 MB resident, of which the shared toolkit pages |

Within the restated NFR-4 (private < 80 MB, resident < 200 MB) and the
first-frame budget.

## 3. What moved where

| From the spike | To | Changed |
|---|---|---|
| `spike/shell/src/{app,row,markup,blockkit,logic,keys,bench}.rs` | `crates/slk-ui` | `app.rs` holds several workspaces and routes events by team; the keymap preset comes from the config; `bench::report` is gated behind `--metrics`/`--bench`; sidebar rows carry one accessible label ("engineering, 1 mention") |
| `spike/shell/src/theme.rs` + `themes/` | `crates/slk-theme` | `load(&Choice)` takes the `[theme]` section; `Applied::new` layers `user.css` at `USER` priority |
| `spike/signin` | `crates/slk-auth::browser` | a library: `sign_in()` returns the outcome, the CLI decides what to store; `src/auth.rs` (the 0600 file, the guided paste) moved in beside it |
| `spike/shell/assets/spike.png` | `crates/slk-api/assets/demo.png` | `MockBackend::with_demo_image()`, so the demo has a picture with no file on disk |
| `spike/a11y/` | `tests/a11y/` | the class is `dev.olivka.slack_light`; the binary is the product with `--anonymous --demo-rows 30 --metrics` |

`spike/` is gone. What it proved is above; what it was is in the history at
`2faefd4`.

## 4. Configuration that is new

```toml
[theme]
source = "auto"      # auto | omarchy | builtin | file
builtin = "auto"     # auto | dark | light
file = ""            # a colors.toml of your own, in Omarchy's schema
user_css = true      # layer ~/.config/slack-light/user.css on top

[window]
width = 1100
height = 720
sidebar_width = 240

[keymap]
preset = "slack"     # non-modal by default now; "vim" adds j/k and a modal composer
```

The default preset changed from `vim` to `slack` (Q9 in the requirements):
a window whose composer swallows `j` because it is in the wrong mode is a
bug report waiting to happen, and `vim` is one line away.

## 5. Not done, and why

- **Threads, reactions, edits, search, files beyond one picture, notifications, mark-read** — M2, as the roadmap says. The engine has every one of them; the window shows a conversation and sends.
- **The shortcuts window and the command palette** — F1 lists the bindings in the status line, which is a placeholder for a `gtk::ShortcutsWindow` generated from the same table. M2.
- **Headless CI** — needs `weston --backend=headless`, which is not installed here (M0-GUI §8). `tests/a11y/e2e.sh` runs on a developer's Hyprland today.
- **Five of thirteen bindings fall back to defaults** because the `slack` preset does not bind them (`next/prev_conversation`, `normal`, `next_workspace`, `workspace_N`). They should be in the preset; a config-file change, M2.
- **A screen reader has not driven it.** The tree is complete and the sidebar rows are named; that is a proxy, and it is not the same as someone using Orca with it.

<div align="center">

# slack-light

**A native, lightweight Slack client for Linux desktops.**

[![Licence: GPL-3.0-or-later](https://img.shields.io/badge/licence-GPL--3.0--or--later-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-stable-orange.svg)](https://www.rust-lang.org)

</div>

Every workspace at once, channels and DMs, threads, reactions, mentions,
edits, files with inline images, Block Kit messages from your bots, search,
presence, status — the official client's shape, as a GTK4 application that
starts in well under a second, idles at nothing, and takes its colours from
your desktop.

Built for Wayland, verified on Hyprland, themed by
[Omarchy](https://omarchy.org) when it is there.

> **Status: M2a — the look.** `slack-light --anonymous` opens a themed window on a demo workspace; with a session it follows a conversation and replies. This is the successor to a
> terminal client that reached parity and turned out to be the wrong shape
> for a medium full of images and threads. The Slack layer — session route,
> websocket, cache, sync engine — carries over intact and verified. The GTK
> shell was spiked and measured — 60 fps over 5 000 rows, 0.06 % idle,
> themed live from Omarchy, driven from the keyboard and asserted through
> the accessibility tree — and then built. See [ADR-001](docs/ADR-001-native-gui.md)
> for why, [the findings](docs/M0-GUI-FINDINGS.md) for the numbers, and
> [M1-STATUS.md](docs/M1-STATUS.md) for what works today.

---

## ⚠️ Read this before signing in

**This is an unofficial client.** It talks to Slack the way the Slack web
client does, using your browser session (an `xoxc` token and the `d`
cookie) and several undocumented endpoints. Slack's API terms say
undocumented behaviour should not be relied on; it can change or break at
any time, and it has, twice in the last four years, each time fixed by the
community within days.

Using an unofficial client is a **grey zone under Slack's terms** and may be
**against your employer's policy** regardless of Slack's. On some plans and
security settings, Slack may notify workspace administrators about unusual
access. There is no documented case of an account being banned for using
wee-slack or emacs-slack, which have run on the same route for a decade —
but the decision, and the conversation with your admin if one is needed,
are yours.

To stay on the right side of that line, slack-light behaves like one person
using one client: one websocket per workspace, history fetched only when
you look at it, cached responses, rate-limited requests, an honest
User-Agent, and **no export, archive or bulk-download feature, ever**.

## Signing in

Choose **Sign in with your browser**. A Chromium window opens in a
throwaway profile — never your real one — you sign in to Slack normally,
including SSO and two-factor, and the application takes the session from
that window and discards the profile. Nothing is copied by hand. If you
would rather paste the token and cookie yourself, that still works.

**Credentials never leave your machine**, are never logged, and are stored
in the OS keyring when one exists, otherwise in a file only you can read.

## Where your data lives

| Path | What |
|---|---|
| `~/.config/slack-light/config.toml` | settings, keys |
| `~/.local/share/slack-light/store.sqlite` | message cache — your company's messages, on your disk, 0600, trimmed to 90 days / 5 000 per channel by default |
| `~/.cache/slack-light/media/` | thumbnails and images, bounded |

`slack-light cache stats` shows what it holds, `cache trim` applies the
retention limits now, `cache purge` deletes it, and `--no-cache` never
writes one.

## Building

```bash
./build.sh                 # cargo build --release, in a cgroup of its own — read CONTRIBUTING.md for why
./build.sh test
target/release/slack-light --anonymous     # the demo workspace, no credentials
tests/a11y/e2e.sh                          # keyboard in, accessibility tree out
```

Needs GTK 4.14+ development files (`gtk4` on Arch). No libadwaita.

## Documentation

| | |
|---|---|
| [docs/ADR-001-native-gui.md](docs/ADR-001-native-gui.md) | Why a native application, why GTK4 and relm4, why not libadwaita, what carried over |
| [docs/ANALYSIS-AND-REQUIREMENTS.md](docs/ANALYSIS-AND-REQUIREMENTS.md) | What and why: vision, domain analysis, requirements, risks, roadmap |
| [docs/SLACK-ACCESS-STRATEGY.md](docs/SLACK-ACCESS-STRATEGY.md) | Session token vs official app: coverage, stability, rate limits, risk |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Crates, data model, storage, sync engine, the GTK runtime, theming, config |
| [docs/M0-GUI-SPIKE-PLAN.md](docs/M0-GUI-SPIKE-PLAN.md) | What the GTK client must prove before anything else is built |
| [docs/M0-GUI-FINDINGS.md](docs/M0-GUI-FINDINGS.md) · [docs/M1-STATUS.md](docs/M1-STATUS.md) · [docs/M2-STATUS.md](docs/M2-STATUS.md) | What the GTK spikes measured; what the skeleton delivered; where parity stands |
| [docs/M0-SPIKE-PLAN.md](docs/M0-SPIKE-PLAN.md) · [docs/M0-FINDINGS.md](docs/M0-FINDINGS.md) | The original spike: what was proved about Slack's API, measured against a live workspace |

## Licence

GPL-3.0-or-later. Slack is a trademark of Slack Technologies, LLC; this
project is not affiliated with or endorsed by Slack or Salesforce.

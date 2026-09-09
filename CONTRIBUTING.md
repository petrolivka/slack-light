# Contributing to slack-light

Short on ceremony, long on the three things that are genuinely easy to get
wrong here.

## Getting set up

```bash
sudo pacman -S gtk4                # or your distribution's GTK 4.14+ dev package
./build.sh                         # cargo build --release, in a cgroup of its own
./build.sh test
cargo run -- --doctor
```

### Build through the wrapper, not through bare cargo

This matters more than it looks. Cargo runs one `rustc` per core, and on the
dependencies here — rustls, a bundled SQLite, tokio, gtk4-rs — a 24-core
machine peaks around **17 GB of RAM and 33 GB of swap**. Under `systemd-oomd`,
which Arch, Fedora and Ubuntu desktops enable by default, a build started from
a terminal shares that terminal's cgroup. When the pressure limit is hit, oomd
does not kill the build. It kills the cgroup, and the terminal window
disappears along with everything running in it.

That happened three times in the predecessor project before the cause was
found. It leaves no coredump and no kernel OOM message; the only trace is in
the journal (`systemd-oomd killed 41 process(es) in this unit`). Two defences,
both in the repository:

- `.cargo/config.toml` caps `jobs` well below the core count.
- `build.sh` runs cargo under `systemd-run --user --scope` with a memory cap,
  so a runaway compile is killed on its own rather than taking the session
  with it.

Bare `cargo build` is fine for an incremental change. Use the wrapper for a
clean or full build, and for anything you leave running.

## Before opening a pull request

```bash
cargo fmt --all
cargo clippy --workspace --all-targets   # must be clean; CI runs with -D warnings
./build.sh test
```

## Three rules that matter more than the rest

### 1. Never point anything automated at a real workspace

The predecessor's predecessor learned this the hard way: an automated UI
test typed into a signed-in instance, one keystroke landed in the wrong
field, and it wrote to a real account. Here the equivalent is a message in
your employer's `#general`.

- **Any test that drives the application must pass `--anonymous`.** That
  selects the mock backend and makes network writes impossible, not merely
  unlikely.
- **Fixtures come only from the `slk-dev` test workspace.** Slack responses
  are full of names, emails and company messages. Never commit a capture
  from anywhere else, and never point a probe at a workspace you did not
  create for this purpose.
- The `d` cookie is **account-wide**: credentials that reach one workspace
  reach all of them. `--read-only` exists for exploring a real workspace by
  hand. Use it.

### 2. Parsers must degrade, never panic

Half of what this client reads is undocumented and changes without notice.
"The shape is wrong" is an expected condition. A panic takes the whole
client down mid-conversation; an empty pane and a log line do not.

Every accessor returns an `Option`, unknown blocks and events are skipped
and logged at debug, and `crates/slk-api/tests/robustness.rs` mutates real
fixtures to prove it. If you add a parser, add it to the robustness suite
and, if it is a text parser, to `fuzz/`.

### 3. The GTK main thread does no I/O

Not a store query, not a file read, not a DNS lookup. It sends a `Command`
and renders whatever it has until the `Event` comes back. The engine lives
on a tokio runtime thread; the interface holds channel ends and nothing
else. If you find yourself awaiting on the main thread, or calling into
`slk-store` from a component, that is the bug.

## Working on the Slack layer

Half of what this client reads is undocumented, so the tools that watch it
are part of the work. They live behind the `dev-tools` feature so they never
land in a user's PATH.

```bash
cargo run --release --features dev-tools --bin probe     # read-only tour of every endpoint,
                                                          # plus an inventory of every type
                                                          # Slack sent that we do not render
cargo run --release --features dev-tools --bin seed -- --post   # fill a test channel with a corpus
cargo run --release --features dev-tools --bin seed -- --dump   # capture responses, scrubbed
cargo run --release --features dev-tools --bin wscheck 90       # listen to the live event stream
```

`probe` is the one to run when something looks wrong. If a parser stops
extracting what it should, Slack changed something. Fix the parser, and
write down what changed in the commit message: that knowledge is most of
the value.

## Working on the sync engine

Every reconnect, mark, echo and gap-fill rule in ARCHITECTURE.md §6 and the
requirements doc §7 is a deterministic scenario test in `crates/slk-sync/tests`.
If you change a rule, change its test first.

## Testing the interface

Three layers, fastest first:

1. **Component logic without widgets.** A relm4 component's `update()` is
   driven with a test sender and asserted on. Most behaviour lives here.
2. **The accessibility tree.** `tests/a11y/e2e.sh` drives the binary with
   `wtype` against `--anonymous` and asserts on the AT-SPI tree through
   `tests/a11y/tree.py` — the way the predecessor's pty harness asserted on
   the screen. On a developer's machine it runs against the live Hyprland;
   CI needs `weston --backend=headless` (GTK's AT-SPI backend exists only
   for X11 and Wayland displays — Broadway will not do). If a widget is not
   in the tree with a usable name, that is a defect twice over: a test
   cannot see it and neither can a screen reader.
3. **Eyes.** Screenshots at two window sizes in the pull request, taken on
   Hyprland.

## Secrets

Never log a token, a cookie or a message body at `info` or above. The
`tracing` redaction layer is a backstop, not a licence. `grep -r xox` on a
log file should find nothing.

## Commit messages

Say what changed and *why*. If you discovered something about Slack's API
or about GTK, write it down.

## Scope

Please do not propose export, archive or bulk-download features. Keeping
the traffic pattern squarely "one person, one client" is what keeps this
defensible, and it is the honest description of what the project does.

# M4 — Delight (v1.2): what was built, and what was measured

M4's acceptance in the roadmap is three words: **`C` items as chosen.** The
choice was made with Petr on 2026-09-10, before any of it was written:

- **Chosen:** FR-C2 (make, rename and archive channels; start a group),
  FR-C3 (the bookmark bar), FR-N6 (Slack's own sidebar sections), and
  FR-M7's `C` half (drafts synced with Slack's).
- **Not chosen:** FR-M11, scheduled messages. It stays `C` and open.
- **Left standing:** M3's refusal of FR-F6, custom emoji inside a message
  body. The reason in M3-STATUS §4 is unchanged.
- **Left for M5:** the three things M3-STATUS §5 called built but not
  proven — the OAuth route against Slack, Enterprise Grid, a screen reader.
  M5 is hardening, and that is what those are.

The rule every milestone since M2 was accepted under still applies — *the
requirements table is the checklist, not the roadmap* — so §1 walks the rows
M3 left open, not the roadmap's one-line description of M4.

Six commits, `b8b1df7` … the one that closes this document.
129 unit and scenario tests (from 96), **88 accessibility checks
passing** in `tests/a11y/e2e.sh` (from 72), clippy clean with `-D warnings`,
`cargo fmt` applied. Every automated run is against `--anonymous`.

---

## 1. The audit

| Req | What it asks | State |
|---|---|---|
| FR-C2 | Create channel (public/private), archive, rename; create group DM | **Met** |
| FR-C3 | Bookmarks list in the conversation header | **Met** for link bookmarks; the other kinds are §4 |
| FR-N6 | Slack's own sidebar sections, when the backend provides them | **Built.** The session route's answer has never been captured — §5 |
| FR-M7 | Drafts synced with Slack's drafts (the `C` half) | **Built.** Same — §5 |
| FR-M11 | Schedule a message; list and cancel | **Not chosen for M4.** Open |
| FR-F6 | Custom emoji as inline images | **Refused in M3; the refusal stands** |
| FR-B4 | Interactive Block Kit | `W`, and stays `W` |

---

## 2. What Slack does, as far as it can be known from here

Nothing in this milestone was captured from `slk-dev`: CONTRIBUTING rule 1
keeps automation away from real workspaces, and the shapes below are the
ones the web client is known to use. Every parser for them is a free
function with its own tests, every accessor is an `Option`, and each one
degrades to "none" rather than failing.

**A bookmark bar holds four kinds of thing, and one of them has a link.**
`bookmarks.list` answers with links, canvases, files and folders. A folder
has no link at all; a canvas or file link opens the web client. The bar
shows the links and nothing else, rather than rows that do nothing.

**Sidebar sections are a linked list, not a list.** `users.channelSections.list`
sends them in no particular order; each names the next in
`next_channel_section_id`, and the first is the one nobody names. Slack's
built-in groups — stars, channels, direct messages, Slack Connect, apps —
are in the same list as the ones people made, told apart by `type`
(`standard` is a made one). A large section's members arrive a page at a
time.

**A draft is rich text, not text.** Slack keeps a draft as `rich_text`
blocks with a list of destinations. Scheduled messages live in the same
list, and an update has to quote back the `last_updated_ts` it is replacing.

**A group direct message is named after its members' handles** —
`mpdm-alice--bob--petr-1` — and `conversations.open` without `return_im`
answers with a bare id and nothing to name it by. Slack caps a group at nine
people, you included.

**Archiving has no undo here, and two refusals.** A direct message cannot
be archived and neither can the workspace's `#general`; an archived channel
keeps its name, so the name is still taken.

---

## 3. Defects found, and what they were hiding

**`toggle_section` had done nothing since M2.** The palette listed "Fold or
unfold this sidebar section", and running it answered "unbound action". An
audit of every action name against the window's dispatcher found it; nothing
had ever enumerated the handlers, and the one test that walks every action
walks their *keys*. It now folds the section the keyboard is in — the
focused row, else the selected one, else the open conversation's — and the
last of those is what lets the same key open it again: folding takes the
selected row with it.

**The first group anybody made would have been called `mpdm-alice--bob--petr-1`.**
The sidebar showed a conversation's name as Slack sent it, and the demo had
never had a group, so nothing had ever shown what that looks like. Found
while writing the accessibility check for `/group`. A group is now called
by its people, without the person looking — which needed the directory to
learn handles, because the name is made of handles and the label is not.

**A comment claimed a reconnect re-runs boot. It does not.** Written for the
sections refresh, and caught by reading the reconnect loop before relying
on it. A reconnect now refreshes sections and drafts itself — a reconnect is
usually a laptop that slept through somebody rearranging things on their
phone.

**The first rule written for drafts would have lost them.** "A draft that
vanished at Slack was finished elsewhere, so delete it here" is right —
until `drafts.list` changes shape, parses to an empty list, and every
synced draft is deleted at once. A draft is the thing people do not forgive
a chat client for losing. The rule that shipped deletes only a copy that is
still exactly what Slack last held; anything edited here since is kept and
saved again. The worst a changed shape can do now is drop copies that the
next good sync brings back.

## 3a. What the first full run of the suite found

The first end-to-end run against M4 passed 82 checks of 86. The four
failures were worth more than the checks that passed.

**Opening a channel from the engine went straight back.** `/create` made
the channel and opened it, and a moment later the window was on `#general`
again. The sidebar opens a conversation when one of its rows is selected,
and it re-selects the open conversation's row at the end of every rebuild —
a selection the window made itself, firing the same signal a click does. The
engine sends the new sidebar and "open this" in one burst, so the stale
"open" queued behind the real one and undid it. Every other time that was
harmless, because it named the conversation already open. It was exactly
wrong whenever the burst also changed *which* one was open: `/create`, and
joining a channel, which has worked this way since M2 and whose landing no
check had ever looked at. A selection the window makes itself is now quiet,
and every open moves the sidebar's highlight to what was opened, quietly,
so the two cannot disagree.

**Two checks passed while it happened.** With the window still on
`#general`, the suite's `/rename` renamed `#general` and its `/archive`
archived it — and both checks passed, because each asserted what the header
said, and the header said the right name about the wrong channel. It is
M3's lesson in a new place: "the thing is there" passes while it is also
somewhere it should not be. Both now also assert that `#general` is where
it was.

**A bookmark told a screen reader nothing about where it goes.** The name
carrying the link was set on the button, and GTK names a button after what
is inside it and ignores a name set on the button itself, so the tree read
back "📚 Runbook". It is the rule this project wrote down in M1 about list
rows, and the fix is the same: the name goes on the child label.

**The suite's own path through the sidebar moved.** Since FR-N6, the demo's
`#design` sits in its "Projects" section, so the conversation above it is
`#engineering` rather than `#leads`. An M2 check pressed alt-Up from
`#design`, and everything the run typed afterwards went into the one
conversation the image check relies on being untouched: two failures, one
cause, neither of them in the product. alt-Up follows the sidebar as drawn,
which is right; the check now starts from the direct message whose
neighbour is still `#leads`.

The run after the fixes: 88 for 88.

---

## 4. Decided against, with the reason

**Scheduled messages (FR-M11)** were not chosen for this milestone.

**Custom emoji in message bodies (FR-F6).** M3's reason stands: the body is
one label so that selection and copy work across a whole message, and a
label cannot hold a picture without giving that up.

**Canvas, file and folder bookmarks.** A canvas opens the web client, which
a native window should not do quietly; a folder has nothing to open.

**The second page of a large section.** Fetching it would be a request per
large section per boot. A conversation past the first page is drawn in its
built-in section instead, which is where it would be without FR-N6.

**Drafts the composer cannot hold.** Scheduled, sent, addressed to several
conversations, or only an attached file: none of them goes into a
composer, so none is synced.

**A draft is saved as the words typed.** `@alice` goes up as the text
`@alice`, not as a mention element; resolving it is what sending does, and
a draft is where words are kept, not sent.

**Syncing drafts on a timer, or from socket events.** At boot, after a
reconnect, on `Refresh`, and whenever a composer is left — not on every
keystroke, and not when nothing changed. The window leaves a composer on
every conversation switch; a request per click is the traffic FR-Z1 exists
to prevent, and a test counts the requests.

**Archive is never on a key.** Neither are create, rename or starting a
group, for the reason topic and leave are not: rare, not undone by pressing
the key again, and in archive's case, seen by everybody in the channel.
The palette reaches all four by name.

---

## 5. What is built but not proven

**`users.channelSections.list` and `drafts.*` have never answered this
client.** Every line is written to the shapes in §2, and every parser
degrades, but no real answer has been read. The first run against `slk-dev`
should be under `--read-only`, which reads sections and drafts and writes
nothing — and the fixture capture M5 plans should include both.

**A mention in a draft typed on the phone** is turned back into `@name`
through the directory. If the directory has not loaded when the sync runs,
it comes back as the id.

**Newer-wins compares two clocks.** A local edit carries the laptop's time,
a copy of Slack's carries Slack's. Two edits within the same few seconds on
two devices can resolve the wrong way; nothing slower than that can.

**Carried from M3, and left for M5 as decided:** the OAuth route has never
talked to Slack, Enterprise Grid is unverified, and no screen reader has
driven the client.

---

## 6. What M4 did not claim

FR-M11 remains the one open `C` row. Everything else in §6 of the
requirements is met, refused with a reason, or `W`.

M5 is hardening: the fixture suite from `slk-dev` — which now has sections,
drafts and bookmarks to capture — robustness and fuzz targets for the new
parsers, CI with the offline guarantee and the headless accessibility suite,
the AUR package, and the three unproven items above.

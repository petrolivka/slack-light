#!/usr/bin/env bash
# Drive the client from the keyboard, assert through the accessibility
# tree. Every run is against `--anonymous`: the mock backend, no network. Runs on the developer's Hyprland; CI needs a headless
# Wayland compositor (see the findings, D1).
#
#   tests/a11y/e2e.sh [path-to-slack-light] [screenshot-dir]
#
# It needs the desktop to itself. Keystrokes go to the compositor, not to a
# window, so whatever holds the keyboard receives them; the run aborts rather
# than typing anywhere else.
#
# Input goes in through wtype (the Wayland virtual-keyboard protocol);
# focus is put on the window through Hyprland's dispatcher; the assertions
# read the AT-SPI tree with tree.py. Nothing here touches a real account:
# the binary runs against the mock.
set -u
BIN=${1:-target/release/slack-light}
OUT=${2:-/tmp}
HERE=$(cd "$(dirname "$0")" && pwd)
CLASS=dev.olivka.slack_light
pass=0; fail=0
check() { if [ "$2" = 1 ]; then echo "  ok   $1"; pass=$((pass+1)); else echo "  FAIL $1   ${3:-}"; fail=$((fail+1)); fi; }
tree() { python3 "$HERE/tree.py" "$@" 2>/dev/null; }
# Measured on Hyprland 0.56.2: `hl.dsp.focus({ window = "class:..." })`
# answers `ok` — it does find the window, an unmatched selector answers
# `window not found` instead — and does not move the keyboard when another
# window is holding it. There is no dispatcher that takes focus by force.
# So this is a request, not a guarantee, and `w` below checks the result.
focus() { hyprctl dispatch "hl.dsp.focus({ window = \"class:$CLASS\" })" >/dev/null 2>&1; sleep 0.4; }

# Nothing is typed until the window under test is the one that will receive
# it. `wtype` talks to the compositor, not to a window: whatever has the
# keyboard gets the keystrokes. On this machine that could be the developer's
# real Slack, which is the accident CONTRIBUTING rule 1 is about, and it is
# also why a run with focus quietly elsewhere reports thirty failures that
# say nothing about the client. So: check, try once to take focus back, and
# if that fails abort the run rather than type into somebody's #general.
# Ours under either spelling. On Wayland the toplevel carries the GTK
# application id; under XWayland — and for a moment before GTK applies the
# app id — it carries the program name instead. Both are this client and
# neither is somebody else's window, which is the only question being asked.
ours() { [ "$1" = "$CLASS" ] || [ "$1" = "slack-light" ]; }

w() {
  local c t
  c=$(hyprctl activewindow -j 2>/dev/null | jq -r '.class // ""')
  if ! ours "$c"; then
    focus
    c=$(hyprctl activewindow -j 2>/dev/null | jq -r '.class // ""')
  fi
  if ! ours "$c"; then
    t=$(hyprctl activewindow -j 2>/dev/null | jq -r '.title // ""')
    echo
    echo "  ABORTED: the keyboard belongs to '${c:-nothing}' — ${t:-no title}."
    echo "  Nothing was typed. This suite drives the real keyboard; it needs"
    echo "  the desktop to itself. Close what took focus and run it again."
    pkill -x slack-light 2>/dev/null
    exit 2
  fi
  wtype "$@"
}
shot() { local g; g=$(hyprctl clients -j | jq -r ".[] | select(.class==\"$CLASS\") | \"\(.at[0]),\(.at[1]) \(.size[0])x\(.size[1])\"" | head -1); [ -n "$g" ] && grim -g "$g" "$1"; }

# A stray window from an earlier run confuses tree.py, which reads the first
# slack-light on the a11y bus. Start clean.
pkill -x slack-light 2>/dev/null && sleep 1
LOG=$(mktemp); LOG2=$(mktemp); LOG3=$(mktemp); LOG4=$(mktemp)
"$BIN" --anonymous --demo-rows 30 --metrics >"$LOG" 2>&1 &
PID=$!
sleep 3
focus

t=$(tree)
check "the window is on the a11y bus with its rows" "$( [ "$(echo "$t" | grep -c 'list item')" -gt 5 ] && echo 1 || echo 0 )"
check "the composer is a text field in the tree" "$( echo "$t" | grep -qE '^ *(text|entry)' && echo 1 || echo 0 )"
# A direct message has no name of its own — Slack's `ims` carry the other
# person's id and nothing else — so the sidebar has to resolve one from the
# user directory. Against a real workspace this row was a presence dot
# followed by nothing, and the demo hid it by inventing a name.
check "a direct message is named after the person in it" "$( echo "$t" | grep -qE "label '[●○] [a-z]" && echo 1 || echo 0 )" "$(echo "$t" | grep -oE "label '[●○][^']*'" | head -2 | tr '\n' ' ')"

# M4. The bookmark bar is buttons with names a screen reader can use: the
# title alone does not say where it goes, and a tooltip is not announced on
# focus. The client lands on #engineering, the demo channel that has a bar.
check "a bookmark says where it goes" "$( echo "$t" | grep -q "label 'Runbook, opens https://example.invalid/runbook'" && echo 1 || echo 0 )" "$(echo "$t" | grep -oE "label '[^']*opens[^']*'" | head -2 | tr '\n' ' ')"
# The person's own Slack section, with its emoji, as a heading in the sidebar.
check "a section made in Slack is a sidebar heading" "$( echo "$t" | grep -q "label '🚀 Projects'" && echo 1 || echo 0 )"

# E1: ctrl-k, type, Enter → the conversation opens.
w -M ctrl k -m ctrl; sleep 0.4
w "des"; sleep 0.3
w -k Return; sleep 1.2
check "ctrl-k, 'des', Enter opens #design" "$( tree | grep -q "label '# design'" && echo 1 || echo 0 )" "$(tree | grep "label '#" | head -2 | tr '\n' ' ')"
# The bar belongs to the conversation it was fetched for: #design has none,
# and a bar left over from #engineering would sit above the wrong messages.
check "a conversation with no bookmarks shows no bar" "$( tree | grep -q "opens https://example.invalid" && echo 0 || echo 1 )"

# E2: after the jump, focus is back on the composer: typing lands there.
w "keyboard test"; sleep 0.2; w -k Return; sleep 2.5
check "typing after the jump goes to the composer and sends" "$( tree | grep -q "keyboard test" && echo 1 || echo 0 )"
check "the send was confirmed by the engine" "$( grep -q send_confirmed_ms "$LOG" && echo 1 || echo 0 )"

# Completion. The popup offers, Enter takes, and the rest is unit-tested:
# a resolved mention renders as the name it started as, so the tree cannot
# tell `<@U0ALICE>` from `@alice` — slk-core's tests do that.
w "hi @al"; sleep 1.0
check "typing @ offers the people" "$( tree | grep -q "label '@alice'" && echo 1 || echo 0 )"
w -k Return; sleep 0.4
w ":roc"; sleep 1.0
check "typing a shortcode offers emoji, best first" "$( tree | grep -A1 "list item" | grep -q ':rocket:' && echo 1 || echo 0 )" "$(tree | grep -oE "':[a-z]+:'" | head -2 | tr '\n' ' ')"
w -k Escape; sleep 0.3
check "escape closes the completions and leaves the draft" "$( tree | grep -q ':rocket:' && echo 0 || echo 1 )"

# A draft survives a look at another conversation. Losing one is the thing
# people never forgive a chat client for.
w -M alt -k Up -m alt; sleep 1.2
w -M alt -k Down -m alt; sleep 1.2
w -k Return; sleep 2.0
check "a draft survives leaving the conversation" "$( tree | grep -q 'hi @alice :roc' && echo 1 || echo 0 )" "$(tree | grep -oE "'hi @alice[^']*'" | head -1)"

# E1: alt-Up moves to the previous conversation without the mouse.
# From the direct message with alice, whose neighbour above is #leads. Not
# from #design any more: since M4, #design sits in the demo's own "Projects"
# section, so the row above it is #engineering — and everything this run
# types from here on has to land in #leads, because #engineering is the one
# conversation it keeps untouched for the checks further down.
w -M ctrl k -m ctrl; sleep 0.4; w "ali"; sleep 0.3; w -k Return; sleep 1.2
w -M alt -k Up -m alt; sleep 1.2
check "alt-Up moves to the previous conversation" "$( tree | grep -q "label '🔒 leads'" && echo 1 || echo 0 )" "$(tree | grep -E "label '(#|🔒)" | head -2 | tr '\n' ' ')"

# Escape from anywhere returns to the composer; ctrl-u clears it.
w "abc"; sleep 0.2; w -M ctrl u -m ctrl; sleep 0.3; w -k Escape; sleep 0.3
w "after escape"; sleep 0.2; w -k Return; sleep 1.2
check "escape and ctrl-u leave a usable, empty composer" "$( tree | grep -q "after escape" && echo 1 || echo 0 )"

# The message cursor, and a reaction on the message it lands on. alt-k
# selects; alt-1 is the first quick reaction, which is :+1:.
w -M alt k -m alt; sleep 0.4
check "alt-k puts a cursor on a message" "$( tree --states | grep -q 'list item .*\[selected\]' && echo 1 || echo 0 )" "$(tree --states | grep -c selected) selected nodes"
before=$(tree | grep -c '👍' || true)
w -M alt 1 -m alt; sleep 1.0
after=$(tree | grep -c '👍' || true)
check "alt-1 adds a reaction chip to it" "$( [ "$after" -gt "$before" ] && echo 1 || echo 0 )" "before=$before after=$after"

# The thread pane: alt-t opens it on the message under the cursor, alt-w
# closes it. Both go through the same code the row's ↳ link does.
w -M alt t -m alt; sleep 1.5
check "alt-t opens the thread pane" "$( tree | grep -q "label 'Thread" && echo 1 || echo 0 )"

# A reply sent in a thread must appear once. It appeared twice: the pane
# looked the row up by the *new* timestamp, and the optimistic row it was
# meant to replace still carried the local one, so the confirmation was
# appended beside it instead of over it. Counting is the whole check —
# "the reply is there" passed the entire time it was there twice.
w "reply once and only once"; sleep 0.3; w -k Return; sleep 2.5
n=$(tree | grep -c 'reply once and only once')
check "a reply in a thread appears exactly once" "$( [ "$n" = 1 ] && echo 1 || echo 0 )" "$n copies"

w -M alt w -m alt; sleep 0.6
check "alt-w closes it again" "$( tree | grep -q "label 'Thread" && echo 0 || echo 1 )"

# Save then pin, back to back, on a message of our own. Two defects in one
# check: the pin mark only renders if the echoed row keeps its header (a
# replacement used to be grouped against the end of the list, which for the
# newest message is itself), and the second action only finds a message if
# the cursor survived the first one's echo.
w "mine to keep"; sleep 0.3; w -k Return; sleep 2.0
w -M alt g -m alt; sleep 0.4
w -M alt s -m alt; sleep 1.2
saved=$( tree | grep -c 'saved for later' )
w -M alt p -m alt; sleep 1.2
marks=$(tree | grep -oE "'📌[^']*'" | tr '\n' ' ')
check "save reports back" "$( [ "$saved" -gt 0 ] && echo 1 || echo 0 )"
check "pin finds the same message, and the row shows both marks" "$( echo "$marks" | grep -q 'pinned · 🔖 saved' && echo 1 || echo 0 )" "marks=[$marks]"

# Editing one's own message, and the refusal on somebody else's.
w -M alt e -m alt; sleep 0.5
check "alt-e says what enter will do now" "$( tree | grep -q 'editing — enter saves' && echo 1 || echo 0 )"
w " (fixed)"; sleep 0.2; w -k Return; sleep 2.0
check "the edit lands, marked as one" "$( tree | grep -q 'mine to keep (fixed)' && echo 1 || echo 0 )"
# #engineering is the one conversation the mock seeds with other people's
# messages and this run never types into, so its oldest message is somebody
# else's for certain — #general and #design start empty.
w -M ctrl k -m ctrl; sleep 0.4; w "engi"; sleep 0.3; w -k Return; sleep 2.5
w -M alt -k Home -m alt; sleep 1.0; w -M alt e -m alt; sleep 0.6
check "editing somebody else's message is refused in words" "$( tree | grep -q 'only edit your own' && echo 1 || echo 0 )" "status=$(tree | grep -oE "label '[^']*'" | tail -2 | head -1)"

# A slash command the workspace does not know must not eat what was typed.
w "/nonsense here"; sleep 0.3; w -k Return; sleep 2.0
check "an unknown slash command says so" "$( tree | grep -q 'not a command this workspace knows' && echo 1 || echo 0 )"
check "and the text comes back to the composer" "$( grep -q '^slash_returned=/nonsense here$' "$LOG" && echo 1 || echo 0 )" "$(grep '^slash_returned=' "$LOG" | head -1)"
w -M ctrl u -m ctrl; sleep 0.3

# The image viewer, from the keyboard so the suite can see it. The demo's
# second-newest message in #engineering is the screenshot.
w -M alt g -m alt; sleep 0.5; w -M alt k -m alt; sleep 0.5
w -M alt v -m alt; sleep 1.2
check "alt-v opens the image at its own size" "$( tree | grep -q "window 'Image'" && echo 1 || echo 0 )" "$(tree | grep -oE "window '[^']*'" | tr '\n' ' ')"
w -k Escape; sleep 0.5
check "escape closes it" "$( tree | grep -q "window 'Image'" && echo 0 || echo 1 )"

# The side pane's lists. One pane shows a thread, a search, a member list
# or a profile, and each answers with the same five row shapes.
w -M ctrl t -m ctrl; sleep 1.5
check "ctrl-t lists the threads with replies" "$( tree | grep -q '2 replies' && echo 1 || echo 0 )"
w -M alt m -m alt; sleep 1.5
check "alt-m lists who is in the conversation" "$( tree | grep -q "label 'alice'" && echo 1 || echo 0 )"
w -M alt c -m alt; sleep 1.5
check "alt-c offers the channels one could join" "$( tree | grep -q '#random' && echo 1 || echo 0 )"

# Managing the conversation: star, mute, and the pinned list. The star and
# the mute are toggles read back out of the header, which is the only place
# either one is visible for a conversation that is already open.
w -k Escape; sleep 0.4
# Whole labels, not fragments. The sidebar names the same conversation —
# `'#  engineering, 1 mention'` — so anything that matches a prefix matches
# the sidebar too, and then the check passes while the header still says the
# old thing. `#engineering` starts starred in the demo, so the first press
# takes it off.
check "the header starts starred, as the demo says" "$( tree | grep -q "label '★ # engineering'" && echo 1 || echo 0 )" "$(tree | grep -oE "label '[^']*engineering[^']*'" | tr '\n' ' ')"
w -M ctrl -M alt s -m alt -m ctrl; sleep 1.5
check "ctrl-alt-s unstars it, and the header says so" "$( tree | grep -q "label '# engineering'" && echo 1 || echo 0 )" "$(tree | grep -oE "label '[^']*engineering[^']*'" | tr '\n' ' ')"
w -M ctrl -M alt m -m alt -m ctrl; sleep 1.5
check "ctrl-alt-m mutes it, and that shows too" "$( tree | grep -q "label '# engineering 🔕'" && echo 1 || echo 0 )" "$(tree | grep -oE "label '[^']*🔕[^']*'" | head -1)"
w -M ctrl -M alt m -m alt -m ctrl; sleep 1.5
check "and unmuting takes the mark away again" "$( tree | grep -q '🔕' && echo 0 || echo 1 )"

w -M ctrl -M alt i -m alt -m ctrl; sleep 1.5
check "ctrl-alt-i lists what is pinned here" "$( tree | grep -q 'retry limit' && echo 1 || echo 0 )" "$(tree | grep -oE "label 'Pinned'" | head -1)"
w -k Escape; sleep 0.5

# The composer as the prompt for the commands that need a word. `/topic`
# through the palette prefills it; typing the rest sends it through the same
# handler the action uses.
w "/topic on-call rotation"; sleep 0.3; w -k Return; sleep 2.0
check "/topic sets the topic and the header shows it" "$( tree | grep -q 'on-call rotation' && echo 1 || echo 0 )" "$(tree | grep -oE "label '[0-9]+ members[^']*'" | head -1)"

# A slash command the interface owns rather than the workspace: nothing is
# sent, a chooser opens. Escape first, because a complete command name has
# the completion popup open and the first Return accepts it — which is what
# Return should do, and is why this needs two keystrokes rather than one.
w "/upload"; sleep 0.6; w -k Escape; sleep 0.3; w -k Return; sleep 2.5
# The chooser is `xdg-desktop-portal-gtk`, a different application on the
# a11y bus. Asserting on our own tree could never have seen it.
check "/upload opens the file chooser rather than posting" "$( tree --app xdg-desktop-portal-gtk | grep -q 'file chooser' && echo 1 || echo 0 )" "$(tree --app xdg-desktop-portal-gtk | head -2 | tr '\n' ' ')"
check "and the command itself was not posted" "$( tree | grep -q "list item.*'/upload'" && echo 0 || echo 1 )"
# Closed by the compositor, not by a keystroke. The chooser is modal and
# transient for our window, so while it is open the window takes no input —
# and Escape typed at *our* window closes nothing. One un-closed dialog
# turned this into nine failures further down the file before the cause was
# obvious, which is the argument for closing it explicitly.
hyprctl dispatch 'hl.dsp.window.close({ window = "class:xdg-desktop-portal-gtk" })' >/dev/null 2>&1
sleep 1.2
check "and the chooser closes again" "$( hyprctl clients -j | jq -r '.[].class' | grep -q portal && echo 0 || echo 1 )"

# Search: the query goes in the pane, so the results stay readable next to
# the conversation they came from.
w -M alt -k slash -m alt; sleep 1.0
w "deploy"; sleep 0.4; w -k Return; sleep 2.0
check "alt-/ searches and counts what it found" "$( tree | grep -qE "label '[0-9]+ results? for" && echo 1 || echo 0 )" "$(tree | grep -oE "label '[0-9]+ results?[^']*'" | head -1)"
check "each hit says which conversation it is in" "$( tree | grep -q '#engineering' && echo 1 || echo 0 )"

# A snippet: Slack sends the first lines, the row shows twelve of them and
# puts the rest behind a disclosure rather than fetching the file.
w -k Escape; sleep 0.4
# At the newest end: the demo's own story is the newest thing in the
# conversation, and `--demo-rows` prepends older filler in front of it. A
# virtualised list only puts realised rows in the tree, so asserting from
# the oldest end asserts about the filler.
w -M alt g -m alt; sleep 1.2
# Walking up from the newest, because a virtualised list only puts the rows
# it has realised into the tree, and in a tiled window that is three or four
# of them. The snippet is a few messages back from the end.
snip=0
for _ in 1 2 3 4 5 6 7 8; do
  if tree | grep -q 'retry.toml'; then snip=1; break; fi
  w -M alt k -m alt; sleep 0.5
done
check "a text file is shown inline, not as a paperclip" "$snip" "$(tree | grep -oE "label '📄[^']*'" | head -1)"
check "with its first lines and a count of the rest" "$( tree | grep -q 'more lines' && echo 1 || echo 0 )"

# A custom emoji is a picture in the chip, and the chip is still named — a
# reaction whose only accessible name is its count tells a screen reader
# nothing and cannot be asserted on either.
check "a workspace emoji renders as a picture with a name" "$( tree | grep -q "button ':shipit: " && echo 1 || echo 0 )" "$(tree | grep -oE "button ':[a-z-]+: [0-9]+'" | head -1)"

# File search, by the prefix rather than by a mode: one box, and a toggle you
# cannot see the state of in a screenshot is one people get wrong.
w -M alt -k slash -m alt; sleep 1.0
w "file:retry"; sleep 0.4; w -k Return; sleep 2.0
check "file: searches files and says how many" "$( tree | grep -qE "label '[0-9]+ file" && echo 1 || echo 0 )" "$(tree | grep -oE "label '[0-9]+ file[^']*'" | head -1)"
check "and each hit says how big and where" "$( tree | grep -q 'retry.toml' && echo 1 || echo 0 )"
# The query is recallable: up-arrow in the box brings it back.
w -M ctrl u -m ctrl; sleep 0.3
w -k Up; sleep 0.5
check "up-arrow recalls the last search" "$( tree | grep -q 'file:retry' && echo 1 || echo 0 )" "$(tree | grep -oE "'file:[^']*'" | head -1)"
w -k Escape; sleep 0.4

# A profile, from the message under the cursor.
w -k Escape; sleep 0.4
w -M alt k -m alt; sleep 0.4; w -M alt i -m alt; sleep 1.5
check "alt-i shows who wrote it" "$( tree | grep -qE "label '(time zone|presence)'" && echo 1 || echo 0 )"
w -k Escape; sleep 0.5
check "escape closes the pane" "$( tree | grep -q "label 'presence'" && echo 0 || echo 1 )"

# The control socket, which is how a Hyprland desktop shows an unread count:
# there is no tray, so a waybar module runs a command every few seconds. The
# demo serves on a socket of its own, so this can be tested without the
# demo's counts ever reaching a bar somebody reads as their real one.
u=$("$BIN" --anonymous unread --json 2>/dev/null)
check "unread --json answers from the running client" "$( echo "$u" | grep -q '"connected":true' && echo 1 || echo 0 )" "$u"
check "and its counts are the ones on screen" "$( echo "$u" | grep -qE '"unread":[1-9]' && echo 1 || echo 0 )"
check "status says where you are" "$( "$BIN" --anonymous status 2>/dev/null | grep -q 'in ' && echo 1 || echo 0 )" "$("$BIN" --anonymous status 2>/dev/null)"
# A name nobody has must fail, and fail with a status a script can see.
"$BIN" --anonymous send '#nope' x >/dev/null 2>&1; rc=$?
check "send refuses a conversation that does not exist" "$( [ "$rc" -ne 0 ] && echo 1 || echo 0 )" "exit=$rc"
"$BIN" --anonymous send '#engineering' 'sent from the shell' >/dev/null 2>&1
sleep 2
check "send from the shell reaches the conversation" "$( tree | grep -q 'sent from the shell' && echo 1 || echo 0 )"
# And the real socket is untouched by any of it.
check "the demo never answers on the real socket" "$( "$BIN" unread 2>/dev/null | grep -q 'not running' && echo 1 || echo 0 )" "$("$BIN" unread 2>/dev/null)"

# Sidebar options. `hide_read` must never hide what is on screen — a sidebar
# that drops the row you are reading loses your place while you watch.
before=$(tree | grep -c 'list item')
w -M ctrl p -m ctrl; sleep 0.6
w "hide_read"; sleep 0.5; w -k Return; sleep 1.2
after=$(tree | grep -c 'list item')
check "hide_read hides the conversations with nothing unread" "$( [ "$after" -lt "$before" ] && echo 1 || echo 0 )" "before=$before after=$after"
check "and the open conversation is still there" "$( tree | grep -q 'engineering' && echo 1 || echo 0 )"
w -M ctrl p -m ctrl; sleep 0.6
w "hide_read"; sleep 0.5; w -k Return; sleep 1.2

# M4 through the window. Last in this run on purpose: these change which
# conversation is open, and nothing after them cares.

# Folding from the keyboard. `toggle_section` was in the palette since M2
# and answered "unbound action". #engineering was unstarred above, so it is
# in CHANNELS now; alt-z folds the section the open conversation is in, and
# the same key has to open it again — folding takes the selected row with
# it, and a key that can close a section and not reopen it is a trap.
check "CHANNELS is open to begin with" "$( tree | grep -q '#  general' && echo 1 || echo 0 )"
w -M alt z -m alt; sleep 0.8
check "alt-z folds the section the open conversation is in" "$( tree | grep -q '#  general' && echo 0 || echo 1 )"
w -M alt z -m alt; sleep 0.8
check "and the same key unfolds it" "$( tree | grep -q '#  general' && echo 1 || echo 0 )"

# A draft typed on the phone. The demo's Slack holds one for #general, and
# it has to be in that composer — read through the text interface, which is
# what a screen reader reads too.
w -M ctrl k -m ctrl; sleep 0.4; w "gene"; sleep 0.3; w -k Return; sleep 2.0
check "a draft typed on the phone is in the composer" "$( tree | grep -q 'half a thought from the phone' && echo 1 || echo 0 )" "$(tree | grep -oE "'half a thought[^']*'" | head -1)"
w -M ctrl u -m ctrl; sleep 0.3

# Making, renaming and archiving a channel, all from the composer. The name
# is normalised the way Slack would store it before anything is sent.
w "/create a11y room"; sleep 0.3; w -k Return; sleep 2.0
check "/create makes the channel and opens it" "$( tree | grep -q "label '# a11y-room'" && echo 1 || echo 0 )" "$(tree | grep -oE "label '# [^']*'" | head -2 | tr '\n' ' ')"
w "/rename a11y renamed"; sleep 0.3; w -k Return; sleep 2.0
check "/rename renames it, and the header follows" "$( tree | grep -q "label '# a11y-renamed'" && echo 1 || echo 0 )" "$(tree | grep -oE "label '# [^']*'" | head -2 | tr '\n' ' ')"
# The run of M4's first draft passed the two checks above while renaming the
# demo's #general: the window had not opened the new channel, so /rename
# went to the conversation still on screen. #general has to be where it was.
check "and the conversation it came from is untouched" "$( tree | grep -q '#  general' && echo 1 || echo 0 )" "$(tree | grep -oE "label '#  [^']*'" | tr '\n' ' ')"
# Archiving always asks. A complete command name has the completion popup
# open, so Escape first and then Return, as for /upload.
w "/archive"; sleep 0.6; w -k Escape; sleep 0.3; w -k Return; sleep 1.5
check "/archive asks first, and says for whom" "$( tree | grep -q 'Archive for everybody' && echo 1 || echo 0 )"
w -k Escape; sleep 1.0
check "cancelling it archives nothing" "$( tree | grep -q "label '# a11y-renamed'" && echo 1 || echo 0 )"
w "/archive"; sleep 0.6; w -k Escape; sleep 0.3; w -k Return; sleep 1.5
# Cancel is the default button, so Return alone would cancel: Tab to the
# other one, which is what a person has to do too.
w -k Tab; sleep 0.3; w -k Return; sleep 2.0
check "confirming it takes the channel out of the sidebar" "$( tree | grep -q 'a11y-renamed' && echo 0 || echo 1 )" "$(tree | grep -oE "'[^']*a11y[^']*'" | head -2 | tr '\n' ' ')"
check "and archives nothing else" "$( tree | grep -q '#  general' && echo 1 || echo 0 )"
# A dialog left open takes all input from the window behind it, and every
# check after this one would fail for that reason alone.
if tree | grep -q 'Archive for everybody'; then w -k Escape; sleep 0.8; fi

# A group, by the people in it. Slack names one `mpdm-alice--bob--petr-1`;
# on screen it has to be the people. Escape closes the mention completion
# that `@bob` opened, or Return would take the completion instead.
w "/group @alice @bob"; sleep 0.8; w -k Escape; sleep 0.3; w -k Return; sleep 2.0
check "/group opens a conversation named by its people" "$( tree | grep -E "label '[^']*alice[^']*bob[^']*'" | grep -vq 'mpdm' && echo 1 || echo 0 )" "$(tree | grep -oE "label '[^']*(alice|mpdm)[^']*'" | head -3 | tr '\n' ' ')"
check "and Slack's own name for it is nowhere on screen" "$( tree | grep -q 'mpdm-' && echo 0 || echo 1 )"

# F1 is the shortcuts window, generated from the live keymap: an action with
# no key has to say so rather than be missing.
w -k F1; sleep 0.8
t=$(tree)
check "F1 opens the shortcuts window" "$( echo "$t" | grep -q "Keyboard shortcuts" && echo 1 || echo 0 )"
check "it is generated from the keymap, not written by hand" "$( echo "$t" | grep -q "label '<Control>k'" && echo 1 || echo 0 )"
w -k Escape; sleep 0.4

shot "$OUT/e2e_keys.png" && echo "  shot $OUT/e2e_keys.png"

kill $PID 2>/dev/null; wait $PID 2>/dev/null
sleep 1

# A second run, with more messages than one page holds, for the two things
# that cannot be seen in a short conversation: where it opens, and what
# happens at the top. The mock pages at fifty, the way Slack does.
"$BIN" --anonymous --no-cache --demo-rows 120 --metrics >"$LOG2" 2>&1 &
PID2=$!
sleep 4
focus
check "a long conversation opens on its newest message" "$( tree | grep -q 'Deploy of v2.4 finished' && echo 1 || echo 0 )" "$(tree | grep -oE "'plain sentence number [0-9]+" | head -1)"
check "and only a page of it is loaded" "$( [ "$(grep -c '^rows=50$' "$LOG2")" -ge 1 ] && echo 1 || echo 0 )" "$(grep '^rows=' "$LOG2" | head -1)"

for _ in 1 2 3 4; do w -M alt -k Home -m alt; sleep 1.8; done
check "scrollback walks back to the start of the conversation" "$( tree | grep -q 'the beginning of the conversation' && echo 1 || echo 0 )"
check "and it got there a page at a time" "$( [ "$(grep -c '^scrollback_asked=' "$LOG2")" -ge 3 ] && echo 1 || echo 0 )" "$(grep -c '^scrollback_asked=' "$LOG2") pages"
check "the oldest message is now on screen" "$( tree | grep -q 'plain sentence number 0,' && echo 1 || echo 0 )"

kill $PID2 2>/dev/null; wait $PID2 2>/dev/null
sleep 1

# A third run, with the demo workspace talking, for the one thing that
# needs somebody else to say something: a notification, and only when the
# user is not already looking at it.
"$BIN" --anonymous --no-cache --demo 2 --demo-rows 4 --metrics >"$LOG3" 2>&1 &
PID3=$!
sleep 3
focus
# Looking straight at #engineering while it talks: nothing may be raised.
sleep 7
quiet=$(grep -c '^notified=' "$LOG3" || true)
check "no notification while looking straight at the conversation" "$( [ "$quiet" = 0 ] && echo 1 || echo 0 )" "$quiet raised"
# Now look somewhere else. The demo's third line mentions the signed-in
# user, which is what the engine thinks is worth interrupting for.
w -M ctrl k -m ctrl; sleep 0.5; w "des"; sleep 0.3; w -k Return; sleep 9
check "a mention elsewhere is raised" "$( [ "$(grep -c '^notified=' "$LOG3")" -ge 1 ] && echo 1 || echo 0 )" "$(grep '^notified=' "$LOG3" | head -1)"
check "and it says who and where" "$( grep -q '^notified=.*mentioned you in #' "$LOG3" && echo 1 || echo 0 )"

# The received typing indicator: the demo stream says somebody is typing
# before each line, so going back to #engineering has to show it under the
# composer, and it has to go away on its own.
w -M ctrl k -m ctrl; sleep 0.5; w "engi"; sleep 0.3; w -k Return; sleep 1.0
typed=0
for _ in 1 2 3 4 5 6 7 8; do
  if tree | grep -q 'is typing…'; then typed=1; break; fi
  sleep 0.6
done
check "somebody typing shows under the composer" "$typed" "$(tree | grep -oE "label '[^']*typing[^']*'" | head -1)"
sleep 6
check "and it clears itself, because Slack sends no 'stopped'" "$( tree | grep -q 'is typing…' && echo 0 || echo 1 )"

kill $PID3 2>/dev/null; wait $PID3 2>/dev/null
sleep 1

# NFR-3: ~0 % CPU when nothing changes. What was in the way was GTK's own
# caret, which fades rather than switches and so drives the frame clock at
# the display's full rate. `[ui] reduced_motion` turns it off, and then an
# idle window paints nothing at all — which is assertable, unlike a CPU
# percentage.
CFG=$(mktemp -d)
mkdir -p "$CFG/slack-light"
printf '[ui]\nreduced_motion = true\n' > "$CFG/slack-light/config.toml"
XDG_CONFIG_HOME="$CFG" "$BIN" --anonymous --no-cache --demo-rows 50 --metrics --bench --idle 5 >"$LOG4" 2>&1 &
PID4=$!
sleep 3
focus
wait $PID4 2>/dev/null
frames=$(grep '^idle_frames=' "$LOG4" | cut -d= -f2)
check "an idle window with reduced motion paints nothing" "$( [ "${frames:-1}" = 0 ] && echo 1 || echo 0 )" "${frames:-no reading} frames"
check "and costs no measurable cpu" "$( awk -F= '/^idle_cpu_pct=/ { exit ($2 < 0.2) ? 0 : 1 }' "$LOG4" && echo 1 || echo 0 )" "$(grep '^idle_cpu_pct=' "$LOG4")"
rm -rf "$CFG"
# Shift on a character key is delivered as the plain keyval and matches no
# accelerator (see logic::bindable). A binding like that is dead on arrival,
# so none may be installed — and every action must have a key or say why not.
dead=$(grep '^binding=' "$LOG" | grep -cE '=<[^>]*Shift>[A-Za-z]( |$)' || true)
check "no binding is shift plus a letter" "$( [ "$dead" = 0 ] && echo 1 || echo 0 )" "$dead such bindings"
# Two are GTK's own (pane focus cycling). The rest are keyless on purpose:
# topic, purpose, invite and leave are rare and not undone by pressing the
# same key again, and so are create, rename, archive and starting a group; status, dnd, forward and view-source want a word or a
# target; unread-first and hide-read are settings. All of them are reached
# by name from the palette. Anything *else* without a key is an action
# nobody can run, which is what this counts.
unbound=$(grep '^binding=' "$LOG" | grep 'no free key' \
          | grep -cvE '^binding=(focus_next|focus_prev|set_topic|set_purpose|invite|leave_channel|create_channel|rename_channel|archive_channel|create_group|status|dnd|unread_first|hide_read|forward_message|view_source)=' || true)
check "every action without a key is one that was meant to have none" "$( [ "$unbound" = 0 ] && echo 1 || echo 0 )" "$unbound unexpected: $(grep '^binding=' "$LOG" | grep 'no free key' | grep -vE '^binding=(focus_next|focus_prev|set_topic|set_purpose|invite|leave_channel|create_channel|rename_channel|archive_channel|create_group|status|dnd|unread_first|hide_read|forward_message|view_source)=' | tr '\n' ' ')"

echo "--- bindings installed ---"; grep '^binding=' "$LOG" | sed 's/^binding=/  /'
echo; echo "$pass passed, $fail failed"
rm -f "$LOG" "$LOG2" "$LOG3" "$LOG4"
[ "$fail" = 0 ]

#!/usr/bin/env bash
# Drive the client from the keyboard, assert through the accessibility
# tree. Every run is against `--anonymous`: the mock backend, no network. Runs on the developer's Hyprland; CI needs a headless
# Wayland compositor (see the findings, D1).
#
#   tests/a11y/e2e.sh [path-to-slack-light] [screenshot-dir]
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
focus() { hyprctl dispatch "hl.dsp.focus({ window = \"class:$CLASS\" })" >/dev/null 2>&1; sleep 0.3; }
shot() { local g; g=$(hyprctl clients -j | jq -r ".[] | select(.class==\"$CLASS\") | \"\(.at[0]),\(.at[1]) \(.size[0])x\(.size[1])\"" | head -1); [ -n "$g" ] && grim -g "$g" "$1"; }

# A stray window from an earlier run confuses tree.py, which reads the first
# slack-light on the a11y bus. Start clean.
pkill -x slack-light 2>/dev/null && sleep 1
LOG=$(mktemp); LOG2=$(mktemp)
"$BIN" --anonymous --demo-rows 30 --metrics >"$LOG" 2>&1 &
PID=$!
sleep 3
focus

t=$(tree)
check "the window is on the a11y bus with its rows" "$( [ "$(echo "$t" | grep -c 'list item')" -gt 5 ] && echo 1 || echo 0 )"
check "the composer is a text field in the tree" "$( echo "$t" | grep -qE '^ *(text|entry)' && echo 1 || echo 0 )"

# E1: ctrl-k, type, Enter → the conversation opens.
wtype -M ctrl k -m ctrl; sleep 0.4
wtype "des"; sleep 0.3
wtype -k Return; sleep 1.2
check "ctrl-k, 'des', Enter opens #design" "$( tree | grep -q "label '# design'" && echo 1 || echo 0 )" "$(tree | grep "label '#" | head -2 | tr '\n' ' ')"

# E2: after the jump, focus is back on the composer: typing lands there.
wtype "keyboard test"; sleep 0.2; wtype -k Return; sleep 2.5
check "typing after the jump goes to the composer and sends" "$( tree | grep -q "keyboard test" && echo 1 || echo 0 )"
check "the send was confirmed by the engine" "$( grep -q send_confirmed_ms "$LOG" && echo 1 || echo 0 )"

# Completion. The popup offers, Enter takes, and the rest is unit-tested:
# a resolved mention renders as the name it started as, so the tree cannot
# tell `<@U0ALICE>` from `@alice` — slk-core's tests do that.
wtype "hi @al"; sleep 1.0
check "typing @ offers the people" "$( tree | grep -q "label '@alice'" && echo 1 || echo 0 )"
wtype -k Return; sleep 0.4
wtype ":roc"; sleep 1.0
check "typing a shortcode offers emoji, best first" "$( tree | grep -A1 "list item" | grep -q ':rocket:' && echo 1 || echo 0 )" "$(tree | grep -oE "':[a-z]+:'" | head -2 | tr '\n' ' ')"
wtype -k Escape; sleep 0.3
check "escape closes the completions and leaves the draft" "$( tree | grep -q ':rocket:' && echo 0 || echo 1 )"

# A draft survives a look at another conversation. Losing one is the thing
# people never forgive a chat client for.
wtype -M alt -k Up -m alt; sleep 1.2
wtype -M alt -k Down -m alt; sleep 1.2
wtype -k Return; sleep 2.0
check "a draft survives leaving the conversation" "$( tree | grep -q 'hi @alice :roc' && echo 1 || echo 0 )" "$(tree | grep -oE "'hi @alice[^']*'" | head -1)"

# E1: alt-Up moves to the previous conversation without the mouse.
wtype -M alt -k Up -m alt; sleep 1.2
check "alt-Up moves to the previous conversation" "$( tree | grep -q "label '🔒 leads'" && echo 1 || echo 0 )" "$(tree | grep -E "label '(#|🔒)" | head -2 | tr '\n' ' ')"

# Escape from anywhere returns to the composer; ctrl-u clears it.
wtype "abc"; sleep 0.2; wtype -M ctrl u -m ctrl; sleep 0.3; wtype -k Escape; sleep 0.3
wtype "after escape"; sleep 0.2; wtype -k Return; sleep 1.2
check "escape and ctrl-u leave a usable, empty composer" "$( tree | grep -q "after escape" && echo 1 || echo 0 )"

# The message cursor, and a reaction on the message it lands on. alt-k
# selects; alt-1 is the first quick reaction, which is :+1:.
wtype -M alt k -m alt; sleep 0.4
check "alt-k puts a cursor on a message" "$( tree --states | grep -q 'list item .*\[selected\]' && echo 1 || echo 0 )" "$(tree --states | grep -c selected) selected nodes"
before=$(tree | grep -c '👍' || true)
wtype -M alt 1 -m alt; sleep 1.0
after=$(tree | grep -c '👍' || true)
check "alt-1 adds a reaction chip to it" "$( [ "$after" -gt "$before" ] && echo 1 || echo 0 )" "before=$before after=$after"

# The thread pane: alt-t opens it on the message under the cursor, alt-w
# closes it. Both go through the same code the row's ↳ link does.
wtype -M alt t -m alt; sleep 1.5
check "alt-t opens the thread pane" "$( tree | grep -q "label 'Thread" && echo 1 || echo 0 )"
wtype -M alt w -m alt; sleep 0.6
check "alt-w closes it again" "$( tree | grep -q "label 'Thread" && echo 0 || echo 1 )"

# Save then pin, back to back, on a message of our own. Two defects in one
# check: the pin mark only renders if the echoed row keeps its header (a
# replacement used to be grouped against the end of the list, which for the
# newest message is itself), and the second action only finds a message if
# the cursor survived the first one's echo.
wtype "mine to keep"; sleep 0.3; wtype -k Return; sleep 2.0
wtype -M alt g -m alt; sleep 0.4
wtype -M alt s -m alt; sleep 1.2
saved=$( tree | grep -c 'saved for later' )
wtype -M alt p -m alt; sleep 1.2
marks=$(tree | grep -oE "'📌[^']*'" | tr '\n' ' ')
check "save reports back" "$( [ "$saved" -gt 0 ] && echo 1 || echo 0 )"
check "pin finds the same message, and the row shows both marks" "$( echo "$marks" | grep -q 'pinned · 🔖 saved' && echo 1 || echo 0 )" "marks=[$marks]"

# Editing one's own message, and the refusal on somebody else's.
wtype -M alt e -m alt; sleep 0.5
check "alt-e says what enter will do now" "$( tree | grep -q 'editing — enter saves' && echo 1 || echo 0 )"
wtype " (fixed)"; sleep 0.2; wtype -k Return; sleep 2.0
check "the edit lands, marked as one" "$( tree | grep -q 'mine to keep (fixed)' && echo 1 || echo 0 )"
# #engineering is the one conversation the mock seeds with other people's
# messages and this run never types into, so its oldest message is somebody
# else's for certain — #general and #design start empty.
wtype -M ctrl k -m ctrl; sleep 0.4; wtype "engi"; sleep 0.3; wtype -k Return; sleep 2.5
wtype -M alt -k Home -m alt; sleep 1.0; wtype -M alt e -m alt; sleep 0.6
check "editing somebody else's message is refused in words" "$( tree | grep -q 'only edit your own' && echo 1 || echo 0 )" "status=$(tree | grep -oE "label '[^']*'" | tail -2 | head -1)"

# The side pane's lists. One pane shows a thread, a search, a member list
# or a profile, and each answers with the same five row shapes.
wtype -M ctrl t -m ctrl; sleep 1.5
check "ctrl-t lists the threads with replies" "$( tree | grep -q '2 replies' && echo 1 || echo 0 )"
wtype -M alt m -m alt; sleep 1.5
check "alt-m lists who is in the conversation" "$( tree | grep -q "label 'alice'" && echo 1 || echo 0 )"
wtype -M alt c -m alt; sleep 1.5
check "alt-c offers the channels one could join" "$( tree | grep -q '#random' && echo 1 || echo 0 )"

# Search: the query goes in the pane, so the results stay readable next to
# the conversation they came from.
wtype -M alt -k slash -m alt; sleep 1.0
wtype "deploy"; sleep 0.4; wtype -k Return; sleep 2.0
check "alt-/ searches and counts what it found" "$( tree | grep -qE "label '[0-9]+ results? for" && echo 1 || echo 0 )" "$(tree | grep -oE "label '[0-9]+ results?[^']*'" | head -1)"
check "each hit says which conversation it is in" "$( tree | grep -q '#engineering' && echo 1 || echo 0 )"

# A profile, from the message under the cursor.
wtype -k Escape; sleep 0.4
wtype -M alt k -m alt; sleep 0.4; wtype -M alt i -m alt; sleep 1.5
check "alt-i shows who wrote it" "$( tree | grep -qE "label '(time zone|presence)'" && echo 1 || echo 0 )"
wtype -k Escape; sleep 0.5
check "escape closes the pane" "$( tree | grep -q "label 'presence'" && echo 0 || echo 1 )"

# F1 is the shortcuts window, generated from the live keymap: an action with
# no key has to say so rather than be missing.
wtype -k F1; sleep 0.8
t=$(tree)
check "F1 opens the shortcuts window" "$( echo "$t" | grep -q "Keyboard shortcuts" && echo 1 || echo 0 )"
check "it is generated from the keymap, not written by hand" "$( echo "$t" | grep -q "label '<Control>k'" && echo 1 || echo 0 )"
wtype -k Escape; sleep 0.4

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

for _ in 1 2 3 4; do wtype -M alt -k Home -m alt; sleep 1.8; done
check "scrollback walks back to the start of the conversation" "$( tree | grep -q 'the beginning of the conversation' && echo 1 || echo 0 )"
check "and it got there a page at a time" "$( [ "$(grep -c '^scrollback_asked=' "$LOG2")" -ge 3 ] && echo 1 || echo 0 )" "$(grep -c '^scrollback_asked=' "$LOG2") pages"
check "the oldest message is now on screen" "$( tree | grep -q 'plain sentence number 0,' && echo 1 || echo 0 )"

kill $PID2 2>/dev/null; wait $PID2 2>/dev/null
# Shift on a character key is delivered as the plain keyval and matches no
# accelerator (see logic::bindable). A binding like that is dead on arrival,
# so none may be installed — and every action must have a key or say why not.
dead=$(grep '^binding=' "$LOG" | grep -cE '=<[^>]*Shift>[A-Za-z]( |$)' || true)
check "no binding is shift plus a letter" "$( [ "$dead" = 0 ] && echo 1 || echo 0 )" "$dead such bindings"
unbound=$(grep '^binding=' "$LOG" | grep -c 'no free key' || true)
check "every action but the two GTK owns has a key" "$( [ "$unbound" -le 2 ] && echo 1 || echo 0 )" "$unbound unbound"

echo "--- bindings installed ---"; grep '^binding=' "$LOG" | sed 's/^binding=/  /'
echo; echo "$pass passed, $fail failed"
rm -f "$LOG" "$LOG2"
[ "$fail" = 0 ]

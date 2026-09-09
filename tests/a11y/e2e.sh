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
tree() { python3 "$HERE/tree.py" 2>/dev/null; }
focus() { hyprctl dispatch "hl.dsp.focus({ window = \"class:$CLASS\" })" >/dev/null 2>&1; sleep 0.3; }
shot() { local g; g=$(hyprctl clients -j | jq -r ".[] | select(.class==\"$CLASS\") | \"\(.at[0]),\(.at[1]) \(.size[0])x\(.size[1])\"" | head -1); [ -n "$g" ] && grim -g "$g" "$1"; }

# A stray window from an earlier run confuses tree.py, which reads the first
# slack-light on the a11y bus. Start clean.
pkill -x slack-light 2>/dev/null && sleep 1
LOG=$(mktemp)
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

# E1: alt-Up moves to the previous conversation without the mouse.
wtype -M alt -k Up -m alt; sleep 1.2
check "alt-Up moves to the previous conversation" "$( tree | grep -q "label '🔒 leads'" && echo 1 || echo 0 )" "$(tree | grep -E "label '(#|🔒)" | head -2 | tr '\n' ' ')"

# Escape from anywhere returns to the composer; ctrl-u clears it.
wtype "abc"; sleep 0.2; wtype -M ctrl u -m ctrl; sleep 0.3; wtype -k Escape; sleep 0.3
wtype "after escape"; sleep 0.2; wtype -k Return; sleep 1.2
check "escape and ctrl-u leave a usable, empty composer" "$( tree | grep -q "after escape" && echo 1 || echo 0 )"

# F1 lists the bindings, which is the shortcuts window's job later.
wtype -k F1; sleep 0.5
check "F1 shows the bindings" "$( tree | grep -q '<Control>k jump_to' && echo 1 || echo 0 )"

shot "$OUT/e2e_keys.png" && echo "  shot $OUT/e2e_keys.png"

kill $PID 2>/dev/null; wait $PID 2>/dev/null
echo "--- bindings installed ---"; grep '^binding=' "$LOG" | sed 's/^binding=/  /'
echo; echo "$pass passed, $fail failed"
rm -f "$LOG"
[ "$fail" = 0 ]

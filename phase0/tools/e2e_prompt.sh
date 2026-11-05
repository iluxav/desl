#!/bin/bash
D=${DESL_DEMO:-/tmp/desl-demo-api}
BIN=${DESL_BIN:-$(dirname "$0")/../target/debug/proto-a}
T="tmux -L desl-proto"; SES=pr
cap() { $T capture-pane -t $SES -p; }
wait_for() { local secs=$1; shift; for i in $(seq 1 "$secs"); do sleep 1; local P; P=$(cap); local ok=1; for pat in "$@"; do echo "$P" | grep -q -- "$pat" || ok=0; done; [ $ok = 1 ] && { echo "(matched after ${i}s)"; return 0; }; done; echo "(TIMEOUT waiting for: $*)"; cap; return 1; }
say() { $T send-keys -t $SES -l "$1"; $T send-keys -t $SES Enter; }
cd $D; $BIN stop >/dev/null 2>&1; sleep 0.4; rm -f ~/.local/state/desl-proto-a/*.jsonl
$T kill-session -t $SES 2>/dev/null; $T new-session -d -s $SES -x 90 -y 28 -c "$D" "$BIN; sleep 60"; sleep 1.2
echo "##### 1. '/' opens the palette; typing filters"
$T send-keys -t $SES '/'; sleep 0.3; cap | sed -n '7,18p'; $T send-keys -t $SES -l 'sw'; sleep 0.3; echo "--- after 'sw' ---"; cap | sed -n '7,12p'
echo "##### 2. enter on /switch leaves '/switch ' in the prompt"; $T send-keys -t $SES Enter; sleep 0.3; cap | sed -n '24,26p'; $T send-keys -t $SES C-u
echo "##### 3. '@' file picker, filter 'mid', enter inserts the path"
$T send-keys -t $SES -l 'Explain '; $T send-keys -t $SES '@'; sleep 0.4; cap | sed -n '8,16p'; $T send-keys -t $SES -l 'mid'; sleep 0.3; $T send-keys -t $SES Enter; sleep 0.3; cap | sed -n '24,26p'; $T send-keys -t $SES C-u
echo "##### 4. markdown reply"
say 'Reply with a markdown sample only: an H2 heading, two bullets (one with **bold**, one with `inline code`), a numbered list of two items, and a 3-line rust code block fenced with the language. No tools.'
wait_for 60 '○ idle' '┄┄' >/dev/null && cap | sed -n '2,24p'
echo "##### 5. ctrl+r history picker"
$T send-keys -t $SES C-r; sleep 0.4; cap | sed -n '8,13p'; $T send-keys -t $SES Enter; sleep 0.3; echo "--- prompt now holds the old prompt ---"; cap | sed -n '21,27p' | grep -E '›|markdown' | head -2
say '/quit'; sleep 0.6; $BIN stop >/dev/null; $T kill-session -t $SES 2>/dev/null

#!/bin/bash
D=${DESL_DEMO:-/tmp/desl-demo-api}
BIN=${DESL_BIN_B:-$(dirname "$0")/../target/debug/proto-b}
T="tmux -L desl-proto"; SES=pb
cap() { $T capture-pane -t $SES -p; }
wait_for() { local secs=$1; shift; for i in $(seq 1 "$secs"); do sleep 1; local P; P=$(cap); local ok=1; for pat in "$@"; do echo "$P" | grep -q -- "$pat" || ok=0; done; [ $ok = 1 ] && { echo "(matched after ${i}s)"; return 0; }; done; echo "(TIMEOUT waiting for: $*)"; return 1; }
$T kill-session -t $SES 2>/dev/null
$T new-session -d -s $SES -x 100 -y 30 -c "$D" "$BIN; echo B-EXITED; sleep 60"
echo "##### 1. claude TUI boots inside the pane"
wait_for 30 '│ ›\|❯\|>' >/dev/null; sleep 4; cap
echo "##### 2. type a prompt into claude's own input, watch the sidebar glyph"
$T send-keys -t $SES -l 'Reply with one short sentence: what directory are you in? No tools.'; sleep 0.5; $T send-keys -t $SES Enter
sleep 2.5; echo "--- status while working ---"; cap | sed -n '4p;29p'
wait_for 60 '○ idle' >/dev/null; sleep 1; echo "--- after ---"; cap | sed -n '2,6p;14,29p'
echo "##### 3. ctrl+b n -> agent picker -> codex"
$T send-keys -t $SES C-b; sleep 0.2; $T send-keys -t $SES 'n'; sleep 0.4; cap | sed -n '9,15p'; $T send-keys -t $SES -l 'cod'; sleep 0.2; $T send-keys -t $SES Enter; sleep 6; echo "--- codex pane ---"; cap | sed -n '2,8p;20,29p'
echo "##### 4. ctrl+b t thread picker"; $T send-keys -t $SES C-b; sleep 0.2; $T send-keys -t $SES 't'; sleep 0.4; cap | sed -n '9,15p'; $T send-keys -t $SES Escape; sleep 0.2
echo "##### 5. ctrl+b 1 back to claude, ctrl+b [ scroll mode, k k"; $T send-keys -t $SES C-b; sleep 0.2; $T send-keys -t $SES '1'; sleep 0.4; $T send-keys -t $SES C-b; sleep 0.2; $T send-keys -t $SES '['; sleep 0.2; $T send-keys -t $SES 'k'; $T send-keys -t $SES 'k'; sleep 0.4; cap | sed -n '2p;27,29p'; $T send-keys -t $SES Escape; sleep 0.2
echo "##### 6. ctrl+b ? help"; $T send-keys -t $SES C-b; sleep 0.2; $T send-keys -t $SES '?'; sleep 0.4; cap | sed -n '4,12p'; $T send-keys -t $SES Escape
echo "##### 7. ctrl+b d exits"; $T send-keys -t $SES C-b; sleep 0.2; $T send-keys -t $SES 'd'; sleep 1.5; cap | grep -v '^$' | tail -3
$T kill-session -t $SES 2>/dev/null
pgrep -af 'claude$|codex$' | grep -v pgrep | head -3 | sed 's/^/leftover: /'

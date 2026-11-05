#!/bin/bash
D=${DESL_DEMO:-/tmp/desl-demo-api}
BIN=${DESL_BIN_B:-$(dirname "$0")/../target/debug/proto-b}
T="tmux -L desl-proto"; SES=pb2
cap() { $T capture-pane -t $SES -p; }
wait_for() { local secs=$1; shift; for i in $(seq 1 "$secs"); do sleep 1; local P; P=$(cap); local ok=1; for pat in "$@"; do echo "$P" | grep -q -- "$pat" || ok=0; done; [ $ok = 1 ] && { echo "(matched after ${i}s)"; return 0; }; done; echo "(TIMEOUT waiting for: $*)"; return 1; }
$T kill-session -t $SES 2>/dev/null
$T new-session -d -s $SES -x 100 -y 30 -c "$D" "$BIN; echo B-EXITED; sleep 60"
echo "##### 1. trust dialog is detected as waiting"
wait_for 30 'Yes, I trust' >/dev/null; sleep 1.5; cap | sed -n '4p;27,29p'
echo "##### 2. answer it: Down, Enter -> claude's prompt appears; status back to idle"
$T send-keys -t $SES Down; sleep 0.3; $T send-keys -t $SES Enter; sleep 5; cap | sed -n '4p;18,29p'
echo "##### 3. prompt; status while working; after"
$T send-keys -t $SES -l 'Reply with one short sentence: what directory are you in? No tools.'; sleep 0.5; $T send-keys -t $SES Enter
for i in 1 2 3 4 5 6; do sleep 1; cap | sed -n '29p' | grep -q 'working' && { echo "working seen after ${i}s"; cap | sed -n '4p;29p'; break; }; done
wait_for 60 '○ idle' >/dev/null; sleep 1; echo "--- after ---"; cap | sed -n '2,5p;12,29p'
echo "##### 4. ctrl+b ? help lists prefixed keys only"
$T send-keys -t $SES C-b; sleep 0.2; $T send-keys -t $SES '?'; sleep 0.4; cap | sed -n '5,22p' | cut -c12-90; $T send-keys -t $SES Escape
echo "##### 5. ctrl+b d exits"; $T send-keys -t $SES C-b; sleep 0.2; $T send-keys -t $SES 'd'; sleep 1.5
$T kill-session -t $SES 2>/dev/null

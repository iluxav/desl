#!/bin/bash
# Prototype B, first panes: Claude Code's own TUI under the desl chrome, a Codex pane beside it.
set -u
D=${DESL_DEMO:-/tmp/desl-demo-api}
BIN=${DESL_BIN_B:-$(dirname "$0")/../target/debug/proto-b}
OUT=$(dirname "$0")/../recordings; mkdir -p "$OUT"; REC=$OUT/week1-proto-b-panes
T="tmux -L desl-proto"; SES=recb
type_slow() { local s="$1"; local i; for ((i=0; i<${#s}; i++)); do $T send-keys -t $SES -l "${s:$i:1}"; sleep 0.04; done; }
cap() { $T capture-pane -t $SES -p; }
wait_for() { local secs=$1; shift; for i in $(seq 1 "$secs"); do sleep 1; local P; P=$(cap); local ok=1; for pat in "$@"; do echo "$P" | grep -q -- "$pat" || ok=0; done; [ $ok = 1 ] && return 0; done; return 1; }
$T kill-session -t $SES 2>/dev/null; rm -f "$REC.out" "$REC.timing" "$REC.cast"
$T new-session -d -s $SES -x 100 -y 28 -c "$D" "script -q -T '$REC.timing' -O '$REC.out' -c '$BIN'; sleep 4"
wait_for 25 "Yes, I trust\|Try \"write" || true; sleep 2
if cap | grep -q "Yes, I trust"; then $T send-keys -t $SES Down; sleep 0.6; $T send-keys -t $SES Enter; sleep 3; fi
type_slow 'Reply with one short sentence: which file defines requireAuth? No tools.'; sleep 0.4; $T send-keys -t $SES Enter
wait_for 60 '○ idle'; sleep 3
$T send-keys -t $SES C-b; sleep 0.4; $T send-keys -t $SES 'n'; sleep 1.5; type_slow 'codex'; sleep 0.6; $T send-keys -t $SES Enter
sleep 7
$T send-keys -t $SES C-b; sleep 0.4; $T send-keys -t $SES 't'; sleep 1.8; $T send-keys -t $SES Escape; sleep 0.8
$T send-keys -t $SES C-b; sleep 0.4; $T send-keys -t $SES '1'; sleep 2
$T send-keys -t $SES C-b; sleep 0.4; $T send-keys -t $SES '?'; sleep 3; $T send-keys -t $SES Escape; sleep 1
$T send-keys -t $SES C-b; sleep 0.4; $T send-keys -t $SES 'd'; sleep 2.5
echo "--- final pane ---"; cap | grep -v '^$' | tail -3
python3 -I $(dirname "$0")/script2cast.py "$REC.out" "$REC.timing" 100 28 > "$REC.cast"
$T kill-session -t $SES 2>/dev/null
ls -la "$OUT" | grep proto-b; echo "--- duration ---"; tail -1 "$REC.cast" | cut -c1-30

#!/bin/bash
# Week 2 demo of Prototype B: a Claude turn in its own TUI, /switch codex typed into Claude's input,
# the brief pasted into Codex, detach, reattach.
set -u
D=${DESL_DEMO:-/tmp/desl-demo-api}
BIN=${DESL_BIN_B:-$(dirname "$0")/../target/debug/proto-b}
OUT=$(dirname "$0")/../recordings; mkdir -p "$OUT"; REC=$OUT/week2-proto-b-switch-detach
T="tmux -L desl-proto"; SES=recb2
type_slow() { local s="$1"; local i; for ((i=0; i<${#s}; i++)); do $T send-keys -t $SES -l "${s:$i:1}"; sleep 0.04; done; }
cap() { $T capture-pane -t $SES -p; }
wait_for() { local secs=$1; shift; for i in $(seq 1 "$secs"); do sleep 1; local P; P=$(cap); local ok=1; for pat in "$@"; do echo "$P" | grep -q -- "$pat" || ok=0; done; [ $ok = 1 ] && return 0; done; return 1; }
cd $D; git checkout -q -- .; $BIN stop >/dev/null 2>&1; sleep 0.5
$T kill-session -t $SES 2>/dev/null; rm -f "$REC.out" "$REC.timing" "$REC.cast"
$T new-session -d -s $SES -x 110 -y 30 -c "$D" "script -q -T '$REC.timing' -O '$REC.out' -c 'bash -c \"$BIN; sleep 2.5; $BIN\"'; sleep 5"
wait_for 25 'auto mode\|Yes, I trust' || true; sleep 1.5
if cap | grep -q 'Yes, I trust'; then $T send-keys -t $SES Down; sleep 0.6; $T send-keys -t $SES Enter; sleep 3; fi
type_slow 'Read src/auth/tokens.ts and tell me in one sentence what ACCESS_TTL is set to.'; sleep 0.4; $T send-keys -t $SES Enter
wait_for 20 '● working'; wait_for 90 '○ idle'; sleep 2.5
type_slow '/switch codex'; sleep 0.5; $T send-keys -t $SES Enter; sleep 2.5; $T send-keys -t $SES 'y'
wait_for 40 'brief is in codex'; sleep 2.5
type_slow 'Without reading files: what is ACCESS_TTL set to? One short sentence.'; sleep 0.4; $T send-keys -t $SES Enter
sleep 2
$T send-keys -t $SES C-b; sleep 0.3; $T send-keys -t $SES 'd'
wait_for 20 'PROJECTS'; wait_for 120 '○ idle'; sleep 3.5
type_slow '/quit'; sleep 0.4; $T send-keys -t $SES Enter; sleep 2.5
echo "--- final pane ---"; cap | grep -v '^$' | tail -3
python3 -I $(dirname "$0")/script2cast.py "$REC.out" "$REC.timing" 110 30 > "$REC.cast"
$T kill-session -t $SES 2>/dev/null; $BIN stop >/dev/null 2>&1
ls -la "$OUT" | grep week2; echo "--- duration ---"; tail -1 "$REC.cast" | cut -c1-30

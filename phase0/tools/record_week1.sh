#!/bin/bash
# Week 1 demo of Prototype A: a Claude turn, /switch codex with the brief, detach mid-way, reattach.
set -u
D=${DESL_DEMO:-/tmp/desl-demo-api}
BIN=${DESL_BIN:-$(dirname "$0")/../target/debug/proto-a}
OUT=$(dirname "$0")/../recordings; mkdir -p "$OUT"; REC=$OUT/week1-proto-a-switch-detach
T="tmux -L desl-proto"; SES=rec1
type_slow() { local s="$1"; local i; for ((i=0; i<${#s}; i++)); do $T send-keys -t $SES -l "${s:$i:1}"; sleep 0.04; done; }
cap() { $T capture-pane -t $SES -p; }
wait_for() { local secs=$1; shift; for i in $(seq 1 "$secs"); do sleep 1; local P; P=$(cap); local ok=1; for pat in "$@"; do echo "$P" | grep -q -- "$pat" || ok=0; done; [ $ok = 1 ] && return 0; done; return 1; }
cd $D; git checkout -q -- .; $BIN stop >/dev/null 2>&1; sleep 0.5; rm -f ~/.local/state/desl-proto-a/*.jsonl
$T kill-session -t $SES 2>/dev/null; rm -f "$REC.out" "$REC.timing" "$REC.cast"
# One recorded shell session: attach, detach, attach again.
$T new-session -d -s $SES -x 100 -y 28 -c "$D" "script -q -T '$REC.timing' -O '$REC.out' -c 'bash -c \"$BIN; sleep 2.5; $BIN\"'; sleep 5"
sleep 1.5
type_slow 'Read src/auth/tokens.ts and tell me in one sentence what ACCESS_TTL is set to.'; sleep 0.4; $T send-keys -t $SES Enter
wait_for 60 '○ idle' '▸ read'; sleep 2
type_slow '/switch codex'; sleep 0.4; $T send-keys -t $SES Enter; sleep 2.2; $T send-keys -t $SES 'y'; sleep 1.5
type_slow 'Without reading any files: what value did we find for ACCESS_TTL? One short sentence.'; sleep 0.4; $T send-keys -t $SES Enter
sleep 2.5
# Detach while Codex is still working; the shell runs the client again after 2.5 s.
$T send-keys -t $SES C-b; sleep 0.3; $T send-keys -t $SES 'd'
wait_for 20 'PROJECTS'; wait_for 90 '○ idle' 'brief sent to codex'; sleep 3
type_slow '/switch claude'; sleep 0.4; $T send-keys -t $SES Enter; sleep 2.2; $T send-keys -t $SES 'y'; sleep 1.2
type_slow 'In one sentence: what did the other assistant answer?'; sleep 0.4; $T send-keys -t $SES Enter
wait_for 90 '○ idle' 'delta brief sent to claude'; sleep 3
type_slow '/quit'; sleep 0.4; $T send-keys -t $SES Enter; sleep 2.5
echo "--- final pane ---"; cap | grep -v '^$' | tail -4
python3 -I $(dirname "$0")/script2cast.py "$REC.out" "$REC.timing" 100 28 > "$REC.cast"
$T kill-session -t $SES 2>/dev/null; $BIN stop >/dev/null 2>&1
ls -la "$OUT" | grep week1; echo "--- duration ---"; tail -1 "$REC.cast" | cut -c1-30

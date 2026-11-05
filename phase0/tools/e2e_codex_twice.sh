#!/bin/bash
# Prototype A: a thread stays on Codex for several turns. Every turn after the first is
# `codex exec ... resume <id>`, which once failed with "unexpected argument '-s'" (exit 2).
D=${DESL_DEMO:-/tmp/desl-demo-api}
BIN=${DESL_BIN:-$(dirname "$0")/../target/release/proto-a}
BIN=$(readlink -f "$BIN")
T="tmux -L desl-proto"; SES=cx2
cap() { $T capture-pane -t $SES -p; }
st() { (cd "$D" && "$BIN" status 2>&1 | grep '▸\|^' | tail -1); }
wait_idle() { local secs=$1; sleep 2; for i in $(seq 1 "$secs"); do sleep 1; cap | tail -3 | grep -q '○ idle\|✕ failed' && { echo "($(cap | tail -3 | grep -o '○ idle\|✕ failed') after ${i}s)"; return 0; }; done; echo "(TIMEOUT)"; return 1; }
say() { $T send-keys -t $SES -l "$1"; sleep 0.3; $T send-keys -t $SES Enter; }
rm -f "$D/probe-a.txt"
$T kill-session -t $SES 2>/dev/null
$T new-session -d -s $SES -x 100 -y 30 -c "$D" "$BIN; echo EXITED; sleep 60"; sleep 1.5
say '/new'; sleep 0.8
echo "##### 1. claude turn"; say 'Reply with the single word: one'; wait_idle 60
echo "##### 2. /switch codex"; say '/switch codex'; sleep 0.8; $T send-keys -t $SES 'y'; sleep 0.8
echo "##### 3. first codex turn (codex exec)"; say 'Reply with the single word: two'; wait_idle 120
echo "##### 4. second codex turn (codex exec resume)"; say 'Reply with the single word: three'; wait_idle 120
echo "##### 5. third codex turn writes a file (the sandbox still allows the workspace)"
say 'Run the shell command: touch probe-a.txt   Then reply with the single word: done'; wait_idle 150
[ -e "$D/probe-a.txt" ] && echo "probe-a.txt written" || echo "probe-a.txt NOT written"; rm -f "$D/probe-a.txt"
cap | sed -n '2,27p' | grep -v '^│ *│ *│$' | cut -c22-100 | grep -v '^ *│\? *$'
say '/quit'; sleep 1.2; (cd "$D" && "$BIN" stop 2>&1 | head -1 | cut -c1-60); $T kill-session -t $SES 2>/dev/null

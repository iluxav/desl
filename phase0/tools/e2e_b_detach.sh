#!/bin/bash
D=${DESL_DEMO:-/tmp/desl-demo-api}
BIN=${DESL_BIN_B:-$(dirname "$0")/../target/debug/proto-b}
T="tmux -L desl-proto"; SES=pbd
cap() { $T capture-pane -t $SES -p; }
wait_for() { local secs=$1; shift; for i in $(seq 1 "$secs"); do sleep 1; local P; P=$(cap); local ok=1; for pat in "$@"; do echo "$P" | grep -q -- "$pat" || ok=0; done; [ $ok = 1 ] && { echo "(matched after ${i}s)"; return 0; }; done; echo "(TIMEOUT waiting for: $*)"; return 1; }
say() { $T send-keys -t $SES -l "$1"; sleep 0.3; $T send-keys -t $SES Enter; }
start() { $T kill-session -t $SES 2>/dev/null; $T new-session -d -s $SES -x 110 -y 32 -c "$D" "$BIN; echo CLIENT-EXITED; sleep 90"; }
cd $D; git checkout -q -- .; $BIN stop >/dev/null 2>&1; sleep 0.5
start
wait_for 25 'Try "write\|Yes, I trust' >/dev/null; if cap | grep -q 'Yes, I trust'; then $T send-keys -t $SES Down; sleep 0.3; $T send-keys -t $SES Enter; sleep 3; fi
echo "##### 1. attached through the host"; cap | sed -n '2,5p;31p'
echo "##### 2. prompt, detach mid-turn"
say 'List 12 short facts about bicycles, one per line, numbered. No tools.'
wait_for 15 '● working' >/dev/null && echo "working seen"; sleep 1; $T send-keys -t $SES C-b; sleep 0.2; $T send-keys -t $SES 'd'; sleep 1; cap | grep -v '^$' | tail -3
echo "##### 3. status from a plain shell, while detached"; (cd $D && $BIN status)
for i in $(seq 1 60); do sleep 1; (cd $D && $BIN status) | grep -q '○' && break; done; (cd $D && $BIN status)
echo "##### 4. reattach: the replay shows the finished answer"
start; sleep 2.5; cap | sed -n '2,5p;14,31p' | grep -v '^│ *│ *│$' | head -24
echo "##### 5. /switch codex through the host"
say '/switch codex'; sleep 1.2; cap | sed -n '29,31p'; $T send-keys -t $SES 'y'; wait_for 40 'Ask Codex\|## Next step' >/dev/null; sleep 3; cap | sed -n '2,3p;31p'
say 'Without reading files: how many facts did the other assistant list? One short sentence.'
wait_for 15 '● working' >/dev/null && echo "codex working seen"; wait_for 120 '○ idle' >/dev/null; sleep 2; cap | sed -n '18,30p' | grep -v '^│ *│ *│$' | tail -8
echo "##### 6. detach, status shows both; stop ends the agents"
$T send-keys -t $SES C-b; sleep 0.2; $T send-keys -t $SES 'd'; sleep 1; (cd $D && $BIN status); (cd $D && $BIN stop); sleep 0.5; (cd $D && $BIN status)
$T kill-session -t $SES 2>/dev/null

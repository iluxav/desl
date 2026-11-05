#!/bin/bash
D=${DESL_DEMO:-/tmp/desl-demo-api}
BIN=${DESL_BIN:-$(dirname "$0")/../target/debug/proto-a}
T="tmux -L desl-proto"; SES=dt
cap() { $T capture-pane -t $SES -p; }
wait_for() { local secs=$1; shift; for i in $(seq 1 "$secs"); do sleep 1; local P; P=$(cap); local ok=1; for pat in "$@"; do echo "$P" | grep -q -- "$pat" || ok=0; done; [ $ok = 1 ] && { echo "(matched after ${i}s)"; return 0; }; done; echo "(TIMEOUT waiting for: $*)"; cap; return 1; }
say() { $T send-keys -t $SES -l "$1"; $T send-keys -t $SES Enter; }
cd $D; $BIN stop >/dev/null 2>&1; sleep 0.5; rm -f ~/.local/state/desl-proto-a/*.jsonl
$T kill-session -t $SES 2>/dev/null
$T new-session -d -s $SES -x 90 -y 26 -c "$D" "$BIN; echo CLIENT-EXITED; sleep 90"; sleep 1.5
echo "##### 1. attached: fresh host, one thread"; cap | sed -n '2,4p;25p'
echo "##### 2. prompt, then detach mid-turn with ctrl+b d"
say 'List 12 short facts about trains, one per line, numbered. No tools.'
wait_for 40 '● working' >/dev/null; sleep 1.5; $T send-keys -t $SES C-b; sleep 0.2; $T send-keys -t $SES 'd'; sleep 0.8
cap | grep -v '^$' | tail -3
echo "##### 3. host alive? status from a plain shell"
(cd $D && $BIN status)
echo "##### 4. wait for the turn to finish in the background, then status again"
for i in $(seq 1 40); do sleep 1; (cd $D && $BIN status) | grep -q '○' && break; done; (cd $D && $BIN status)
echo "##### 5. reattach: the finished answer is there"
$T kill-session -t $SES 2>/dev/null; $T new-session -d -s $SES -x 90 -y 26 -c "$D" "$BIN; echo CLIENT-EXITED; sleep 90"; sleep 1.2
cap | sed -n '2,6p;16,21p;25p'
echo "##### 6. scroll up, detach, reattach: same scroll and pill"
$T send-keys -t $SES PageUp; sleep 0.3; $T send-keys -t $SES Up; sleep 0.3; BEFORE=$(cap | sed -n '3,5p'); echo "$BEFORE"; cap | grep -o '↓ [0-9]* new'
$T send-keys -t $SES C-b; sleep 0.2; $T send-keys -t $SES 'd'; sleep 0.8
$T kill-session -t $SES 2>/dev/null; $T new-session -d -s $SES -x 90 -y 26 -c "$D" "$BIN; echo CLIENT-EXITED; sleep 90"; sleep 1.2
AFTER=$(cap | sed -n '3,5p'); [ "$BEFORE" = "$AFTER" ] && echo "SCROLL RESTORED OK" || { echo "SCROLL DIFFERS:"; echo "$AFTER"; }; cap | grep -o '↓ [0-9]* new'
echo "##### 7. second client at the same time sees the same thread"
(cd $D && $BIN status)
echo "##### 8. stop the host while attached -> client sees Bye"
(cd $D && $BIN stop); sleep 0.8; cap | grep -v '^$' | tail -3
echo "##### 9. restart: log replays, thread and bindings survive"
$T kill-session -t $SES 2>/dev/null; $T new-session -d -s $SES -x 90 -y 26 -c "$D" "$BIN; echo CLIENT-EXITED; sleep 90"; sleep 1.5
say '/info'; sleep 0.6; cap | sed -n '2,4p;17,24p'
echo "##### 10. resume works after host restart: ask claude what it listed"
say 'Without tools: what was the first fact you listed? One sentence.'
wait_for 60 '○ idle' 'first fact' >/dev/null || true; cap | sed -n '19,25p'
say '/quit'; sleep 0.8; cap | grep -v '^$' | tail -2
(cd $D && $BIN stop); $T kill-session -t $SES 2>/dev/null
echo "##### files"; ls -la ~/.local/state/desl-proto-a/ | tail -3; echo "--- host stderr ---"; tail -4 ~/.local/state/desl-proto-a/*.host.log

#!/bin/bash
# Prototype B: the status must not stick on "working" (which makes /switch refuse).
# Covers an agent's own local command, an interrupted turn, and a long quiet tool run.
D=${DESL_DEMO:-/tmp/desl-demo-api}
BIN=${DESL_BIN_B:-$(dirname "$0")/../target/release/proto-b}
BIN=$(readlink -f "$BIN")
T="tmux -L desl-proto"; SES=pbst
cap() { $T capture-pane -t $SES -p; }
st() { (cd "$D" && "$BIN" status 2>&1 | head -1); }
wait_st() { local want=$1 secs=$2; for i in $(seq 1 "$secs"); do sleep 1; if st | grep -q " $want "; then echo "($want after ${i}s)"; return 0; fi; done; echo "(TIMEOUT waiting for $want; status: $(st))"; return 1; }
wait_for() { local secs=$1; shift; for i in $(seq 1 "$secs"); do sleep 1; if cap | grep -q -- "$1"; then return 0; fi; done; echo "(TIMEOUT waiting for: $1)"; return 1; }
say() { $T send-keys -t $SES -l "$1"; sleep 0.4; $T send-keys -t $SES Enter; }
$T kill-session -t $SES 2>/dev/null
$T new-session -d -s $SES -x 110 -y 32 -c "$D" "$BIN; echo B-EXITED; sleep 60"
wait_for 25 'Try "\|Yes, I trust\|❯' >/dev/null; if cap | grep -q 'Yes, I trust'; then $T send-keys -t $SES Enter; sleep 3; fi
sleep 2
echo "##### 1. one claude turn, then idle"
say 'Reply with the single word: ready'
wait_st working 15; wait_st idle 60
echo "##### 2. session file is the one desl chose (claude --session-id)"
say '/info'; sleep 1; cap | grep -E 'session file|session [0-9a-f]{8}-' | sed 's/^[│ ]*//' | cut -c1-150; $T send-keys -t $SES Escape; sleep 0.5
echo "##### 3. claude bash mode (! echo hi): claude answers the output unprompted; working, then idle"
$T send-keys -t $SES -l '!'; sleep 0.4; say 'echo hi'; wait_st working 8; wait_st idle 20
echo "##### 4. long tool run in the foreground: status holds on working the whole time"
say "Run this exact shell command in the foreground and wait for it to finish: python3 -c 'import time; time.sleep(22)'   Then reply with the single word: done"
wait_st working 20
for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14; do sleep 1; printf '%s ' "$(st | awk '{print $(NF-1)}')"; done; echo
cap | grep -i -E 'do you want|esc to interrupt|Running' | head -3 | cut -c1-120
echo "##### 5. esc interrupts mid-turn: status must return to idle"
$T send-keys -t $SES Escape; wait_st idle 12
echo "##### 5b. claude's file dialog (manual mode): waiting, desl's strip, n denies, back to idle"
$T send-keys -t $SES BTab; sleep 1; cap | grep -o 'manual mode on\|auto mode on\|accept edits on\|plan mode on' | head -1
rm -f "$D/probe.txt"
say 'Create a file named probe.txt containing the word hi. Do not do anything else.'
wait_st waiting 30; sleep 1; cap | grep -E 'Do you want to|asking for approval' | sed 's/^[│ ]*//' | sed 's/  */ /g' | cut -c1-100
$T send-keys -t $SES 'n'; wait_st idle 10; [ -e "$D/probe.txt" ] && echo "probe.txt WAS CREATED" || echo "probe.txt not created"
echo "##### 6. /switch codex is offered, not refused; accept it"
say '/switch codex'; sleep 1.5; cap | grep -E 'Handing|still working|Nothing to hand' | sed 's/^[│ ]*//' | cut -c1-120
$T send-keys -t $SES 'y'; wait_for 45 'Pasted\|pasted\|chars' >/dev/null; sleep 1; echo "status: $(st)"
echo "##### 6b. codex: long run, then esc; status must return to idle"
say "Run this exact shell command and wait for it to finish: python3 -c 'import time; time.sleep(30)'"
wait_st working 25
for i in 1 2 3 4 5 6 7 8; do sleep 1; printf '%s ' "$(st | awk '{print $(NF-1)}')"; done; echo
$T send-keys -t $SES Escape; wait_st idle 12
echo "##### 6c. /switch claude is offered"
say '/switch claude'; sleep 1.5; cap | grep -E 'Handing|still working|Nothing to hand|Switched' | sed 's/^[│ ]*//' | cut -c1-120
$T send-keys -t $SES 'n'; sleep 0.5
echo "##### 7. stop"
(cd "$D" && "$BIN" stop 2>&1 | head -2); sleep 1; $T kill-session -t $SES 2>/dev/null

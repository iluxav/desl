#!/bin/bash
# Prototype B: an answer that merely reads like a dialog ("Do you want to proceed?") must not leave the
# thread on "waiting" once the turn is over.
D=${DESL_DEMO:-/tmp/desl-demo-api}
BIN=${DESL_BIN_B:-$(dirname "$0")/../target/release/proto-b}
BIN=$(readlink -f "$BIN")
T="tmux -L desl-proto"; SES=pbpr
cap() { $T capture-pane -t $SES -p; }
st() { (cd "$D" && "$BIN" status 2>&1 | head -1); }
say() { $T send-keys -t $SES -l "$1"; sleep 0.4; $T send-keys -t $SES Enter; }
$T kill-session -t $SES 2>/dev/null
$T new-session -d -s $SES -x 110 -y 32 -c "$D" "$BIN; echo B-EXITED; sleep 60"
for i in $(seq 1 25); do sleep 1; cap | grep -q 'Try "\|❯' && break; done; sleep 2
say 'Print the numbers 1 to 20, one per line, and then on a final line exactly this sentence: Do you want to proceed with the plan?'
for i in 1 2 3 4 5 6 7 8 9 10 11 12; do sleep 1; printf '%s ' "$(st | awk '{print $(NF-1)}')"; done; echo
cap | grep -E 'Do you want' | sed 's/^[│ ]*//' | cut -c1-100 | head -3
echo "final status: $(st)"
say '/switch codex'; sleep 1.5; cap | grep -E 'Handing|still working|Nothing to hand' | sed 's/^[│ ]*//' | cut -c1-120
$T send-keys -t $SES 'n'; sleep 0.5
(cd "$D" && "$BIN" stop 2>&1 | head -1 | cut -c1-60); sleep 1; $T kill-session -t $SES 2>/dev/null

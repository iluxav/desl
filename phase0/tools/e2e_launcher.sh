#!/bin/bash
S=${DESL_SCRATCH:-/tmp/desl-scratch}
D=$S/api2; BIND=${DESL_BIN_DIR:-$(dirname "$0")/../target/debug}
export XDG_STATE_HOME=$S/state-test
T="tmux -L desl-proto"; SES=lau
cap() { $T capture-pane -t $SES -p; }
start() { $T kill-session -t $SES 2>/dev/null; $T new-session -d -s $SES -x 100 -y 28 -c "$D" -e XDG_STATE_HOME=$XDG_STATE_HOME "$BIND/desl-proto $1; echo LAUNCHER-EXITED; sleep 60"; }
which_proto() { if cap | grep -q 'Type a prompt and press enter'; then echo "A (rendered)"; elif cap | grep -q 'Claude Code\|trust\|Starting claude'; then echo "B (native)"; else echo "unknown"; cap | head -8; fi; }
echo "##### 1. no sessions yet"; (cd $D && $BIND/desl-proto log)
echo "##### 2. first launch picks one"; start ""; sleep 4; FIRST=$(which_proto); echo "running: $FIRST"
echo "##### 3. detach, launch again: rejoins the same one"; $T send-keys -t $SES C-b; sleep 0.3; $T send-keys -t $SES 'd'; sleep 1.2; cap | grep -v '^$' | tail -2 | cut -c1-90
start ""; sleep 3; echo "running: $(which_proto)"
echo "##### 4. status and stop go to the session's prototype"; (cd $D && $BIND/desl-proto status); (cd $D && $BIND/desl-proto stop); sleep 0.8
echo "##### 5. three more fresh sessions: the pick balances"
for n in 1 2 3; do start ""; sleep 3.5; echo "session $n: $(which_proto)"; $T send-keys -t $SES C-b; sleep 0.3; $T send-keys -t $SES 'd'; sleep 1; (cd $D && $BIND/desl-proto stop >/dev/null); sleep 0.8; done
echo "##### 6. forced b"; start "b"; sleep 3.5; echo "running: $(which_proto)"; $T send-keys -t $SES C-b; sleep 0.3; $T send-keys -t $SES 'd'; sleep 1; (cd $D && $BIND/desl-proto stop >/dev/null)
echo "##### 7. the observer's log"; (cd $D && $BIND/desl-proto log)
$T kill-session -t $SES 2>/dev/null

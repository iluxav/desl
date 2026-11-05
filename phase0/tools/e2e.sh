#!/bin/bash
# Drives Prototype A inside a private tmux socket and prints screen captures at each checkpoint.
D=${DESL_DEMO:-/tmp/desl-demo-api}
BIN=${DESL_BIN:-$(dirname "$0")/../target/debug/proto-a}
T="tmux -L desl-proto"
cap() { $T capture-pane -t e2e -p; }
# wait_for <seconds> <grep pattern...>: all patterns must match the screen
wait_for() { local secs=$1; shift; for i in $(seq 1 "$secs"); do sleep 1; local P; P=$(cap); local ok=1; for pat in "$@"; do echo "$P" | grep -q -- "$pat" || ok=0; done; [ $ok = 1 ] && { echo "(matched after ${i}s)"; return 0; }; done; echo "(TIMEOUT waiting for: $*)"; return 1; }
$T kill-session -t e2e 2>/dev/null; $T kill-session -t desl 2>/dev/null
rm -f /tmp/desl-ui-probe
$T new-session -d -s e2e -x 80 -y 24 -c "$D" "$BIN; echo EXITED; sleep 60"
sleep 1
echo "##### A. approval strip"
$T send-keys -t e2e -l 'Use the Bash tool to run exactly this command: touch /tmp/desl-ui-probe. Then say done in three words.'; $T send-keys -t e2e Enter
wait_for 60 '◐ waiting' 'Allow claude' && cap
echo "##### B. press y"
$T send-keys -t e2e 'y'
wait_for 60 '○ idle' 'allowed run' && cap; echo "probe file: $(ls /tmp/desl-ui-probe 2>&1)"
echo "##### C. long output, scroll up mid-stream (pill expected)"
$T send-keys -t e2e -l 'List 30 short facts about diesel engines, one per line, numbered. No tools.'; $T send-keys -t e2e Enter
wait_for 60 '● working' '▍'; sleep 2; $T send-keys -t e2e Up; sleep 0.2; $T send-keys -t e2e Up; sleep 0.2; $T send-keys -t e2e Up; sleep 2; cap
echo "##### D. esc resumes following; wait idle"
$T send-keys -t e2e Escape; wait_for 60 '○ idle' && cap
echo "##### E. ctrl+b b hides sidebar"
$T send-keys -t e2e C-b; sleep 0.2; $T send-keys -t e2e 'b'; sleep 0.5; cap | head -6
$T send-keys -t e2e C-b; sleep 0.2; $T send-keys -t e2e 'b'; sleep 0.3
echo "##### F. help overlay at 80x24"
$T send-keys -t e2e '?'; sleep 0.5; cap; $T send-keys -t e2e 'j'; $T send-keys -t e2e 'j'; $T send-keys -t e2e 'j'; sleep 0.3; echo "--- after jjj ---"; cap | sed -n '3,6p;19,22p'; $T send-keys -t e2e Escape; sleep 0.3
echo "##### G. ctrl+c interrupts a turn"
$T send-keys -t e2e -l 'Write a 500 word essay about trains. No tools.'; $T send-keys -t e2e Enter
wait_for 60 '● working' '▍'; sleep 1; $T send-keys -t e2e C-c; wait_for 30 '○ idle' 'interrupted' && cap | sed -n '1,4p;12,24p'
echo "##### H. ctrl+c when idle, then /quit"
$T send-keys -t e2e C-c; sleep 0.5; cap | tail -3 | head -1
$T send-keys -t e2e -l '/quit'; $T send-keys -t e2e Enter; sleep 1.5; echo "--- after quit ---"; cap | grep -v '^$' | head -4
$T kill-session -t e2e 2>/dev/null

#!/bin/bash
D=${DESL_DEMO:-/tmp/desl-demo-api}
BIN=${DESL_BIN:-$(dirname "$0")/../target/debug/proto-a}
T="tmux -L desl-proto"; SES=sw
cap() { $T capture-pane -t $SES -p; }
wait_for() { local secs=$1; shift; for i in $(seq 1 "$secs"); do sleep 1; local P; P=$(cap); local ok=1; for pat in "$@"; do echo "$P" | grep -q -- "$pat" || ok=0; done; [ $ok = 1 ] && { echo "(matched after ${i}s)"; return 0; }; done; echo "(TIMEOUT waiting for: $*)"; cap; return 1; }
say() { $T send-keys -t $SES -l "$1"; $T send-keys -t $SES Enter; }
git -C $D checkout -q -- .; $T kill-session -t $SES 2>/dev/null
$T new-session -d -s $SES -x 100 -y 30 -c "$D" "$BIN; echo EXITED; sleep 60"; sleep 1
echo "##### 1. claude turn"
say 'Read src/auth/tokens.ts and tell me in one sentence what ACCESS_TTL is set to.'
wait_for 90 '○ idle' '▸ read' >/dev/null && cap | sed -n '2,12p'
echo "##### 2. /switch codex -> confirmation strip"
say '/switch codex'; sleep 0.6; cap | sed -n '22,30p'
echo "##### 3. y -> divider, agent changes"
$T send-keys -t $SES 'y'; sleep 0.6; cap | sed -n '2,3p;8,16p;29p'
echo "##### 4. prompt to codex (brief rides along)"
say 'Without reading any files: what value did we find for ACCESS_TTL? One short sentence.'
wait_for 150 '○ idle' 'brief sent to codex' >/dev/null && cap | sed -n '2,3p;10,24p;29p'
echo "##### 5. /switch claude -> delta brief"
say '/switch claude'; sleep 0.6; cap | sed -n '26,30p'; $T send-keys -t $SES 'y'; sleep 0.5
say 'In one sentence: what did the other assistant answer just now?'
wait_for 120 '○ idle' 'delta brief sent to claude' >/dev/null && cap | sed -n '2,3p;12,29p'
echo "##### 6. transcript focus: tab, k k k, enter expands the selected item"
$T send-keys -t $SES Tab; sleep 0.2; $T send-keys -t $SES 'k'; $T send-keys -t $SES 'k'; $T send-keys -t $SES 'k'; sleep 0.2; $T send-keys -t $SES Enter; sleep 0.4; cap | sed -n '2,29p'
$T send-keys -t $SES Escape; sleep 0.2
echo "##### 7. /info"
say '/info'; sleep 0.4; cap | sed -n '18,28p'
say '/quit'; sleep 1.5; echo "##### 8. after quit"; cap | grep -v '^$' | head -6
$T kill-session -t $SES 2>/dev/null

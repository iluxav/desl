#!/bin/bash
D=${DESL_DEMO:-/tmp/desl-demo-api}
BIN=${DESL_BIN_B:-$(dirname "$0")/../target/debug/proto-b}
T="tmux -L desl-proto"; SES=pbs
cap() { $T capture-pane -t $SES -p; }
wait_for() { local secs=$1; shift; for i in $(seq 1 "$secs"); do sleep 1; local P; P=$(cap); local ok=1; for pat in "$@"; do echo "$P" | grep -q -- "$pat" || ok=0; done; [ $ok = 1 ] && { echo "(matched after ${i}s)"; return 0; }; done; echo "(TIMEOUT waiting for: $*)"; return 1; }
say() { $T send-keys -t $SES -l "$1"; sleep 0.3; $T send-keys -t $SES Enter; }
git -C $D checkout -q -- .
$T kill-session -t $SES 2>/dev/null
$T new-session -d -s $SES -x 110 -y 32 -c "$D" "$BIN; echo B-EXITED; sleep 60"
wait_for 25 'Try "write\|Yes, I trust' >/dev/null; if cap | grep -q 'Yes, I trust'; then $T send-keys -t $SES Down; sleep 0.3; $T send-keys -t $SES Enter; sleep 3; fi
echo "##### 1. claude turn; status from the session file"
say 'Read src/auth/tokens.ts and tell me in one sentence what ACCESS_TTL is set to.'
wait_for 15 '● working' >/dev/null && echo "working seen"; wait_for 90 '○ idle' >/dev/null; sleep 2; cap | sed -n '2,5p;31p'
echo "##### 2. /info shows the transcript path and seen counts"
say '/info'; sleep 0.8; cap | grep -E 'transcript|seen|turns' | head -4; $T send-keys -t $SES Escape; sleep 0.3
echo "##### 3. /switch codex typed into claude's input -> desl strip"
say '/switch codex'; sleep 1; cap | sed -n '30,32p'
echo "##### 4. y -> codex pane boots, brief pasted when ready"
$T send-keys -t $SES 'y'; sleep 1; cap | sed -n '2,4p'; wait_for 40 'Pasted\|pasted\|chars' >/dev/null; sleep 1; cap | sed -n '2,4p;22,32p'
echo "##### 5. ask codex the question; it answers from the brief"
say 'Without reading files: what is ACCESS_TTL set to? One short sentence.'
wait_for 15 '● working' >/dev/null && echo "codex working seen"; wait_for 120 '○ idle' >/dev/null; sleep 2; cap | sed -n '2,4p;10,30p' | grep -v '^│ *│ *$' | head -14
echo "##### 6. /brief popup (what claude would get back)"
say '/brief'; sleep 0.8; cap | sed -n '5,14p'; $T send-keys -t $SES Escape; sleep 0.3
echo "##### 7. /switch claude -> delta brief"
say '/switch claude'; sleep 1; cap | sed -n '31p'; $T send-keys -t $SES 'y'; sleep 4; cap | sed -n '2,4p;24,32p'
echo "##### 8. leave"; $T send-keys -t $SES C-b; sleep 0.2; $T send-keys -t $SES 'd'; sleep 1.5; cap | grep -v '^$' | tail -2
$T kill-session -t $SES 2>/dev/null; git -C $D checkout -q -- .

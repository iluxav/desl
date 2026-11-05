#!/bin/bash
# Records a ~30 s session of Prototype A with util-linux `script` inside a private tmux socket,
# typing like a person, then converts it to asciicast v2.
set -u
D=${DESL_DEMO:-/tmp/desl-demo-api}
BIN=${DESL_BIN:-$(dirname "$0")/../target/debug/proto-a}
OUT=$(dirname "$0")/../recordings; mkdir -p "$OUT"
REC=$OUT/day1-proto-a
T="tmux -L desl-proto"
type_slow() { local s="$1"; local i; for ((i=0; i<${#s}; i++)); do $T send-keys -t rec -l "${s:$i:1}"; sleep 0.045; done; }
cap() { $T capture-pane -t rec -p; }
wait_for() { local secs=$1; shift; for i in $(seq 1 "$secs"); do sleep 1; local P; P=$(cap); local ok=1; for pat in "$@"; do echo "$P" | grep -q -- "$pat" || ok=0; done; [ $ok = 1 ] && return 0; done; return 1; }
$T kill-session -t rec 2>/dev/null
rm -f "$REC.out" "$REC.timing" "$REC.cast"
$T new-session -d -s rec -x 80 -y 24 -c "$D" "script -q -T '$REC.timing' -O '$REC.out' -c '$BIN'; sleep 5"
sleep 1.5
type_slow 'Read src/auth/middleware.ts and explain in two sentences what it does.'
sleep 0.6; $T send-keys -t rec Enter
wait_for 60 '○ idle' '▸ read'
sleep 2.5
$T send-keys -t rec '?'; sleep 3.5; $T send-keys -t rec Escape; sleep 1.5
type_slow '/quit'; sleep 0.5; $T send-keys -t rec Enter
sleep 2
echo "--- final pane ---"; cap | grep -v '^$' | head -3
python3 -I $(dirname "$0")/script2cast.py "$REC.out" "$REC.timing" 80 24 > "$REC.cast"
$T kill-session -t rec 2>/dev/null
ls -la "$OUT"; echo "--- cast head ---"; head -c 300 "$REC.cast"; echo; echo "--- duration ---"; tail -1 "$REC.cast" | cut -c1-40

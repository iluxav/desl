#!/bin/bash
# Records one hallway session: the terminal (script + timing), the microphone (pw-record) and a
# manifest. Opens a plain shell in the demo repository where `desl` starts the blind launcher.
# Usage: hallway-session.sh <participant-id>      e.g. hallway-session.sh p03
set -u
P=${1:?Give a participant id, for example: hallway-session.sh p03}
HERE=$(cd "$(dirname "$0")" && pwd)
DEMO=${DESL_HALLWAY_REPO:-$HOME/desl-hallway/api}
OUT=${DESL_HALLWAY_OUT:-$HOME/desl-hallway/sessions}/$P-$(date +%Y%m%d-%H%M%S)
command -v desl-proto >/dev/null || { echo "desl-proto is not installed. Run \`make install\` in the repository first."; exit 1; }
[ -d "$DEMO/.git" ] || { echo "The demo repository is missing. Run $HERE/hallway-setup.sh first."; exit 1; }
mkdir -p "$OUT"
LOG=${XDG_STATE_HOME:-$HOME/.local/state}/desl-proto/launches.jsonl
BEFORE=$( [ -f "$LOG" ] && wc -l < "$LOG" || echo 0 )
START_EPOCH=$(date +%s); START_ISO=$(date -Is)
SIZE=$(stty size 2>/dev/null || echo "30 100"); ROWS=${SIZE% *}; COLS=${SIZE#* }

AUDIO_PID=""
if command -v pw-record >/dev/null; then
  pw-record "$OUT/audio.wav" >/dev/null 2>&1 & AUDIO_PID=$!
else
  echo "pw-record is not installed, so there will be no audio. Record the room another way."
fi

RC=$(mktemp)
cat > "$RC" <<RCEOF
PS1='\$ '
desl() { desl-proto "\$@"; }
cd "$DEMO"
echo "Recording. Type desl to begin; exit ends the session."
RCEOF
script -q -T "$OUT/terminal.timing" -O "$OUT/terminal.out" -c "bash --noprofile --rcfile $RC -i"
rm -f "$RC"

# SIGTERM, not SIGINT: a background job of a script ignores SIGINT. Give it two seconds to close the file.
if [ -n "$AUDIO_PID" ]; then
  kill -TERM "$AUDIO_PID" 2>/dev/null
  for _ in $(seq 1 20); do kill -0 "$AUDIO_PID" 2>/dev/null || break; sleep 0.1; done
  kill -KILL "$AUDIO_PID" 2>/dev/null
fi
END_EPOCH=$(date +%s)
python3 "$HERE/script2cast.py" "$OUT/terminal.out" "$OUT/terminal.timing" "$COLS" "$ROWS" > "$OUT/terminal.cast" 2>/dev/null || true
PICKS=$( [ -f "$LOG" ] && tail -n +$((BEFORE + 1)) "$LOG" | paste -sd, - || echo "" )
cat > "$OUT/manifest.json" <<JSON
{ "participant": "$P", "start": "$START_ISO", "start_epoch": $START_EPOCH, "end_epoch": $END_EPOCH,
  "rows": $ROWS, "cols": $COLS, "demo": "$DEMO", "audio": $( [ -n "$AUDIO_PID" ] && echo true || echo false ),
  "launches": [${PICKS}] }
JSON
echo "Session $P saved in $OUT ($(( (END_EPOCH - START_EPOCH) / 60 )) min)."
echo "Prototypes used: $(echo "$PICKS" | grep -o '"label":"[AB]"' | tr -d '"' | cut -d: -f2 | paste -sd' ' -)"

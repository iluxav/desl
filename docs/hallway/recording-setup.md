# Recording setup

Every session produces three things in `~/desl-hallway/sessions/<participant>-<timestamp>/`:

| File | What | How |
| --- | --- | --- |
| `terminal.out`, `terminal.timing`, `terminal.cast` | The exact terminal session, with timing | util-linux `script`; replay with `scriptreplay -t terminal.timing terminal.out` or any asciinema player |
| `audio.wav` | The room: participant thinking aloud, observer's goals | `pw-record` from the default PipeWire source |
| `manifest.json` | Participant, start and end times, terminal size, the launcher's picks | written by `hallway-session.sh` |

The terminal recording is the screen recording: desl is the only thing on screen, and a replay is
sharper than video and searchable. A score's wall-clock time minus the manifest's start time is the
offset into both the terminal recording and the audio.

If you also want video of the whole desktop, start Omarchy's own recorder before the session
(`omarchy-capture-screenrecording`) and stop it after; `gpu-screen-recorder` is installed and is
what it drives. This is optional and not used by the report.

Checks before the first participant:

- `pw-record /tmp/test.wav`, speak, ctrl+c, `pw-play /tmp/test.wav`. If it is silent, pick the
  microphone as the default source in the audio settings.
- Use a terminal at least 100 columns by 30 rows; the manifest records the size.
- Turn the terminal bell on. Moment 5 depends on hearing it.

# Hallway protocol, phase 0

How the ten sessions run. The rules come from `SPEC.md` "The feel bar"; this folder holds what
the kickoff asks to be prepared: the task script, the scoring sheet and the recording setup.
The spec's author runs the sessions; the observer stays silent.

## Shape of one session (45 minutes)

| Minutes | What |
| --- | --- |
| 0–3 | Welcome. Read the framing below aloud. Start the recording. |
| 3–20 | First prototype: tasks 1 to 7, scores after each task. |
| 20–23 | Reset the demo repository. Start the second prototype with the other label. |
| 23–40 | Second prototype: the same tasks. |
| 40–45 | Debrief: "Which one would you keep, and why?" Stop the recording. |

Every participant uses both prototypes. The launcher picks the first one so that first-prototype
picks stay balanced across participants; the observer forces the other for the second half
(`desl a` or `desl b`). The participant is told only "this is version one" and "this is version two".

**Framing, read aloud:** "This is a prototype of a terminal tool for running coding agents. I am
testing the tool, not you. I will give you goals, not steps. Think aloud if you can. I will not
answer questions about how to do things, but I will write them down. After each goal I will ask
you for a few scores from 1 to 5 and one sentence about what you expected."

## Rules

- Participants: daily users of both Claude Code and Codex who also use tmux or lazygit. The same
  ten for every gate.
- Tasks are goals, never instructions. If a participant is stuck for two minutes, score the moment
  and move on; do not teach.
- After each task: the scores for that task's moments, then "Before you started, what did you
  expect to happen?" Write the answer down verbatim, one sentence.
- A moment passes when its median is 4 or higher and no participant scores it 1.
- Every 1 or 2 becomes an issue with the recording timestamp attached. Note the wall-clock time
  next to the score; `gate_report.py` turns it into an offset into the recording.
- Phase 0's gate: one prototype has a median of 4 or better on moments 1, 2, 3, 4 and 6. The
  other is deleted.

## Before the first session

1. `make install` from the repository root.
2. `phase0/tools/hallway-setup.sh` creates the demo repository at `~/desl-hallway/api`.
3. Run `claude` once inside it and accept the trust dialog, then quit. Do the same with `codex`.
   Otherwise the native prototype opens on the agent's trust dialog and the rendered one does not,
   which would bias moment 1.
4. Check the microphone: `pw-record /tmp/test.wav`, speak, ctrl+c, `pw-play /tmp/test.wav`.

## Each session

```
phase0/tools/hallway-setup.sh               # reset the demo repository
phase0/tools/hallway-session.sh p03         # records terminal and microphone, opens a shell
```

The participant types `desl` in that shell. When the first half is done, the observer types
`desl stop`, resets the repository in another terminal, and has the participant run `desl a` or
`desl b` (the label the launcher did not pick; `desl log` shows it). `exit` ends the session and
writes the manifest. Files land in `~/desl-hallway/sessions/<participant>-<timestamp>/`.

Task 6 needs the observer to break something: run `phase0/tools/hallway-break.sh` in another
terminal while the agent is working.

## After the ten sessions

Type the scores into `scoring-sheet.csv` (one row per participant, prototype, task and moment),
then:

```
python3 phase0/tools/gate_report.py docs/hallway/scoring-sheet.csv ~/desl-hallway/sessions
```

It prints, per prototype, the median and minimum per moment, pass or fail, and every 1 or 2 with
its offset into that participant's recording. Paste it into `gate-report-template.md` and write the
one-page recommendation.

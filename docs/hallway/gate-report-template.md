# Phase 0 gate report

Date: ________   Sessions: 10   Observer: ________

## Result

Winning prototype: ____ (label ____).  Gate (median ≥ 4 on moments 1, 2, 3, 4, 6): passed / not passed.

## Scores

Paste the output of `python3 phase0/tools/gate_report.py docs/hallway/scoring-sheet.csv ~/desl-hallway/sessions`.

## Every 1 and 2

From the same output: participant, prototype, task, moment, recording file and offset, the
participant's sentence. Each becomes an issue.

## Recommendation (one page)

- Which approach, and the two or three moments that decided it.
- What the losing prototype did better, and whether the winner can borrow it.
- What neither prototype could do that the spec promises.

## Changes to SPEC.md

A list of concrete edits to "Interface design", "Commands, keybindings and CLI" and
"Cross-agent continuity", each with the decision number in `docs/phase0-decisions.md` or the
session finding that motivates it. Sent as one change against the spec.

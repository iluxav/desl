#!/usr/bin/env python3
"""Phase 0 gate numbers from the hallway scoring sheet.

Usage: gate_report.py scoring-sheet.csv [sessions-dir]

Per prototype and moment: median, minimum, pass or fail (median >= 4 and no 1). Then every 1 or 2
with its offset into that participant's recording, when a sessions directory is given.
"""
import csv, json, pathlib, statistics, sys
from datetime import datetime

MOMENTS = {1: "First reply in ten seconds", 2: "Keys you already know", 3: "Switching feels like changing the model",
           4: "Nothing moves under your eyes", 5: "Waiting is impossible to miss or be nagged by",
           6: "The agent is the hero", 7: "Coming back is instant", 8: "Errors read like a colleague",
           9: "Leaving is always safe"}
GATE = (1, 2, 3, 4, 6)

def load(path):
    rows = []
    for r in csv.DictReader(open(path, newline="")):
        try:
            rows.append({**r, "moment": int(r["moment"]), "score": int(r["score"])})
        except (KeyError, ValueError):
            continue
    return rows

def session_start(sessions, participant):
    """The newest session directory for a participant: (path, start datetime)."""
    if not sessions:
        return None
    dirs = sorted(pathlib.Path(sessions).glob(f"{participant}-*"))
    for d in reversed(dirs):
        m = d / "manifest.json"
        if m.exists():
            try:
                return d, datetime.fromisoformat(json.loads(m.read_text())["start"])
            except Exception:
                continue
    return None

def offset(start, hhmmss):
    try:
        t = datetime.strptime(hhmmss.strip(), "%H:%M:%S").time()
    except ValueError:
        return None
    then = start.replace(hour=t.hour, minute=t.minute, second=t.second, microsecond=0)
    secs = int((then - start).total_seconds())
    return secs if secs >= 0 else None

def report(rows, sessions=None, out=sys.stdout):
    protos = sorted({r["prototype"] for r in rows})
    verdict = {}
    for p in protos:
        print(f"\nPrototype {p.upper()}", file=out)
        print(f"  {'moment':<50} {'n':>3} {'median':>6} {'min':>4}  result", file=out)
        ok_gate = True
        for m, name in MOMENTS.items():
            # A participant's score for a moment is the lowest they gave it across tasks.
            per = {}
            for r in rows:
                if r["prototype"] == p and r["moment"] == m:
                    per[r["participant"]] = min(per.get(r["participant"], 5), r["score"])
            if not per:
                print(f"  {m}. {name:<47} {'-':>3} {'-':>6} {'-':>4}  no scores", file=out)
                ok_gate &= m not in GATE
                continue
            med, low = statistics.median(per.values()), min(per.values())
            passed = med >= 4 and low > 1
            ok_gate &= (med >= 4) or m not in GATE
            print(f"  {m}. {name:<47} {len(per):>3} {med:>6.1f} {low:>4}  {'pass' if passed else 'FAIL'}", file=out)
        verdict[p] = ok_gate
        print(f"  Gate (median >= 4 on moments {', '.join(map(str, GATE))}): {'met' if ok_gate else 'not met'}", file=out)
    low = [r for r in rows if r["score"] <= 2]
    print(f"\nEvery 1 and 2 ({len(low)})", file=out)
    for r in sorted(low, key=lambda r: (r["prototype"], r["participant"], r["task"])):
        where = ""
        s = session_start(sessions, r["participant"])
        if s and r.get("time"):
            off = offset(s[1], r["time"])
            if off is not None:
                where = f"  {s[0].name}/terminal.cast at {off // 60}:{off % 60:02d}"
        print(f"  {r['prototype'].upper()} {r['participant']} task {r['task']} moment {r['moment']} score {r['score']}{where}", file=out)
        if r.get("expectation"):
            print(f"      \"{r['expectation']}\"", file=out)
    return verdict

if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    rows = load(sys.argv[1])
    if not rows:
        sys.exit(f"No scores in {sys.argv[1]}. Fill in the sheet first.")
    report(rows, sys.argv[2] if len(sys.argv) > 2 else None)

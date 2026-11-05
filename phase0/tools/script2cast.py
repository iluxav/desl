#!/usr/bin/env python3
"""Convert a util-linux `script` recording (-O out -T timing, classic format) to asciicast v2.

Usage: script2cast.py OUT.log TIMING.log WIDTH HEIGHT > recording.cast
"""
import codecs, json, sys, time

out_path, timing_path, width, height = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
data = open(out_path, "rb").read()
# `script` writes one header line ("Script started on ...") before timing begins.
nl = data.find(b"\n")
body = data[nl + 1:] if nl >= 0 else data
decoder = codecs.getincrementaldecoder("utf-8")(errors="replace")
header = {"version": 2, "width": width, "height": height, "timestamp": int(time.time()),
          "env": {"TERM": "xterm-256color", "SHELL": "/bin/bash"}, "title": "desl phase 0, prototype A"}
print(json.dumps(header))
pos, t = 0, 0.0
for line in open(timing_path):
    parts = line.split()
    if len(parts) != 2:
        continue
    delay, count = float(parts[0]), int(parts[1])
    t += delay
    chunk = body[pos:pos + count]
    pos += count
    text = decoder.decode(chunk)
    if text:
        print(json.dumps([round(t, 6), "o", text]))

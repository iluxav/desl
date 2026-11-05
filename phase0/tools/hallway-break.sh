#!/bin/bash
# Task 6: breaks the running agent so the participant meets an error. Run it in another terminal
# while the agent is working. It ends the agent processes the prototypes started, nothing else.
set -u
killed=0
# Prototype A drives headless agents; prototype B runs their interfaces under its host.
for pat in 'claude -p --output-format stream-jso[n]' 'codex exec .*--jso[n]'; do
  if pkill -f "$pat" 2>/dev/null; then killed=1; fi
done
for host in $(pgrep -f 'proto-[b] --host' 2>/dev/null); do
  for child in $(pgrep -P "$host" 2>/dev/null); do kill "$child" 2>/dev/null && killed=1; done
done
[ "$killed" = 1 ] && echo "Agent stopped. Watch what the participant reads and does next." || echo "No agent was running. Wait until the status reads working, then run this again."

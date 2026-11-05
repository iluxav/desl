# Task script

Seven goals, read aloud exactly as written, in this order, for each prototype. The "observer"
lines are not read aloud. Moments are the nine in `SPEC.md` "The feel bar".

The demo repository is a small Node service with an auth middleware, a token module and tests
that run with `npm test` (no dependencies to install).

---

### Task 1. First answer

> "You have just installed this tool. In this repository, find out what the auth middleware does."

- Observer: start a stopwatch when they press enter on `desl`. Stop at the first agent reply.
  Write the seconds down. Note anything they read or were asked before the reply.
- Scores: **moment 1** (first reply in ten seconds), **moment 6** (the agent is the hero).
- Then: "Before you started, what did you expect to happen?"

### Task 2. A change, while reading

> "Have it make token verification reject tokens that are on a revoked list, and run the tests.
> While it works, go back and reread its first answer."

- Observer: watch whether the text they are reading moves; whether they find their way back to
  the newest output; how they answer when the agent asks to run a command.
- Scores: **moment 4** (nothing moves under your eyes), **moment 5** (waiting is impossible to
  miss and impossible to be nagged by).
- Then the expectation question.

### Task 3. A second opinion

> "You would like the other agent to carry on with this work. Get it to continue from here, and
> ask it whether the change is safe."

- Observer: do not say "switch". Note the first thing they try. Note whether the other agent's
  reply answers their question or talks about the handoff.
- Scores: **moment 3** (switching feels like changing the model), **moment 2** (keys you already
  know).
- Then the expectation question.

### Task 4. Two things at once

> "Start a second, separate piece of work: have an agent draft a short README section about
> authentication. Keep both going, and deal with whichever needs you."

- Observer: how they open the second thread, how they notice which thread needs them, how they
  move between the two.
- Scores: **moment 2**, **moment 5**.
- Then the expectation question.

### Task 5. Leave and come back

> "You have to close your laptop now. Leave."
> (Wait two minutes; talk about something else.)
> "You are back. Pick up where you were."

- Observer: what they do to leave and whether they hesitate about losing work. On return, time
  from pressing enter to the restored screen, and whether it is where they left it.
- Scores: **moment 9** (leaving is always safe), **moment 7** (coming back is instant).
- Then the expectation question.

### Task 6. Something breaks

> "Ask it to explain the token module in detail."
> (Observer runs `hallway-break.sh` while the agent is working.)
> "Get it going again."

- Observer: read the message on screen silently; note whether they understood what happened and
  what to do without help.
- Scores: **moment 8** (errors read like a colleague).
- Then the expectation question.

### Task 7. What can I press

> "Without leaving the tool, find out everything you can press right now."

- Observer: the first key they try. Whether the list they find matches what worked for them.
- Scores: **moment 2**.
- Then the expectation question.

---

### Debrief, after both prototypes

1. "Which one would you keep, and why?"
2. "What was the most surprising moment in either?"
3. "Was there a key or a word that did something you did not expect?"

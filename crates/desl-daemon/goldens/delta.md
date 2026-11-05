# Handoff brief (delta)
You are continuing a thread you have seen before. Below is only what happened
since you last saw it. Treat it as your own history and continue from "Next step".

## Decisions
- I'll use a rotating refresh token because a stolen one then stops working after one use.

## Files touched
- src/auth/middleware.ts (+2 −1)
- src/auth/tokens.ts (new)

## Recent conversation
Assistant: I'll use a rotating refresh token because a stolen one then stops working after one use.

- edit src/auth/middleware.ts ✓
- edit src/auth/tokens.ts
- run pnpm test ✕

Assistant: Rotation is in, one test fails.

Want me to add the revocation list?

## Open questions
- Want me to add the revocation list?

## Next step
Want me to add the revocation list?

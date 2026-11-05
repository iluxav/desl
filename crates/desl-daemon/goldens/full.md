# Handoff brief
You are continuing work that another assistant started. Treat this as your
own history. Do not re-explain it; continue from "Next step".

## Task
Add refresh-token rotation to the auth middleware

## Decisions
- I'll use a rotating refresh token because a stolen one then stops working after one use.

## Files touched
- src/auth/middleware.ts (+2 −1)
- src/auth/tokens.ts (new)

## Current diff
diff --git a/src/auth/tokens.ts b/src/auth/tokens.ts
+x

## Recent conversation
User: Add refresh-token rotation to the auth middleware

Assistant: I'll start by reading the current middleware.

- read src/auth/middleware.ts ✓

Assistant: I'll use a rotating refresh token because a stolen one then stops working after one use.

- edit src/auth/middleware.ts ✓
- edit src/auth/tokens.ts
- run pnpm test ✕

Assistant: Rotation is in, one test fails.

Want me to add the revocation list?

## Open questions
- Want me to add the revocation list?

## Next step
Add the revocation list too

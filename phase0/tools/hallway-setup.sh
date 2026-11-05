#!/bin/bash
# Creates or resets the demo repository the hallway tasks run in.
# Usage: hallway-setup.sh [dir]     (default ~/desl-hallway/api)
set -eu
DIR=${1:-${DESL_HALLWAY_REPO:-$HOME/desl-hallway/api}}
mkdir -p "$DIR"; cd "$DIR"
# Stop any session still running here so the next one starts clean.
command -v desl-proto >/dev/null && desl-proto stop >/dev/null 2>&1 || true
find . -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf {} +
mkdir -p src/auth test
cat > package.json <<'JSON'
{ "name": "api", "private": true, "type": "module", "scripts": { "test": "node --test" } }
JSON
cat > src/auth/tokens.js <<'JS'
import { createHmac, timingSafeEqual } from "node:crypto";

const SECRET = process.env.TOKEN_SECRET ?? "dev-secret";
export const ACCESS_TTL_SECONDS = 15 * 60;

export class AuthError extends Error {}

function sign(payload) {
  return createHmac("sha256", SECRET).update(payload).digest("base64url");
}

/** A token is `<base64url json>.<signature>`. */
export function signAccessToken(sub, now = Date.now()) {
  const body = Buffer.from(JSON.stringify({ sub, exp: Math.floor(now / 1000) + ACCESS_TTL_SECONDS })).toString("base64url");
  return `${body}.${sign(body)}`;
}

export function verifyAccessToken(token, now = Date.now()) {
  const [body, signature] = String(token).split(".");
  if (!body || !signature) throw new AuthError("malformed token");
  const expected = Buffer.from(sign(body));
  const given = Buffer.from(signature);
  if (expected.length !== given.length || !timingSafeEqual(expected, given)) throw new AuthError("bad signature");
  const claims = JSON.parse(Buffer.from(body, "base64url").toString());
  if (claims.exp < Math.floor(now / 1000)) throw new AuthError("expired token");
  return claims;
}
JS
cat > src/auth/middleware.js <<'JS'
import { verifyAccessToken, AuthError } from "./tokens.js";

/** Rejects requests without a valid bearer token; attaches the user id to the request. */
export function requireAuth(req, res, next) {
  const header = req.headers.authorization ?? "";
  const token = header.startsWith("Bearer ") ? header.slice(7) : null;
  if (!token) {
    res.statusCode = 401;
    return res.end(JSON.stringify({ error: "missing bearer token" }));
  }
  try {
    req.userId = verifyAccessToken(token).sub;
    next();
  } catch (err) {
    if (err instanceof AuthError) {
      res.statusCode = 401;
      return res.end(JSON.stringify({ error: err.message }));
    }
    next(err);
  }
}
JS
cat > test/tokens.test.js <<'JS'
import { test } from "node:test";
import assert from "node:assert/strict";
import { signAccessToken, verifyAccessToken, AuthError, ACCESS_TTL_SECONDS } from "../src/auth/tokens.js";

test("a fresh token verifies and carries its subject", () => {
  assert.equal(verifyAccessToken(signAccessToken("u1")).sub, "u1");
});

test("a tampered token is rejected", () => {
  const token = signAccessToken("u1");
  assert.throws(() => verifyAccessToken(token.slice(0, -2) + "xx"), AuthError);
});

test("an expired token is rejected", () => {
  const then = Date.now() - (ACCESS_TTL_SECONDS + 5) * 1000;
  assert.throws(() => verifyAccessToken(signAccessToken("u1", then)), /expired/);
});
JS
printf '# api\n\nA small service used to try desl. `npm test` runs the tests.\n' > README.md
if [ ! -d .git ]; then git init -q -b main; fi
git add -A
git -c user.email=hallway@example.com -c user.name=hallway commit -qm "reset for a hallway session" --allow-empty
echo "Demo repository ready at $DIR ($(git rev-parse --short HEAD)). Tests: $(npm test --silent >/dev/null 2>&1 && echo pass || echo FAIL)."

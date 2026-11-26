// The tab asks for the user's eyes (web-spec.md, "Attention"): its title leads with how many
// things wait, and while the tab is in the background and something new waits, the title and
// the icon's dot blink until the user comes back. Something waiting when they left does not blink.

/** The title, led by how many things wait. */
export function titled(base, count) {
  return count ? `(${count}) ${base}` : base;
}

/** The title's other half while it blinks. */
export function flashed(count) {
  return `● ${count} waiting · desl`;
}

/**
 * What the user has seen and whether the tab blinks, now that `keys` wait: with the tab in
 * focus, everything waiting is seen; behind, a key not seen yet blinks it.
 */
export function step(seen, keys, focused) {
  if (focused) return { seen: new Set(keys), blink: false };
  return { seen, blink: keys.some((k) => !seen.has(k)) };
}

// desl's mark, and the same with an amber dot on it.
const DOT = '/icon-waiting.svg';
const NONE = '/icon.svg';

let seen = new Set();
let timer = null;
let flip = false;
let last = { base: 'desl', keys: [] };

function focused() {
  return !document.hidden && document.hasFocus();
}

function setIcon(href) {
  const link = document.querySelector('link[rel="icon"]');
  if (link && link.getAttribute('href') !== href) link.setAttribute('href', href);
}

function tick() {
  flip = !flip;
  const n = last.keys.length;
  document.title = flip ? flashed(n) : titled(last.base, n);
  setIcon(flip ? NONE : DOT);
}

/** The tab's title and icon for `base` (the thread on screen) with `keys` waiting. */
export function attend(base, keys) {
  last = { base, keys };
  const s = step(seen, keys, focused());
  seen = s.seen;
  if (s.blink && !timer) {
    flip = false;
    timer = setInterval(tick, 1000);
  } else if (!s.blink && timer) {
    clearInterval(timer);
    timer = null;
  }
  if (!timer) {
    document.title = titled(base, keys.length);
    setIcon(keys.length ? DOT : NONE);
  }
}

/** Coming back to the tab stops the blinking. */
export function setupAttention() {
  const again = () => attend(last.base, last.keys);
  window.addEventListener('focus', again);
  window.addEventListener('blur', again);
  document.addEventListener('visibilitychange', again);
}

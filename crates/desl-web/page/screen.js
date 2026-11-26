// The agent's own screen on desl web's page (docs/design/2026-11-24-web-alone.md): xterm.js
// draws what the agent draws, and the keys, pastes and the mouse go to it. The page attaches to
// a thread while its Screen view shows, as the TUI does: the replay is drawn part by part at the
// size each part was drawn at, then every byte as it comes. While a TUI shows the same thread its
// size wins, and the daemon says it with `pty_size` (docs/protocol.md).

import { Terminal } from '/vendor/xterm.mjs';
import { FitAddon } from '/vendor/addon-fit.mjs';

const screen = {
  term: null,
  fit: null,
  /** The thread attached, whose screen this is. */
  thread: null,
  /** Its replay is drawn: what comes next is drawn as it comes. */
  ready: false,
  /** What came while the replay was being drawn. */
  queue: [],
  /** The size last asked for. */
  asked: null,
  timer: null,
  /** What the page gave: `request` (answered), `fire` (never answered), `note`, `keys`, `who`, `agent`. */
  io: null,
};

export function fromB64(s) {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

export function toB64(bytes) {
  let bin = '';
  for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
  return btoa(bin);
}

const paint = (bytes) => new Promise((resolve) => screen.term.write(bytes, resolve));

// What the daemon kept of a terminal marks each size it was given: `ESC _ desl;size;R;C ESC \`.
// Each part is drawn at its size, as it was while it happened (docs/protocol.md, `attach`).
const MARK = new TextEncoder().encode('\x1b_desl;size;');

/** Where each size mark is in `b`, and the size it gives. */
export function marks(b) {
  const out = [];
  for (let i = 0; i + MARK.length <= b.length; i++) {
    let same = true;
    for (let k = 0; k < MARK.length && same; k++) same = b[i + k] === MARK[k];
    if (!same) continue;
    let j = i + MARK.length;
    let text = '';
    while (j < b.length && b[j] !== 0x1b && text.length < 12) text += String.fromCharCode(b[j++]);
    if (b[j] !== 0x1b || b[j + 1] !== 0x5c) continue;
    const [rows, cols] = text.split(';').map(Number);
    if (rows > 0 && cols > 0) out.push({ at: i, end: j + 2, rows, cols });
    i = j + 1;
  }
  return out;
}

async function play(bytes) {
  let from = 0;
  for (const m of marks(bytes)) {
    await paint(bytes.subarray(from, m.at));
    screen.term.resize(m.cols, m.rows);
    from = m.end;
  }
  await paint(bytes.subarray(from));
}

/** The size that fits the pane. */
function proposed() {
  const d = screen.fit.proposeDimensions();
  return d && d.cols > 1 && d.rows > 1 ? { cols: d.cols, rows: d.rows } : { cols: 100, rows: 30 };
}

function note(text, again) {
  screen.io.note(text, again);
}

/** Shows thread `id`'s screen: the agent's interface starts if it is not running. */
export async function attach(id) {
  if (screen.thread === id) return;
  if (screen.thread) screen.io.fire('leave', { thread_id: screen.thread });
  screen.thread = id;
  screen.ready = false;
  screen.queue = [];
  note('');
  const want = proposed();
  screen.asked = want;
  const reply = await screen.io.request('attach', { thread_id: id, rows: want.rows, cols: want.cols });
  if (screen.thread !== id) return;
  if (reply.error || !reply.ok) {
    note(reply.error || 'desl could not show this thread.', true);
    return;
  }
  const { replay, rows, cols } = reply.ok;
  screen.term.reset();
  screen.term.resize(cols || want.cols, rows || want.rows);
  await play(fromB64(replay || ''));
  if (screen.thread !== id) return;
  // The size the terminal has now: a TUI's, when one shows the thread too.
  screen.term.resize(cols || want.cols, rows || want.rows);
  for (const bytes of screen.queue.splice(0)) await paint(bytes);
  screen.ready = true;
}

/** The page stops showing the screen: the daemon no longer sizes the terminal for it. */
export function detach() {
  if (screen.thread) screen.io.fire('leave', { thread_id: screen.thread });
  screen.thread = null;
  screen.ready = false;
}

/** The connection went: what it showed went with it, and the next one attaches again. */
export function lost() {
  screen.thread = null;
  screen.ready = false;
}

export function shown() {
  return screen.thread;
}

/** A frame from the daemon about a terminal; true when it was this screen's. */
export function frame(msg) {
  if (!screen.term || msg.thread_id !== screen.thread) return false;
  switch (msg.type) {
    case 'pty': {
      const bytes = fromB64(msg.data);
      if (screen.ready) screen.term.write(bytes);
      else screen.queue.push(bytes);
      return true;
    }
    case 'pty_size':
      if (msg.rows > 0 && msg.cols > 0) screen.term.resize(msg.cols, msg.rows);
      return true;
    case 'pty_replace':
      // The thread moved to another agent: its screen is the new agent's.
      replaced(fromB64(msg.data));
      return true;
    case 'pty_ended': {
      screen.ready = false;
      note(`${screen.io.who()} ended. Start it again on the same conversation, or pick another thread.`, true);
      return true;
    }
    default:
      return false;
  }
}

async function replaced(bytes) {
  screen.ready = false;
  screen.term.reset();
  await play(bytes);
  screen.ready = true;
}

// xterm.js answers the questions an agent asks its terminal by itself. The daemon answers them
// for a page (docs/protocol.md, "A client that says hello as web"), so these never reach the
// agent from here, and it hears each answer once.
const REPLIES = [
  /^\x1b\[\??\d+;\d+R$/, // the cursor's place
  /^\x1b\[[?>=][\d;]*c$/, // what kind of terminal
  /^\x1b\[\d*n$/, // its state
  /^\x1b\[\??[\d;]+\$y$/, // a mode's state
  /^\x1b\][\d;]+[^\x07\x1b]*(\x07|\x1b\\)$/, // a colour
  /^\x1bP[\s\S]*\x1b\\$/, // its name and settings
  /^\x1b\[\?\d+u$/, // its keyboard flags
  /^\x1b\[\d+;\d+;\d+t$/, // its size
];

/** An answer xterm.js gave by itself, which the page drops. */
export function reply(text) {
  return REPLIES.some((r) => r.test(text));
}

/** Bytes for the agent, as the terminal would send them. */
export function send(bytes) {
  if (!screen.ready || !screen.thread) return;
  screen.io.fire('input', { thread_id: screen.thread, data: toB64(bytes) });
}

function typed(bytes, text) {
  if (reply(text)) return;
  send(bytes);
}

/** The pane changed size: ask for the size that fits it, a moment after it settles. */
export function refit() {
  clearTimeout(screen.timer);
  screen.timer = setTimeout(() => {
    if (!screen.thread || !screen.ready) return;
    const want = proposed();
    if (screen.asked && want.cols === screen.asked.cols && want.rows === screen.asked.rows) return;
    screen.asked = want;
    screen.io.fire('resize', { thread_id: screen.thread, rows: want.rows, cols: want.cols });
  }, 120);
}

export function focus() {
  if (screen.term) screen.term.focus();
}

/**
 * A paste the browser gives as a picture alone, as a screenshot is: xterm.js would send nothing.
 * All paste shortcuts use the browser's paste event, which tells us whether it holds text
 * or a picture (ctrl+v, ctrl+shift+v, cmd+v, shift+insert, the browser's menu).
 */
export function pictureOnly(data) {
  if (!data || data.getData('text/plain')) return false;
  return [...(data.items || [])].some((i) => i.kind === 'file' && i.type.startsWith('image/'));
}

/** The agent gets ctrl+v for a pasted picture and reads the clipboard itself, as in a terminal. */
function pasted(e) {
  if (!pictureOnly(e.clipboardData)) return;
  e.preventDefault();
  e.stopPropagation();
  if (screen.io.agent()) send(new Uint8Array([0x16]));
}

/** A key the screen keeps for the page: a selection is copied with ctrl+c or ctrl+shift+c. */
function copied(e) {
  if (e.type !== 'keydown' || !e.ctrlKey || e.altKey || e.metaKey || e.key.toLowerCase() !== 'c') return false;
  if (!screen.term.hasSelection()) return false;
  const text = screen.term.getSelection();
  screen.term.clearSelection();
  if (navigator.clipboard) navigator.clipboard.writeText(text).catch(() => {});
  e.preventDefault();
  return true;
}

/**
 * Draws the screen in `into`, watching `pane` for its size. `io.keys(e)` sees each key first and
 * returns true for one the page takes (its prefix key).
 */
export function setupScreen(into, pane, io) {
  screen.io = io;
  const term = new Terminal({
    fontFamily: '"JetBrainsMono Nerd Font", "JetBrainsMono NF", "Symbols Nerd Font Mono", "JetBrains Mono", ui-monospace, Menlo, monospace',
    fontSize: 13,
    scrollback: 5000,
    cursorBlink: false,
    theme: {
      background: '#0b0c10',
      foreground: '#d6d9e0',
      cursor: '#d6d9e0',
      selectionBackground: 'rgba(106, 166, 255, 0.35)',
    },
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.open(into);
  // Before xterm.js's own listeners, on the textarea and the terminal inside `into`.
  into.addEventListener('paste', pasted, true);
  term.attachCustomKeyEventHandler((e) => {
    if (copied(e) || io.keys(e)) return false;
    // Let the browser paste. xterm.js otherwise turns ctrl+v into 0x16, which agents treat
    // as "paste an image" even when the clipboard holds text. Do not prevent the default:
    // the resulting paste event lets xterm.js bracket text, or pasted() handle a picture.
    if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === 'v') return false;
    return true;
  });
  term.onData((text) => typed(new TextEncoder().encode(text), text));
  term.onBinary((text) => typed(Uint8Array.from(text, (c) => c.charCodeAt(0) & 0xff), text));
  screen.term = term;
  screen.fit = fit;
  new ResizeObserver(refit).observe(pane);
}

/** Starts the agent again on its conversation, after it ended. */
export function again() {
  const id = screen.thread;
  screen.thread = null;
  if (id) attach(id);
}

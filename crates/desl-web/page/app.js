// desl web's page (web-spec.md). It speaks the daemon's protocol through desl web's bridge and
// draws the `view` the bridge sends: the projects and their agent threads as desl itself words
// them for the terminal, and the selected thread's conversation, its highlights or its agent's own
// screen (screen.js). Text from agents only ever goes into the page as text nodes. Forms an agent
// puts in front of the user are drawn as cards in its conversation (forms.js).

import { card } from '/forms.js';
import * as screen from '/screen.js';
import * as pop from '/popover.js';
import { MORE, SEGMENTS, counts, drawList, icon } from '/tasks.js';
import { attend, setupAttention } from '/attention.js';

const KEY = 'desl-web-key';
const SELECTED = 'desl-web-thread';
const SVG = 'http://www.w3.org/2000/svg';

const $ = (id) => document.getElementById(id);

const state = {
  view: null,
  selected: read(SELECTED),
  ws: null,
  retry: 500,
  nextId: 1,
  pending: new Map(),
  /** Each project's task list by its id, as the daemon last told it. */
  tasks: new Map(),
  /** The projects whose list was asked for on this connection. */
  tasksAsked: new Set(),
  /** Ready tasks whose merge was refused because main moved on: `project#task`. */
  needsRebase: new Set(),
  /** The agents installed and logged in, for the task panel's Agent list. */
  agents: [],
  /** The last pick in the task panel: the agent ('' for the project's default) and the place. */
  startWith: Object.assign({ agent: '', place: 'worktree' }, readJson('desl-web-start')),
  /** The task column's filters: project (`auto` follows the selected thread), status, more, find. */
  tf: Object.assign({ project: 'auto', status: 'open', more: '', find: '' }, readJson('desl-web-tasks')),
  /** The prefix key as config.toml sets it, the user's shell and file manager (desl web says). */
  prefix: 'ctrl+space',
  shell: 'shell',
  fileManager: null,
  /** The thread shown before the one selected, for `prefix l`. */
  previous: null,
  /** A thread just made here, selected before the view lists it. */
  awaiting: null,
  /** The prefix was pressed: the next key is the page's. */
  armed: false,
  /** The directory lists asked for, by id. */
  dirs: new Map(),
};

function readJson(name) {
  try {
    return JSON.parse(localStorage.getItem(name) || '{}');
  } catch (e) {
    return {};
  }
}

function read(name) {
  try { return sessionStorage.getItem(name); } catch { return null; }
}

function write(name, value) {
  try { sessionStorage.setItem(name, value); } catch { /* a private window: the page works without it */ }
}

/** The key from the address desl web printed, kept for this tab and taken out of the address bar. */
function takeKey() {
  const m = location.hash.match(/(?:^#|&)t=([0-9a-f]{32})\b/);
  // `prefix v` names the thread on the terminal's screen: the page opens on it, at its open form
  // when it has one.
  const on = location.hash.match(/&thread=([\w/-]+)/);
  if (on) {
    state.selected = on[1];
    state.toForm = true;
    write(SELECTED, on[1]);
  }
  if (m) {
    write(KEY, m[1]);
    history.replaceState(null, '', location.pathname + location.search);
    return m[1];
  }
  return read(KEY);
}

function el(tag, className, text) {
  const e = document.createElement(tag);
  if (className) e.className = className;
  if (text !== undefined && text !== null) e.textContent = text;
  return e;
}

// The agents' marks, drawn as lines. Unknown kinds get a diamond.
const MARKS = {
  claude: [['path', { d: 'M12 3v18M3 12h18M5.6 5.6l12.8 12.8M18.4 5.6L5.6 18.4', 'stroke-width': '2.4' }]],
  codex: [['path', { d: 'M12 2.5l8.2 4.75v9.5L12 21.5l-8.2-4.75v-9.5z' }], ['path', { d: 'M9 10l2.5 2L9 14M13 14.5h3' }]],
  copilot: [
    ['rect', { x: '2.5', y: '7', width: '8.5', height: '8', rx: '3' }],
    ['rect', { x: '13', y: '7', width: '8.5', height: '8', rx: '3' }],
    ['path', { d: 'M11 11h2' }],
  ],
  tasks: [['rect', { x: '4', y: '4', width: '16', height: '16', rx: '3' }], ['path', { d: 'M8 12.5l3 3 5-6' }]],
  shell: [['path', { d: 'M4 17l5-5-5-5M11 19h9' }]],
  files: [['path', { d: 'M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z' }]],
  other: [['path', { d: 'M12 3l9 9-9 9-9-9z' }]],
};

function mark(agent, status, size = 15) {
  const kind = MARKS[agent] ? agent : 'other';
  const svg = document.createElementNS(SVG, 'svg');
  for (const [k, v] of Object.entries({
    width: size, height: size, viewBox: '0 0 24 24', fill: 'none', stroke: 'currentColor',
    'stroke-width': '2', 'stroke-linecap': 'round', 'stroke-linejoin': 'round', 'aria-hidden': 'true',
  })) svg.setAttribute(k, String(v));
  svg.setAttribute('class', `mark agent-${kind} ${status}`);
  for (const [tag, attrs] of MARKS[kind]) {
    const part = document.createElementNS(SVG, tag);
    for (const [k, v] of Object.entries(attrs)) part.setAttribute(k, v);
    svg.appendChild(part);
  }
  return svg;
}

function projectDot(hue) {
  const d = el('span', 'project-dot');
  d.setAttribute('aria-hidden', 'true');
  d.style.setProperty('--hue', `rgb(${hue[0]} ${hue[1]} ${hue[2]})`);
  return d;
}

// ---- the connection ---------------------------------------------------------------------------

function setLink(s, text) {
  $('link').dataset.state = s;
  $('link-text').textContent = text || s;
  render();
}

function connect(key) {
  setLink('connecting');
  const ws = new WebSocket(`ws://${location.host}/ws`);
  state.ws = ws;
  ws.onopen = () => ws.send(JSON.stringify({ token: key }));
  ws.onmessage = (e) => {
    let msg;
    try { msg = JSON.parse(e.data); } catch { return; }
    handle(msg);
  };
  ws.onclose = (e) => {
    state.ws = null;
    conv.thread = null;
    conv.asking = null;
    screen.lost();
    for (const [, done] of state.pending) done({ error: 'desl web lost the daemon before it answered. Send it again.' });
    state.pending.clear();
    if (e.code === 4401) {
      $('nokey').hidden = false;
      return;
    }
    setLink('reconnecting');
    setTimeout(() => connect(key), state.retry);
    state.retry = Math.min(state.retry * 2, 5000);
  };
}

function handle(msg) {
  switch (msg.type) {
    case 'view':
      // For the budget "page load to a usable thread list" (web-spec.md "Budgets").
      if (!state.view) performance.mark('desl-first-view');
      state.view = msg.view;
      state.fileManager = msg.file_manager || null;
      state.retry = 500;
      askTasks();
      if ($('link').dataset.state !== 'connected') setLink('connected');
      else render();
      break;
    case 'bridge':
      // desl web started again from a new install: this page is the old one.
      // A new connection asks for every task list again, and which agents there are.
      if (msg.state === 'ready') {
        state.tasksAsked.clear();
        askAgents();
        if (msg.prefix) state.prefix = msg.prefix;
        if (msg.shell) state.shell = msg.shell;
      }
      if (msg.state === 'ready' && msg.build) {
        if (state.build && state.build !== msg.build) location.reload();
        state.build = msg.build;
      }
      if (msg.state === 'no_daemon') setLink('no-daemon', 'no daemon');
      if (msg.error) notice(msg.error, true);
      break;
    case 'reply': {
      const done = state.pending.get(msg.id);
      if (done) {
        state.pending.delete(msg.id);
        done(msg);
      }
      break;
    }
    case 'notice':
      if (msg.thread_id === conv.thread || msg.thread_id === screen.shown()) notice(msg.text);
      break;
    case 'pty':
    case 'pty_size':
    case 'pty_replace':
    case 'pty_ended':
      screen.frame(msg);
      break;
    case 'conversation':
      conversationArrived(msg);
      break;
    case 'dirs': {
      const done = state.dirs.get(msg.id);
      state.dirs.delete(msg.id);
      if (done) done(msg.dirs || []);
      break;
    }
    case 'event':
      if (msg.change && msg.change.what === 'tasks') {
        state.tasks.set(msg.change.project_id, msg.change.tasks);
        drawTasks();
        break;
      }
      // Something was added to the log of the conversation on screen: read it again.
      if (msg.change && msg.change.what === 'appended' && msg.change.event && msg.change.event.thread_id === conv.thread) {
        // A form shown or closed is read at once (web-forms design, "Budgets"); the rest in a batch.
        const kind = msg.change.event.kind;
        conversationChanged(kind === 'form' || kind === 'form_answer');
      }
      break;
    default:
      break;
  }
}

/** Sends one request; resolves with the daemon's reply frame. */
function request(command, fields) {
  return new Promise((resolve) => {
    if (!state.ws || state.ws.readyState !== WebSocket.OPEN) {
      resolve({ error: 'desl web is not connected to the daemon yet. Try again in a moment.' });
      return;
    }
    const id = state.nextId++;
    state.pending.set(id, resolve);
    state.ws.send(JSON.stringify({ id, command, ...fields }));
  });
}

/** Sends one request the daemon never answers: keys, a size, leaving a screen. */
function fire(command, fields) {
  if (!state.ws || state.ws.readyState !== WebSocket.OPEN) return;
  state.ws.send(JSON.stringify({ id: state.nextId++, command, ...fields }));
}

// ---- drawing ----------------------------------------------------------------------------------

function threads() {
  return state.view ? state.view.projects.flatMap((p) => p.threads.map((t) => ({ t, p }))) : [];
}

function current() {
  const all = threads();
  let found = all.find(({ t }) => t.id === state.selected);
  if (found && state.awaiting === found.t.id) state.awaiting = null;
  // A thread made here is selected before the view lists it.
  if (!found && state.awaiting && state.awaiting === state.selected) return null;
  if (!found && all.length) {
    const waiting = state.view.waiting[0];
    found = all.find(({ t }) => t.id === waiting) || all[0];
    state.selected = found.t.id;
  }
  return found || null;
}

function select(id) {
  if (state.selected && state.selected !== id) state.previous = state.selected;
  state.selected = id;
  write(SELECTED, id);
  notice('');
  render();
}

function render() {
  // Opened on a thread with no form open: nothing to go to, so a form shown later does not take
  // the page and the keys from the user.
  if (state.toForm === true && state.view && !(state.view.forms || []).some((f) => f.thread_id === state.selected)) {
    state.toForm = null;
  }
  const cur = current();
  drawSide(cur);
  drawHead(cur);
  drawWaiting();
  drawTasks();
  drawChrome(cur);
  // The last entry says "working" while the agent is.
  if (shownView() !== conv.applied) applyView();
  else viewButtons(cur);
  if (shownView() === 'highlights') drawHighlights(false);
  const live = cur && $('link').dataset.state === 'connected';
  if (live && talks(cur.t) && cur.t.id !== conv.thread) readConversation(cur.t.id);
  if (live && shownView() === 'screen' && cur.t.id !== screen.shown()) showScreen(cur.t.id);
  // On the Screen the keys belong to the agent: when nothing holds them (a redraw took the
  // button that had them), they go back to it. Not at phone width, where that opens a keyboard.
  if (shownView() === 'screen' && !pop.isOpen() && document.activeElement === document.body && !NARROW.matches) screen.focus();
  composer();
}

/** A thread with a conversation: an agent's, not a shell's or a file manager's. */
function talks(t) {
  return !['shell', 'files', 'tasks'].includes(t.agent);
}

function drawSide(cur) {
  const nav = $('projects');
  nav.replaceChildren();
  if (!state.view) {
    nav.append(el('p', 'empty', 'Reading the workspace…'));
    return;
  }
  const open = el('button', 'side-open', 'Open a directory');
  open.type = 'button';
  open.title = `Open a directory as a project (${state.prefix} o)`;
  open.addEventListener('click', openDirectory);
  nav.append(open);
  if (!state.view.projects.length) {
    nav.append(el('p', 'empty', 'No threads yet. Open a directory to start one.'));
  }
  for (const p of state.view.projects) {
    const section = el('section', 'project');
    section.setAttribute('aria-label', p.name);
    const row = el('div', 'project-row');
    row.append(projectDot(p.hue), el('span', 'project-name', p.name));
    if (p.branch) row.append(el('span', 'branch', `⎇ ${p.branch}`));
    if (p.dirty > 0) row.append(el('span', 'dirty', `±${p.dirty}`));
    if (p.parent) row.append(el('span', 'parent', p.parent));
    const add = el('button', 'project-add', '+');
    add.type = 'button';
    add.title = `New thread in ${p.name} (${state.prefix} c)`;
    add.setAttribute('aria-label', `New thread in ${p.name}`);
    add.addEventListener('click', () => newThread(p, add));
    row.append(add);
    section.append(row);
    for (const t of p.threads) {
      const b = el('button', 'row');
      b.type = 'button';
      b.title = `${t.title} · ${t.word} · ${t.id}`;
      if (cur && cur.t.id === t.id) b.setAttribute('aria-current', 'true');
      b.append(mark(t.agent, t.status), el('span', 'title', t.title));
      if (t.worktree) {
        const wt = el('span', 'wt', '⎇');
        wt.title = 'Its own git worktree';
        b.append(wt);
      }
      if (t.age) b.append(el('span', 'age', t.age));
      // Its latest highlight, to catch up on every agent without opening one.
      if (t.latest) {
        const line = el('span', 'latest', t.latest);
        b.append(line);
        b.title = `${t.title} · ${t.word} · ${t.id}\n${t.latest}`;
      }
      b.addEventListener('click', () => select(t.id));
      section.append(b);
    }
    nav.append(section);
  }
}

function drawHead(cur) {
  const head = $('head');
  head.replaceChildren();
  if (!cur) return;
  const { t } = cur;
  const h = t.header;
  const left = el('div', 'head-left');
  left.append(mark(t.agent, t.status, 17), el('span', 'head-title', t.title), el('span', 'sep', '·'));
  const proj = el('span', 'chip-project');
  proj.append(projectDot(cur.p.hue), el('span', '', h.project));
  left.append(proj);
  if (h.branch) left.append(el('span', 'branch', `⎇ ${h.branch}`));
  if (h.dirty > 0) left.append(el('span', 'dirty', `±${h.dirty}`));
  if (h.model) left.append(el('span', 'sep', '·'), el('span', 'model', h.model));
  const right = el('div', 'head-right');
  // `⚠ yolo` says the mode itself, as the terminal's header does.
  if (h.yolo) right.append(el('span', 'yolo', '⚠ yolo'));
  right.append(el('span', `status-${t.status}`, `${t.glyph} ${t.word}`));
  if (!h.yolo) right.append(el('span', 'mode', h.mode));
  right.append(el('span', 'tid', t.id));
  const more = el('button', 'head-actions', '⋯');
  more.type = 'button';
  more.title = 'Switch, restart, fork, rename or close this thread';
  more.setAttribute('aria-label', 'Thread actions');
  more.addEventListener('click', () => actions(more));
  right.append(more);
  // Ends the turn, as esc does in the agent's own screen; there while there is a turn to end.
  if (t.status === 'working' && $('link').dataset.state === 'connected') {
    const b = el('button', 'stop', 'Stop');
    b.type = 'button';
    b.title = 'Stop this turn';
    b.addEventListener('click', stop);
    right.append(b);
  }
  head.append(left, right);
}

function drawWaiting() {
  const list = $('waitlist');
  list.replaceChildren();
  const forms = state.view ? state.view.forms || [] : [];
  // Threads whose agent asks in its own screen, a form open beside it or not: answered on its
  // Screen. A thread waiting on a form alone is answered on its card.
  const ids = state.view ? state.view.screen || [] : [];
  const count = forms.length + ids.length;
  $('wait-title').textContent = count ? `Waiting · ${count}` : 'Waiting';
  if (!count) {
    list.append(el('p', 'quiet', state.view ? 'Nothing is waiting for you.' : ''));
    return;
  }
  const all = threads();
  for (const f of forms) {
    const found = all.find(({ t }) => t.id === f.thread_id);
    const line = el('button', 'wait-line wait-form');
    line.type = 'button';
    line.setAttribute('aria-label', `Form: ${f.title}`);
    const top = el('span', 'wait-top');
    top.append(el('span', 'form-mark', '▣'), el('span', 'title', f.title), el('span', 'age', f.age));
    line.append(top, el('span', 'wait-where', `${found ? found.t.title : f.thread_id} · ${f.project}`));
    line.addEventListener('click', () => {
      state.toForm = f.form_id;
      if (state.selected === f.thread_id) goToForm();
      else select(f.thread_id);
    });
    list.append(line);
  }
  for (const id of ids) {
    const found = all.find(({ t }) => t.id === id);
    if (!found) continue;
    const { t, p } = found;
    // Its agent asks in its own screen: a click opens that screen, where it is answered.
    const line = el('button', 'wait-line');
    line.type = 'button';
    line.setAttribute('aria-label', `${t.title} is waiting in its own screen`);
    const top = el('span', 'wait-top');
    top.append(mark(t.agent, t.status, 13), el('span', 'title', `${t.title} · ${p.name}`));
    if (t.age) top.append(el('span', 'age', t.age));
    line.append(top, el('span', 'wait-where', 'Asks in its own screen: open it to answer.'));
    line.addEventListener('click', () => openScreen(t.id));
    list.append(line);
  }
}

function drawChrome(cur) {
  const ids = state.view ? state.view.waiting : [];
  const pill = $('waiting-pill');
  pill.hidden = ids.length === 0;
  pill.textContent = `${ids.length} waiting`;
  // The tab's title leads with what waits, each form and each agent asking in its screen; in
  // the background, something new blinks it (attention.js).
  let title = cur ? `desl · ${cur.t.agent} · ${cur.t.id}` : 'desl';
  if (cur && cur.t.header.yolo) title += ' · ⚠ yolo';
  const forms = state.view ? state.view.forms || [] : [];
  const screens = state.view ? state.view.screen || [] : [];
  attend(title, [...forms.map((f) => f.form_id), ...screens.map((id) => `screen:${id}`)]);
  const ready = Boolean(cur) && $('link').dataset.state === 'connected';
  const input = $('prompt');
  input.disabled = !ready;
  input.placeholder = cur ? `Prompt to ${cur.t.title} (${cur.t.agent}). Queued while it works.` : 'Pick a thread';
  $('send').disabled = !ready;
}

// On the Screen the agent's own input takes the keys: the page's prompt shows only where there is
// none, under the Conversation and the Highlights, and at phone width, where a terminal is hard to
// type in.
const NARROW = window.matchMedia('(max-width: 899px)');

function composer() {
  const form = shownView() !== 'screen' || NARROW.matches;
  $('prompt-form').hidden = !form;
  $('composer').hidden = !form && !$('notice').textContent;
}

let noticeTimer = null;

/** A sentence under the thread, for a few seconds: longer when something went wrong. */
function notice(text, error = false) {
  const n = $('notice');
  n.textContent = text;
  n.classList.toggle('error', error);
  clearTimeout(noticeTimer);
  if (text) noticeTimer = setTimeout(() => notice(''), error ? 15000 : 8000);
  composer();
}

// ---- the agent's screen (screen.js) -----------------------------------------------------------

function screenNote(text, again) {
  $('screen-note').hidden = !text;
  $('screen-note-text').textContent = text || '';
  $('screen-again').hidden = !again;
}

/** Thread `id`'s screen, once the pane has its size; the keys go to it. */
function showScreen(id) {
  requestAnimationFrame(() => {
    if (shownView() !== 'screen' || state.selected !== id) return;
    screen.attach(id).then(() => {
      if (shownView() === 'screen' && !pop.isOpen() && document.activeElement === document.body) screen.focus();
    });
    if (!pop.isOpen()) screen.focus();
  });
}

/** A waiting thread's own screen, with the keys: it is answered there. */
function openScreen(id) {
  if (state.selected !== id) select(id);
  setView('screen');
}

function setupScreenPane() {
  screen.setupScreen($('screen'), $('screen-pane'), {
    request,
    fire,
    note: screenNote,
    keys: () => false,
    // A pasted picture is for an agent; a shell gets nothing.
    agent: () => {
      const cur = current();
      return Boolean(cur) && talks(cur.t);
    },
    who: () => {
      const cur = current();
      return cur ? (talks(cur.t) ? agentName(cur.t.agent) : cur.t.title) : 'The agent';
    },
  });
  $('screen-again').addEventListener('click', () => screen.again());
  // A box over the page that closes gives the keys back to the agent when it is on screen.
  pop.onLost(() => {
    if (shownView() === 'screen') screen.focus();
  });
  NARROW.addEventListener('change', composer);
}

// ---- the conversation --------------------------------------------------------------------------

const conv = {
  /** The thread whose conversation is drawn, or asked for. */
  thread: null,
  items: [],
  /** Its story in short questions and answers (docs/design/2026-11-22-highlights.md). */
  highlights: [],
  /** The view shown now, which is the Screen for a shell whatever `view` says. */
  applied: null,
  /** `conversation`, `highlights` or `screen`, as the user left it. */
  view: ['highlights', 'screen'].includes(readJson('desl-web-view').view) ? readJson('desl-web-view').view : 'conversation',
  /** The request in flight, by its id. */
  asking: null,
  timer: null,
  /** The search's matches, and which of them is shown. */
  found: [],
  at: -1,
};

function readConversation(id) {
  if (!state.ws || state.ws.readyState !== WebSocket.OPEN) return;
  if (conv.thread !== id) {
    conv.thread = id;
    conv.items = [];
    conv.highlights = [];
    drawConversation(true);
    drawHighlights(true);
  }
  const reqId = state.nextId++;
  conv.asking = reqId;
  state.ws.send(JSON.stringify({ type: 'conversation', id: reqId, thread_id: id }));
}

function conversationArrived(msg) {
  if (msg.id !== conv.asking || msg.thread_id !== conv.thread) return;
  conv.asking = null;
  if (msg.error) {
    notice(msg.error, true);
    return;
  }
  const list = $('conv');
  const atEnd = list.scrollHeight - list.scrollTop - list.clientHeight < 40 || !conv.items.length;
  const high = $('highlights');
  const highAtEnd = high.scrollHeight - high.scrollTop - high.clientHeight < 40 || !conv.highlights.length;
  conv.items = msg.items || [];
  conv.highlights = msg.highlights || [];
  drawConversation(atEnd);
  drawHighlights(highAtEnd);
}

const ASKED_BY = { you: 'You', task: 'Task', form: 'Form', desl: 'desl', agent: 'Agent' };

/**
 * The thread's story: each entry who asked and when, the question, and the answer in at most two
 * sentences; the last one still being answered while the agent works. A click opens the
 * conversation at its moment.
 */
function drawHighlights(toEnd) {
  const list = $('highlights');
  const top = list.scrollTop;
  if (!conv.highlights.length) {
    list.replaceChildren(el('p', 'empty', conv.asking ? 'Reading the conversation…' : 'Nothing yet. Each question and its answer shows here.'));
    return;
  }
  const cur = current();
  const working = cur && cur.t.id === conv.thread && cur.t.status === 'working';
  const nodes = conv.highlights.map((h, i) => {
    const last = i === conv.highlights.length - 1;
    const node = el('button', `hl by-${h.by}${last && working ? ' running' : ''}`);
    node.type = 'button';
    node.title = 'Open the conversation here';
    node.append(el('div', 'hl-who', `${ASKED_BY[h.by] || h.by} · ${clock(h.at)}`));
    const q = el('div', 'hl-q');
    q.append(el('span', 'hl-mark', 'Q'), el('span', 'hl-text', h.question));
    const a = el('div', 'hl-a');
    const answer = h.answer || (last && working ? 'Working on it…' : 'No answer.');
    a.append(el('span', 'hl-mark', 'A'), el('span', 'hl-text', answer));
    node.append(q, a);
    node.addEventListener('click', () => openAt(h.event));
    return node;
  });
  list.replaceChildren(...nodes);
  list.scrollTop = toEnd ? list.scrollHeight : top;
}

/** Shows the conversation, at the first item from event `event` on. */
function openAt(event) {
  setView('conversation');
  const items = [...document.querySelectorAll('#conv .item')];
  const target = items.find((n) => Number(n.dataset.event) >= event) || items[items.length - 1];
  if (!target) return;
  target.scrollIntoView({ block: 'start' });
  target.classList.add('flash');
  setTimeout(() => target.classList.remove('flash'), 1600);
}

const VIEWS = { conversation: 'view-conv', highlights: 'view-high', screen: 'view-screen' };

/** A shell or a file manager has no conversation: only its Screen can be picked. */
function viewButtons(cur) {
  const quiet = Boolean(cur) && !talks(cur.t);
  $('view-conv').disabled = quiet;
  $('view-high').disabled = quiet;
}

/** The view the user picked, or the Screen for a thread with no conversation. */
function shownView() {
  const cur = current();
  return cur && !talks(cur.t) ? 'screen' : conv.view;
}

function setView(view) {
  conv.view = view;
  try {
    localStorage.setItem('desl-web-view', JSON.stringify({ view }));
  } catch (e) {
    // The choice is only for this visit, then.
  }
  applyView();
}

/** Shows the view in effect: the panes, the tools over them, and the screen attached or not. */
function applyView() {
  const view = shownView();
  conv.applied = view;
  const cur = current();
  viewButtons(cur);
  const high = view === 'highlights';
  const term = view === 'screen';
  $('conv').hidden = view !== 'conversation';
  $('highlights').hidden = !high;
  $('screen-pane').hidden = !term;
  for (const id of ['conv-find', 'conv-count', 'conv-toggle']) $(id).hidden = view !== 'conversation';
  $('screen-hint').hidden = !term;
  for (const [v, id] of Object.entries(VIEWS)) {
    $(id).classList.toggle('on', v === view);
    $(id).setAttribute('aria-pressed', String(v === view));
  }
  if (high) drawHighlights(true);
  if (term) {
    screen.refit();
    if (cur && $('link').dataset.state === 'connected') showScreen(cur.t.id);
  } else {
    // Out of sight, the agent's terminal is sized by whoever else shows it.
    screen.detach();
    screenNote('');
  }
  composer();
}

function conversationChanged(now = false) {
  clearTimeout(conv.timer);
  conv.timer = setTimeout(() => {
    if (conv.thread) readConversation(conv.thread);
  }, now ? 0 : 600);
}

/** Inline Markdown as nodes: code, bold, italics and links. Never HTML. */
function inline(text, into) {
  const re = /(`[^`\n]+`)|(\*\*[^*\n]+\*\*)|(\*[^*\s][^*\n]*\*)|(\[[^\]\n]+\]\([^)\s]+\))/g;
  let last = 0;
  for (const m of text.matchAll(re)) {
    if (m.index > last) into.append(document.createTextNode(text.slice(last, m.index)));
    const t = m[0];
    if (m[1]) into.append(el('code', '', t.slice(1, -1)));
    else if (m[2]) into.append(el('strong', '', t.slice(2, -2)));
    else if (m[3]) into.append(el('em', '', t.slice(1, -1)));
    else {
      const [, label, href] = t.match(/^\[([^\]]+)\]\(([^)]+)\)$/);
      if (/^https?:\/\//.test(href)) {
        const a = el('a', '', label);
        a.href = href;
        a.target = '_blank';
        a.rel = 'noopener noreferrer';
        into.append(a);
      } else {
        into.append(document.createTextNode(label));
      }
    }
    last = m.index + t.length;
  }
  if (last < text.length) into.append(document.createTextNode(text.slice(last)));
}

/** A reply's Markdown as nodes: paragraphs, headings, lists, code blocks, quotes. Never HTML. */
function markdown(text) {
  const out = document.createDocumentFragment();
  const lines = text.split('\n');
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    const fence = line.match(/^\s*(`{3,}|~{3,})/);
    if (fence) {
      // Closed by a run of the same mark, at least as long, alone on its line.
      const close = (l) => {
        const t = l.trim();
        return t.length >= fence[1].length && [...t].every((c) => c === fence[1][0]);
      };
      const code = [];
      i++;
      while (i < lines.length && !close(lines[i])) code.push(lines[i++]);
      i++;
      const pre = el('pre');
      pre.append(el('code', '', code.join('\n')));
      out.append(pre);
      continue;
    }
    const h = line.match(/^(#{1,6})\s+(.*)$/);
    if (h) {
      const head = el(`h${Math.min(h[1].length + 1, 4)}`);
      inline(h[2], head);
      out.append(head);
      i++;
      continue;
    }
    if (/^\s*([-*+]|\d+[.)])\s+/.test(line)) {
      const ordered = /^\s*\d+[.)]\s+/.test(line);
      const list = el(ordered ? 'ol' : 'ul');
      while (i < lines.length && /^\s*([-*+]|\d+[.)])\s+/.test(lines[i])) {
        const li = el('li');
        inline(lines[i].replace(/^\s*([-*+]|\d+[.)])\s+/, ''), li);
        list.append(li);
        i++;
      }
      out.append(list);
      continue;
    }
    if (/^>\s?/.test(line)) {
      const quote = el('blockquote');
      const body = [];
      while (i < lines.length && /^>\s?/.test(lines[i])) body.push(lines[i++].replace(/^>\s?/, ''));
      inline(body.join(' '), quote);
      out.append(quote);
      continue;
    }
    if (/^\s*\|/.test(line)) {
      // A table stays as it was written, in a fixed-width block.
      const rows = [];
      while (i < lines.length && /^\s*\|/.test(lines[i])) rows.push(lines[i++]);
      const pre = el('pre');
      pre.append(el('code', '', rows.join('\n')));
      out.append(pre);
      continue;
    }
    if (!line.trim()) {
      i++;
      continue;
    }
    const para = [];
    while (i < lines.length && lines[i].trim() && !/^(#{1,6}\s|\s*```|\s*~~~|\s*([-*+]|\d+[.)])\s|>|\s*\|)/.test(lines[i])) para.push(lines[i++]);
    // A line break in a reply stays one: agents break lines on purpose.
    const p = el('p');
    para.forEach((l, k) => {
      if (k) p.append(el('br'));
      inline(l, p);
    });
    out.append(p);
  }
  return out;
}

function lines(block, into, diff) {
  const pre = el('pre');
  for (const l of block.text.split('\n')) {
    const kind = diff && l.startsWith('+') && !l.startsWith('+++') ? 'add' : diff && l.startsWith('-') && !l.startsWith('---') ? 'del' : '';
    pre.append(el('span', kind, `${l}\n`));
  }
  if (block.more) pre.append(el('span', 'more', `… ${block.more} more lines`));
  into.append(pre);
}

function clock(ms) {
  if (!ms) return '';
  const d = new Date(ms);
  return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

function drawConversation(toEnd) {
  const list = $('conv');
  // A field the user is typing in keeps the keys, the caret and the place on screen: the cards
  // are put back as they were, and nothing else moves.
  const active = list.contains(document.activeElement) ? document.activeElement : null;
  const caret = active && typeof active.selectionStart === 'number' ? [active.selectionStart, active.selectionEnd] : null;
  const top = list.scrollTop;
  if (!conv.items.length) {
    list.replaceChildren(el('p', 'empty', conv.asking ? 'Reading the conversation…' : 'Nothing yet.'));
    return;
  }
  const nodes = [];
  for (const item of conv.items) {
    let node;
    switch (item.kind) {
      case 'rule':
        node = el('div', 'item item-rule', item.text);
        break;
      case 'user': {
        node = el('div', 'item item-user');
        node.append(el('span', 'caret', '›'), el('span', 'text', item.text));
        if (item.at) node.append(el('span', 'at', clock(item.at)));
        break;
      }
      case 'assistant':
        node = el('div', 'item item-assistant');
        node.append(markdown(item.text));
        break;
      case 'tool': {
        node = el('details', 'item item-tool');
        const sum = el('summary');
        const outcome = el('span', item.ok === false ? 'outcome bad' : 'outcome', item.summary || '');
        sum.append(el('span', 'verb', item.verb), el('span', 'target', item.target), outcome);
        node.append(sum);
        if (item.diff) lines(item.diff, node, true);
        else if (item.detail && item.detail.text.trim()) lines(item.detail, node, false);
        else node.append(el('p', 'more', item.summary ? '(no output)' : '(no output yet)'));
        break;
      }
      case 'brief':
        node = el('details', 'item item-brief');
        node.append(el('summary', '', `${item.label} · ~${item.tokens} tokens`));
        lines({ text: item.text, more: 0 }, node, false);
        break;
      case 'error':
        node = el('div', 'item item-error', `✕ ${item.text}`);
        break;
      case 'form':
        node = formNode(item);
        break;
      default:
        node = el('div', 'item item-note', item.text);
        break;
    }
    // Which event it shows, for a highlight's link to it.
    if (item.event !== undefined) node.dataset.event = String(item.event);
    nodes.push(node);
  }
  place(list, nodes);
  findAgain(false);
  if (toEnd) list.scrollTop = list.scrollHeight;
  else list.scrollTop = top;
  if (active && active.isConnected) {
    active.focus({ preventScroll: true });
    if (caret) active.setSelectionRange(caret[0], caret[1]);
  }
  goToForm();
}

/**
 * Puts `nodes` in `list` in order, leaving each one already there where it is: a form card is
 * never taken off the page, so a preview's frame does not load again and a drag in it goes on.
 */
function place(list, nodes) {
  const keep = new Set(nodes);
  for (const child of [...list.childNodes]) if (!keep.has(child)) child.remove();
  let at = list.firstChild;
  for (const n of nodes) {
    if (n === at) {
      at = at.nextSibling;
      continue;
    }
    list.insertBefore(n, at);
  }
}

// ---- tasks (docs/design/2026-11-22-tasks-first-class.md) ---------------------------------------

/** Asks for the task list of each project that has one and was not asked for yet. */
function askTasks() {
  for (const p of (state.view && state.view.task_lists) || []) {
    if (state.tasksAsked.has(p.id)) continue;
    state.tasksAsked.add(p.id);
    request('tasks', { project_id: p.id }).then((reply) => {
      if (reply.ok && reply.ok.tasks) {
        state.tasks.set(p.id, reply.ok.tasks);
        drawTasks();
      }
    });
  }
}

function saveFilters() {
  try {
    localStorage.setItem('desl-web-tasks', JSON.stringify(state.tf));
  } catch (e) {
    /* A private window keeps no filters; the column works the same. */
  }
}

/** The projects the column shows: the selected thread's, one picked, or all. */
function shownLists() {
  const lists = (state.view && state.view.task_lists) || [];
  let pick = lists;
  if (state.tf.project === 'auto') {
    const cur = current();
    pick = lists.filter((p) => cur && p.id === cur.p.id);
    if (!pick.length) pick = lists.slice(0, 1);
  } else if (state.tf.project !== 'all') {
    pick = lists.filter((p) => p.id === state.tf.project);
  }
  return pick.map((project) => ({ project, tasks: state.tasks.get(project.id) || [] }));
}

function drawTasks() {
  if (!state.view) return;
  const lists = (state.view.task_lists) || [];
  $('tasks').hidden = !lists.length;
  if (!lists.length) return;
  // The project: the selected thread's, one by name, or all of them.
  const sel = $('t-project');
  const cur = current();
  const here = cur ? lists.find((p) => p.id === cur.p.id) : null;
  const options = [['auto', here ? `This thread's project: ${here.name}` : "This thread's project"], ...lists.map((p) => [p.id, p.name]), ['all', 'All projects']];
  sel.replaceChildren(...options.map(([value, label]) => {
    const o = document.createElement('option');
    o.value = value;
    o.textContent = label;
    return o;
  }));
  sel.value = options.some(([v]) => v === state.tf.project) ? state.tf.project : 'auto';
  const shown = shownLists();
  const n = counts(shown, state.tf.find);
  // The status, one segment on unless a view under More is.
  const seg = $('t-seg');
  seg.replaceChildren(...SEGMENTS.map(([id, label]) => {
    const b = document.createElement('button');
    b.type = 'button';
    b.className = !state.tf.more && state.tf.status === id ? 'on' : '';
    b.setAttribute('aria-pressed', String(b.className === 'on'));
    b.append(document.createTextNode(label), Object.assign(document.createElement('b'), { textContent: String(n[id]) }));
    b.addEventListener('click', () => {
      state.tf.status = id;
      state.tf.more = '';
      saveFilters();
      drawTasks();
    });
    return b;
  }));
  const more = $('t-more');
  more.replaceChildren(...[['', 'More'], ...MORE.map(([id, label]) => [id, `${label} · ${n[id]}`])].map(([value, label]) => {
    const o = document.createElement('option');
    o.value = value;
    o.textContent = label;
    return o;
  }));
  more.value = state.tf.more;
  if ($('t-find').value !== state.tf.find && document.activeElement !== $('t-find')) $('t-find').value = state.tf.find;
  drawList($('tasklist'), shown, state.tf, {
    thread: (id) => {
      const found = threads().find(({ t }) => t.id === id);
      return found ? { title: found.t.title, agent: found.t.agent } : null;
    },
    select: (id) => select(id),
    action: taskAction,
  });
}

function askAgents() {
  request('agents', { refresh: false }).then((reply) => {
    const all = (reply.ok && reply.ok.agents) || [];
    state.agents = all.filter((a) => a.detection && a.detection.installed && a.detection.logged_in !== false).map((a) => a.id);
  });
}

const AGENT_NAME = { claude: 'Claude', codex: 'Codex', copilot: 'Copilot' };
const agentName = (id) => AGENT_NAME[id] || id;

/** Puts an agent on a task, as `with` says; a sentence under the page says how it went. */
async function startTask(t, project, withPick) {
  const reply = await request('task_start', { project_id: project.id, task: t.id, agent: withPick.agent || null, place: withPick.place });
  if (reply.error) {
    notice(reply.error, true);
    return;
  }
  const task = reply.ok.task;
  const where = task.branch ? `, in its own worktree on ${task.branch}` : '';
  notice(`Started #${t.id} in ${task.thread}${where}. Its row links to the thread.`);
}

/** The place the panel offers for the selected thread, when it is an agent's. */
function selectedPlace() {
  const cur = current();
  return cur ? { value: `thread:${cur.t.id}`, label: `The selected thread: ${cur.t.title}` } : null;
}

/** The panel under ▾: the agent and the place, picked apart, remembered for next time. */
function openStartPanel(t, project, anchor) {
  for (const old of document.querySelectorAll('.tpanel')) old.remove();
  const panel = document.createElement('div');
  panel.className = 'tpanel';
  panel.setAttribute('role', 'dialog');
  panel.setAttribute('aria-label', `Start #${t.id}`);
  panel.append(el('div', 'tp-title', `Start #${t.id} with`));
  panel.append(el('div', 'tp-label', 'Agent'));
  const agent = document.createElement('select');
  agent.setAttribute('aria-label', 'Agent');
  for (const [value, label] of [['', "The project's default"], ...state.agents.map((a) => [a, agentName(a)])]) {
    const o = document.createElement('option');
    o.value = value;
    o.textContent = label;
    agent.append(o);
  }
  agent.value = state.agents.includes(state.startWith.agent) ? state.startWith.agent : '';
  panel.append(agent);
  panel.append(el('div', 'tp-label', 'Where'));
  const here = selectedPlace();
  const places = [
    ['worktree', 'A new thread, in its own worktree'],
    ['main', 'A new thread, in the main checkout'],
    ...(here ? [[here.value, here.label]] : []),
  ];
  const name = `tp-${t.id}`;
  const chosen = places.some(([v]) => v === state.startWith.place) ? state.startWith.place : 'worktree';
  for (const [value, label] of places) {
    const row = el('label', 'tp-place');
    const r = document.createElement('input');
    r.type = 'radio';
    r.name = name;
    r.value = value;
    r.checked = value === chosen || (value.startsWith('thread:') && state.startWith.place === 'thread');
    row.append(r, document.createTextNode(` ${label}`));
    panel.append(row);
  }
  const bar = el('div', 'tp-bar');
  const go = el('button', 'primary', 'Start');
  go.type = 'button';
  go.addEventListener('click', () => {
    const place = panel.querySelector(`input[name="${name}"]:checked`).value;
    // A thread is remembered as "the selected one", whichever that is next time.
    state.startWith = { agent: agent.value, place: place.startsWith('thread:') ? 'thread' : place };
    try {
      localStorage.setItem('desl-web-start', JSON.stringify(state.startWith));
    } catch (e) {
      /* remembered until the tab closes */
    }
    panel.remove();
    startTask(t, project, { agent: agent.value, place });
  });
  const cancel = el('button', 'secondary', 'Cancel');
  cancel.type = 'button';
  cancel.addEventListener('click', () => panel.remove());
  bar.append(cancel, go);
  panel.append(bar);
  panel.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') panel.remove();
  });
  anchor.closest('.trow').append(panel);
  agent.focus();
}

/** The action on a task's row: start work, or what its status says. */
function taskAction(t, project) {
  if (['todo', 'issue', 'parked'].includes(t.status)) {
    const split = el('span', 'split');
    const main = el('button', 'primary', t.type === 'bug' ? 'Fix now' : 'Work on it');
    main.type = 'button';
    const place = state.startWith.place === 'thread' ? (selectedPlace() || { value: 'worktree' }).value : state.startWith.place;
    const who = state.startWith.agent ? agentName(state.startWith.agent) : "the project's default agent";
    main.title = `Start #${t.id} on ${who}, ${place === 'main' ? 'in the main checkout' : place === 'worktree' ? 'in its own worktree' : 'in the selected thread'}`;
    main.addEventListener('click', () => startTask(t, project, { agent: state.startWith.agent, place }));
    const caret = el('button', 'primary split-more');
    caret.append(icon('down'));
    caret.type = 'button';
    caret.setAttribute('aria-label', `Pick the agent and the place for #${t.id}`);
    caret.addEventListener('click', () => openStartPanel(t, project, caret));
    split.append(main, caret);
    return split;
  }
  if (t.status === 'in-progress') return el('span', 'tstate working', '✻ in progress');
  if (t.status === 'ready') {
    const bar = el('span', 'tready');
    const key = `${project.id}#${t.id}`;
    if (state.needsRebase.has(key)) {
      const ask = el('button', 'secondary', 'Ask to rebase');
      ask.type = 'button';
      ask.title = 'Main moved on since its branch left it';
      ask.addEventListener('click', async () => {
        const reply = await request('task_rebase', { project_id: project.id, task: t.id });
        if (reply.error) return notice(reply.error, true);
        state.needsRebase.delete(key);
        notice(`Asked ${t.thread} to rebase #${t.id} onto main; it comes back ready.`);
      });
      bar.append(ask);
    }
    {
      // Merge stays: after a rebase made by hand it goes through.
      const merge = el('button', 'primary merge', 'Merge');
      merge.type = 'button';
      merge.title = `Fast-forward main to ${t.branch}`;
      merge.addEventListener('click', async () => {
        merge.disabled = true;
        const reply = await request('task_merge', { project_id: project.id, task: t.id });
        merge.disabled = false;
        if (reply.error) return notice(reply.error, true);
        if (reply.ok.reply === 'merge_refused') {
          if (reply.ok.rebase) state.needsRebase.add(key);
          else state.needsRebase.delete(key);
          notice(reply.ok.reason, true);
          drawTasks();
          return;
        }
        state.needsRebase.delete(key);
        notice(`Merged #${t.id}: ${t.branch} is in the main checkout's branch now, and #${t.id} is done.`);
      });
      bar.append(merge);
    }
    const diff = el('button', 'secondary', 'Diff');
    diff.type = 'button';
    diff.addEventListener('click', () => showDiff(t, project));
    bar.append(diff);
    return bar;
  }
  return null;
}

/** A task's changes against main, read-only, over the conversation. */
async function showDiff(t, project) {
  const reply = await request('task_diff', { project_id: project.id, task: t.id });
  if (reply.error) return notice(reply.error, true);
  $('diff-title').textContent = `#${t.id} ${t.title} · ${t.branch} against ${reply.ok.base}`;
  const body = $('diff-body');
  body.replaceChildren();
  const text = reply.ok.text || '(no change against main)';
  for (const l of text.split('\n')) {
    const kind = l.startsWith('+') && !l.startsWith('+++') ? 'add' : l.startsWith('-') && !l.startsWith('---') ? 'del' : l.startsWith('@@') ? 'hunk' : '';
    body.append(el('span', kind, `${l}\n`));
  }
  if (reply.ok.more) body.append(el('span', 'more', `… ${reply.ok.more} more lines`));
  $('diffview').hidden = false;
  $('diff-close').focus();
}

function setupTasks() {
  $('diff-close').addEventListener('click', () => {
    $('diffview').hidden = true;
  });
  $('diffview').addEventListener('keydown', (e) => {
    if (e.key === 'Escape') $('diffview').hidden = true;
  });
  $('t-project').addEventListener('change', (e) => {
    state.tf.project = e.target.value;
    saveFilters();
    drawTasks();
  });
  $('t-more').addEventListener('change', (e) => {
    state.tf.more = e.target.value;
    saveFilters();
    drawTasks();
  });
  $('t-find').addEventListener('input', (e) => {
    state.tf.find = e.target.value;
    saveFilters();
    drawTasks();
  });
}

// ---- forms ------------------------------------------------------------------------------------

/** Each form's card by its id, kept across redraws so what the user typed stays. */
const formCards = new Map();

/** The card for a conversation's form item: kept while the form is as it was, else asked for. */
function formNode(item) {
  const kept = formCards.get(item.form_id);
  if (kept && kept.by === item.by && kept.thread === conv.thread) return kept.node;
  const node = el('div', 'item item-form');
  node.append(el('p', 'more', `Reading the form ${item.form_id}…`));
  formCards.set(item.form_id, { by: item.by, thread: conv.thread, node });
  request('form_get', { form_id: item.form_id }).then((reply) => {
    if (reply.error || !reply.ok) {
      node.replaceChildren(el('p', 'item-error', reply.error || 'desl web could not read the form.'));
      return;
    }
    node.replaceChildren(formCard(reply.ok));
    goToForm();
  });
  return node;
}

function formCard(got) {
  // A fork shows its parent's forms as they were; they are answered in the thread that asked.
  const elsewhere = got.thread_id !== conv.thread;
  return card(got, {
    markdown,
    section: async (widget) => {
      const reply = await request('form_section', { form_id: got.form_id, widget });
      if (reply.error || !reply.ok) return { error: reply.error || 'desl web could not read the file.' };
      return { text: typeof reply.ok.text === 'string' ? reply.ok.text : null };
    },
    image: async (path) => {
      const reply = await request('form_image', { form_id: got.form_id, path });
      if (reply.error || !reply.ok) return { error: reply.error || 'desl web could not read the picture.' };
      return { url: `data:${reply.ok.mime};base64,${reply.ok.data}` };
    },
    readOnly: elsewhere,
    note: elsewhere ? `Asked in ${got.thread_id}, where it is answered.` : '',
    onDraft: (draft) => request('form_draft', { form_id: got.form_id, draft }),
    onSend: async (answer) => {
      const reply = await request('form_answer', { form_id: got.form_id, answer });
      if (reply.error) {
        // Also under the page: an answer kept but not delivered redraws the card as answered.
        notice(reply.error, true);
        return reply.error;
      }
      notice(`Sent your answer to ${got.form_id}; the agent gets it as its next prompt.`);
      return null;
    },
    onDismiss: async () => {
      const reply = await request('form_close', { form_id: got.form_id, by: 'dismissed' });
      if (reply.error) return reply.error;
      notice(`Dismissed ${got.form_id}; the agent hears so before its next prompt.`);
      return null;
    },
  });
}

/** After `prefix v` or a click in the waiting column: the form on screen, its first field focused. */
function goToForm() {
  if (!state.toForm) return;
  const cards = [...document.querySelectorAll('#conv .form-card')];
  const target = typeof state.toForm === 'string' ? cards.find((c) => c.dataset.form === state.toForm) : cards.find((c) => !c.classList.contains('done'));
  if (!target) return;
  state.toForm = null;
  target.scrollIntoView({ block: 'center' });
  target.classList.add('flash');
  setTimeout(() => target.classList.remove('flash'), 1600);
  const first = target.querySelector('input:not([disabled]), textarea, select');
  if (first) first.focus({ preventScroll: true });
}

/** The items that hold the search's text: letters in either case, unless it has a capital. */
function findAgain(move) {
  const q = $('conv-find').value;
  for (const n of document.querySelectorAll('#conv .found')) n.classList.remove('found');
  if (!q) {
    conv.found = [];
    conv.at = -1;
    $('conv-count').textContent = '';
    return;
  }
  const exact = /[A-Z]/.test(q);
  const needle = exact ? q : q.toLowerCase();
  conv.found = [...document.querySelectorAll('#conv .item')].filter((n) => {
    if (n.classList.contains('item-tool') && $('conv').classList.contains('no-tools')) return false;
    const text = exact ? n.textContent : n.textContent.toLowerCase();
    return text.includes(needle);
  });
  if (!conv.found.length) {
    conv.at = -1;
    $('conv-count').textContent = 'not found';
    return;
  }
  if (move || conv.at < 0 || conv.at >= conv.found.length) conv.at = Math.max(0, Math.min(conv.at, conv.found.length - 1));
  const hit = conv.found[conv.at];
  hit.classList.add('found');
  if (move) hit.scrollIntoView({ block: 'center' });
  $('conv-count').textContent = `${conv.at + 1} of ${conv.found.length}`;
}

function setupConversation() {
  $('view-conv').addEventListener('click', () => setView('conversation'));
  $('view-high').addEventListener('click', () => setView('highlights'));
  $('view-screen').addEventListener('click', () => setView('screen'));
  setView(conv.view);
  $('conv-tools').addEventListener('change', (e) => {
    $('conv').classList.toggle('no-tools', !e.target.checked);
    findAgain(false);
  });
  $('conv-find').addEventListener('input', () => {
    conv.at = -1;
    findAgain(true);
  });
  // enter goes on to the next match, shift+enter back; esc clears the search.
  $('conv-find').addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && conv.found.length) {
      e.preventDefault();
      conv.at = (conv.at + (e.shiftKey ? conv.found.length - 1 : 1)) % conv.found.length;
      findAgain(true);
    } else if (e.key === 'Escape') {
      $('conv-find').value = '';
      findAgain(false);
    }
  });
}

// ---- the prompt -------------------------------------------------------------------------------

async function sendPrompt(e) {
  e.preventDefault();
  const cur = current();
  const input = $('prompt');
  const text = input.value.trim();
  if (!cur || !text) return;
  const reply = await request('prompt', { thread_id: cur.t.id, text });
  if (reply.error) {
    notice(reply.error, true);
    return;
  }
  input.value = '';
  const queued = reply.ok && reply.ok.queued;
  notice(queued ? `Queued: ${cur.t.title} is working, and it goes in when the turn ends.` : `Sent to ${cur.t.title}.`);
}

async function stop() {
  const cur = current();
  if (!cur) return;
  const reply = await request('interrupt', { thread_id: cur.t.id });
  notice(reply.error ? reply.error : `Asked ${cur.t.title} to stop its turn.`, Boolean(reply.error));
}

// ---- actions on threads (docs/design/2026-11-24-web-alone.md) ----------------------------------

/** The menu rows for a new thread in `cwd`: the agents, the shell, the file manager. */
function newRows(cwd, beside) {
  const rows = state.agents.map((id) => ({ label: agentName(id), note: 'agent', run: () => create(cwd, id, beside) }));
  rows.push({ label: state.shell, note: 'shell', run: () => create(cwd, 'shell', beside) });
  if (state.fileManager) rows.push({ label: state.fileManager, note: 'files', run: () => create(cwd, 'files', beside) });
  return rows;
}

/** A new thread in project `p`: from a shell of it, where that shell is now, as in the TUI. */
function newThread(p, anchor) {
  const cur = current();
  const beside = cur && cur.p.id === p.id ? cur.t.id : null;
  pop.menu({ title: `New thread in ${p.name}`, rows: newRows(p.path, beside), anchor });
}

async function create(cwd, agent, beside) {
  const reply = await request('new_thread', { cwd, agent, prompt: null, beside, worktree: false });
  if (reply.error) {
    notice(reply.error, true);
    return;
  }
  const id = reply.ok && reply.ok.thread_id;
  if (!id) return;
  state.awaiting = id;
  select(id);
  notice(`Opened ${id}.`);
}

/** The directories `prefix o` lists, from desl web. */
function dirs() {
  return new Promise((resolve) => {
    if (!state.ws || state.ws.readyState !== WebSocket.OPEN) {
      resolve([]);
      return;
    }
    const id = state.nextId++;
    state.dirs.set(id, resolve);
    state.ws.send(JSON.stringify({ type: 'dirs', id }));
  });
}

/** Opens a directory as a project: the finder, then what to start there. */
function openDirectory() {
  pop.finder({
    title: 'Open a directory',
    dirs: dirs(),
    pick: (path) => {
      const name = path.replace(/\/+$/, '').split('/').pop() || path;
      pop.menu({ title: `Start in ${name}`, rows: newRows(path.replace(/(.)\/+$/, '$1'), null) });
    },
  });
}

/** The thread's own menu: what the TUI's prefix keys do to it. */
function actions(anchor) {
  const cur = current();
  if (!cur) return;
  const k = (key) => `${state.prefix} ${key}`;
  const rows = [];
  if (talks(cur.t)) {
    rows.push({ label: 'Switch agent…', note: k('s'), run: () => switchMenu(cur, anchor) });
    rows.push({ label: 'Restart', note: k('r'), run: () => restart(cur) });
    rows.push({ label: 'Fork…', note: k('f'), run: () => forkMenu(cur, anchor) });
  }
  rows.push({ label: 'Rename…', note: k(','), run: () => rename(cur, anchor) });
  rows.push({ label: 'Close…', note: k('x'), run: () => closeThread(cur, anchor) });
  pop.menu({ title: cur.t.title, rows, anchor });
}

function noConversation(cur) {
  notice(`${cur.t.title} is a ${cur.t.agent === 'files' ? 'file manager' : 'shell'}: it has no conversation to switch, fork or restart.`, true);
}

function switchMenu(cur, anchor) {
  if (!talks(cur.t)) return noConversation(cur);
  const rows = state.agents.map((id) => ({
    label: agentName(id),
    note: id === cur.t.agent ? '✓ current' : 'agent',
    mark: id === cur.t.agent,
    disabled: id === cur.t.agent,
    run: async () => {
      notice(`Switching ${cur.t.title} to ${agentName(id)}…`);
      const reply = await request('switch', { thread_id: cur.t.id, agent: id });
      if (reply.error) notice(reply.error, true);
    },
  }));
  pop.menu({ title: `Switch ${cur.t.title} to`, rows, anchor });
}

function forkMenu(cur, anchor) {
  if (!talks(cur.t)) return noConversation(cur);
  const rows = state.agents.map((id) => ({
    label: agentName(id),
    note: id === cur.t.agent ? '✓ this thread’s' : 'agent',
    mark: id === cur.t.agent,
    run: async () => {
      const reply = await request('fork', { thread_id: cur.t.id, agent: id, at: null, worktree: false });
      if (reply.error) {
        notice(reply.error, true);
        return;
      }
      const made = reply.ok && reply.ok.thread_id;
      if (made) {
        state.awaiting = made;
        select(made);
        notice(`Forked ${cur.t.title} to ${made} on ${agentName(id)}.`);
      }
    },
  }));
  pop.menu({ title: `Fork ${cur.t.title} to`, rows, anchor });
}

async function restart(cur) {
  if (!talks(cur.t)) return noConversation(cur);
  const reply = await request('restart', { thread_id: cur.t.id });
  notice(reply.error || `Restarting ${agentName(cur.t.agent)} on the same conversation.`, Boolean(reply.error));
}

function rename(cur, anchor) {
  pop.field({
    title: `Rename ${cur.t.id}`,
    value: cur.t.title,
    anchor,
    save: async (name) => {
      const reply = await request('rename', { thread_id: cur.t.id, name });
      if (reply.error) notice(reply.error, true);
    },
  });
}

function closeThread(cur, anchor) {
  const what = talks(cur.t) ? `Its agent ends; ${agentName(cur.t.agent)}'s own history keeps the conversation.` : `The ${cur.t.agent === 'files' ? 'file manager' : 'shell'} ends.`;
  pop.confirm({
    title: `Close ${cur.t.title}?`,
    text: `${what} It leaves the list.`,
    yesLabel: 'Close',
    anchor,
    yes: async () => {
      const reply = await request('remove_thread', { thread_id: cur.t.id });
      if (reply.error) {
        notice(reply.error, true);
        return;
      }
      const all = threads().filter(({ t }) => t.id !== cur.t.id);
      const back = all.find(({ t }) => t.id === state.previous) || all[0];
      if (back) select(back.t.id);
      notice(`Closed ${cur.t.title}.`);
    },
  });
}

/** The thread `by` places from the selected one in the left column, round the end. */
function step(by) {
  const all = threads();
  if (!all.length) return;
  const i = all.findIndex(({ t }) => t.id === state.selected);
  select(all[(i + by + all.length) % all.length].t.id);
}

/** Every key after the prefix, as the TUI's `prefix ?` lists them. */
function prefixKeys() {
  const cur = current();
  return [
    { key: 'c', label: 'New thread in this project', run: () => cur && newThread(cur.p, null) },
    { key: 'o', label: 'Open a directory', run: openDirectory },
    { key: 's', label: 'Switch agent', run: () => cur && switchMenu(cur, null) },
    { key: 'r', label: 'Restart the agent', run: () => cur && restart(cur) },
    { key: 'f', label: 'Fork', run: () => cur && forkMenu(cur, null) },
    { key: ',', label: 'Rename', run: () => cur && rename(cur, null) },
    { key: 'x', label: 'Close', run: () => cur && closeThread(cur, null) },
    { key: 'n', label: 'Next thread', run: () => step(1) },
    { key: 'p', label: 'Previous thread', run: () => step(-1) },
    { key: 'l', label: 'The thread shown before', run: () => state.previous && select(state.previous) },
    { key: '[', label: 'Conversation, or back to the Screen', run: () => setView(shownView() === 'screen' ? 'conversation' : 'screen') },
    { key: '1…9', label: 'Thread 1 to 9 in the list', run: () => {} },
  ];
}

/** The prefix key as the browser reports it: ctrl and space, or ctrl and its letter. */
function isPrefix(e) {
  if (!e.ctrlKey || e.altKey || e.metaKey) return false;
  const key = state.prefix.replace(/^ctrl\+/, '');
  return key === 'space' ? e.code === 'Space' : e.key.toLowerCase() === key;
}

/** What the prefix key sends to an agent when pressed twice: NUL for space, else its control byte. */
function prefixByte() {
  const key = state.prefix.replace(/^ctrl\+/, '');
  return new Uint8Array([key === 'space' ? 0 : key.charCodeAt(0) & 0x1f]);
}

function afterPrefix(e) {
  if (['Shift', 'Control', 'Alt', 'Meta'].includes(e.key)) return false;
  state.armed = false;
  $('prefix-pill').hidden = true;
  if (e.key === 'Escape') return true;
  if (isPrefix(e)) {
    screen.send(prefixByte());
    return true;
  }
  if (/^[1-9]$/.test(e.key)) {
    const all = threads();
    const t = all[Number(e.key) - 1];
    if (t) select(t.t.id);
    return true;
  }
  if (e.key === '?') {
    pop.menu({ title: `Keys after ${state.prefix}`, rows: prefixKeys().filter((k) => k.key !== '1…9').map((k) => ({ label: k.label, note: k.key, run: k.run })) });
    return true;
  }
  const hit = prefixKeys().find((k) => k.key === e.key);
  if (hit) hit.run();
  else notice(`${state.prefix} ${e.key} does nothing on the page. ${state.prefix} ? lists the keys.`);
  return true;
}

/**
 * The page's keys, read before the agent's screen or any field sees them: the prefix and the key
 * after it, and alt+j/alt+k to move between threads, as in the TUI. Everything else goes on.
 */
function setupKeys() {
  window.addEventListener(
    'keydown',
    (e) => {
      if (pop.isOpen()) return;
      let used = false;
      if (state.armed) used = afterPrefix(e);
      else if (isPrefix(e)) {
        state.armed = true;
        const pill = $('prefix-pill');
        pill.textContent = `${state.prefix} …`;
        pill.hidden = false;
        used = true;
      } else if (e.altKey && !e.ctrlKey && !e.metaKey && (e.key === 'j' || e.key === 'k')) {
        step(e.key === 'j' ? 1 : -1);
        used = true;
      }
      if (used) {
        e.preventDefault();
        e.stopPropagation();
      }
    },
    true,
  );
}

// ---- resizing the outer columns ---------------------------------------------------------------

const WIDTHS = 'desl-web-widths';
// Each grip: the column it sizes, the custom property it sets, its usual width, its narrowest.
const COLUMNS = {
  'grip-side': { column: 'projects', prop: '--side-w', usual: 280, least: 180, fromLeft: true },
  // Saved under a name of its own since the column holds the task list too: a width dragged for
  // the waiting list alone starts over.
  'grip-wait': { column: 'wait', prop: '--wait-w', usual: 460, least: 280, fromLeft: false, store: 'right-column' },
};
// What the middle column keeps whatever the outer ones take.
const MIDDLE_LEAST = 360;

function savedWidths() {
  try { return JSON.parse(localStorage.getItem(WIDTHS)) || {}; } catch { return {}; }
}

function saveWidth(id, px) {
  const all = savedWidths();
  if (px === null) delete all[id]; else all[id] = px;
  try { localStorage.setItem(WIDTHS, JSON.stringify(all)); } catch { /* the widths last until the tab closes */ }
}

/** Gives a column `px` wide, as far as the window allows; null goes back to the usual width. */
function setWidth(id, px) {
  const c = COLUMNS[id];
  const grip = $(id);
  const other = Object.keys(COLUMNS).find((k) => k !== id);
  const otherWidth = $(COLUMNS[other].column).getBoundingClientRect().width;
  const most = Math.max(c.least, Math.min(window.innerWidth * 0.45, window.innerWidth - otherWidth - MIDDLE_LEAST));
  const width = px === null ? c.usual : Math.round(Math.min(Math.max(px, c.least), most));
  if (px === null) document.documentElement.style.removeProperty(c.prop);
  else document.documentElement.style.setProperty(c.prop, `${width}px`);
  grip.setAttribute('aria-valuenow', String(width));
  grip.setAttribute('aria-valuemin', String(c.least));
  grip.setAttribute('aria-valuemax', String(Math.round(most)));
  return width;
}

function resizable() {
  const saved = savedWidths();
  for (const [id, c] of Object.entries(COLUMNS)) {
    const grip = $(id);
    const key = c.store || id;
    setWidth(id, typeof saved[key] === 'number' ? saved[key] : null);
    grip.addEventListener('pointerdown', (e) => {
      if (e.button !== 0) return;
      e.preventDefault();
      // The arrows go on from where the mouse left it.
      grip.focus({ preventScroll: true });
      grip.setPointerCapture(e.pointerId);
      grip.classList.add('held');
      document.body.classList.add('resizing');
      const edge = $(c.column).getBoundingClientRect();
      const move = (ev) => setWidth(id, c.fromLeft ? ev.clientX - edge.left : edge.right - ev.clientX);
      const up = (ev) => {
        grip.releasePointerCapture(ev.pointerId);
        grip.classList.remove('held');
        document.body.classList.remove('resizing');
        grip.removeEventListener('pointermove', move);
        grip.removeEventListener('pointerup', up);
        grip.removeEventListener('pointercancel', up);
        saveWidth(key, Number(grip.getAttribute('aria-valuenow')));
      };
      grip.addEventListener('pointermove', move);
      grip.addEventListener('pointerup', up);
      grip.addEventListener('pointercancel', up);
    });
    grip.addEventListener('dblclick', () => {
      setWidth(id, null);
      saveWidth(key, null);
    });
    // The keys, for whoever does not drag: the arrows by 16 pixels, with shift by 64.
    grip.addEventListener('keydown', (e) => {
      if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
      e.preventDefault();
      const step = (e.shiftKey ? 64 : 16) * (e.key === 'ArrowRight' ? 1 : -1) * (c.fromLeft ? 1 : -1);
      saveWidth(key, setWidth(id, $(c.column).getBoundingClientRect().width + step));
    });
  }
}

function main() {
  // A new address pasted into this tab (desl web started again, with a new key) changes only
  // the part after #, which loads nothing by itself.
  window.addEventListener('hashchange', () => {
    if (/(?:^#|&)t=[0-9a-f]{32}\b/.test(location.hash)) location.reload();
  });
  const key = takeKey();
  if (!key) {
    $('nokey').hidden = false;
    return;
  }
  resizable();
  setupScreenPane();
  setupKeys();
  setupConversation();
  setupTasks();
  setupAttention();
  $('prompt-form').addEventListener('submit', sendPrompt);
  connect(key);
}

main();

// desl web's page (web-spec.md). It speaks the daemon's protocol through desl web's bridge and
// draws the `view` the bridge sends: the sidebar as desl itself words it for the terminal, so
// both clients say the same. Text from agents only ever goes into the page as text nodes.

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
};

function read(name) {
  try { return sessionStorage.getItem(name); } catch { return null; }
}

function write(name, value) {
  try { sessionStorage.setItem(name, value); } catch { /* a private window: the page works without it */ }
}

/** The key from the address desl web printed, kept for this tab and taken out of the address bar. */
function takeKey() {
  const m = location.hash.match(/(?:^#|&)t=([0-9a-f]{32})\b/);
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
      state.retry = 500;
      if ($('link').dataset.state !== 'connected') setLink('connected');
      else render();
      break;
    case 'bridge':
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

// ---- drawing ----------------------------------------------------------------------------------

function threads() {
  return state.view ? state.view.projects.flatMap((p) => p.threads.map((t) => ({ t, p }))) : [];
}

function current() {
  const all = threads();
  let found = all.find(({ t }) => t.id === state.selected);
  if (!found && all.length) {
    const waiting = state.view.waiting[0];
    found = all.find(({ t }) => t.id === waiting) || all[0];
    state.selected = found.t.id;
  }
  return found || null;
}

function select(id) {
  state.selected = id;
  write(SELECTED, id);
  notice('');
  render();
}

function render() {
  const cur = current();
  drawSide(cur);
  drawHead(cur);
  drawBody(cur);
  drawWaiting();
  drawChrome(cur);
}

function drawSide(cur) {
  const nav = $('projects');
  nav.replaceChildren();
  if (!state.view) {
    nav.append(el('p', 'empty', 'Reading the workspace…'));
    return;
  }
  if (!state.view.projects.length) {
    nav.append(el('p', 'empty', 'No agent threads yet. Start one in the terminal with ctrl+space c.'));
  }
  for (const p of state.view.projects) {
    const section = el('section', 'project');
    section.setAttribute('aria-label', p.name);
    const row = el('div', 'project-row');
    row.append(projectDot(p.hue), el('span', 'project-name', p.name));
    if (p.branch) row.append(el('span', 'branch', `⎇ ${p.branch}`));
    if (p.dirty > 0) row.append(el('span', 'dirty', `±${p.dirty}`));
    if (p.parent) row.append(el('span', 'parent', p.parent));
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
      b.addEventListener('click', () => select(t.id));
      section.append(b);
      t.subagents.forEach((s, i) => {
        const sub = el('div', 'sub');
        sub.dataset.state = s.state;
        const glyph = { running: '●', done: '✓', unfinished: '○' }[s.state] || '○';
        const tree = el('span', 'tree', i === t.subagents.length - 1 ? '└─' : '├─');
        tree.setAttribute('aria-hidden', 'true');
        sub.append(tree, el('span', 'glyph', glyph), el('span', 'label-text', s.label));
        section.append(sub);
      });
    }
    nav.append(section);
  }
  nav.append(el('p', 'side-note', 'Shells and file managers stay in the terminal.'));
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
  head.append(left, right);
}

function drawBody(cur) {
  const body = $('body');
  body.replaceChildren();
  if (!cur) {
    body.append(el('p', 'empty', state.view ? 'Pick a thread on the left.' : ''));
    return;
  }
  const { t, p } = cur;
  const h = t.header;
  const facts = el('dl', 'facts');
  const fact = (k, v) => facts.append(el('dt', '', k), el('dd', '', v));
  fact('Status', `${t.glyph} ${t.word}`);
  if (t.age) fact('Last change', t.age === 'now' ? 'just now' : `${t.age} ago`);
  fact('Agent', t.agent);
  if (h.model) fact('Model', h.model);
  fact('Permissions', h.yolo ? `${h.mode}: the agent does anything without asking` : h.mode);
  fact('Project', p.parent ? `${h.project} in ${p.parent}` : h.project);
  if (h.branch) fact('Worktree', `its own, on ${h.branch}`);
  if (t.subagents.length) {
    const running = t.subagents.filter((s) => s.state === 'running').length;
    fact('Subagents', running ? `${t.subagents.length}, ${running} running` : String(t.subagents.length));
  }
  fact('Thread', t.id);
  body.append(facts, el('p', 'next', 'Next on this page: the agent\'s own screen, and the whole conversation.'));
}

function drawWaiting() {
  const list = $('waitlist');
  list.replaceChildren();
  const ids = state.view ? state.view.waiting : [];
  $('wait-title').textContent = ids.length ? `Waiting · ${ids.length}` : 'Waiting';
  if (!ids.length) {
    list.append(el('p', 'quiet', state.view ? 'Nothing is waiting for you.' : ''));
    return;
  }
  const all = threads();
  for (const id of ids) {
    const found = all.find(({ t }) => t.id === id);
    if (!found) continue;
    const { t, p } = found;
    const card = el('section', 'card');
    card.setAttribute('aria-label', `${t.title} is waiting`);
    const top = el('div', 'card-top');
    top.append(mark(t.agent, t.status, 13), el('span', '', `${t.title} · ${p.name}`));
    if (t.age) top.append(el('span', 'age', t.age));
    const actions = el('div', 'actions');
    const show = el('button', 'secondary', 'Show');
    show.type = 'button';
    show.addEventListener('click', () => select(t.id));
    actions.append(show);
    card.append(top, el('p', '', 'It asks you something on its screen.'), actions);
    list.append(card);
  }
}

function drawChrome(cur) {
  const ids = state.view ? state.view.waiting : [];
  const pill = $('waiting-pill');
  pill.hidden = ids.length === 0;
  pill.textContent = `${ids.length} waiting`;
  // As the terminal's title: the waiting threads other than the one on screen.
  const others = ids.filter((id) => !cur || id !== cur.t.id).length;
  let title = cur ? `desl · ${cur.t.agent} · ${cur.t.id}` : 'desl';
  if (cur && cur.t.header.yolo) title += ' · ⚠ yolo';
  if (others) title += ` · ${others} waiting`;
  document.title = title;
  const ready = Boolean(cur) && $('link').dataset.state === 'connected';
  const input = $('prompt');
  input.disabled = !ready;
  input.placeholder = cur ? `Prompt to ${cur.t.title} (${cur.t.agent}). Queued while it works.` : 'Pick a thread';
  $('send').disabled = !ready;
  $('stop').disabled = !ready || !cur || cur.t.status !== 'working';
}

function notice(text, error = false) {
  const n = $('notice');
  n.textContent = text;
  n.classList.toggle('error', error);
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

// ---- resizing the outer columns ---------------------------------------------------------------

const WIDTHS = 'desl-web-widths';
// Each grip: the column it sizes, the custom property it sets, its usual width, its narrowest.
const COLUMNS = {
  'grip-side': { column: 'projects', prop: '--side-w', usual: 280, least: 180, fromLeft: true },
  'grip-wait': { column: 'wait', prop: '--wait-w', usual: 300, least: 220, fromLeft: false },
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
    setWidth(id, typeof saved[id] === 'number' ? saved[id] : null);
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
        saveWidth(id, Number(grip.getAttribute('aria-valuenow')));
      };
      grip.addEventListener('pointermove', move);
      grip.addEventListener('pointerup', up);
      grip.addEventListener('pointercancel', up);
    });
    grip.addEventListener('dblclick', () => {
      setWidth(id, null);
      saveWidth(id, null);
    });
    // The keys, for whoever does not drag: the arrows by 16 pixels, with shift by 64.
    grip.addEventListener('keydown', (e) => {
      if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
      e.preventDefault();
      const step = (e.shiftKey ? 64 : 16) * (e.key === 'ArrowRight' ? 1 : -1) * (c.fromLeft ? 1 : -1);
      saveWidth(id, setWidth(id, $(c.column).getBoundingClientRect().width + step));
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
  $('prompt-form').addEventListener('submit', sendPrompt);
  $('stop').addEventListener('click', stop);
  render();
  connect(key);
}

main();

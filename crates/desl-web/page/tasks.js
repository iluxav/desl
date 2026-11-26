// The task column of desl web's page (docs/design/2026-11-22-tasks-first-class.md): a project's
// task list as rows, under a filter bar, live as the daemon tells the page of each change. All
// text a task holds goes in as text nodes.

function el(tag, className, text) {
  const e = document.createElement(tag);
  if (className) e.className = className;
  if (text !== undefined && text !== null) e.textContent = text;
  return e;
}

const SVG = 'http://www.w3.org/2000/svg';

/** Line icons, drawn here: the page loads nothing from outside. Each a 24×24 path. */
const PATHS = {
  bug: 'M9 7a3 3 0 0 1 6 0M12 7v12M7 13a5 6 0 0 0 10 0a5 6 0 0 0-10 0M7 10H4M7 14H3M7 18H4M17 10h3M17 14h4M17 18h3',
  feature: 'M12 3l2.2 5.6 5.8.6-4.5 3.9 1.5 5.9-5-3-5 3 1.5-5.9L4 9.2l5.8-.6z',
  task: 'M7 4h10a3 3 0 0 1 3 3v10a3 3 0 0 1-3 3H7a3 3 0 0 1-3-3V7a3 3 0 0 1 3-3zM8 12.5l2.5 2.5L16 9',
  refactor: 'M4 12a8 8 0 0 1 13.7-5.7L20 8M20 4v4h-4M20 12a8 8 0 0 1-13.7 5.7L4 16M4 20v-4h4',
  test: 'M9 3h6M10 3v6l-5 9a2 2 0 0 0 1.8 3h10.4a2 2 0 0 0 1.8-3l-5-9V3M7.5 15h9',
  docs: 'M6 3h12v16H6a2 2 0 0 0-2 2V5a2 2 0 0 1 2-2zM4 21a2 2 0 0 1 2-2M8 7h7M8 11h5',
  idea: 'M9 18h6M10 21h4M12 3a6 6 0 0 0-4 10.5c.8.8 1 1.5 1 2.5h6c0-1 .2-1.7 1-2.5A6 6 0 0 0 12 3z',
  high: 'M7 13l5-5 5 5M7 18l5-5 5 5',
  medium: 'M6 10h12M6 14h12',
  low: 'M7 6l5 5 5-5M7 11l5 5 5-5',
  down: 'M6 9l6 6 6-6',
  folder: 'M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z',
  'folder-open': 'M3 11V7a2 2 0 0 1 2-2h4l2 2h7a2 2 0 0 1 2 2v2M3 11h18l-3 9H5z',
  branch: 'M6 6v12M6 12h7a5 5 0 0 0 5-5M8 4a2 2 0 1 1-4 0 2 2 0 0 1 4 0M8 20a2 2 0 1 1-4 0 2 2 0 0 1 4 0M20 5a2 2 0 1 1-4 0 2 2 0 0 1 4 0',
  plus: 'M12 5v14M5 12h14',
  more: 'M5 12h.01M12 12h.01M19 12h.01',
  eye: 'M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12zM15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0',
};

const KIND_WORD = { bug: 'Bug', feature: 'Feature', task: 'Task', refactor: 'Refactor', test: 'Test', docs: 'Docs', idea: 'Idea' };

/** An icon by name, with what it means as its tooltip and for a screen reader. */
export function icon(name, label, className) {
  const svg = document.createElementNS(SVG, 'svg');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('class', `ico ${className || ''}`.trim());
  if (label) {
    svg.setAttribute('role', 'img');
    svg.setAttribute('aria-label', label);
    const title = document.createElementNS(SVG, 'title');
    title.textContent = label;
    svg.append(title);
  } else {
    svg.setAttribute('aria-hidden', 'true');
  }
  const path = document.createElementNS(SVG, 'path');
  path.setAttribute('d', PATHS[name] || PATHS.task);
  svg.append(path);
  return svg;
}

/** The status filter's segments, and which statuses each shows. */
export const SEGMENTS = [
  ['open', 'Open', (t) => ['todo', 'in-progress', 'issue', 'ready'].includes(t.status)],
  ['todo', 'To do', (t) => t.status === 'todo'],
  ['in-progress', 'In progress', (t) => t.status === 'in-progress'],
  ['ready', 'Ready', (t) => t.status === 'ready'],
  ['done', 'Done', (t) => t.status === 'done'],
];

/** The rarer views, under More. */
export const MORE = [
  ['issue', 'Issue', (t) => t.status === 'issue'],
  ['parked', 'Parked', (t) => t.status === 'parked'],
  ['bugs', 'Bugs only', (t) => t.type === 'bug' && ['todo', 'in-progress', 'issue', 'ready'].includes(t.status)],
];

const STATUS_WORD = {
  todo: 'to do',
  'in-progress': 'in progress',
  issue: 'issue',
  ready: 'ready',
  done: 'done',
  ignored: 'ignored',
  parked: 'parked',
};

/** Whether `t` matches what is typed in Find: its number, or words of its title or description. */
function found(t, find) {
  const q = find.trim().toLowerCase();
  if (!q) return true;
  if (q.replace(/^#/, '') === String(t.id)) return true;
  return `${t.title}\n${t.body || ''}`.toLowerCase().includes(q);
}

/**
 * One task as a row of two lines: its number, kind and title; then its priority, status and,
 * once work started, the thread on it. `ctx.thread(id)` gives the thread's title and agent;
 * `ctx.action(task, project)` the node on the right of the row, or null.
 */
function row(t, project, ctx) {
  const r = el('div', `trow status-${t.status}`);
  r.dataset.task = String(t.id);
  r.dataset.project = project.id;
  // Its kind as an icon, in the number's place; the number leads the second line.
  const kind = el('span', `tkind kind-${t.type}`);
  kind.append(icon(t.type, KIND_WORD[t.type] || t.type));
  r.append(kind);
  const title = el('span', 'ttitle', t.title);
  if (t.body) title.title = t.body.length > 400 ? `${t.body.slice(0, 400)}…` : t.body;
  r.append(title);
  const act = el('span', 'tact');
  const node = ctx.action ? ctx.action(t, project) : null;
  if (node) act.append(node);
  r.append(act);
  const meta = el('span', 'tmeta');
  const parts = [el('span', 'tnum', `#${t.id}`)];
  const prio = el('span', `tprio prio-${t.priority}`);
  prio.append(icon(t.priority, `${t.priority[0].toUpperCase()}${t.priority.slice(1)} priority`));
  parts.push(prio);
  parts.push(document.createTextNode(STATUS_WORD[t.status] || t.status));
  if (t.thread) {
    const th = ctx.thread ? ctx.thread(t.thread) : null;
    parts.push(document.createTextNode(' · '));
    const link = el('button', 'tthread', th ? th.title : t.thread);
    link.type = 'button';
    link.title = `Go to ${t.thread}`;
    link.addEventListener('click', () => ctx.select && ctx.select(t.thread));
    parts.push(link);
    if (th && th.agent) parts.push(document.createTextNode(` · ${th.agent}`));
  }
  if (t.branch) parts.push(document.createTextNode(` · ${t.branch}`));
  meta.append(...parts);
  r.append(meta);
  return r;
}

/**
 * Draws the list into `into`. `lists`: [{project: {id, name}, tasks}] for the projects shown;
 * `filter`: {status, more, find}. Folded at the bottom of Open: Parked and Done.
 */
export function drawList(into, lists, filter, ctx) {
  into.replaceChildren();
  const pick = filter.more ? MORE.find(([id]) => id === filter.more) : SEGMENTS.find(([id]) => id === filter.status);
  const keep = pick ? pick[2] : SEGMENTS[0][2];
  let shown = 0;
  const grouped = lists.length > 1;
  for (const { project, tasks } of lists) {
    const rows = tasks.filter((t) => keep(t) && found(t, filter.find));
    const parked = tasks.filter((t) => t.status === 'parked' && found(t, filter.find));
    const done = tasks.filter((t) => t.status === 'done' && found(t, filter.find));
    const folds = !filter.more && filter.status === 'open';
    if (!rows.length && !(folds && (parked.length || done.length))) continue;
    if (grouped) into.append(el('div', 'tproject', project.name));
    for (const t of rows) into.append(row(t, project, ctx));
    shown += rows.length;
    if (!folds) continue;
    for (const [name, list] of [['Parked', parked], ['Done', done]]) {
      if (!list.length) continue;
      const fold = el('details', 'tfold');
      fold.append(el('summary', '', `${name} · ${list.length}`));
      for (const t of list) fold.append(row(t, project, ctx));
      into.append(fold);
    }
  }
  if (!shown && !into.childElementCount) {
    const any = lists.some((l) => l.tasks.length);
    into.append(el('p', 'quiet tempty', any ? 'No task here.' : 'No task yet. Ask an agent to add one, or `desl task add`.'));
  }
}

/** Each segment's count, for the filter bar, from what the project and Find leave. */
export function counts(lists, find) {
  const all = lists.flatMap((l) => l.tasks).filter((t) => found(t, find));
  const n = {};
  for (const [id, , keep] of [...SEGMENTS, ...MORE]) n[id] = all.filter(keep).length;
  return n;
}

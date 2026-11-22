// The task column of desl web's page (docs/design/2026-11-22-tasks-first-class.md): a project's
// task list as rows, under a filter bar, live as the daemon tells the page of each change. All
// text a task holds goes in as text nodes.

function el(tag, className, text) {
  const e = document.createElement(tag);
  if (className) e.className = className;
  if (text !== undefined && text !== null) e.textContent = text;
  return e;
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
  r.append(el('span', 'tid', `#${t.id}`));
  const title = el('span', 'ttitle');
  title.append(el('span', `tkind kind-${t.type}`, t.type), document.createTextNode(t.title));
  if (t.body) title.title = t.body.length > 400 ? `${t.body.slice(0, 400)}…` : t.body;
  r.append(title);
  const act = el('span', 'tact');
  const node = ctx.action ? ctx.action(t, project) : null;
  if (node) act.append(node);
  r.append(act);
  const meta = el('span', 'tmeta');
  const parts = [];
  if (t.priority === 'high') parts.push(el('span', 'hi', 'high'));
  else parts.push(document.createTextNode(t.priority));
  parts.push(document.createTextNode(` · ${STATUS_WORD[t.status] || t.status}`));
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

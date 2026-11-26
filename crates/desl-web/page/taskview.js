// A task opened from the right column, over the conversation (web-spec.md "Tasks"): its whole
// title, its fields and its description as Markdown. Edit changes them and + adds a task, through
// the daemon's `task_set` and `task_add`, under the same lock on the project's folder as
// `desl task` and the agents. All text a task holds goes in as text nodes.
import * as pop from '/popover.js';
import { KIND_WORD, STATUS_WORD, icon } from '/tasks.js';

const $ = (id) => document.getElementById(id);

function el(tag, className, text) {
  const e = document.createElement(tag);
  if (className) e.className = className;
  if (text !== undefined && text !== null) e.textContent = text;
  return e;
}

const KINDS = ['feature', 'bug', 'task', 'refactor', 'test', 'docs', 'idea'];
const STATUSES = ['todo', 'in-progress', 'issue', 'ready', 'done', 'parked', 'ignored'];
const PRIORITIES = ['high', 'medium', 'low'];
const FIELDS = [
  ['title', 'title'],
  ['type', 'kind'],
  ['status', 'status'],
  ['priority', 'priority'],
  ['body', 'description'],
];
const word = (s) => s[0].toUpperCase() + s.slice(1);

/** As the daemon keeps it: a title on one line, a description without blank ends. */
function tidy(field, value) {
  const v = String(value ?? '');
  return field === 'title' ? v.replace(/\s+/g, ' ').trim() : field === 'body' ? v.trim() : v;
}

/**
 * Installs the view. `ctx`: `request(command, fields)`; `task(projectId, id)`, the task as the
 * daemon last told it, or null; `markdown(text)`; `thread(id)`, `{title, agent}` or null;
 * `select(id)` goes to a thread; `added(project, task)` keeps a task the daemon replied with
 * until its list comes; `changed()` redraws the list, for the open task's row.
 */
export function setupTaskView(ctx) {
  /** The open task: its project `{id, name}`, its number (null for a new one), and an edit's start. */
  const v = { project: null, id: null, editing: false, orig: null };

  function selectOf(id, values, words, value) {
    const s = $(id);
    s.replaceChildren(...values.map((x) => {
      const o = el('option', '', words ? words(x) : word(x));
      o.value = x;
      return o;
    }));
    s.value = value;
  }

  function form() {
    return {
      title: $('task-f-title').value,
      type: $('task-f-kind').value,
      status: $('task-f-status').value,
      priority: $('task-f-priority').value,
      body: $('task-f-body').value,
    };
  }

  /** Whether the form holds something its task does not. */
  function dirty() {
    if (!v.editing) return false;
    const now = form();
    return FIELDS.some(([f]) => tidy(f, now[f]) !== tidy(f, v.orig[f]));
  }

  function error(text = '') {
    $('task-error').textContent = text;
    $('task-error').hidden = !text;
  }

  function draw() {
    const box = $('taskview');
    box.hidden = !v.project;
    if (!v.project) return;
    const t = v.id === null ? null : ctx.task(v.project.id, v.id);
    $('task-form').hidden = !v.editing;
    $('task-read').hidden = v.editing;
    $('task-edit').hidden = v.editing || !t;
    const kind = $('task-kind');
    kind.className = `tkind kind-${(v.editing ? $('task-f-kind').value : t?.type) || 'task'}`;
    kind.replaceChildren(icon((v.editing ? $('task-f-kind').value : t?.type) || 'task'));
    $('task-num').textContent = v.id === null ? '' : `#${v.id}`;
    $('task-project').textContent = v.project.name;
    if (v.editing) {
      $('task-title').textContent = v.id === null ? 'New task' : 'Edit task';
      return;
    }
    if (!t) {
      $('task-title').textContent = `Task #${v.id}`;
      $('task-meta').replaceChildren();
      $('task-body').replaceChildren(el('p', 'quiet', 'This task is no longer in the project\'s list.'));
      return;
    }
    $('task-title').textContent = t.title;
    const meta = $('task-meta');
    const prio = el('span', `tprio prio-${t.priority}`);
    prio.append(icon(t.priority));
    const parts = [
      el('span', `tstatus status-${t.status}`, STATUS_WORD[t.status] || t.status),
      el('span', '', KIND_WORD[t.type] || t.type),
    ];
    const p = el('span', '');
    p.append(prio, document.createTextNode(`${word(t.priority)} priority`));
    parts.push(p);
    if (t.thread) {
      const th = ctx.thread(t.thread);
      const link = el('button', 'tthread', th ? `${th.title} · ${th.agent}` : t.thread);
      link.type = 'button';
      link.title = `Go to ${t.thread}`;
      link.addEventListener('click', () => ctx.select(t.thread));
      parts.push(link);
    }
    if (t.branch) parts.push(el('span', 'branch', t.branch));
    meta.replaceChildren(...parts);
    $('task-body').replaceChildren(t.body ? ctx.markdown(t.body) : el('p', 'quiet', 'No description.'));
  }

  function fill(t) {
    $('task-f-title').value = t.title;
    selectOf('task-f-kind', KINDS, (k) => KIND_WORD[k], t.type);
    selectOf('task-f-status', STATUSES, (s) => word(STATUS_WORD[s] || s), t.status);
    selectOf('task-f-priority', PRIORITIES, null, t.priority);
    $('task-f-body').value = t.body;
    $('task-save').disabled = !tidy('title', t.title);
    $('task-save').textContent = v.id === null ? 'Add task' : 'Save';
    error();
  }

  /** Runs `then` once an edit with changes in it is given up, after asking; at once otherwise. */
  function leave(then, anchor) {
    if (!dirty()) return then();
    pop.confirm({
      title: 'Discard your changes?',
      text: v.id === null ? 'The new task is not added.' : `Your changes to #${v.id} are not saved.`,
      yesLabel: 'Discard',
      anchor,
      yes: then,
    });
  }

  function show(project, id) {
    Object.assign(v, { project, id, editing: false, orig: null });
    error();
    draw();
    ctx.changed();
    $('task-close').focus();
  }

  function edit() {
    const t = ctx.task(v.project.id, v.id);
    if (!t) return;
    v.editing = true;
    v.orig = { ...t };
    fill(t);
    draw();
    $('task-f-title').focus();
  }

  function close() {
    Object.assign(v, { project: null, id: null, editing: false, orig: null });
    draw();
    ctx.changed();
  }

  function cancel() {
    if (v.id === null) return close();
    v.editing = false;
    v.orig = null;
    error();
    draw();
    $('task-edit').focus();
  }

  async function save(e) {
    e.preventDefault();
    const now = form();
    if (!tidy('title', now.title)) return;
    $('task-save').disabled = true;
    try {
      if (v.id === null) {
        const reply = await ctx.request('task_add', {
          project_id: v.project.id, title: tidy('title', now.title), kind: now.type,
          status: now.status, priority: now.priority, body: tidy('body', now.body),
        });
        if (reply.error) return error(reply.error);
        v.id = reply.ok.task.id;
        ctx.added(v.project, reply.ok.task);
      } else {
        // Only what was changed here goes, so a change an agent made meanwhile to another field
        // stays; one it made to a field changed here too is not overwritten unseen.
        const cur = ctx.task(v.project.id, v.id);
        if (!cur) return error(`#${v.id} was removed from the list meanwhile. Copy your text if you want to keep it, then press Cancel.`);
        const edit = {};
        for (const [f, name] of FIELDS) {
          if (tidy(f, now[f]) === tidy(f, v.orig[f])) continue;
          if (tidy(f, cur[f]) !== tidy(f, v.orig[f])) {
            return error(`The task's ${name} changed meanwhile, perhaps by an agent. Copy your text, press Cancel to see the current version, and edit again.`);
          }
          edit[f === 'type' ? 'kind' : f] = tidy(f, now[f]);
        }
        if (Object.keys(edit).length) {
          const reply = await ctx.request('task_set', { project_id: v.project.id, task: v.id, edit });
          if (reply.error) return error(reply.error);
          ctx.added(v.project, reply.ok.task);
        }
      }
    } finally {
      $('task-save').disabled = !tidy('title', $('task-f-title').value);
    }
    v.editing = false;
    v.orig = null;
    error();
    draw();
    ctx.changed();
    $('task-edit').focus();
  }

  $('task-close').addEventListener('click', (e) => leave(close, e.currentTarget));
  $('task-edit').addEventListener('click', edit);
  $('task-cancel').addEventListener('click', (e) => leave(cancel, e.currentTarget));
  $('task-form').addEventListener('submit', save);
  $('task-f-title').addEventListener('input', () => {
    $('task-save').disabled = !tidy('title', $('task-f-title').value);
  });
  $('task-f-kind').addEventListener('change', draw);
  $('taskview').addEventListener('keydown', (e) => {
    if (e.key !== 'Escape' || pop.isOpen()) return;
    e.preventDefault();
    if (v.editing) leave(v.id === null ? close : cancel, e.target);
    else close();
  });

  return {
    /** Opens task `id` of `project`; a change being edited is given up only once agreed. */
    open(project, id, anchor) {
      if (v.project && v.project.id === project.id && v.id === id && !v.editing) return;
      leave(() => show(project, id), anchor);
    },
    /** A form for a new task in `project`. */
    add(project, anchor) {
      leave(() => {
        Object.assign(v, { project, id: null, editing: true });
        v.orig = { title: '', type: 'task', status: 'todo', priority: 'medium', body: '' };
        fill(v.orig);
        draw();
        ctx.changed();
        $('task-f-title').focus();
      }, anchor);
    },
    /** Whether task `id` of `projectId` is the one open. */
    isOpen: (projectId, id) => Boolean(v.project) && v.project.id === projectId && v.id === id,
    /** The list changed: what is read is drawn again; a form being filled keeps its text. */
    refresh() {
      if (v.project && !v.editing) draw();
    },
  };
}

// Small boxes over desl web's page for the actions on threads (docs/design/2026-11-24-web-alone.md):
// a menu, a question with Yes and No, a one-line field and the directory finder. One is open at
// a time. Each takes the keys while it is open, as the TUI's menus do (j/k or the arrows, enter,
// 1…9, esc), and gives the focus back to where it was when it closes. Text goes in as text nodes.

function el(tag, className, text) {
  const e = document.createElement(tag);
  if (className) e.className = className;
  if (text !== undefined && text !== null) e.textContent = text;
  return e;
}

let open = null;
/** Where the focus goes when what had it before a box opened is gone. */
let fallback = () => {};

/** Sets where the focus goes when a box closes and what had it is no longer on the page. */
export function onLost(fn) {
  fallback = fn;
}

/** True while a box is open: the page's own keys wait. */
export function isOpen() {
  return open !== null;
}

/** Closes the open box, if any. */
export function close() {
  if (!open) return;
  const { box, back, onKey, onDown } = open;
  open = null;
  box.remove();
  document.removeEventListener('keydown', onKey, true);
  document.removeEventListener('pointerdown', onDown, true);
  if (back && back.isConnected && back !== document.body) back.focus({ preventScroll: true });
  else fallback();
}

/**
 * Puts `box` on the page, under `anchor` when there is one, else near the top in the middle.
 * `keys(e)` sees each key first and returns true for one it used.
 */
function show(box, anchor, keys) {
  close();
  const back = document.activeElement;
  box.classList.add('pop');
  box.setAttribute('role', 'dialog');
  document.body.append(box);
  // An anchor a redraw took off the page has no place: the box goes in the middle then.
  if (anchor && anchor.isConnected) {
    const r = anchor.getBoundingClientRect();
    const w = box.offsetWidth;
    box.style.left = `${Math.max(8, Math.min(r.left, window.innerWidth - w - 8))}px`;
    box.style.top = `${Math.min(r.bottom + 4, window.innerHeight - box.offsetHeight - 8)}px`;
  } else {
    box.classList.add('centred');
  }
  const onKey = (e) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      close();
      return;
    }
    if (keys(e)) {
      e.preventDefault();
      e.stopPropagation();
    }
  };
  // A click anywhere else closes it, as in the TUI a key it does not know does.
  const onDown = (e) => {
    if (!box.contains(e.target)) close();
  };
  document.addEventListener('keydown', onKey, true);
  document.addEventListener('pointerdown', onDown, true);
  open = { box, back, onKey, onDown };
}

/** The rows' buttons, moved through with j/k or the arrows; 1…9 and enter pick. */
function rowKeys(buttons, e, at) {
  const live = buttons.filter((b) => !b.disabled);
  const i = live.indexOf(document.activeElement);
  if (['ArrowDown', 'ArrowUp'].includes(e.key) || (at && ['j', 'k'].includes(e.key))) {
    const down = e.key === 'ArrowDown' || e.key === 'j';
    const next = i < 0 ? live[down ? 0 : live.length - 1] : live[(i + (down ? 1 : live.length - 1)) % live.length];
    if (next) next.focus();
    return true;
  }
  if (at && /^[1-9]$/.test(e.key)) {
    const b = buttons[Number(e.key) - 1];
    if (b && !b.disabled) b.click();
    return true;
  }
  return false;
}

/**
 * A menu: `rows` are `{label, note, disabled, run}`, the current one marked with `mark`. The
 * first row that can be picked takes the focus.
 */
export function menu({ title, rows, anchor }) {
  const box = el('div', 'pop-menu');
  box.setAttribute('aria-label', title);
  box.append(el('div', 'pop-title', title));
  const buttons = rows.map((r, i) => {
    const b = el('button', `pop-row${r.mark ? ' marked' : ''}`);
    b.type = 'button';
    b.disabled = Boolean(r.disabled);
    b.append(el('span', 'pop-key', i < 9 ? String(i + 1) : ''), el('span', 'pop-label', r.label));
    if (r.note) b.append(el('span', 'pop-note', r.note));
    b.addEventListener('click', () => {
      close();
      r.run();
    });
    box.append(b);
    return b;
  });
  show(box, anchor, (e) => rowKeys(buttons, e, true));
  const first = buttons.find((b) => !b.disabled);
  if (first) first.focus();
}

/** A question: `yes` runs on its button or `y`; No, `n` and esc close it. */
export function confirm({ title, text, yes, yesLabel, anchor }) {
  const box = el('div', 'pop-menu pop-ask');
  box.setAttribute('aria-label', title);
  box.append(el('div', 'pop-title', title));
  if (text) box.append(el('p', 'pop-text', text));
  const row = el('div', 'pop-actions');
  const ok = el('button', 'primary danger', yesLabel || 'Yes');
  ok.type = 'button';
  const no = el('button', 'secondary', 'No');
  no.type = 'button';
  ok.addEventListener('click', () => {
    close();
    yes();
  });
  no.addEventListener('click', close);
  row.append(ok, no);
  box.append(row);
  show(box, anchor, (e) => {
    if (e.key === 'y') {
      ok.click();
      return true;
    }
    if (e.key === 'n') {
      close();
      return true;
    }
    return false;
  });
  no.focus();
}

/** A one-line field: enter saves what it holds, esc cancels. */
export function field({ title, value, placeholder, save, anchor }) {
  const box = el('form', 'pop-menu pop-field');
  box.setAttribute('aria-label', title);
  box.append(el('div', 'pop-title', title));
  const input = el('input');
  input.type = 'text';
  input.value = value || '';
  input.placeholder = placeholder || '';
  input.setAttribute('aria-label', title);
  box.append(input);
  box.addEventListener('submit', (e) => {
    e.preventDefault();
    const text = input.value.trim();
    if (!text) return;
    close();
    save(text);
  });
  show(box, anchor, () => false);
  input.focus();
  input.select();
}

/** How well `text` matches what was typed, as the TUI's finder scores it; null when it does not. */
export function score(text, query) {
  const q = [...query.toLowerCase()].filter((c) => !/\s/.test(c));
  if (!q.length) return 0;
  const t = [...text.toLowerCase()];
  const base = t.lastIndexOf('/') + 1;
  const run = (from) => {
    let s = 0;
    let at = from;
    let last = -2;
    for (const c of q) {
      let i = at;
      while (i < t.length && t[i] !== c) i++;
      if (i >= t.length) return null;
      s += 1;
      if (last === i - 1) s += 8;
      if (i >= base) s += 4;
      if (i === 0 || '/-_.'.includes(t[i - 1])) s += 6;
      last = i;
      at = i + 1;
    }
    return s;
  };
  const a = run(base);
  const b = run(0);
  if (a === null && b === null) return null;
  return Math.max(a ?? -Infinity, b ?? -Infinity) * 100 - t.length;
}

/** The home folder, from a listed directory shown under `~`. */
export function homeOf(all) {
  const d = all.find((x) => x.shown.startsWith('~/'));
  return d ? d.path.slice(0, d.path.length - (d.shown.length - 1)) : '';
}

/**
 * The directory finder: `dirs` (a promise of `{path, shown, note}`, best first) filtered as the
 * user types; a typed path (`/srv/app`, `~/src/x`) can be opened as it is. `pick(path)` runs on
 * enter or a click, with the full path.
 */
export function finder({ title, dirs, pick }) {
  let home = '';
  const box = el('div', 'pop-menu pop-finder');
  box.setAttribute('aria-label', title);
  box.append(el('div', 'pop-title', title));
  const input = el('input');
  input.type = 'text';
  input.placeholder = 'Type to filter, or a path like ~/src/app';
  input.setAttribute('aria-label', 'Directory');
  const list = el('div', 'pop-list');
  list.append(el('p', 'pop-text', 'Reading the directories…'));
  box.append(input, list);
  let all = [];
  let buttons = [];
  const typedPath = () => {
    const q = input.value.trim();
    if (!/^(\/|~(\/|$))/.test(q)) return null;
    return q.startsWith('~') && home ? home + q.slice(1) : q;
  };
  const draw = () => {
    const q = input.value.trim();
    const typed = typedPath();
    let rows = typed
      ? all.filter((d) => d.path.startsWith(typed.replace(/\/$/, '')))
      : all
          .map((d, i) => ({ d, i, s: score(d.shown, q) }))
          .filter((r) => r.s !== null)
          .sort((a, b) => (q ? b.s - a.s || a.i - b.i : a.i - b.i))
          .map((r) => r.d);
    rows = rows.slice(0, 50);
    if (typed && !rows.some((d) => d.path === typed.replace(/\/$/, ''))) rows.unshift({ path: typed, shown: q, note: 'typed' });
    buttons = rows.map((d) => {
      const b = el('button', 'pop-row');
      b.type = 'button';
      b.append(el('span', 'pop-label', d.shown), el('span', 'pop-note', d.note));
      b.addEventListener('click', () => {
        close();
        pick(d.path);
      });
      return b;
    });
    list.replaceChildren(...(buttons.length ? buttons : [el('p', 'pop-text', 'No directory matches. Type a path like ~/src/app.')]));
    if (buttons[0]) buttons[0].classList.add('first');
  };
  input.addEventListener('input', draw);
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && buttons[0]) {
      e.preventDefault();
      buttons[0].click();
    }
  });
  show(box, null, (e) => (document.activeElement === input ? ['ArrowDown', 'ArrowUp'].includes(e.key) && rowKeys(buttons, e, false) : rowKeys(buttons, e, false)));
  input.focus();
  dirs.then((got) => {
    all = got;
    home = homeOf(got);
    if (open && open.box === box) draw();
  });
}

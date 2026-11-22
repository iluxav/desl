// Forms on desl web's page (docs/design/2026-11-21-web-forms.md): the widgets an agent asks for,
// drawn by the page's own code from the form's JSON. Agent text goes in as text nodes, or through
// the page's Markdown renderer, which makes elements and never HTML.

function el(tag, className, text) {
  const e = document.createElement(tag);
  if (className) e.className = className;
  if (text !== undefined && text !== null) e.textContent = text;
  return e;
}

let seq = 0;
/** An id for a label's `for`, unique on the page. */
function uid() {
  seq += 1;
  return `fw-${seq}`;
}

function heading(w, into) {
  if (!w.label) return;
  const h = el('div', 'fw-label', w.label);
  if (w.optional) h.append(el('span', 'fw-optional', 'optional'));
  into.append(h);
}

const DRAW = {
  markdown(w, into, ctx) {
    into.append(ctx.markdown(w.text || ''));
  },

  choice(w, into, ctx, value) {
    heading(w, into);
    const name = uid();
    // A draft or an answer wins; else where the agent said it starts.
    const given = value === undefined || value === null ? w.value : value;
    const picked = new Set(Array.isArray(given) ? given : given === undefined || given === null ? [] : [given]);
    const labels = (w.options || []).map((o) => o.label);
    for (const o of w.options || []) {
      const row = el('label', 'fw-option');
      const box = el('input');
      box.type = w.multi ? 'checkbox' : 'radio';
      box.name = name;
      box.value = o.label;
      box.checked = picked.has(o.label);
      const words = el('span', 'fw-option-words');
      words.append(el('span', 'fw-option-label', o.label));
      if (o.detail) words.append(el('span', 'fw-option-detail', o.detail));
      row.append(box, words);
      into.append(row);
    }
    if (w.other) {
      const row = el('label', 'fw-option fw-other');
      const box = el('input');
      box.type = w.multi ? 'checkbox' : 'radio';
      box.name = name;
      box.value = '';
      box.dataset.other = 'true';
      const text = el('input', 'fw-other-text');
      text.type = 'text';
      text.placeholder = 'Something else';
      const typed = [...picked].find((p) => !labels.includes(p));
      if (typed !== undefined) {
        box.checked = true;
        text.value = typed;
      }
      text.addEventListener('input', () => {
        box.checked = text.value.trim() !== '';
        box.dispatchEvent(new Event('change', { bubbles: true }));
      });
      row.append(box, text);
      into.append(row);
    }
  },

  checklist(w, into, ctx, value) {
    heading(w, into);
    const list = el('ol', 'fw-steps');
    const byId = new Map((w.items || []).map((s) => [s.id, s]));
    // A draft or an answer keeps the user's order, ticks and notes.
    const order = Array.isArray(value) && value.length ? value : (w.items || []).map((s) => ({ id: s.id, checked: s.checked !== false }));
    for (const v of order) {
      const s = byId.get(v.id);
      if (!s) continue;
      const li = el('li', 'fw-step');
      li.dataset.id = s.id;
      const box = el('input');
      box.type = 'checkbox';
      box.checked = v.checked !== false;
      const words = el('span', 'fw-option-words');
      words.append(el('span', 'fw-option-label', s.label));
      if (s.detail) words.append(el('span', 'fw-option-detail', s.detail));
      const note = el('input', 'fw-note');
      note.type = 'text';
      note.placeholder = 'Note';
      note.value = v.note || '';
      const line = el('label', 'fw-step-line');
      line.append(box, words);
      li.append(line);
      if (w.reorder) {
        const moves = el('span', 'fw-moves');
        for (const [glyph, dir, title] of [['↑', -1, 'Move up'], ['↓', 1, 'Move down']]) {
          const b = el('button', 'fw-move', glyph);
          b.type = 'button';
          b.title = title;
          b.setAttribute('aria-label', `${title}: ${s.label}`);
          b.addEventListener('click', () => {
            const sib = dir < 0 ? li.previousElementSibling : li.nextElementSibling;
            if (!sib) return;
            if (dir < 0) list.insertBefore(li, sib);
            else list.insertBefore(sib, li);
            b.focus();
            list.dispatchEvent(new Event('change', { bubbles: true }));
          });
          moves.append(b);
        }
        li.append(moves);
      }
      li.append(note);
      list.append(li);
    }
    into.append(list);
  },

  text(w, into, ctx, value) {
    const id = uid();
    const label = el('label', 'fw-label', w.label);
    label.htmlFor = id;
    if (w.optional) label.append(el('span', 'fw-optional', 'optional'));
    const lines = Math.max(1, Math.min(20, w.lines || 1));
    const input = lines > 1 ? el('textarea', 'fw-text') : el('input', 'fw-text');
    if (lines > 1) input.rows = lines;
    else input.type = 'text';
    input.id = id;
    input.placeholder = w.placeholder || '';
    input.value = typeof value === 'string' ? value : w.value || '';
    into.append(label, input);
  },

  number(w, into, ctx, value) {
    const id = uid();
    const label = el('label', 'fw-label', w.label);
    label.htmlFor = id;
    if (w.optional) label.append(el('span', 'fw-optional', 'optional'));
    const row = el('div', 'fw-number');
    const range = el('input', 'fw-range');
    range.type = 'range';
    const box = el('input', 'fw-number-box');
    box.type = 'number';
    box.id = id;
    for (const i of [range, box]) {
      i.min = String(w.min);
      i.max = String(w.max);
      i.step = w.step ? String(w.step) : 'any';
    }
    const start = typeof value === 'number' ? value : typeof w.value === 'number' ? w.value : w.min;
    range.value = String(start);
    box.value = String(start);
    range.addEventListener('input', () => { box.value = range.value; });
    box.addEventListener('input', () => { range.value = box.value; });
    row.append(range, box);
    if (w.unit) row.append(el('span', 'fw-unit', w.unit));
    into.append(label, row);
  },

  toggle(w, into, ctx, value) {
    const row = el('label', 'fw-toggle');
    const box = el('input');
    box.type = 'checkbox';
    box.setAttribute('role', 'switch');
    box.checked = typeof value === 'boolean' ? value : Boolean(w.value);
    row.append(box, el('span', 'fw-option-label', w.label));
    into.append(row);
  },

  select(w, into, ctx, value) {
    const id = uid();
    const label = el('label', 'fw-label', w.label);
    label.htmlFor = id;
    if (w.optional) label.append(el('span', 'fw-optional', 'optional'));
    const sel = el('select', 'fw-select');
    sel.id = id;
    const none = el('option', '', 'Choose…');
    none.value = '';
    sel.append(none);
    for (const o of w.options || []) {
      const opt = el('option', '', o.detail ? `${o.label} — ${o.detail}` : o.label);
      opt.value = o.label;
      sel.append(opt);
    }
    sel.value = typeof value === 'string' ? value : w.value || '';
    into.append(label, sel);
  },

  preview(w, into, ctx, value) {
    const top = el('div', 'fw-preview-top');
    heading(w, top);
    const wide = el('button', 'fw-wide', 'Wide');
    wide.type = 'button';
    wide.title = 'Give the preview the width of the waiting list too';
    wide.addEventListener('click', () => {
      const on = document.body.classList.toggle('wide');
      for (const b of document.querySelectorAll('.fw-wide')) b.textContent = on ? 'Narrow' : 'Wide';
    });
    top.append(wide);
    into.append(top);
    const controls = w.controls || [];
    const start = (c) => {
      const given = value && typeof value === 'object' ? value[c.var] : undefined;
      if (given !== undefined && given !== null) return given;
      if (c.value !== undefined && c.value !== null) return c.value;
      if (c.kind === 'range') return c.min;
      if (c.kind === 'select') return (c.options || [])[0] || '';
      return '#000000';
    };
    const vars = new Map(controls.map((c) => [c.var, start(c)]));
    const box = el('div', 'fw-preview-box');
    const frame = el('iframe', 'fw-preview-frame');
    // Its own document, same origin so the page can fill it, and no scripts: nothing the agent
    // wrote runs, and the page's CSP holds inside it too.
    frame.setAttribute('sandbox', 'allow-same-origin');
    frame.title = w.label ? `Preview: ${w.label}` : 'Preview';
    frame.srcdoc = '<!doctype html><html><head><meta charset="utf-8"></head><body></body></html>';
    const paint = () => {
      const root = frame.contentDocument && frame.contentDocument.documentElement;
      if (!root) return;
      for (const c of controls) root.style.setProperty(c.var, cssValue(c, vars.get(c.var)));
    };
    // Each load, as after the card is put back on the page, draws the agent's page again.
    frame.addEventListener('load', () => {
      fill(frame, w);
      paint();
    });
    box.append(frame);
    into.append(box);
    if (!controls.length) return;
    const list = el('div', 'fw-controls');
    for (const c of controls) {
      const id = uid();
      const row = el('div', 'fw-control');
      row.dataset.var = c.var;
      row.dataset.kind = c.kind === 'color' ? 'colour' : c.kind;
      const label = el('label', 'fw-control-label', c.label || c.var);
      label.htmlFor = id;
      row.append(label);
      const set = (v) => {
        vars.set(c.var, v);
        paint();
      };
      if (row.dataset.kind === 'range') {
        const range = el('input', 'fw-range');
        range.type = 'range';
        const num = el('input', 'fw-number-box');
        num.type = 'number';
        num.id = id;
        for (const i of [range, num]) {
          i.min = String(c.min);
          i.max = String(c.max);
          i.step = c.step ? String(c.step) : 'any';
          i.value = String(vars.get(c.var));
        }
        range.addEventListener('input', () => {
          num.value = range.value;
          set(Number(range.value));
        });
        num.addEventListener('input', () => {
          if (num.value === '') return;
          range.value = num.value;
          set(Number(num.value));
        });
        const line = el('div', 'fw-number');
        line.append(range, num);
        if (c.unit) line.append(el('span', 'fw-unit', c.unit));
        row.append(line);
      } else if (row.dataset.kind === 'colour') {
        const pick = el('input', 'fw-colour');
        pick.type = 'color';
        pick.id = id;
        pick.value = String(vars.get(c.var)).toLowerCase();
        const code = el('span', 'fw-unit', pick.value);
        pick.addEventListener('input', () => {
          code.textContent = pick.value;
          set(pick.value);
        });
        const line = el('div', 'fw-number');
        line.append(pick, code);
        row.append(line);
      } else {
        const sel = el('select', 'fw-select');
        sel.id = id;
        for (const o of c.options || []) {
          const opt = el('option', '', o);
          opt.value = o;
          sel.append(opt);
        }
        sel.value = String(vars.get(c.var));
        sel.addEventListener('change', () => set(sel.value));
        row.append(sel);
      }
      list.append(row);
    }
    into.append(list);
  },

  table(w, into, ctx, value) {
    heading(w, into);
    const columns = w.columns || [];
    // A draft or an answer: the cells the user changed, by row.
    const changed = new Map((Array.isArray(value) ? value : []).map((r) => [r.id, r]));
    const wrap = el('div', 'fw-table-wrap');
    const table = el('table', 'fw-table');
    const head = el('tr');
    for (const c of columns) head.append(el('th', c.edit ? 'fw-edit' : '', c.label || c.id));
    const thead = el('thead');
    thead.append(head);
    const body = el('tbody');
    for (const row of w.rows || []) {
      const tr = el('tr');
      tr.dataset.row = row.id;
      const now = changed.get(row.id) || {};
      for (const c of columns) {
        const was = row[c.id];
        const cur = c.id in now ? now[c.id] : was;
        const td = el('td');
        const name = `${c.label || c.id}, ${row.id}`;
        if (c.edit === 'check') {
          const box = el('input', 'fw-cell');
          box.type = 'checkbox';
          box.checked = cur === true;
          box.setAttribute('aria-label', name);
          box.dataset.col = c.id;
          box.dataset.was = JSON.stringify(was === true);
          td.append(box);
        } else if (c.edit === 'select') {
          const sel = el('select', 'fw-cell');
          sel.setAttribute('aria-label', name);
          const none = el('option', '', '—');
          none.value = '';
          sel.append(none);
          for (const o of c.options || []) {
            const opt = el('option', '', o);
            opt.value = o;
            sel.append(opt);
          }
          sel.value = typeof cur === 'string' ? cur : '';
          sel.dataset.col = c.id;
          sel.dataset.was = JSON.stringify(typeof was === 'string' ? was : '');
          td.append(sel);
        } else if (c.edit === 'text') {
          const input = el('input', 'fw-cell');
          input.type = 'text';
          input.value = typeof cur === 'string' ? cur : '';
          input.setAttribute('aria-label', name);
          input.dataset.col = c.id;
          input.dataset.was = JSON.stringify(typeof was === 'string' ? was : '');
          td.append(input);
        } else {
          const shown = c.id === 'id' ? row.id : was;
          td.textContent = shown === true ? '✓' : shown === false || shown === null || shown === undefined ? '' : String(shown);
        }
        tr.append(td);
      }
      body.append(tr);
    }
    table.append(thead, body);
    wrap.append(table);
    into.append(wrap);
  },

  images(w, into, ctx, value) {
    heading(w, into);
    const name = uid();
    const given = value === undefined || value === null ? w.value : value;
    const picked = new Set(Array.isArray(given) ? given : given === undefined || given === null ? [] : [given]);
    const grid = el('div', 'fw-images');
    for (const p of w.images || []) {
      const tile = el('label', 'fw-image');
      const box = el('input');
      box.type = w.multi ? 'checkbox' : 'radio';
      box.name = name;
      box.value = p.path;
      box.checked = picked.has(p.path);
      const frame = el('span', 'fw-image-frame');
      // The file's name stands in until the picture comes, and for good if it cannot be read.
      const stand = el('span', 'fw-image-name', p.path);
      frame.append(stand);
      const words = el('span', 'fw-image-words');
      words.append(el('span', 'fw-option-label', p.caption || p.path.split('/').pop()));
      if (p.caption) words.append(el('span', 'fw-option-detail', p.path));
      tile.append(box, frame, words);
      grid.append(tile);
      if (ctx.image) {
        ctx.image(p.path).then((got) => {
          if (got.url) {
            const img = el('img', 'fw-image-img');
            img.alt = p.caption || p.path;
            img.src = got.url;
            frame.replaceChildren(img);
          } else if (got.error) {
            stand.title = got.error;
            frame.append(el('span', 'fw-image-why', 'could not be read'));
          }
        });
      }
    }
    into.append(grid);
  },
};

/** What a control sets its variable to: a range's number with its unit. */
function cssValue(c, v) {
  return c.kind === 'range' && c.unit ? `${v}${c.unit}` : String(v);
}

// What a preview never keeps: it would load, run or go somewhere. The frame runs no script
// anyway (no `allow-scripts`, and no `'unsafe-inline'` in the CSP); this is the second lock.
const DROP = 'script, iframe, frame, frameset, object, embed, applet, base, meta, link, noscript, portal';

/**
 * Draws the agent's HTML in the preview's frame. Its `<style>` elements and `style` attributes
 * become a constructed stylesheet and CSSOM, since the page's CSP refuses inline style.
 */
function fill(frame, w) {
  const doc = frame.contentDocument;
  if (!doc || !doc.body) return;
  const parsed = new DOMParser().parseFromString(w.html || '', 'text/html');
  for (const bad of parsed.querySelectorAll(DROP)) bad.remove();
  let css = w.css || '';
  for (const s of parsed.querySelectorAll('style')) {
    css += `\n${s.textContent}`;
    s.remove();
  }
  const inline = [];
  for (const e of parsed.querySelectorAll('*')) {
    for (const a of [...e.attributes]) {
      const name = a.name.toLowerCase();
      if (name.startsWith('on') || name === 'srcdoc' || /^[\s\u0000-\u001f]*(javascript|vbscript):/i.test(a.value)) {
        e.removeAttribute(a.name);
      } else if (name === 'style') {
        e.removeAttribute(a.name);
        e.setAttribute('data-desl-s', String(inline.length));
        inline.push(a.value);
      }
    }
  }
  for (const a of [...parsed.body.attributes]) doc.body.setAttribute(a.name, a.value);
  for (const a of [...parsed.documentElement.attributes]) if (a.name !== 'style') doc.documentElement.setAttribute(a.name, a.value);
  doc.body.replaceChildren(...[...parsed.body.childNodes].map((n) => doc.importNode(n, true)));
  for (const e of doc.querySelectorAll('[data-desl-s]')) {
    e.style.cssText = inline[Number(e.getAttribute('data-desl-s'))] || '';
    e.removeAttribute('data-desl-s');
  }
  try {
    const sheet = new frame.contentWindow.CSSStyleSheet();
    sheet.replaceSync(css);
    doc.adoptedStyleSheets = [sheet];
  } catch (e) {
    doc.adoptedStyleSheets = [];
  }
  // A link would take the frame away from the preview; it stays where it is.
  doc.addEventListener('click', (e) => {
    if (e.target instanceof doc.defaultView.Element && e.target.closest('a, area')) e.preventDefault();
  });
}

/** What a widget holds now: undefined when nothing is given. */
function read(w, box) {
  switch (w.kind) {
    case 'choice': {
      const chosen = [...box.querySelectorAll('input[type=radio], input[type=checkbox]')]
        .filter((i) => i.checked)
        .map((i) => (i.dataset.other ? box.querySelector('.fw-other-text').value.trim() : i.value))
        .filter((v) => v !== '');
      if (w.multi) return chosen.length ? chosen : undefined;
      return chosen[0];
    }
    case 'checklist':
      return [...box.querySelectorAll('.fw-step')].map((li) => {
        const v = { id: li.dataset.id, checked: li.querySelector('input[type=checkbox]').checked };
        const note = li.querySelector('.fw-note').value.trim();
        if (note) v.note = note;
        return v;
      });
    case 'text': {
      const t = box.querySelector('.fw-text').value;
      return t.trim() === '' ? undefined : t;
    }
    case 'number': {
      const n = Number(box.querySelector('.fw-number-box').value);
      return Number.isFinite(n) && box.querySelector('.fw-number-box').value !== '' ? n : undefined;
    }
    case 'toggle':
      return box.querySelector('input').checked;
    case 'select': {
      const v = box.querySelector('select').value;
      return v === '' ? undefined : v;
    }
    case 'images': {
      const chosen = [...box.querySelectorAll('.fw-image input')].filter((i) => i.checked).map((i) => i.value);
      if (w.multi) return chosen.length ? chosen : undefined;
      return chosen[0];
    }
    case 'table': {
      const out = [];
      for (const tr of box.querySelectorAll('tr[data-row]')) {
        const row = { id: tr.dataset.row };
        for (const cell of tr.querySelectorAll('[data-col]')) {
          const now = cell.type === 'checkbox' ? cell.checked : cell.value;
          if (now !== JSON.parse(cell.dataset.was)) row[cell.dataset.col] = now;
        }
        if (Object.keys(row).length > 1) out.push(row);
      }
      return out;
    }
    case 'preview': {
      const out = {};
      for (const row of box.querySelectorAll('.fw-control')) {
        if (row.dataset.kind === 'range') out[row.dataset.var] = Number(row.querySelector('.fw-range').value);
        else if (row.dataset.kind === 'colour') out[row.dataset.var] = row.querySelector('.fw-colour').value;
        else out[row.dataset.var] = row.querySelector('select').value;
      }
      return out;
    }
    default:
      return undefined;
  }
}

/** Every widget's value as the card holds it, keyed by widget id; nothing for what is empty. */
export function valuesOf(form, cardEl) {
  const out = {};
  for (const w of form.widgets || []) {
    if (w.kind === 'markdown') continue;
    const box = cardEl.querySelector(`[data-widget="${CSS.escape(w.id)}"]`);
    if (!box) continue;
    const v = read(w, box);
    if (v !== undefined) out[w.id] = v;
  }
  return out;
}

/** The answer to send, or null while a widget that is not optional has nothing. */
export function answerOf(form, cardEl) {
  const v = valuesOf(form, cardEl);
  for (const w of form.widgets || []) {
    if (w.kind === 'markdown' || w.optional) continue;
    if (v[w.id] === undefined) return null;
  }
  return v;
}

const CLOSED = {
  user: 'Answered',
  dismissed: 'Dismissed',
  withdrawn: 'Withdrawn by the agent',
};

/**
 * A form's card. `got` is the daemon's `form` reply: the form, its draft, its answer, how it
 * closed. `opts`: `markdown(text)` renders Markdown as nodes; `image(path)` resolves to `{url}` or
 * `{error}` for a picture the form shows; `readOnly` and `note` for a card
 * that cannot be answered here; `onDraft(values)`, `onSend(answer)` (resolves to an error
 * sentence or null) and `onDismiss()` for a live one.
 */
export function card(got, opts) {
  const form = got.form || {};
  const closed = got.closed_by || null;
  const live = !closed && !opts.readOnly;
  const root = el('section', live ? 'form-card' : 'form-card done');
  root.dataset.form = got.form_id;
  root.setAttribute('aria-label', `Form: ${form.title || got.form_id}`);
  const top = el('div', 'form-top');
  top.append(el('span', 'form-mark', '▣'), el('h3', 'form-title', form.title || ''));
  top.append(el('span', 'form-id', got.form_id));
  root.append(top);
  const values = closed === 'user' ? got.answer || {} : got.draft || {};
  const ctx = { markdown: opts.markdown, image: opts.image };
  for (const w of form.widgets || []) {
    const box = el('div', `fw fw-kind-${w.kind}`);
    box.dataset.widget = w.id;
    const draw = DRAW[w.kind];
    if (draw) draw(w, box, ctx, values[w.id]);
    else box.append(el('p', 'fw-unknown', `This page does not know the widget "${w.kind}". Update desl.`));
    root.append(box);
  }
  if (!live) {
    for (const i of root.querySelectorAll('input, textarea, select, button:not(.fw-wide)')) i.disabled = true;
    const said = closed ? CLOSED[closed] || closed : opts.note || '';
    if (said) root.append(el('p', 'form-state', said));
    return root;
  }
  const bar = el('div', 'form-actions');
  const err = el('p', 'form-error');
  err.setAttribute('role', 'status');
  const dismiss = el('button', 'secondary', 'Dismiss');
  dismiss.type = 'button';
  dismiss.title = 'Close it unanswered; the agent hears so before its next prompt';
  const send = el('button', 'primary', form.send_label || 'Send');
  send.type = 'button';
  bar.append(err, dismiss, send);
  root.append(bar);
  let timer = null;
  const changed = () => {
    send.disabled = answerOf(form, root) === null;
    clearTimeout(timer);
    timer = setTimeout(() => opts.onDraft(valuesOf(form, root)), 1000);
  };
  root.addEventListener('input', changed);
  root.addEventListener('change', changed);
  send.disabled = answerOf(form, root) === null;
  send.addEventListener('click', async () => {
    const answer = answerOf(form, root);
    if (!answer) return;
    clearTimeout(timer);
    send.disabled = true;
    err.textContent = '';
    const why = await opts.onSend(answer);
    if (why) {
      err.textContent = why;
      send.disabled = false;
    }
  });
  dismiss.addEventListener('click', async () => {
    clearTimeout(timer);
    const why = await opts.onDismiss();
    if (why) err.textContent = why;
  });
  return root;
}

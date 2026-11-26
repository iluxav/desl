// The browser's saved prompts, separated by the project's full path. Each prompt has its own
// storage entry, so adding one in another tab cannot overwrite the rest of the list.
import * as pop from '/popover.js';

const PREFIX = 'desl-web-prompt:';
const TAB = 'desl-web-project-tab';
const $ = (id) => document.getElementById(id);

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function copyIcon() {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('class', 'ico');
  svg.setAttribute('aria-hidden', 'true');
  const path = document.createElementNS(svg.namespaceURI, 'path');
  path.setAttribute('d', 'M8 8h12v12H8zM16 8V4H4v12h4');
  svg.append(path);
  return svg;
}

function error(id, text = '') {
  $(id).textContent = text;
  $(id).hidden = !text;
}

function projectPrefix(path) {
  return `${PREFIX}${encodeURIComponent(path)}:`;
}

/** Installs the two tabs and the prompt editor; update follows the selected thread's project. */
export function setupPrompts({ notice }) {
  let project = null;
  // Changing threads, tabs or collapsing the sidebar never discards an editor's unsaved text.
  const drafts = new Map();
  let tab = 'tasks';
  try { if (localStorage.getItem(TAB) === 'prompts') tab = 'prompts'; } catch { /* default */ }

  function showTab(next) {
    tab = next;
    for (const name of ['tasks', 'prompts']) {
      const active = name === tab;
      const button = $(`tab-${name}`);
      button.setAttribute('aria-selected', String(active));
      button.tabIndex = active ? 0 : -1;
      $(name).hidden = !active;
    }
    $('prompt-add').hidden = tab !== 'prompts';
    try { localStorage.setItem(TAB, tab); } catch { /* This tab keeps the choice. */ }
  }

  for (const name of ['tasks', 'prompts']) {
    const button = $(`tab-${name}`);
    button.addEventListener('click', () => showTab(name));
    button.addEventListener('keydown', (e) => {
      if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(e.key)) return;
      e.preventDefault();
      showTab(e.key === 'Home' ? 'tasks' : e.key === 'End' ? 'prompts' : tab === 'tasks' ? 'prompts' : 'tasks');
      $(`tab-${tab}`).focus();
    });
  }

  function readList() {
    const prefix = projectPrefix(project.path);
    const list = [];
    for (let i = 0; i < localStorage.length; i++) {
      const key = localStorage.key(i);
      if (!key || !key.startsWith(prefix)) continue;
      const raw = localStorage.getItem(key);
      if (raw === null) continue;
      const value = JSON.parse(raw);
      if (!value || typeof value.text !== 'string' || typeof value.created !== 'number') {
        throw new Error('Invalid saved prompt');
      }
      list.push({ key, raw, ...value });
    }
    return list.sort((a, b) => b.created - a.created || a.key.localeCompare(b.key));
  }

  function drawList() {
    const list = $('prompt-list');
    list.replaceChildren();
    error('prompt-list-error');
    if (!project) {
      list.append(el('p', 'empty', 'Open a project to save prompts for it.'));
      return;
    }
    let rows;
    try { rows = readList(); } catch {
      error('prompt-list-error', 'Saved prompts could not be read. Allow browser storage and reload this page; existing entries have been kept.');
      return;
    }
    if (!rows.length) {
      list.append(el('p', 'empty', 'No saved prompts yet. Use + to add one for this project.'));
    }
    for (const row of rows) {
      const item = el('article', 'saved-prompt');
      const title = row.text.trim().split('\n')[0].slice(0, 80);
      const body = el('p', 'saved-prompt-text', row.text);
      body.title = row.text;
      const actions = el('div', 'saved-prompt-actions');
      const copy = el('button', 'icon-button');
      copy.type = 'button';
      copy.title = 'Copy prompt';
      copy.setAttribute('aria-label', `Copy prompt: ${title}`);
      copy.append(copyIcon());
      copy.addEventListener('click', async () => {
        try {
          await navigator.clipboard.writeText(row.text);
          notice('Prompt copied to the clipboard.');
        } catch {
          notice('The prompt could not be copied. Choose Edit in its menu and copy the text there.', true);
        }
      });
      const more = el('button', 'icon-button', '⋯');
      more.type = 'button';
      more.title = 'Edit or delete prompt';
      more.setAttribute('aria-label', `Actions for prompt: ${title}`);
      more.setAttribute('aria-haspopup', 'dialog');
      const owner = project.path;
      more.addEventListener('click', () => pop.menu({
        title: 'Prompt', anchor: more,
        rows: [
          { label: 'Edit', disabled: drafts.has(owner), run: () => edit(row, owner) },
          { label: 'Delete', run: () => pop.confirm({
            title: 'Delete prompt?', text: 'This removes the saved prompt from this project.', yesLabel: 'Delete',
            yes: () => remove(row, owner),
          }) },
        ],
      }));
      actions.append(copy, more);
      item.append(body, actions);
      list.append(item);
    }
  }

  function drawEditor() {
    const draft = project && drafts.get(project.path);
    $('prompt-editor').hidden = !draft;
    $('prompt-add').disabled = !project || Boolean(draft);
    $('prompt-text').value = draft ? draft.text : '';
    $('prompt-editor-label').textContent = draft && draft.key ? 'Edit prompt' : 'New prompt';
    $('prompt-save').disabled = !draft || !draft.text.trim();
    error('prompt-editor-error');
  }

  function edit(row = null, owner = project?.path) {
    if (project?.path !== owner) {
      notice('The selected project changed. Return to that project to edit its prompt.', true);
      return;
    }
    if (!project || drafts.has(project.path)) return;
    drafts.set(project.path, { text: row ? row.text : '', key: row && row.key, raw: row && row.raw, created: row && row.created });
    drawEditor();
    $('prompt-text').focus();
  }

  function cancel() {
    if (project) drafts.delete(project.path);
    drawEditor();
    $('prompt-add').focus();
  }

  function remove(row, owner) {
    try {
      if (localStorage.getItem(row.key) !== row.raw) {
        error('prompt-list-error', 'This prompt changed in another tab. Open its menu again to delete the current version.');
        return;
      }
      localStorage.removeItem(row.key);
      if (drafts.get(owner)?.key === row.key) drafts.delete(owner);
      drawList();
      drawEditor();
      $('prompt-add').focus();
      notice('Prompt deleted.');
    } catch {
      error('prompt-list-error', 'The prompt could not be deleted. Allow browser storage and try again.');
    }
  }

  $('prompt-add').addEventListener('click', () => edit());
  $('prompt-cancel').addEventListener('click', cancel);
  $('prompt-text').addEventListener('input', () => {
    const draft = project && drafts.get(project.path);
    if (!draft) return;
    draft.text = $('prompt-text').value;
    $('prompt-save').disabled = !draft.text.trim();
  });
  $('prompt-editor').addEventListener('submit', (e) => {
    e.preventDefault();
    const draft = project && drafts.get(project.path);
    if (!draft || !draft.text.trim()) return;
    try {
      if (draft.key && localStorage.getItem(draft.key) !== draft.raw) {
        error('prompt-editor-error', 'This prompt changed in another tab. Copy your text, then press Cancel and reopen Edit to review the current version.');
        return;
      }
      const key = draft.key || `${projectPrefix(project.path)}${crypto.randomUUID()}`;
      localStorage.setItem(key, JSON.stringify({ text: draft.text, created: draft.created || Date.now() }));
    } catch {
      error('prompt-editor-error', 'The prompt could not be saved. Your text is still here; allow browser storage or free some space and press Save prompt again.');
      return;
    }
    drafts.delete(project.path);
    drawEditor();
    drawList();
    $('prompt-add').focus();
    notice('Prompt saved.');
  });

  window.addEventListener('storage', (e) => {
    if (project && (!e.key || e.key.startsWith(projectPrefix(project.path)))) drawList();
  });
  showTab(tab);
  drawList();
  drawEditor();

  return {
    update(current) {
      if (project?.path === current?.path) return;
      project = current || null;
      drawList();
      drawEditor();
    },
  };
}

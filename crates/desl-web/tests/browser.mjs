// Run with `node --test crates/desl-web/tests/browser.mjs` (Playwright and Chromium installed).
// Only the daemon connection is a fixture; these tests use the real page, storage and clipboard.
import { after, before, test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
const { chromium } = createRequire(import.meta.url)('playwright');
const pageRoot = new URL('../page/', import.meta.url);
const key = '0123456789abcdef0123456789abcdef';
let browser, server, origin;

const project = (id, name) => ({
  id, name, path: `/src/${id}`, hue: [90, 160, 120], dirty: 0, branch: 'main', parent: '/src',
  threads: [{ id: `t-${id}`, title: `${name} thread`, agent: 'claude', status: 'idle', word: 'idle', glyph: '○', age: 'now',
    header: { project: name, mode: 'edits', model: 'opus', yolo: false } }],
});
const view = { projects: [project('one', 'First'), project('two', 'Second')], task_lists: [{ id: 'one', name: 'First' }], waiting: [], forms: [], screen: [] };

before(async () => {
  server = createServer(async (req, res) => {
    const path = new URL(req.url, 'http://localhost').pathname;
    try {
      const file = new URL(path === '/' ? 'index.html' : path.slice(1), pageRoot);
      if (!file.href.startsWith(pageRoot.href)) throw new Error('Outside page');
      const body = await readFile(file);
      res.writeHead(200, { 'Content-Type': path.endsWith('.js') || path.endsWith('.mjs') ? 'text/javascript' : path.endsWith('.css') ? 'text/css' : 'text/html' });
      res.end(body);
    } catch { res.writeHead(404).end(); }
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  origin = `http://127.0.0.1:${server.address().port}`;
  browser = await chromium.launch({ executablePath: process.env.CHROMIUM || '/usr/bin/chromium', headless: true });
});
after(async () => {
  await browser?.close();
  if (server) await new Promise((resolve) => server.close(resolve));
});

async function fixture(t, options = {}) {
  const context = await browser.newContext({ viewport: { width: 1440, height: 1000 }, permissions: ['clipboard-read', 'clipboard-write'], ...options });
  t.after(() => context.close());
  const errors = [];
  const sockets = new Set();
  const input = [];
  const commands = [];
  let currentView = structuredClone(view);
  let removalError = null;
  const publish = (next) => {
    currentView = next;
    for (const socket of sockets) socket.send(JSON.stringify({ type: 'view', view: next }));
  };
  context.on('page', (page) => page.on('pageerror', (e) => errors.push(e.message)));
  t.after(() => assert.deepEqual(errors, [], 'no page errors'));
  await context.routeWebSocket('**/ws', (ws) => {
    sockets.add(ws);
    ws.onClose(() => sockets.delete(ws));
    ws.onMessage((raw) => {
      const msg = JSON.parse(raw);
      if (msg.token) {
        ws.send(JSON.stringify({ type: 'bridge', state: 'ready', prefix: 'ctrl+space' }));
        ws.send(JSON.stringify({ type: 'view', view: currentView }));
      } else if (msg.type === 'conversation') {
        ws.send(JSON.stringify({ type: 'conversation', id: msg.id, thread_id: msg.thread_id, items: [], highlights: [] }));
      } else if (msg.command === 'remove_project') {
        commands.push(msg);
        if (!removalError) publish({ ...currentView,
          projects: currentView.projects.filter((p) => p.id !== msg.project_id),
          task_lists: currentView.task_lists.filter((p) => p.id !== msg.project_id),
        });
        ws.send(JSON.stringify({ type: 'reply', id: msg.id, ok: removalError ? null : { reply: 'done' }, error: removalError }));
      } else if (msg.command === 'input') {
        input.push({ thread: msg.thread_id, text: Buffer.from(msg.data, 'base64').toString() });
      } else if (msg.command === 'attach') {
        const replay = Buffer.from('\x1b[?2004hPaste fixture ready').toString('base64');
        ws.send(JSON.stringify({ type: 'reply', id: msg.id, ok: { replay, rows: 24, cols: 80 } }));
      } else if (msg.command) {
        ws.send(JSON.stringify({ type: 'reply', id: msg.id, ok: msg.command === 'agents' ? { agents: [] } : { tasks: [] } }));
      }
    });
  });
  async function open() {
    const page = await context.newPage();
    await page.goto(`${origin}/#t=${key}`);
    await page.locator('#link[data-state="connected"]').waitFor();
    return page;
  }
  return { context, page: await open(), open, input, commands, publish,
    refuseRemoval: (error) => { removalError = error; } };
}

async function prompts(page) { await page.getByRole('tab', { name: 'Prompts', exact: true }).click(); }
async function add(page, text) {
  await page.getByRole('button', { name: 'Add prompt', exact: true }).click();
  await page.getByLabel('New prompt', { exact: true }).fill(text);
  await page.getByRole('button', { name: 'Save prompt', exact: true }).click();
}
async function edit(page) {
  await page.getByRole('button', { name: /^Actions for prompt:/ }).first().click();
  await page.getByRole('button', { name: '1 Edit', exact: true }).click();
}

test('prompts stay with their project, preserve exact text and survive reload', async (t) => {
  const { page } = await fixture(t);
  await prompts(page);
  assert.equal(await page.getByRole('combobox', { name: 'Whose prompts' }).count(), 0, 'the left sidebar owns project selection');
  const text = '  Review this code.\n\nKeep <script>literal</script> text.  ';
  await add(page, text);
  assert.equal(await page.locator('.saved-prompt-text').textContent(), text);
  await page.reload();
  await page.locator('.saved-prompt').waitFor();
  await page.getByRole('button', { name: /^Copy prompt:/ }).click();
  assert.equal(await page.evaluate(() => navigator.clipboard.readText()), text);
  await page.locator('#projects .row').nth(1).click();
  assert.equal(await page.locator('.saved-prompt').count(), 0);
  await add(page, 'Second project only');
  await page.locator('#projects .row').first().click();
  assert.equal(await page.locator('.saved-prompt-text').textContent(), text);
});

test('a copy marks its own button and says nothing under the page, which has no prompt box', async (t) => {
  const { page } = await fixture(t);
  assert.equal(await page.locator('#prompt, #send, #prompt-form').count(), 0, 'the agent is prompted on its screen');
  await prompts(page);
  await add(page, 'Copy me');
  await page.reload();
  await page.locator('.saved-prompt').waitFor();
  const height = () => page.locator('.cols').evaluate((e) => e.getBoundingClientRect().height);
  const before = await height();
  const copy = page.getByRole('button', { name: /^Copy prompt:/ });
  await copy.click();
  await page.locator('.icon-button.copied').waitFor();
  assert.equal(await page.locator('#notice').textContent(), '');
  assert.equal(await page.locator('#notice').isVisible(), false);
  assert.equal(await height(), before, 'nothing moved');
  await page.locator('.icon-button.copied').waitFor({ state: 'detached' });
  assert.equal(await page.evaluate(() => navigator.clipboard.readText()), 'Copy me');
});

test("a thread's menu shows on hover alone, on the selected thread too", async (t) => {
  const { page } = await fixture(t);
  const line = page.locator('#projects .thread-line').filter({ has: page.locator('.row[aria-current="true"]') });
  const shown = async (sel) => line.locator(sel).evaluate((e) => getComputedStyle(e).opacity !== '0' && getComputedStyle(e).visibility !== 'hidden');
  await page.mouse.move(700, 500);
  assert.equal(await line.count(), 1);
  assert.equal(await shown('.row-more'), false);
  assert.equal(await shown('.age'), true);
  await line.hover();
  assert.equal(await shown('.row-more'), true);
  assert.equal(await shown('.age'), false);
  await page.mouse.move(700, 500);
  await line.locator('.row-more').focus();
  await page.keyboard.press('Shift+Tab');
  await page.keyboard.press('Tab');
  assert.equal(await shown('.row-more'), true, 'the keys reach it too');
});

test('Ctrl+V pastes a copied prompt into the agent screen as text, without an image shortcut', async (t) => {
  const { page, input } = await fixture(t);
  const text = 'Review this code.\nKeep the change small.';
  await prompts(page);
  await add(page, text);
  await page.getByRole('button', { name: /^Copy prompt:/ }).click();
  await page.getByRole('button', { name: 'Screen', exact: true }).click();
  await page.locator('.xterm-rows').getByText('Paste fixture ready', { exact: true }).waitFor();
  await page.locator('.xterm-helper-textarea').focus();
  await page.keyboard.press('Control+v');
  await new Promise((resolve) => setTimeout(resolve, 100));
  assert.deepEqual(input, [{ thread: 't-one', text: '\x1b[200~Review this code.\rKeep the change small.\x1b[201~' }]);
});

test('paste shortcuts use clipboard content: text stays text and pictures reach only agents', async (t) => {
  const { page, input, publish } = await fixture(t);
  await page.getByRole('button', { name: 'Screen', exact: true }).click();
  await page.locator('.xterm-rows').getByText('Paste fixture ready', { exact: true }).waitFor();
  await page.locator('.xterm-helper-textarea').focus();
  await page.evaluate(() => navigator.clipboard.writeText('Plain text'));
  await page.keyboard.press('Control+Shift+v');
  await new Promise((resolve) => setTimeout(resolve, 100));
  assert.deepEqual(input.splice(0), [{ thread: 't-one', text: '\x1b[200~Plain text\x1b[201~' }]);

  // A real browser paste from a synthetic PNG, without reading the user's clipboard.
  await page.evaluate(async () => {
    const canvas = document.createElement('canvas');
    canvas.width = canvas.height = 2;
    const blob = await new Promise((resolve) => canvas.toBlob(resolve));
    await navigator.clipboard.write([new ClipboardItem({ 'image/png': blob })]);
  });
  await page.keyboard.press('Control+v');
  await new Promise((resolve) => setTimeout(resolve, 100));
  assert.deepEqual(input.splice(0), [{ thread: 't-one', text: '\x16' }], 'Ctrl+V sends one image shortcut');
  // Chromium's paste-as-plain-text shortcut strips the image out of the paste event.
  await page.keyboard.press('Control+Shift+v');
  await new Promise((resolve) => setTimeout(resolve, 100));
  assert.equal(input.splice(0).some((item) => item.text.includes('\x16')), false);

  // Rich clipboard data that also holds text must take the text path.
  await page.locator('.xterm-helper-textarea').evaluate((node) => {
    const data = new DataTransfer();
    data.items.add(new File(['png'], 'image.png', { type: 'image/png' }));
    data.setData('text/plain', 'Text with a picture');
    node.dispatchEvent(new ClipboardEvent('paste', { bubbles: true, cancelable: true, clipboardData: data }));
  });
  await new Promise((resolve) => setTimeout(resolve, 100));
  assert.deepEqual(input.splice(0), [{ thread: 't-one', text: '\x1b[200~Text with a picture\x1b[201~' }]);

  const shell = project('shell', 'Shell');
  shell.threads[0].agent = 'shell';
  publish({ ...view, projects: [shell], task_lists: [] });
  await page.locator('#head .tid').filter({ hasText: 't-shell' }).waitFor();
  await page.locator('.xterm-rows').getByText('Paste fixture ready', { exact: true }).waitFor();
  await page.locator('.xterm-helper-textarea').focus();
  await page.keyboard.press('Control+v');
  await new Promise((resolve) => setTimeout(resolve, 100));
  assert.deepEqual(input, [], 'an image does not send an agent shortcut to a shell');
  await page.evaluate(() => navigator.clipboard.writeText('echo text'));
  await page.keyboard.press('Control+v');
  await new Promise((resolve) => setTimeout(resolve, 100));
  assert.deepEqual(input, [{ thread: 't-shell', text: '\x1b[200~echo text\x1b[201~' }]);
});

test('edit, cancel and delete keep the saved text and keyboard tabs work', async (t) => {
  const { page } = await fixture(t);
  await page.getByRole('tab', { name: 'Tasks', exact: true }).focus();
  await page.keyboard.press('ArrowRight');
  assert.equal(await page.getByRole('tab', { name: 'Prompts', exact: true }).getAttribute('aria-selected'), 'true');
  await add(page, 'Original');
  await edit(page);
  await page.locator('#prompt-text').fill('Cancelled');
  await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  assert.equal(await page.locator('.saved-prompt-text').textContent(), 'Original');
  await edit(page);
  await page.locator('#prompt-text').fill('Edited');
  await page.getByRole('button', { name: 'Save prompt', exact: true }).click();
  await page.getByRole('button', { name: /^Actions for prompt:/ }).click();
  await page.getByRole('button', { name: '2 Delete', exact: true }).click();
  await page.getByRole('button', { name: 'Delete', exact: true }).click();
  await page.reload();
  await page.locator('#link[data-state="connected"]').waitFor();
  assert.equal(await page.locator('.saved-prompt').count(), 0);
});

test('drafts survive project changes, tab changes and sidebar collapse', async (t) => {
  const { page } = await fixture(t);
  await prompts(page);
  await page.getByRole('button', { name: 'Add prompt', exact: true }).click();
  await page.locator('#prompt-text').fill('   ');
  assert.equal(await page.locator('#prompt-save').isDisabled(), true);
  await page.locator('#prompt-text').fill('Unfinished first project');
  await page.locator('#projects .row').nth(1).click();
  assert.equal(await page.locator('#prompt-editor').isVisible(), false);
  await page.locator('#projects .row').first().click();
  assert.equal(await page.locator('#prompt-text').inputValue(), 'Unfinished first project');
  await page.getByRole('tab', { name: 'Tasks', exact: true }).click();
  await prompts(page);
  await page.getByRole('button', { name: 'Collapse right sidebar', exact: true }).click();
  assert.equal(await page.locator('#wait').isVisible(), false);
  await page.getByRole('button', { name: 'Expand right sidebar', exact: true }).click();
  assert.equal(await page.locator('#prompt-text').inputValue(), 'Unfinished first project');
});

test('storage and clipboard failures explain recovery without losing the draft', async (t) => {
  const { page } = await fixture(t);
  await prompts(page);
  await add(page, 'Copy this prompt');
  await page.evaluate(() => {
    navigator.clipboard.writeText = async () => { throw new DOMException('Denied', 'NotAllowedError'); };
  });
  await page.getByRole('button', { name: /^Copy prompt:/ }).click();
  await page.waitForFunction(() => document.querySelector('#notice').textContent.includes('could not be copied'));
  await page.evaluate(() => {
    const set = Storage.prototype.setItem;
    Storage.prototype.setItem = function (key, value) {
      if (key.startsWith('desl-web-prompt:')) throw new DOMException('Quota', 'QuotaExceededError');
      return set.call(this, key, value);
    };
  });
  await add(page, 'Keep this draft');
  assert.equal(await page.locator('#prompt-text').inputValue(), 'Keep this draft');
  assert.match(await page.locator('#prompt-editor-error').textContent(), /could not be saved/);
  assert.equal(await page.locator('.saved-prompt').count(), 1);
});

test('a project removed while its prompt menu is open cannot edit another project', async (t) => {
  const { page, publish } = await fixture(t);
  await prompts(page);
  await add(page, 'Only for the first project');
  await page.getByRole('button', { name: /^Actions for prompt:/ }).click();
  publish({ ...view, projects: [view.projects[1]], task_lists: [] });
  await page.locator('#head .head-title').filter({ hasText: 'Second thread' }).waitFor();
  await page.getByRole('button', { name: '1 Edit', exact: true }).click();
  assert.equal(await page.locator('#prompt-editor').isVisible(), false);
  assert.match(await page.locator('#notice').textContent(), /selected project changed/);
});

test('another tab adds prompts live and cannot silently overwrite an edit', async (t) => {
  const { page, open } = await fixture(t);
  await prompts(page);
  await add(page, 'First saved');
  const other = await open();
  await other.locator('.saved-prompt').waitFor();
  await edit(page);
  await page.locator('#prompt-text').fill('Draft in first tab');
  await edit(other);
  await other.locator('#prompt-text').fill('Saved in second tab');
  await other.getByRole('button', { name: 'Save prompt', exact: true }).click();
  await page.waitForFunction(() => document.querySelector('.saved-prompt-text')?.textContent === 'Saved in second tab');
  await page.getByRole('button', { name: 'Save prompt', exact: true }).click();
  assert.match(await page.locator('#prompt-editor-error').textContent(), /changed in another tab/);
  assert.equal(await page.locator('#prompt-text').inputValue(), 'Draft in first tab');
  await add(other, 'A new prompt');
  await page.waitForFunction(() => document.querySelectorAll('.saved-prompt').length === 2);
});

test('sidebars restore visibility and widths without squeezing the conversation', async (t) => {
  const { page } = await fixture(t);
  await page.setViewportSize({ width: 1200, height: 900 });
  await page.locator('#grip-side').focus();
  await page.keyboard.press('Shift+ArrowRight');
  await page.getByRole('button', { name: 'Collapse projects sidebar', exact: true }).click();
  await page.locator('#grip-wait').focus();
  for (let i = 0; i < 4; i++) await page.keyboard.press('Shift+ArrowLeft');
  await page.getByRole('button', { name: 'Expand projects sidebar', exact: true }).click();
  assert.ok((await page.locator('.main').boundingBox()).width >= 360);
  await page.getByRole('button', { name: 'Collapse projects sidebar', exact: true }).click();
  await page.getByRole('button', { name: 'Collapse right sidebar', exact: true }).click();
  await page.reload();
  await page.locator('#link[data-state="connected"]').waitFor();
  assert.equal(await page.locator('#projects').isVisible(), false);
  assert.equal(await page.locator('#wait').isVisible(), false);
  assert.equal(await page.locator('#grip-wait').isVisible(), false);
  await page.keyboard.press('Control+Space');
  await page.keyboard.press('b');
  assert.equal(await page.locator('#projects').isVisible(), true);
  await page.locator('#grip-side').focus();
  for (let i = 0; i < 8; i++) await page.keyboard.press('Shift+ArrowLeft');
  const side = await page.locator('#projects').boundingBox();
  assert.equal(side.width, 180);
  for (const selector of ['.side-open', '.show-hidden', '.project-add', '.project-more']) {
    const bounds = await page.locator(selector).first().boundingBox();
    assert.ok(bounds.x >= side.x && bounds.x + bounds.width <= side.x + side.width, `${selector} stays reachable at minimum width`);
  }
  assert.equal(await page.locator('.side-open').evaluate((e) => e.scrollWidth <= e.clientWidth), true);
  await page.screenshot({ path: '/tmp/desl-sidebar-narrow.png' });
});

test('mobile and light layouts keep the editor and both reopen controls reachable', async (t) => {
  const { page } = await fixture(t, { viewport: { width: 390, height: 844 }, colorScheme: 'light', isMobile: true });
  await prompts(page);
  await add(page, 'A reusable review prompt with enough text to wrap across several lines in the narrow sidebar.');
  await edit(page);
  assert.ok((await page.locator('#prompt-text').boundingBox()).width > 200);
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true);
  await page.screenshot({ path: '/tmp/desl-web-prompts-mobile.png', fullPage: true });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.emulateMedia({ colorScheme: 'dark' });
  await page.screenshot({ path: '/tmp/desl-web-prompts-desktop.png', fullPage: true });
});

async function projectAction(page, name, action) {
  await page.getByRole('button', { name: `Actions for project ${name}`, exact: true }).click();
  await page.getByRole('button', { name: action, exact: true }).click();
}

test('working Codex pulses continuously through live updates and changes color with status', async (t) => {
  const { page, publish } = await fixture(t);
  const next = structuredClone(view);
  const codex = next.projects[0].threads[0];
  Object.assign(codex, { agent: 'codex', status: 'working', word: 'working' });
  publish(next);
  const logo = page.locator('#projects .mark.agent-codex');
  await logo.waitFor();
  await page.evaluate(() => { window.testLogo = document.querySelector('#projects .mark.agent-codex'); });
  for (let i = 0; i < 12; i++) {
    codex.latest = `Live progress ${i}`;
    publish(next);
    await page.waitForTimeout(60);
  }
  assert.equal(await logo.evaluate((e) => e === window.testLogo), true, 'live updates preserve the animated logo');
  assert.ok(await logo.evaluate((e) => e.getAnimations()[0].currentTime > 500), 'the pulse is not restarted on every update');
  const working = await logo.evaluate((e) => getComputedStyle(e).color);
  for (const status of ['waiting', 'failed', 'idle']) {
    codex.status = codex.word = status;
    publish(next);
    await page.locator(`#projects .mark.agent-codex.${status}`).waitFor();
    assert.equal(await logo.evaluate((e) => e.getAnimations().length), 0);
    assert.notEqual(await logo.evaluate((e) => getComputedStyle(e).color), working);
  }
  assert.ok(await logo.evaluate((e) => +getComputedStyle(e).opacity < 1));
  await page.emulateMedia({ reducedMotion: 'reduce' });
  codex.status = codex.word = 'working';
  publish(next);
  await page.locator('#projects .mark.working').waitFor();
  assert.equal(await logo.evaluate((e) => e.getAnimations().length), 0);
  assert.equal(await logo.evaluate((e) => getComputedStyle(e).color), working, 'reduced motion still shows working color');
});

test('the sidebar toolbar stays fixed while its list scrolls and live updates preserve focus', async (t) => {
  const { page, publish } = await fixture(t);
  const next = structuredClone(view);
  next.projects[0].threads[0].latest = 'Previous progress';
  next.projects.push(...Array.from({ length: 18 }, (_, i) => project(`extra-${i}`, `Project ${i}`)));
  publish(next);
  const toolbar = page.locator('.side-toolbar');
  const top = (await toolbar.boundingBox()).y;
  const list = page.locator('#project-list');
  await list.waitFor();
  await list.evaluate((e) => { e.scrollTop = 380; });
  assert.equal((await toolbar.boundingBox()).y, top);
  const scroll = await list.evaluate((e) => e.scrollTop);
  assert.ok(scroll > 0);
  await page.getByRole('button', { name: 'Show hidden projects', exact: true }).focus();
  next.projects[0].threads[0].latest = 'A new update';
  publish(next);
  await page.waitForFunction(() => document.querySelector('#projects .latest').textContent === 'A new update');
  assert.equal(await page.evaluate(() => document.activeElement.id), 'show-hidden-projects');
  assert.equal(await list.evaluate((e) => e.scrollTop), scroll);
  assert.equal((await toolbar.boundingBox()).y, top);
  await page.screenshot({ path: '/tmp/desl-sidebar-scroll.png' });
});

test('hidden projects survive reload, stay out of keyboard navigation and can be shown again', async (t) => {
  const { page } = await fixture(t);
  await projectAction(page, 'Second', '1 Hide');
  assert.equal(await page.locator('#projects .project').count(), 1);
  await page.keyboard.press('Alt+j');
  assert.equal(await page.locator('#head .head-title').textContent(), 'First thread');
  await page.reload();
  await page.locator('#link[data-state="connected"]').waitFor();
  assert.equal(await page.locator('#projects .project').count(), 1);
  await page.getByRole('button', { name: 'Show hidden projects', exact: true }).click();
  assert.equal(await page.locator('#projects .project').count(), 2);
  assert.equal(await page.locator('.project-hidden .hidden-label').textContent(), 'Hidden');
  await projectAction(page, 'Second', '1 Show');
  await page.getByRole('button', { name: 'Show hidden projects', exact: true }).click();
  assert.equal(await page.locator('#projects .project').count(), 2);
  await page.reload();
  await page.locator('#link[data-state="connected"]').waitFor();
  assert.equal(await page.locator('#projects .project').count(), 2);
});

test('removing a project confirms workspace scope and updates all open pages', async (t) => {
  const { page, open, commands } = await fixture(t);
  const other = await open();
  await projectAction(page, 'First', '2 Remove…');
  assert.match(await page.getByRole('dialog').textContent(), /every client/);
  await page.getByRole('button', { name: 'No', exact: true }).click();
  assert.equal(commands.length, 0);
  await projectAction(page, 'First', '2 Remove…');
  await page.getByRole('button', { name: 'Remove', exact: true }).click();
  await page.locator('#head .head-title').filter({ hasText: 'Second thread' }).waitFor();
  await other.locator('#head .head-title').filter({ hasText: 'Second thread' }).waitFor();
  assert.equal(commands[0].project_id, 'one');
  await page.getByRole('button', { name: 'Show hidden projects', exact: true }).click();
  assert.equal(await page.locator('#projects .project').count(), 1);
  await page.reload();
  await page.locator('#link[data-state="connected"]').waitFor();
  assert.equal(await page.locator('#projects .project').count(), 1);
});

test('removal refusal leaves a project intact and removing the last project clears its screen', async (t) => {
  const { page, refuseRemoval } = await fixture(t);
  await page.getByRole('button', { name: 'Screen', exact: true }).click();
  await page.locator('.xterm-rows').getByText('Paste fixture ready', { exact: true }).waitFor();
  refuseRemoval('t-one is still working. Stop its turn and remove the project again.');
  await projectAction(page, 'First', '2 Remove…');
  await page.getByRole('button', { name: 'Remove', exact: true }).click();
  await page.waitForFunction(() => document.querySelector('#notice').textContent.includes('still working'));
  assert.equal(await page.locator('#projects .project').count(), 2);
  refuseRemoval(null);
  for (const name of ['First', 'Second']) {
    await projectAction(page, name, '2 Remove…');
    await page.getByRole('button', { name: 'Remove', exact: true }).click();
  }
  await page.waitForFunction(() => document.querySelectorAll('#projects .project').length === 0);
  assert.equal(await page.locator('#head').textContent(), '');
  assert.equal(await page.locator('#screen-note').isVisible(), true);
  assert.match(await page.locator('#screen-note-text').textContent(), /Open a directory/);
  await prompts(page);
  assert.equal(await page.locator('#prompt-add').isDisabled(), true);
});

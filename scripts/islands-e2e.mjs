#!/usr/bin/env node
// Islands end to end in headless Chrome, on the counter's
// /components/islands page served with a strict nonce CSP. No npm
// dependencies: Node 22+'s built-in WebSocket speaks the DevTools protocol.
//
//   (cd examples/counter && prism build web)      # needs wasm32-unknown-unknown
//   PRISM_STRICT_CSP=1 PRISM_ISLANDS_DIR=target/prism/islands/release target/web/release/counter &
//   node scripts/islands-e2e.mjs --url http://127.0.0.1:3000 --out /tmp/shots
//
// Env: CHROME (browser binary).
import { spawn } from 'node:child_process';
import { mkdtempSync, writeFileSync, mkdirSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const args = process.argv.slice(2);
const opt = (name, fallback) => { const i = args.indexOf(name); return i >= 0 ? args[i + 1] : fallback; };
const BASE = opt('--url', 'http://127.0.0.1:3000');
const OUT = opt('--out', mkdtempSync(path.join(os.tmpdir(), 'prism-islands-')));
mkdirSync(OUT, { recursive: true });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const chrome = process.env.CHROME || (process.platform === 'darwin'
  ? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'
  : 'google-chrome');
const port = 9300 + Math.floor(Math.random() * 500);
// Mock keychain: no macOS keychain prompt.
const browser = spawn(chrome, ['--headless=new', `--remote-debugging-port=${port}`, `--user-data-dir=${mkdtempSync(path.join(os.tmpdir(), 'cdp'))}`,
  '--hide-scrollbars', '--lang=en-US', '--accept-lang=en-US', '--use-mock-keychain', '--password-store=basic', 'about:blank'], { stdio: 'ignore' });
let target;
for (let i = 0; i < 50 && !target; i++) { await sleep(200); try { target = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find((t) => t.type === 'page'); } catch {} }

/** One CDP session on a page. */
async function session(wsUrl) {
  const ws = new WebSocket(wsUrl);
  await new Promise((r) => (ws.onopen = r));
  let id = 0;
  const pending = new Map();
  const s = { requests: [], frames: [], logs: [], ws };
  ws.onmessage = (e) => {
    const m = JSON.parse(e.data);
    if (pending.has(m.id)) { pending.get(m.id)(m.result ?? m.error); pending.delete(m.id); }
    if (m.method === 'Network.requestWillBeSent') s.requests.push(m.params.request.url);
    if (m.method === 'Network.webSocketFrameSent') s.frames.push(m.params.response.payloadData);
    if (m.method === 'Log.entryAdded') s.logs.push(`${m.params.entry.level} ${m.params.entry.source}: ${m.params.entry.text}`);
    if (m.method === 'Runtime.exceptionThrown') s.logs.push(`exception: ${m.params.exceptionDetails.exception?.description ?? m.params.exceptionDetails.text}`);
    if (m.method === 'Runtime.consoleAPICalled' && ['error', 'warning'].includes(m.params.type)) s.logs.push(`console.${m.params.type}: ${m.params.args.map((a) => a.value ?? a.description).join(' ')}`);
  };
  s.send = (method, params = {}) => new Promise((r) => { pending.set(++id, r); ws.send(JSON.stringify({ id, method, params })); });
  s.eval = async (expression) => (await s.send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true })).result?.value;
  s.waitFor = async (expr, ms = 15000) => { for (let t = 0; t < ms; t += 100) { if (await s.eval(expr)) return true; await sleep(100); } return false; };
  s.shot = async (name) => {
    const r = await s.send('Page.captureScreenshot', { format: 'png' });
    writeFileSync(`${OUT}/${name}`, Buffer.from(r.data, 'base64'));
    console.log('screenshot', `${OUT}/${name}`);
  };
  s.mouse = (type, x, y, extra = {}) => s.send('Input.dispatchMouseEvent', { type, x, y, button: 'none', ...extra });
  for (const d of ['Page', 'Runtime', 'Network', 'Log']) await s.send(`${d}.enable`);
  await s.send('Network.setExtraHTTPHeaders', { headers: { 'Accept-Language': 'en-US,en;q=0.9' } });
  await s.send('Emulation.setLocaleOverride', { locale: 'en-US' });
  await s.send('Emulation.setDeviceMetricsOverride', { width: 520, height: 1100, deviceScaleFactor: 2, mobile: false });
  // CSP violations, recorded from the start of every document.
  await s.send('Page.addScriptToEvaluateOnNewDocument', { source:
    "window.__csp = []; document.addEventListener('securitypolicyviolation', (e) => window.__csp.push(e.violatedDirective + ' ' + e.blockedURI));" });
  return s;
}

const report = {};
const fail = [];
const check = (name, ok, detail = '') => { report[name] = ok ? 'ok' : `FAIL ${detail}`; if (!ok) fail.push(name); };
const s = await session(target.webSocketDebuggerUrl);
const online = "document.documentElement.dataset.prismConnection === 'online' && !document.getElementById('prism-ssr')";

// 1. A page without islands requests no island file.
await s.send('Page.navigate', { url: `${BASE}/` });
await s.waitFor(online);
await sleep(800);
const islandFiles = (urls) => urls.filter((u) => u.includes('/_prism/islands/') || u.includes('time-ago'));
check('home: no island files', islandFiles(s.requests).length === 0, JSON.stringify(islandFiles(s.requests)));
const policy = (await fetch(`${BASE}/components/islands`)).headers.get('content-security-policy') || '';
check('strict nonce CSP with wasm-unsafe-eval', /script-src 'self' 'nonce-[0-9a-f]+' 'wasm-unsafe-eval'/.test(policy) && !policy.includes('unsafe-inline'), policy);
report.csp = policy;

// 2. The islands page: its JS + WASM once (and time-ago.js once).
s.requests.length = 0;
await s.send('Page.navigate', { url: `${BASE}/components/islands` });
await s.waitFor(online);
const mounted = await s.waitFor("(() => { const el = document.querySelector('prism-island-chart'); return el && el.prismHandle !== undefined && el.shadowRoot && el.shadowRoot.querySelector('canvas'); })()");
check('chart island mounted', mounted);
await s.waitFor("!!document.querySelector('time-ago')?.shadowRoot?.textContent");
await sleep(500);
const files = islandFiles(s.requests);
const js = files.filter((u) => /prism-island-chart\.[0-9a-f]+\.js$/.test(u));
const wasm = files.filter((u) => /prism-island-chart\.[0-9a-f]+\.wasm$/.test(u));
const ta = files.filter((u) => /time-ago\.[0-9a-f]+\.js$/.test(u));
check('island JS requested once', js.length === 1, JSON.stringify(files));
check('island WASM requested once', wasm.length === 1, JSON.stringify(files));
check('time-ago.js requested once', ta.length === 1, JSON.stringify(files));
report.files = files.map((u) => u.replace(BASE, ''));
const timeAgo = await s.eval("document.querySelector('time-ago').shadowRoot.textContent");
check('time-ago ticks client-side', /now|second/.test(timeAgo), timeAgo);
report.timeAgo = timeAgo;

// The chart's box, in the viewport.
await s.eval("document.querySelector('prism-island-chart').scrollIntoView({block: 'center'})");
await sleep(300);
const box = await s.eval("(() => { const r = document.querySelector('prism-island-chart').getBoundingClientRect(); return {x: r.left, y: r.top, w: r.width, h: r.height}; })()");
report.box = box;
const handle = await s.eval("(() => { const el = document.querySelector('prism-island-chart'); window.__chart = el; window.__canvas = el.shadowRoot.querySelector('canvas'); return el.prismHandle; })()");

// 3. Pointer moves: zero socket messages. (The first input focuses the
// window, which reports the app's lifecycle once: done before measuring.)
await s.send('Page.bringToFront');
await s.mouse('mouseMoved', 5, 5);
await sleep(400);
s.frames.length = 0;
const y = box.y + box.h * 0.5;
for (let i = 0; i <= 60; i++) {
  await s.mouse('mouseMoved', box.x + 40 + (box.w - 60) * i / 60, y);
  await sleep(16);
}
await s.mouse('mouseMoved', box.x + 40 + (box.w - 52) * 0.62, y);
await sleep(200);
check('60 pointer moves: zero WebSocket messages', s.frames.length === 0, JSON.stringify(s.frames));
report.framesDuringMoves = s.frames.length;
await s.shot('islands-pointer.png');

// 4. A click: one event, the server's UI updates.
s.frames.length = 0;
const cx = box.x + 40 + (box.w - 52) * 0.62;
await s.mouse('mousePressed', cx, y, { button: 'left', clickCount: 1 });
await s.mouse('mouseReleased', cx, y, { button: 'left', clickCount: 1 });
const selected = await s.waitFor("document.querySelector('[data-pid]') && /Selected|point/.test([...document.querySelectorAll('[role=status]')].map((e) => e.textContent).join(' '))", 5000);
await sleep(300);
const islandFrames = s.frames.filter((f) => f.includes('"t":"island"'));
check('click: one island event', islandFrames.length === 1 && s.frames.length === 1, JSON.stringify(s.frames));
report.clickFrame = islandFrames[0];
const selText = await s.eval("[...document.querySelectorAll('[role=status]')].map((e) => e.textContent).filter((t) => /Selected/.test(t)).join(' | ')");
check('server UI shows the selection', selected && /Selected: point \d+, value/.test(selText), selText);
report.selection = selText;
await s.shot('islands-click.png');

// 5. A server prop change updates the live island without remounting.
const before = await s.eval("document.querySelector('prism-island-chart').prismProps.points.length");
const btn = await s.eval("(() => { const b = [...document.querySelectorAll('button, [role=button]')].filter((b) => b.textContent.trim() === 'Add a point').pop(); const r = b.getBoundingClientRect(); return {x: r.left + r.width / 2, y: r.top + r.height / 2}; })()");
for (let i = 0; i < 3; i++) {
  await s.mouse('mousePressed', btn.x, btn.y, { button: 'left', clickCount: 1 });
  await s.mouse('mouseReleased', btn.x, btn.y, { button: 'left', clickCount: 1 });
  await sleep(250);
}
await s.waitFor(`document.querySelector('prism-island-chart').prismProps.points.length === ${before + 3}`, 5000);
const after = await s.eval(`(() => { const el = document.querySelector('prism-island-chart'); return { points: el.prismProps.points.length, sameElement: el === window.__chart, sameCanvas: el.shadowRoot.querySelector('canvas') === window.__canvas, handle: el.prismHandle }; })()`);
check('prop change: same element, same mount', after.points === before + 3 && after.sameElement && after.sameCanvas && after.handle === handle, JSON.stringify({ before, after, handle }));
report.propUpdate = { before, ...after };
// Hover the newest point for the screenshot.
await s.mouse('mouseMoved', box.x + box.w - 14, y);
await sleep(200);
await s.shot('islands-added.png');

// 6. No CSP violation under the strict nonce policy.
const csp = await s.eval('window.__csp');
const cspLogs = s.logs.filter((l) => /Content Security Policy|Refused/.test(l));
check('no CSP violations', csp.length === 0 && cspLogs.length === 0, JSON.stringify({ csp, cspLogs }));
report.logs = s.logs;

// 7. Without JavaScript: the fallback shows.
const created = await s.send('Target.createTarget', { url: 'about:blank' });
const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
const nojs = await session(list.find((t) => t.id === created.targetId).webSocketDebuggerUrl);
await nojs.send('Emulation.setScriptExecutionDisabled', { value: true });
await nojs.send('Page.navigate', { url: `${BASE}/components/islands` });
await sleep(1500);
const fallback = await nojs.eval("(() => { const el = document.querySelector('prism-island-chart'); return el ? { children: el.children.length, shadow: !!el.shadowRoot, html: el.innerHTML.length, svg: !!el.querySelector('svg, [data-kind=shape]') } : null; })()");
// Script execution is off, but DevTools' evaluate still runs.
check('no JS: fallback chart in the light DOM', fallback && fallback.children > 0 && !fallback.shadow, JSON.stringify(fallback));
report.fallback = fallback;
await nojs.eval("document.querySelector('prism-island-chart').scrollIntoView({block: 'start'})");
await sleep(300);
await nojs.shot('islands-nojs.png');

console.log(JSON.stringify(report, null, 2));
console.log(fail.length ? `FAILED: ${fail.join(', ')}` : 'ALL OK');
browser.kill();
process.exit(fail.length ? 1 : 0);

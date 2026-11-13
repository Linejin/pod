// SPDX-License-Identifier: Apache-2.0
import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { access, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { EventEmitter } from 'node:events';
import { BridgeDouble, fixtures, origin, base, csp, bootstrap, allowedContext, allowedPost, decodeIntent } from './bridge-double.mjs';
import { validIntent, validResult, validMessage, visible } from '../web/contract.js';
const root = resolve(import.meta.dirname, '../..');
const edge = 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe';
const context = {mainFrame: true, origin, url: base};
const intent = (op, params = {}, id = 'test1') => ({schema: 'pod-bridge/1', id, intent: op, params});

test('closed intents, DTO budgets, context and native-only POST policy', () => {
  for (const [op, dto] of Object.entries(fixtures)) assert.ok(validResult(op, dto), op);
  assert.ok(validIntent(intent('message', {task: 't', text: '가'.repeat(1000), comments: []})));
  for (const s of ['a\r\nb', 'a\t', '\u202e', '\u061c', '\u0085', 'a'.repeat(1001), Array(21).fill('a').join('\n'), '']) assert.equal(validMessage(s), false);
  for (const bad of [{...intent('tasks'), url: 'x'}, intent('fetch'), intent('tasks', {rpc: 'x'}), {...intent('tasks'), id: 'x'.repeat(65)}, intent('message', {task: 't', text: 'x', comments: Array(21).fill('c')})]) assert.equal(validIntent(bad), false);
  for (const c of [{...context, mainFrame: false}, {...context, origin: 'null'}, {...context, origin: 'https://evil.invalid'}, {...context, url: origin + '/other'}]) assert.equal(allowedContext(c), false);
  const csrf = 'c'.repeat(64);
  assert.ok(allowedPost({origin, csrf}, csrf));
  for (const h of [{origin}, {origin: 'null', csrf}, {csrf}, {origin: 'https://evil.invalid', csrf}]) assert.equal(allowedPost(h, csrf), false);
  const double = new BridgeDouble(); double.handle(intent('tasks'), context);
  assert.throws(() => double.handle(intent('tasks'), context));
  assert.throws(() => double.handle(intent('output', {task: 'other'}, 'r2'), context));
  assert.equal(validResult('tasks', {...fixtures.tasks, tasks: Array(21).fill(fixtures.tasks.tasks[0])}), false);
  assert.equal(validResult('output', {...fixtures.output, lines: ['x'.repeat(16385)]}), false);
  assert.equal(validResult('task', {...fixtures.task, secret: 'canary'}), false);
  assert.deepEqual(decodeIntent(JSON.stringify(intent('tasks'))), intent('tasks'));
  assert.throws(() => decodeIntent('{"schema":"pod-bridge/1","id":"r","id":"s","intent":"tasks","params":{}}'));
  assert.throws(() => decodeIntent(' '.repeat(32769)));
});

class Pipe extends EventEmitter {
  counter = 0; pending = new Map(); buffer = Buffer.alloc(0);
  constructor(child) {
    super(); this.child = child;
    child.stdio[4].on('data', chunk => {
      this.buffer = Buffer.concat([this.buffer, chunk]);
      let end;
      while ((end = this.buffer.indexOf(0)) >= 0) {
        const msg = JSON.parse(this.buffer.subarray(0, end).toString()); this.buffer = this.buffer.subarray(end + 1);
        if (msg.id) { const p = this.pending.get(msg.id); this.pending.delete(msg.id); if (p) { clearTimeout(p.timer); msg.error ? p.reject(Error(JSON.stringify(msg.error))) : p.resolve(msg.result); } }
        else this.emit(msg.method, msg.params, msg.sessionId);
      }
    });
  }
  send(method, params = {}, sessionId) {
    return new Promise((resolve, reject) => {
      const id = ++this.counter;
      const timer = setTimeout(() => { this.pending.delete(id); reject(Error('CDP timeout: ' + method)); }, 15000);
      this.pending.set(id, {resolve, reject, timer});
      this.child.stdio[3].write(JSON.stringify({id, method, params, ...(sessionId ? {sessionId} : {})}) + '\0');
    });
  }
  close() { for (const p of this.pending.values()) { clearTimeout(p.timer); p.reject(Error('closed')); } this.pending.clear(); }
}

test('Edge pipe: bundle CSP, text rendering, native boundary and stale screens', {timeout: 2100000}, async t => {
  await access(edge).catch(() => { throw Error('Installed Microsoft Edge is required at ' + edge + '; no engine will be downloaded.'); });
  const gate = await new Promise((resolveGate, reject) => {
    const p = spawn('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', 'tools/cpu-gate.ps1', '-MaxPct', '70', '-MaxCargo', '1'], {cwd: root, windowsHide: true});
    let stdout = '', stderr = '';
    p.stdout.on('data', b => { stdout += b; process.stdout.write(b); });
    p.stderr.on('data', b => { stderr += b; }); p.on('error', reject);
    p.on('exit', status => resolveGate({status, stdout, stderr}));
  });
  assert.equal(gate.status, 0, gate.stderr); assert.doesNotMatch(gate.stdout, /timeout/i, 'CPU gate must say go');
  const profile = await mkdtemp(join(tmpdir(), 'pod-edge-p4-'));
  let child, pipe;
  try {
    child = spawn(edge, ['--headless=new', '--remote-debugging-pipe', '--user-data-dir=' + profile,
      '--no-first-run', '--no-default-browser-check', '--disable-background-networking', '--disable-component-update',
      '--disable-sync', '--disable-extensions', '--disable-default-apps', '--disable-domain-reliability',
      '--disable-features=MediaRouter,OptimizationHints,AutofillServerCommunication,msEdgeSidebarV2',
      '--host-resolver-rules=MAP * ~NOTFOUND', '--proxy-server=http://127.0.0.1:9', 'about:blank'],
      {stdio: ['ignore', 'ignore', 'pipe', 'pipe', 'pipe'], windowsHide: true});
    let stderr = ''; child.stderr.on('data', b => { stderr += b; });
    pipe = new Pipe(child);
    const {targetId} = await pipe.send('Target.createTarget', {url: 'about:blank'}).catch(e => { throw Error(e.message + '\n' + stderr); });
    const {sessionId: sid} = await pipe.send('Target.attachToTarget', {targetId, flatten: true});
    const send = (m, p = {}) => pipe.send(m, p, sid);
    const evaluate = async expression => {
      const r = await send('Runtime.evaluate', {expression, awaitPromise: true, returnByValue: true, allowUnsafeEvalBlockedByCSP: false, userGesture: true});
      if (r.exceptionDetails) throw Error(JSON.stringify(r.exceptionDetails));
      return r.result.value;
    };
    const wait = async expression => {
      for (let i = 0; i < 200; i++) { if (await evaluate(expression)) return; await new Promise(r => setTimeout(r, 25)); }
      throw Error('UI wait failed: ' + expression);
    };
    const double = new BridgeDouble(), blocked = [], served = [], errors = [], contexts = new Map(), downloads = [];
    let popupBlocks = 0;
    pipe.on('Target.attachedToTarget', async p => {
      if (p.targetInfo.targetId === targetId) return;
      popupBlocks++;
      await pipe.send('Target.closeTarget', {targetId: p.targetInfo.targetId}).catch(e => errors.push(e));
    });
    await pipe.send('Target.setAutoAttach', {autoAttach: true, waitForDebuggerOnStart: true, flatten: true});
    await send('Page.enable'); await send('Runtime.enable'); await send('Network.enable');
    await pipe.send('Browser.setDownloadBehavior', {behavior: 'deny', eventsEnabled: true});
    pipe.on('Browser.downloadProgress', p => downloads.push(p.state));
    pipe.on('Runtime.executionContextCreated', p => contexts.set(p.context.id, p.context));
    pipe.on('Runtime.bindingCalled', async p => {
      if (p.name !== '__podIntent') return;
      let request, response = null;
      try {
        request = decodeIntent(p.payload); const ctx = contexts.get(p.executionContextId);
        const tree = await send('Page.getFrameTree');
        response = double.handle(request, {origin: ctx?.origin, mainFrame: ctx?.auxData?.frameId === tree.frameTree.frame.id,
          url: tree.frameTree.frame.url});
      } catch { /* Fixed denial, no payload reflection. */ }
      if (request?.id) await send('Runtime.evaluate', {expression: `globalThis.__podReply?.(${JSON.stringify(request.id)},${JSON.stringify(response)})`, contextId: p.executionContextId}).catch(e => errors.push(e));
    });
    await send('Runtime.addBinding', {name: '__podIntent'});
    await send('Page.addScriptToEvaluateOnNewDocument', {source: bootstrap});
    await send('Fetch.enable', {patterns: [{urlPattern: '*'}]});
    pipe.on('Fetch.requestPaused', async p => {
      try {
        const files = new Map([[base, ['index.html', 'text/html']], ...['app.js', 'contract.js', 'style.css'].map(f => [base + f, [f, f.endsWith('css') ? 'text/css' : 'text/javascript']])]);
        const file = files.get(p.request.url);
        if (!file || p.request.method !== 'GET' || p.resourceType === 'Document' && p.request.url !== base) {
          blocked.push(p.request.url); await send('Fetch.failRequest', {requestId: p.requestId, errorReason: 'BlockedByClient'}); return;
        }
        served.push(p.request.url);
        await send('Fetch.fulfillRequest', {requestId: p.requestId, responseCode: 200,
          responseHeaders: [{name: 'Content-Type', value: file[1] + '; charset=utf-8'}, {name: 'Content-Security-Policy', value: csp}, {name: 'X-Content-Type-Options', value: 'nosniff'}],
          body: (await readFile(join(root, 'mobile/web', file[0]))).toString('base64')});
      } catch(e) { errors.push(e); }
    });
    const load = async () => { double.seen.clear(); await send('Page.navigate', {url: base}); await wait("document.querySelector('#tasks button') !== null"); };
    const click = async id => { await evaluate(`document.getElementById(${JSON.stringify(id)}).click()`); };
    const detail = async () => { await evaluate("document.querySelector('#tasks button').click()"); await wait("document.getElementById('status').textContent.includes('상세')"); };
    await load(); await detail();
    await t.test('positive decision once, message and delivery/output screens', async () => {
      assert.equal(await evaluate("document.getElementById('decide').disabled"), false);
      await click('decide'); await click('decide'); await wait("document.getElementById('status').textContent.includes('처리 응답')");
      assert.equal(double.attempts.filter(r => r.intent === 'decide').length, 1);
      await click('refresh'); await wait("!document.getElementById('send').disabled");
      await evaluate("document.getElementById('message').value='짧은 메시지'"); await click('send');
      await wait("!document.getElementById('delivery').disabled"); await click('delivery'); await wait("document.getElementById('deliveryStatus').textContent.includes('delivered')");
      await click('output'); await wait("document.getElementById('lines').textContent.includes('생략')");
      assert.equal(double.attempts.filter(r => r.intent === 'message').length, 1);
    });
    await t.test('invalid messages never cross the bridge', async () => {
      await click('refresh'); await wait("!document.getElementById('send').disabled");
      const before = double.attempts.length;
      for (const value of ['x\u202e', 'x\t', 'x'.repeat(1001), Array(21).fill('x').join('\n')]) {
        await evaluate(`document.getElementById('message').value=${JSON.stringify(value)}`); await click('send');
      }
      assert.equal(double.attempts.length, before);
    });
    await t.test('incomplete, offline, native stale, 25 second expiry disable mutations', async () => {
      const before = double.attempts.filter(r => ['message', 'decide'].includes(r.intent)).length;
      for (const event of ['offline', 'pod-stale', 'pagehide']) {
        await click('refresh'); await wait("!document.getElementById('send').disabled");
        await evaluate(`dispatchEvent(new Event('${event}'))`); await click('send'); await click('decide');
        assert.ok(await evaluate("document.getElementById('send').disabled && document.getElementById('decide').disabled"));
      }
      double.snapshot.task.gates[0].complete = false; await click('refresh'); await wait("document.getElementById('status').textContent.includes('미완성')");
      await click('send'); await click('decide');
      double.snapshot.task.gates[0].complete = true;
      await click('refresh'); await wait("!document.getElementById('send').disabled");
      await new Promise(r => setTimeout(r, 25100));
      await wait("document.getElementById('send').disabled"); await click('send'); await click('decide');
      assert.equal(double.attempts.filter(r => ['message', 'decide'].includes(r.intent)).length, before);
    });
    await t.test('all DTO string leaves: malicious text is escaped or schema-rejected', async () => {
      const paths = (x, prefix = []) => typeof x === 'string' ? [prefix] : x && typeof x === 'object' ? Object.entries(x).flatMap(([k,v]) => paths(v, [...prefix,k])) : [];
      const payloads = ['<img src=x onerror=globalThis.pwned=1>', 'javascript:globalThis.pwned=1', 'https://evil.invalid/x', 'x\u202e', 'x\u0001'];
      let checked = 0;
      for (const [op, dto] of Object.entries(fixtures)) for (const path of paths(dto)) for (const payload of payloads) {
        const changed = structuredClone(dto); let parent = changed;
        for (const k of path.slice(0, -1)) parent = parent[k]; parent[path.at(-1)] = payload;
        // Exercise the exact renderer in the engine even for enum/fingerprint strings the schema rejects.
        const result = await evaluate(`(async () => { const {renderText,validResult}=await import('./contract.js'); const n=document.createElement('pre'); n.id='probe'; renderText(n,${JSON.stringify(payload)}); document.body.append(n); const r={text:n.textContent,children:n.children.length,attrs:n.getAttributeNames(),valid:validResult(${JSON.stringify(op)},${JSON.stringify(changed)})}; n.remove(); return r; })()`);
        assert.equal(result.text, visible(payload)); assert.equal(result.children, 0); assert.deepEqual(result.attrs, ['id']);
        assert.equal(result.valid, validResult(op, changed)); checked++;
      }
      // Feed accepted hostile values through the real task/output UI, with identifiers kept correlated.
      const x = '<img src=x onerror=globalThis.pwned=1>\u202e\u0001';
      double.snapshot = structuredClone(fixtures);
      Object.assign(double.snapshot.task.gates[0], {title: x, body: x, options: [x], origin: {kind:'mod',id:x}});
      double.snapshot.output.lines = [x];
      await click('refresh'); await wait("document.getElementById('detail').textContent.includes('onerror')");
      await click('output'); await wait("document.getElementById('lines').textContent.includes('onerror')");
      assert.equal(await evaluate("!!globalThis.pwned || !!document.querySelector('img, iframe, [onerror]')"), false);
      assert.equal(checked, Object.values(fixtures).reduce((n, dto) => n + paths(dto).length * payloads.length, 0));
      assert.ok(checked > 0); t.diagnostic('DTO string injection cases: ' + checked);
    });
    await t.test('CSP blocks inline/eval, fetch, frames, mixed content, form and popups', async () => {
      await evaluate("globalThis.violations=[];document.addEventListener('securitypolicyviolation',e=>violations.push(e.violatedDirective))");
      await evaluate(`(() => {
        const s=document.createElement('script');s.textContent='globalThis.pwned=1';document.body.append(s);
        const f=document.createElement('iframe');f.src='https://evil.invalid/frame';document.body.append(f);
        const i=document.createElement('img');i.src='http://evil.invalid/mixed';document.body.append(i);
        fetch('${origin}/mobile/v1/call',{method:'POST',body:'{}'}).catch(()=>{});
        fetch('https://evil.invalid/').catch(()=>{});
        const form=document.createElement('form');form.action='https://evil.invalid/post';form.method='post';document.body.append(form);form.submit();
        try { (0,eval)('globalThis.pwned=1'); } catch {}
      })()`);
      await wait("violations.includes('connect-src') && violations.includes('frame-src') && violations.includes('form-action')");
      assert.equal(await evaluate("!!globalThis.pwned"), false);
      assert.equal(await evaluate("window.open('https://evil.invalid/')"), null);
      assert.ok(await evaluate("violations.some(x=>x.startsWith('script-src'))"));
    });
    await t.test('other paths, external, file/content navigation and downloads cannot load', async () => {
      const canary = join(profile, 'file-canary.html');
      await writeFile(canary, '<script>globalThis.pwned=1</script>FILE_CANARY');
      for (const url of [origin + '/other', 'https://evil.invalid/', 'http://evil.invalid/', pathToFileURL(canary).href, 'content://pod/forbidden']) {
        const r = await send('Page.navigate', {url});
        assert.ok(r.errorText, 'navigation must fail: ' + url);
      }
      await load();
      await evaluate("location.href='javascript:globalThis.pwned=1'");
      assert.equal(await evaluate('!!globalThis.pwned'), false);
      await evaluate("const d=document.createElement('a');d.href=URL.createObjectURL(new Blob(['DOWNLOAD_CANARY']));d.download='canary.txt';document.body.append(d);d.click()");
      for (let i=0; i<100 && !downloads.includes('canceled'); i++) await new Promise(r=>setTimeout(r,20));
      assert.ok(downloads.includes('canceled'), 'native download deny cancels a real download');
      assert.equal(downloads.includes('completed'), false);
      await evaluate("const a=document.createElement('a');a.href='https://evil.invalid/download';a.download='bad';document.body.append(a);a.click()");
      const popupsBefore = popupBlocks;
      await evaluate("const p=document.createElement('a');p.href='https://evil.invalid/popup';p.target='_blank';document.body.append(p);p.click()");
      for (let i=0; i<100 && popupBlocks === popupsBefore; i++) await new Promise(r=>setTimeout(r,20));
      assert.ok(popupBlocks > popupsBefore, 'new target is paused and closed by native double');
      assert.ok(blocked.includes(origin + '/other'));
      assert.ok(served.every(url => url.startsWith(base)));
    });
    await t.test('real background lifecycle disables both writes', async () => {
      await load(); await detail(); await wait("!document.getElementById('send').disabled");
      const before = double.attempts.filter(r => ['message', 'decide'].includes(r.intent)).length;
      await send('Page.setWebLifecycleState', {state: 'frozen'});
      await send('Page.setWebLifecycleState', {state: 'active'});
      await wait("document.hidden && document.getElementById('send').disabled && document.getElementById('decide').disabled");
      await click('send'); await click('decide');
      assert.equal(double.attempts.filter(r => ['message', 'decide'].includes(r.intent)).length, before);
    });
    assert.deepEqual(errors, []);
    t.diagnostic(`Edge via pipe; ${served.length} local fulfillments, ${blocked.length} blocked requests; no Fetch.continueRequest or listener.`);
  } finally {
    if (child?.pid) {
      spawnSync('taskkill.exe', ['/PID', String(child.pid), '/T', '/F'], {windowsHide: true, stdio: 'ignore'});
      child.kill(); pipe?.close();
    }
    // This directory was created exclusively above; no real profile is ever used.
    assert.ok(resolve(profile).startsWith(resolve(tmpdir()) + '\\') && profile.includes('pod-edge-p4-'));
    await rm(profile, {recursive: true, force: true, maxRetries: 10, retryDelay: 200});
  }
});

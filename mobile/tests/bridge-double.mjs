// SPDX-License-Identifier: Apache-2.0
import { readFile } from 'node:fs/promises';
import { validIntent, validResult } from '../web/contract.js';
export const origin = 'https://appassets.androidplatform.net';
export const base = origin + '/assets/pod/';
export const csp = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; connect-src 'none'; font-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-src 'none'; frame-ancestors 'none'";
export const fixtures = JSON.parse(await readFile(new URL('./fixtures.json', import.meta.url), 'utf8'));
export function decodeIntent(raw) {
  if (typeof raw !== 'string' || Buffer.byteLength(raw) > 32768) throw Error('too large');
  const value = JSON.parse(raw);
  // The injected adapter emits canonical JSON.stringify bytes. This also rejects
  // duplicate object keys before the parsed object can become an intent.
  if (JSON.stringify(value) !== raw || !validIntent(value)) throw Error('invalid intent');
  return value;
}
// This is a desktop native-side policy double, not an HTTP server or credentials implementation.
export function allowedContext(context) {
  return context.mainFrame === true && context.origin === origin && context.url === base;
}
export function allowedPost(headers, expectedCsrf) {
  return headers.origin === origin && typeof expectedCsrf === 'string' && expectedCsrf.length === 64
    && headers.csrf === expectedCsrf;
}
export class BridgeDouble {
  records = []; attempts = []; seen = new Set(); snapshot = structuredClone(fixtures); enabled = true;
  handle(intent, context) {
    this.attempts.push(structuredClone(intent));
    if (!allowedContext(context) || !validIntent(intent) || this.seen.has(intent.id)) throw Error('denied');
    this.seen.add(intent.id);
    const {id, schema, params, intent: op} = intent;
    if (params.task && params.task !== this.snapshot.task.task.id) throw Error('scope');
    if (op === 'delivery' && params.delivery !== this.snapshot.message.delivery) throw Error('scope');
    if (op === 'decide' || op === 'message') {
      if (!this.enabled || !this.snapshot.task.gates.every(g => g.complete) || this.snapshot.task.cursor !== null) throw Error('stale');
      if (op === 'decide') {
        const g = this.snapshot.task.gates.find(g => g.gate === params.gate);
        if (!g || g.machine !== params.machine || g.revision !== params.revision || g.state !== 'pending'
          || params.choice !== null && !g.options.includes(params.choice)) throw Error('stale');
      }
    }
    const result = structuredClone(this.snapshot[op]);
    if (!validResult(op, result)) throw Error('invalid DTO');
    this.records.push(structuredClone(intent));
    // A decide record is an intent awaiting native confirmation, not a committed approval.
    return {schema, id, result};
  }
}
// Installed only in the validated main-world context. No bearer/CSRF/URL enters JS.
export const bootstrap = `(() => {
  if (window !== top) return;
  const pending = new Map();
  Object.defineProperty(globalThis, 'PodNative', {value: Object.freeze({request(intent) {
    return new Promise((resolve, reject) => {
      pending.set(intent.id, {resolve, reject});
      __podIntent(JSON.stringify(intent));
    });
  }})});
  Object.defineProperty(globalThis, '__podReply', {value: (id, response) => {
    const p = pending.get(id); if (!p) return; pending.delete(id);
    if (response === null) p.reject(Error('denied')); else p.resolve(response);
  }});
  Object.defineProperty(window, 'open', {value: () => null});
})();`;

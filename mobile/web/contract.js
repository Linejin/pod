// SPDX-License-Identifier: Apache-2.0
// Closed, bounded bridge DTOs. Native owns authentication and final confirmation.
const bytes = s => new TextEncoder().encode(s).length;
const str = n => x => typeof x === 'string' && bytes(x) <= n;
const id = x => str(128)(x) && x.length > 0;
const num = x => Number.isSafeInteger(x) && x >= 0;
const bool = x => typeof x === 'boolean';
const one = (...xs) => x => xs.includes(x);
const nullable = f => x => x === null || f(x);
const list = (f, n) => x => Array.isArray(x) && x.length <= n && x.every(f);
const obj = (fields, optional = []) => x => x !== null && typeof x === 'object' && !Array.isArray(x)
  && Object.keys(x).every(k => Object.hasOwn(fields, k))
  && Object.entries(fields).every(([k, f]) => optional.includes(k) && !Object.hasOwn(x, k) || Object.hasOwn(x, k) && f(x[k]));
const origin = x => ({ unknown: obj({kind: one('unknown')}), local_user: obj({kind: one('local_user')}),
  mod: obj({kind: one('mod'), id}), orch: obj({kind: one('orch'), run: id, attempt: id}) }[x?.kind]?.(x) === true);
const state = one('pending', 'approved', 'rejected', 'expired');
const deliveryState = one('queued', 'delivered', 'failed', 'cancelled', 'preview');
const fingerprint = x => typeof x === 'string' && /^[a-f0-9]{64}$/.test(x);
const opId = x => typeof x === 'string' && /^[A-Za-z0-9_-]{1,64}$/.test(x);
const gate = obj({machine: id, boot: id, task: id, gate: id, revision: num, issued_ms: num,
  expires_ms: nullable(num), origin, title: str(4096), body: str(49152), options: list(str(4096), 20),
  state, complete: bool, body_fingerprint: fingerprint});
const summary = obj({id, name: str(1024), state: str(64), origin, received_ms: num});
const decisionResult = obj({gate, replayed: bool});
const messageResult = obj({delivery: id, state: deliveryState, replayed: bool});
const operationResult = x => x?.kind === 'decision' ? obj({kind: one('decision'), result: decisionResult})(x)
  : obj({kind: one('message'), result: messageResult})(x);
const errorCode = one('invalid_request', 'unauthenticated', 'denied', 'conflict', 'stale', 'too_large', 'rate_limited', 'unavailable', 'storage', 'unknown_operation');
export const unsafeChars = /[\u0000-\u0009\u000b-\u001f\u007f-\u009f\u061c\u200e\u200f\u202a-\u202e\u2066-\u2069]/u;
export const validMessage = s => str(4096)(s) && [...s].length > 0 && [...s].length <= 1000 && s.split('\n').length <= 20 && !unsafeChars.test(s);
const requests = {
  tasks: obj({cursor: nullable(str(512))}, ['cursor']),
  task: obj({task: id, gate: nullable(id)}, ['gate']),
  output: obj({task: id}),
  decide: obj({machine: id, task: id, gate: id, revision: x => num(x) && x > 0, decision: one('approve', 'reject'), choice: nullable(str(4096))}),
  message: obj({task: id, text: validMessage, comments: list(id, 20)}),
  delivery: obj({task: id, delivery: id})
};
const results = {
  tasks: obj({tasks: list(summary, 20), cursor: nullable(str(512)), server_ms: num}),
  task: obj({task: summary, gates: list(gate, 20), cursor: nullable(str(512)), operation: nullable(operationResult), server_ms: num}),
  output: x => obj({task: id, lines: list(str(16384), 100), truncated: bool})(x) && bytes(x.lines.join('\n')) <= 16384,
  decide: decisionResult, message: messageResult,
  delivery: obj({task: id, delivery: id, state: deliveryState, error: nullable(errorCode)})
};
export function validIntent(x) {
  return obj({schema: one('pod-bridge/1'), id: opId, intent: one(...Object.keys(requests)), params: p => requests[x?.intent]?.(p) === true})(x)
    && bytes(JSON.stringify(x)) <= 32768;
}
export function validResult(op, x) { return bytes(JSON.stringify(x)) <= 65536 && results[op]?.(x) === true; }
export function visible(value) {
  return String(value).replace(/[\u0000-\u001f\u007f-\u009f\u061c\u200e\u200f\u202a-\u202e\u2066-\u2069]/gu,
    c => '\\u' + c.codePointAt(0).toString(16).padStart(4, '0'));
}
export function renderText(node, value) { node.textContent = visible(value); }

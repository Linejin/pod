// SPDX-License-Identifier: Apache-2.0
import { validIntent, validResult, validMessage, renderText } from './contract.js';
const el = id => document.getElementById(id);
const text = renderText;
let seq = 0, generation = 0, selected = null, detail = null, currentGate = null, cursor = null, delivery = null;
let fresh = false, busy = false, timer;
function controls() {
  const ready = fresh && !busy && !document.hidden && navigator.onLine && detail !== null;
  el('decide').disabled = !ready || !currentGate?.complete || currentGate.state !== 'pending' || currentGate.revision < 1;
  el('send').disabled = !ready || !detail.gates.every(g => g.complete) || detail.cursor !== null;
  el('output').disabled = !selected;
  el('delivery').disabled = !delivery;
}
function stale(reason) { fresh = false; generation++; clearTimeout(timer); text(el('status'), reason); controls(); }
async function request(intent, params) {
  const r = {schema: 'pod-bridge/1', id: 'r' + (++seq), intent, params};
  if (!validIntent(r) || !globalThis.PodNative) throw Error('bridge');
  let deadline;
  const answer = await Promise.race([globalThis.PodNative.request(r), new Promise((_, reject) => { deadline = setTimeout(() => reject(Error('timeout')), 25000); })]).finally(() => clearTimeout(deadline));
  if (!answer || Object.keys(answer).sort().join(',') !== 'id,result,schema' || answer.id !== r.id || answer.schema !== r.schema || !validResult(intent, answer.result)) throw Error('response');
  return answer.result;
}
function fields(parent, values) {
  const dl = document.createElement('dl');
  for (const [label, value] of Object.entries(values)) {
    const dt = document.createElement('dt'), dd = document.createElement('dd');
    text(dt, label); text(dd, typeof value === 'object' ? JSON.stringify(value) : value); dl.append(dt, dd);
  }
  parent.append(dl);
}
function showGate(g) {
  currentGate = g; el('detail').replaceChildren(); el('choice').replaceChildren();
  fields(el('detail'), {'머신': g.machine, '부팅': g.boot, '작업': g.task, '게이트': g.gate, '판': g.revision,
    '출처': g.origin, '제목': g.title, '본문': g.body, '선택지': g.options, '상태': g.state,
    '전체': g.complete, '발행': g.issued_ms, '만료': g.expires_ms, '지문': g.body_fingerprint});
  for (const choice of g.options) { const o = document.createElement('option'); text(o, choice); el('choice').append(o); }
  controls();
}
async function readTask(id) {
  stale('읽는 중'); const epoch = generation; selected = id; detail = null; currentGate = null;
  try {
    const d = await request('task', {task: id});
    if (epoch !== generation || document.hidden || !navigator.onLine) return;
    if (d.task.id !== id || d.gates.some(g => g.task !== id) || new Set(d.gates.map(g => g.boot)).size > 1) throw Error('snapshot');
    detail = d; el('gates').replaceChildren(); el('detail').replaceChildren();
    fields(el('detail'), {'작업': d.task.name, '상태': d.task.state, '출처': d.task.origin, '수신': d.task.received_ms});
    for (const g of d.gates) {
      const b = document.createElement('button'); text(b, g.title); b.addEventListener('click', () => showGate(g)); el('gates').append(b);
    }
    if (d.gates.length) showGate(d.gates[0]);
    fresh = d.cursor === null && d.gates.every(g => g.complete && g.revision > 0 && (g.expires_ms === null || g.expires_ms > d.server_ms));
    text(el('status'), fresh ? '최신 상세' : '상세 미완성 · 새로 읽기 필요');
    const lifetime = Math.min(25000, ...d.gates.filter(g => g.expires_ms !== null).map(g => Math.max(0, g.expires_ms - d.server_ms)));
    timer = setTimeout(() => stale('오래된 화면 · 새로 읽기 필요'), lifetime); controls();
  } catch { stale('연결 끊김 · 새로 읽기 필요'); }
}
async function tasks(next = false) {
  stale('목록 읽는 중');
  try {
    const d = await request('tasks', next && cursor !== null ? {cursor} : {});
    el('tasks').replaceChildren(); cursor = d.cursor; el('more').hidden = cursor === null;
    for (const t of d.tasks) {
      const b = document.createElement('button'); text(b, [t.id, t.name, t.state, JSON.stringify(t.origin), t.received_ms].join(' · '));
      b.addEventListener('click', () => readTask(t.id)); el('tasks').append(b);
    }
    text(el('status'), '작업을 선택하세요');
  } catch { stale('연결 끊김'); }
}
async function mutate(intent, params) {
  controls(); if (el(intent === 'decide' ? 'decide' : 'send').disabled) return;
  busy = true; controls();
  try {
    const d = await request(intent, params);
    if (intent === 'message') { delivery = d.delivery; text(el('deliveryStatus'), d.state); }
    stale('처리 응답 · 상세를 다시 읽으세요');
  } catch { stale('결과 불명 · 재전송하지 말고 상태를 확인하세요'); }
  finally { busy = false; controls(); }
}
el('refresh').addEventListener('click', () => selected ? readTask(selected) : tasks());
el('more').addEventListener('click', () => tasks(true));
el('decide').addEventListener('click', () => {
  if (!currentGate) return;
  const g = currentGate;
  mutate('decide', {machine: g.machine, task: g.task, gate: g.gate, revision: g.revision,
    decision: el('decision').value, choice: el('choice').selectedIndex < 0 ? null : g.options[el('choice').selectedIndex]});
});
el('send').addEventListener('click', () => {
  const value = el('message').value;
  if (!validMessage(value)) { text(el('status'), '길이·줄 수·제어 문자를 확인하세요'); return; }
  mutate('message', {task: selected, text: value, comments: []});
});
el('output').addEventListener('click', async () => {
  try { const d = await request('output', {task: selected}); text(el('lines'), (d.truncated ? '[일부 출력 생략]\n' : '') + d.lines.join('\n')); }
  catch { stale('출력 읽기 실패'); }
});
el('delivery').addEventListener('click', async () => {
  try { const d = await request('delivery', {task: selected, delivery}); text(el('deliveryStatus'), JSON.stringify(d)); }
  catch { stale('전달 상태 불명'); }
});
globalThis.addEventListener('offline', () => stale('오프라인'));
document.addEventListener('visibilitychange', () => stale(document.hidden ? '백그라운드' : '복귀 · 새로 읽기 필요'));
globalThis.addEventListener('pagehide', () => stale('화면 종료'));
// Native lifecycle notifications never carry data or grant freshness.
globalThis.addEventListener('pod-stale', () => stale('연결 변경 · 새로 읽기 필요'));
tasks();

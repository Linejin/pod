# 16. 확장: 샌드박스 JS 모드와 레이아웃 프리셋 (P6 part 2)

작성: 2026-11-09 · 브랜치 `SJin-Choi/p6-ext` · 크레이트 `crates/pod-mods` · podd `server/mods.rs` · 앱 `pod-app/src/mods_ui.rs`
선행 문서: [01 §4.2(6), §5, §6](01-plan.md) · [07 상태 엔진](07-pod-status.md) · [09 앱](09-pod-app.md) · [11 오케스트레이션](11-review-and-orchestration-backend.md)

작은 스크립트 확장과 선언형 레이아웃 프리셋은 에이전트 터미널 도구에서 흔한 생각이다(docs/01 §4.2(6)). Pod는 이것을 **독자적으로(클린룸)** 설계했다. 다른 프로젝트의 코드·API 이름·자산은 쓰지 않았다. 이 문서는 설계(§1–§6)와 모드 작성 안내·API 참조(§7–§9), 검증(§10)으로 되어 있다.

---

## 1. v1 범위: 작게, 안전하게

모드는 **이벤트에 반응하고, 권한을 확인받은 몇 가지 동작만** 한다.

| 할 수 있는 것 | 권한 | 범위 제한 |
|---|---|---|
| 알림 띄우기 (`pod.notify`) | `notify` | 분당 횟수 제한 |
| 명령 팔레트에 명령 추가 (`pod.command`) | `commands` | 모드 이름이 앞에 붙음 |
| 터미널에 글자 보내기 (`pod.terminal.send`) | `terminal.send` | **호출 범위(작업)의 터미널만**, 4 KiB 이하 |
| 상태 힌트 / 상태 감지기 (`pod.terminal.setStatus`, `pod.detector`) | `status` | 출처 `mod`, hook·osc보다 낮음 |
| 작업에서 검사 명령 실행 (`pod.task.runCheck`) | `check` | **manifest에 적은 명령만**, 호출 범위의 작업만 |
| 사람 승인(Gate) 요청 (`pod.gate.request`) | `gate` | 호출 범위의 작업만 |
| 터미널·작업·그룹 목록 읽기 (`pod.terminals()` 등) | 없음 | 읽기 전용 요약 |
| 타이머 (`setTimeout`, `setInterval`) | 없음 | 모드당 32개, 최소 10 ms / 100 ms (§8.1) |
| 레이아웃 프리셋 제공 (manifest `layouts`) | 없음 | 선언형 JSON, 코드 없음 |

할 수 **없는** 것 (v1): 파일 읽기·쓰기, 네트워크, 프로세스 실행(검사 명령은 manifest에 고정된 argv를 podd가 대신 실행), 다른 모드 모듈 불러오기(`import`/`require`), UI 직접 그리기. QuickJS의 `std`/`os` 모듈(quickjs-libc)은 아예 컴파일되지 않고 모듈 로더도 없다.

## 2. 어디서 도는가: podd (결정)

**모든 모드는 podd 안의 전용 스레드 하나에서 돈다.** 앱은 목록·설정·로그·명령 실행을 RPC로 요청하고, 알림은 상태 스트림 이벤트로 받는다.

이유:
- **상태 감지기는 앱 없이도 돌아야 한다.** Pod 원칙은 "UI는 상태를 소유하지 않는다"(docs/01 §5.3)이다. `pod ls`, `pod wait --status`, 오케스트레이션 Gate가 모두 podd의 상태를 쓰므로, 감지기가 앱에 있으면 앱을 끈 동안 aider 같은 에이전트의 상태가 사라진다.
- **이벤트 출처가 podd다.** `status.changed`, `task.changed`, `review.delivery`, `gate.changed`, `group.check`는 모두 podd 허브에서 나온다. 모드를 앱에 두면 앱이 꺼져 있을 때 "그룹이 다 끝나면 알림" 같은 반응이 빠진다(앱이 다시 켜지면 알림은 그때 보인다. §5).
- **권한 확인을 한 곳에서.** 터미널 전송·검사 실행·Gate는 이미 podd 동작이다. 같은 프로세스에서 범위(작업)를 확인하면 앱을 우회하는 경로가 없다.
- 앱 쪽 일(팔레트 항목, 알림 토스트, 레이아웃 적용, 설정 화면)은 데이터만 받는다. 앱 재시작에도 모드 상태는 그대로다.

비용: podd에 QuickJS(C, 약 1 MB)가 들어간다. 확장이 켜져 있으면 스레드 하나가 대기한다(할 일이 없으면 CPU 0, 런타임은 켜진 모드만 만든다). 모드 하나가 무한 루프여도 스레드 하나에서 시간 제한으로 끊기므로 데몬의 tokio 작업에는 영향이 없다.

```
 podd ────────────────────────────────────────────────────────────────┐
 │ status hub ──(event bridge: JSON 요약)──▶ ModManager 스레드         │
 │ TermStatus.sample() ──(화면/타이틀, 감지기 일치 터미널만)──▶ │       │
 │                                              ├ mod A: Runtime+Context (16 MiB, 50 ms)
 │ mod.* RPC ◀──────────────────────────────────┤ mod B: ...           │
 │ PoddHost (Host trait) ◀── 권한·범위 확인된 동작 ┘                     │
 │   notify → hub `mod.notify` · send → Terminal · status → TermStatus.on_mod
 │   runCheck → groups::run_check (blocking) → `check.finished` 이벤트  │
 │   gate → gate_create                                                 │
 └────────────────────────────────────────────────────────────────────┘
 pod-app: mod.list / mod.set_enabled / mod.grant / mod.log / mod.command
          `mod.notify` → 토스트, `mod.changed` → 설정 화면 갱신, layouts → 팔레트
```

## 3. 설치 위치, manifest, 켜고 끄기

- 위치: `%APPDATA%\Pod\mods\<name>\mod.json` (+ 엔트리 JS, 레이아웃 JSON). `POD_MODS_DIR`로 바꿀 수 있다(테스트는 항상 임시 폴더). `POD_MODS=off`이면 확장을 아예 켜지 않는다.
- 설치 = 폴더 복사. podd는 폴더를 **만들지 않는다**(읽기만 한다). `mod.reload`(설정 화면 "다시 읽기")로 다시 찾는다.
- 켜기/권한 상태: `<mods 폴더>\pod-mods.json` (`{"mods": {"<name>": {"enabled": true, "granted": ["notify", ...]}}}`). 새 모드는 **꺼진 상태**로 발견된다.
- 켤 때 기본 허용: 요청한 권한 중 `notify`, `commands`, `status`, `gate`. **`terminal.send`, `check`, `orch`는 설정 화면에서 따로 허용해야** 한다(글자를 보내거나 명령을 실행하거나 에이전트 우편 제목을 읽는 권한이라). 권한을 바꾸면 모드를 다시 불러온다.
- 로그: 모드마다 메모리에 최근 500줄, 파일 `<podd state dir>\mods\<name>.log`(256 KiB 넘으면 `.log.1`로 넘김). 설정 화면에서 볼 수 있다.

### 3.1 manifest (`mod.json`)

```json
{
  "pod": 1,
  "name": "group-done-notify",
  "version": "0.1.0",
  "title": "그룹 완료 알림",
  "description": "비교 그룹의 에이전트가 모두 끝나면 알린다",
  "entry": "main.js",
  "permissions": ["notify", "commands"],
  "checks": { "test": ["cargo", "test", "--quiet"] },
  "layouts": ["layouts/review-focus.json"]
}
```

| 필드 | 필수 | 규칙 |
|---|---|---|
| `pod` | 예 | API 버전, v1은 `1` |
| `name` | 예 | `[a-z0-9][a-z0-9-]{0,47}`, 폴더 이름과 같아야 함 |
| `version` | 예 | 64자 이하 문자열 |
| `title`, `description` | 아니오 | 표시용 (120자 / 500자) |
| `entry` | 아니오 | 모드 폴더 안의 `.js` (상대 경로, `..` 금지, 512 KiB 이하). 없으면 레이아웃만 주는 모드 |
| `permissions` | 아니오 | §6의 이름만. 모르는 이름은 manifest 오류 |
| `checks` | 아니오 | 이름 → argv 배열 (최대 8개). `check` 권한과 함께 써야 의미가 있음 |
| `layouts` | 아니오 | 모드 폴더 안 JSON 파일 (최대 8개, 각 64 KiB) |

manifest가 잘못되면 그 모드만 `invalid` 상태가 되고 이유가 설정 화면에 나온다.

## 4. 런타임과 제한

### 4.1 스레드와 큐

- `pod-mods` 스레드 하나. 켜진 모드마다 QuickJS `Runtime` + `Context` 하나(서로 메모리를 공유하지 않음).
- 입력(이벤트, 명령, 감지기 샘플)은 크기 1024의 큐로 들어간다. 꽉 차면 버리고 `mod.list`의 `dropped_events`를 센다(podd를 막지 않는다). 등록한 모드가 없는 이벤트와, 감지기와 맞지 않는 터미널의 샘플은 큐에 넣지도 않는다(읽기 잠금 한 번).
- 모든 콜백은 같은 스레드에서 차례로 돈다. 모드끼리 실행이 섞이지 않는다.

### 4.2 제한과 정지 (충돌 격리)

| 제한 | 값 | 넘으면 |
|---|---|---|
| 메모리 | 모드당 16 MiB (`set_memory_limit`) | 즉시 **정지** (`stopped: 메모리 한도`) |
| 스택 | 256 KiB | 예외 → 오류로 셈 |
| CPU (불러오기) | 200 ms | 즉시 정지 (`stopped: 시간 한도`) |
| CPU (콜백 한 번) | 50 ms (Promise 작업 포함) | 즉시 정지 |
| 잡히지 않은 예외 | 연속 3회 | 정지 (`stopped: 오류 반복`). 성공하면 0으로 |
| 동작 수 | 콜백 한 번에 20회, 분당 60회(알림·전송·검사·Gate) | 그 호출만 거부(예외) |
| 알림 | 제목 100자, 본문 500자 | 잘라냄 |
| 전송 | 4 KiB | 거부 |
| 타이머 | 모드당 대기 중 32개, 지연 최대 24시간 | 그 호출만 거부(예외) |

- CPU 제한은 QuickJS 인터럽트 처리기로 건다. 인터럽트 예외는 JS `try/catch`로 잡을 수 없다. 처리기는 콜백 시작 때 정한 마감 시각만 본다.
- 메모리는 QuickJS 할당자 한도다. OOM 예외가 콜백 밖으로 나오거나, 콜백이 끝난 뒤(GC 후에도) 사용량이 한도의 90 %를 넘으면 정지한다. Promise 작업 안의 OOM은 예외가 아니라 `null` 거부로만 보이므로, 작업 1024개마다 사용량을 재고 90 %를 넘은 적이 있으면서 처리 안 된 거부가 있으면 메모리 한도로 본다. JS가 OOM을 잡고 계속 돌면 결국 CPU 한도에 걸린다.
- 처리 안 된 Promise 거부(`async` 콜백의 throw 등)는 잡히지 않은 예외로 센다.
- QuickJS의 내부 assert는 빌드에서 뺐다(`disable-assertions`): 엔진 assert가 podd 전체를 끝내지 않게.
- 정지된 모드는 런타임을 버린다(메모리 반환). 그 모드가 넣은 상태 힌트는 지운다. 설정 화면에서 다시 켜면(또는 "다시 불러오기") 새로 시작한다.

## 5. 이벤트 다리 (podd 허브 → 모드)

허브 이벤트를 작은 JSON으로 바꿔 넘긴다. 모드가 `pod.on(...)`으로 등록한 이벤트만 큐에 넣는다.

| 이벤트 | 페이로드 | 호출 범위 |
|---|---|---|
| `status.changed` | `{terminal, task?, state, kind?, from, source?, badge, agent?}` (상태가 **바뀔 때만**, 신뢰도·진행률 변화는 빼고) | 그 터미널(과 그 작업) |
| `terminal.changed` | `{terminal, change, task?}` | 그 작업 |
| `task.changed` | `{task, change, state?}` | 그 작업 |
| `review.delivery` | `{delivery, task, terminal, state}` | 그 작업 |
| `gate.changed` | `{gate, task?, terminal?, state, title}` | 그 작업 |
| `group.check` | `{group, state, all_passed?}` | 없음 |
| `check.finished` | `{task, check, passed, exit_code?, timed_out, output_tail}` (`runCheck` 결과, 요청한 모드에만) | 그 작업 |
| `orch.changed` | `{run, item?, attempt?, state?, task?}` (docs/19 §16.2, **`orch` 권한**) | 워커의 작업 |
| `orch.mail` | `{run, id, from, to, type, subject, text?, attempt?, task?}` (본문 없음, **`orch` 권한**) | 워커의 작업 |

**호출 범위(scope)** 는 콜백이 다룰 수 있는 작업이다. `terminal.send`, `setStatus`, `runCheck`, `gate.request`는 대상이 그 범위 안일 때만 허용된다(§6). 팔레트 명령의 범위는 앱에서 지금 보고 있는 작업이다.

알림은 상태 스트림의 `mod.notify` 이벤트로 나간다. 앱이 꺼져 있으면 알림은 버려진다(로그에는 남는다).

## 6. 권한과 범위 확인

권한은 **manifest가 요청했고 사용자가 허용한 것**만 유효하다. 확인은 `pod-mods`(Rust)에서 하고, 거부되면 JS 예외(`PermissionDenied: ...`)가 나고 로그에 남는다.

| 권한 | API | 범위 규칙 |
|---|---|---|
| `notify` | `pod.notify` | — |
| `commands` | `pod.command` | 불러오는 동안에만 등록 가능 |
| `status` | `pod.detector`, `pod.terminal.setStatus` | 감지기: 일치한 터미널만. `setStatus`: 범위 터미널, 또는 범위 작업의 터미널 |
| `terminal.send` | `pod.terminal.send` | 범위 작업의 터미널만 (범위 작업이 없으면 거부) |
| `check` | `pod.task.runCheck` | 범위 작업만, manifest `checks`의 이름만 |
| `gate` | `pod.gate.request` | 범위 작업만 |
| `orch` | `pod.on("orch.changed" \| "orch.mail")` | 등록은 mod.json 요청이 있어야, 전달은 허용됐을 때만(`"*"`도 같음). 기본 허용 아님(우편 제목은 에이전트의 글이라) |

## 7. 모드 작성법

1. `%APPDATA%\Pod\mods\hello\` 폴더를 만든다.
2. `mod.json`:
   ```json
   { "pod": 1, "name": "hello", "version": "0.1.0", "entry": "main.js", "permissions": ["notify", "commands"] }
   ```
3. `main.js`:
   ```js
   pod.command({ id: "hi", title: "인사 알림" }, (ctx) => {
     pod.notify("안녕하세요", `작업: ${ctx.task ?? "없음"}`);
   });
   pod.on("status.changed", (ev) => {
     if (ev.state === "error") pod.notify("에이전트 오류", `${ev.terminal}: ${ev.badge}`, "error");
   });
   ```
4. Pod 설정(Ctrl+,) → **확장** → "다시 읽기" → `hello` 켜기. 팔레트(Ctrl+K)에 "hello: 인사 알림"이 생긴다.
5. 문제가 있으면 같은 화면의 로그를 본다(`console.log`도 여기로 간다).

규칙: 최상위 코드는 등록만 하고 빨리 끝낸다(200 ms). 콜백은 50 ms 안에 끝낸다. 상태는 모드 전역 변수에 둔다(다시 불러오면 초기화).

### 7.1 상태 감지기

```js
pod.detector({
  match: ["aider"],                  // argv[0] 또는 argv 안 실행 파일 이름 (.exe/.cmd 무시, 대소문자 무시)
  onScreen(ctx) {                    // ctx: {terminal, task?, lines: [화면 줄...], title?}
    const last = ctx.lines.filter((l) => l.trim()).pop() ?? "";
    if (/^\S*> ?$/.test(last)) return { state: "idle" };
    if (/\(Y\)es\/\(N\)o/.test(last)) return { state: "blocked", kind: "permission", msg: last };
    return null;                     // 모르겠으면 그대로 둔다
  },
  onTitle(ctx) { return null; },     // ctx: {terminal, task?, title}
});
```

- 화면이 바뀌었을 때 최대 250 ms마다 한 번, 타이틀은 바뀔 때 부른다(podd 상태 샘플러와 같은 주기).
- 반환: `null`/`undefined` = 변화 없음, `{state:"unknown"}` = 이 모드의 증거 지우기, `{state, kind?, msg?}` = 설정. `state`는 `idle|working|blocked|done|error`, `kind`는 `permission|question|auth|other`.
- 우선순위: **hook > osc > mod > heuristic**. 훅이나 OSC 7501을 보내는 에이전트에서는 모드 증거가 이기지 못한다. 모드 증거는 프로세스가 끝나거나 모드가 정지·꺼지면 지워진다. 같은 터미널에 두 모드가 쓰면 나중 것이 이긴다.
- 한계: 일치는 터미널을 띄운 argv로만 본다(셸 안에서 나중에 띄운 프로그램은 podd가 아직 포그라운드 프로세스를 추적하지 않아 보이지 않는다, docs/07 §8.7).

### 7.2 레이아웃 프리셋

모드 폴더의 JSON 파일이다. 코드가 없으므로 권한이 필요 없고, 모드를 켜면 팔레트에 "레이아웃: <제목>"으로 나온다.

```json
{
  "id": "review-focus",
  "title": "리뷰 집중",
  "description": "왼쪽에 변경 검토, 오른쪽에 에이전트 터미널과 승인 요청",
  "center": {
    "split": "h",
    "children": [
      { "panel": "review", "size": 0.62 },
      { "split": "v", "children": [ { "panel": "terminal", "index": 0 }, { "panel": "gates" } ] }
    ]
  }
}
```

| 노드 | 뜻 |
|---|---|
| `{"split": "h"\|"v", "children": [...]}` | 가로(h, 좌우) / 세로(v, 위아래) 나누기, 자식 2–6개, `size`(0–1 비율) 선택 |
| `{"tabs": [leaf...]}` | 한 칸에 탭으로 |
| `{"panel": "terminal", "index": N}` | 현재 작업의 N번째 터미널(없으면 열린 터미널 패널 N번째) |
| `{"panel": "terminals"}` | 열린 터미널 패널 전부를 탭으로 |
| `{"panel": "review" \| "editor"}` | 현재 작업의 검토 / 파일 패널 |
| `{"panel": "gates" \| "agents"}` | 승인 요청함 / 에이전트 트리 |

깊이 4, 잎 16개까지. 프리셋은 **가운데 영역**을 바꾼다. `gates`/`agents`는 원래 자리인 오른쪽 도크에 연다(같은 패널이 두 곳에 있지 않게). 만들 수 없는 잎(현재 작업 없음, 터미널 부족)은 건너뛰고, 빈 나누기는 없앤다(자식이 하나 남으면 그 자식이 나누기의 `size`를 받는다). 프리셋에 없는 열린 터미널 패널은 첫 터미널 칸에, 검토·파일 패널은 그 뒤에 탭으로 들어간다(닫히지 않음: 저장 안 한 파일 보호). 팔레트 항목의 프리셋이 그새 사라졌으면(모드를 끔) "레이아웃을 찾을 수 없습니다" 오류가 뜬다. 프리셋 파일이 잘못되면 그 모드 전체가 `invalid`가 되고 설정 카드에 "레이아웃 프리셋 오류: …"가 나온다. 다 비면 "적용할 패널이 없습니다" 알림만 뜬다. `size`는 창 크기에 대한 비율로 픽셀을 정한다.

## 8. JS API 참조 (`pod`)

| API | 반환 | 설명 |
|---|---|---|
| `pod.name`, `pod.version` | string | 이 모드의 이름·버전 |
| `pod.on(event, fn)` | — | §5 이벤트 등록. `"*"`는 모든 이벤트(`fn(ev, name)`) |
| `pod.command({id, title, detail?}, fn)` | — | 팔레트 명령. `fn(ctx)`, `ctx = {task?, terminal?}`. id는 `[a-z0-9-]{1,32}` |
| `pod.detector({match, onScreen?, onTitle?})` | — | §7.1 |
| `pod.notify(title, body?, level?)` | — | level: `info`(기본)·`success`·`warning`·`error` |
| `pod.log(...args)`, `console.log/warn/error` | — | 모드 로그 |
| `pod.terminals()` | `[{id, name?, task?, state, kind?, agent?, argv0, running}]` | |
| `pod.tasks()` | `[{id, name, repo, state?, terminals: [id]}]` | |
| `pod.groups()` | `[{id, name, members: [{task, state?}]}]` | `state`는 작업 집계 상태 |
| `pod.terminal.send(id, text, {enter?})` | — | `terminal.send` 권한, 범위 확인 |
| `pod.terminal.setStatus(id, state, {kind?, msg?})` | — | `status` 권한. `state: "unknown"`이면 지움 |
| `pod.task.runCheck(task, name)` | — | `check` 권한. 작업 worktree에서 manifest의 argv를 실행(창 없음, `group.check`와 같은 실행기). 비동기: 끝나면 `check.finished` 이벤트. 시간 제한 10분 |
| `pod.gate.request(task, title, message)` | gate id | `gate` 권한. 제목 앞에 `[모드 이름]`이 붙는다 |

| `setTimeout(fn, ms, ...args)` / `setInterval(fn, ms, ...args)` | timer id | §8.1. 문자열 코드는 받지 않는다 |
| `clearTimeout(id)` / `clearInterval(id)` | — | 없는 id는 무시 |

모든 거부·검증 실패는 `Error`를 던진다(메시지 `PermissionDenied: ...`, `InvalidArgument: ...`, `RateLimited: ...`). `Promise`와 `async` 함수는 쓸 수 있지만 한 콜백(Promise 작업 포함)은 50 ms 안에 끝나야 한다. 오래 기다릴 일은 타이머로 나눈다.

### 8.1 타이머

```js
let left = 3;
const id = setInterval(() => {
  pod.log("남은 횟수", left);
  if (--left === 0) clearInterval(id);
}, 1000);
pod.on("status.changed", (ev) => {
  if (ev.state === "blocked") setTimeout(() => pod.notify("아직 기다리는 중", ev.terminal), 60_000);
});
```

- **모드 스레드가 돌린다**: podd의 모드 스레드가 입력 큐를 기다리는 동안 다음 타이머 시각까지만 기다렸다가, 때가 된 타이머를 부른다(tokio 작업과 무관). 같은 시각이면 만든 순서대로, 늦은 것은 다음 입력보다 먼저 부르지만 그 사이에 새로 때가 된 타이머는 다음 차례로 미룬다(큐를 굶기지 않음).
- **제한**: 모드당 대기 중 32개(넘으면 `RateLimited` 예외), `setTimeout`은 10 ms, `setInterval`은 100 ms보다 짧으면 그 값으로 올린다, 24시간 넘으면 `InvalidArgument`. 타이머 콜백도 **콜백 한 번 50 ms CPU 한도**와 동작 수 제한(§4.2)을 그대로 받는다. 무한 루프면 모드가 정지된다(`stopped: 시간 한도`), 예외 3회 연속이면 정지.
- **범위**: 타이머는 만든 콜백의 범위(작업·터미널)로 실행된다. 이벤트 콜백에서 만든 타이머는 그 작업에 `notify`·`send` 등을 할 수 있고, 불러오기 중에 만든 타이머는 범위가 없다. 타이머로 범위를 넓힐 수는 없다.
- **정리**: 모드를 끄거나, 정지되거나, 다시 불러오면(권한 변경 포함) 대기 중인 타이머는 모두 사라진다. `setInterval`은 콜백이 끝난 시각부터 다시 잰다(밀린 횟수를 몰아서 부르지 않음).

### 8.2 알림이 보이는 곳

`pod.notify`는 앱 안 알림(오른쪽 아래 토스트, 알림 기록)을 항상 띄운다. 그리고 **Pod 창이 앞에 있지 않으면**(다른 창이 활성, 또는 최소화) Windows 알림(토스트)도 띄운다.

- 끄기: 설정(Ctrl+,) → **Windows 알림: 끔**. 기본은 켬.
- 내용: 제목, 본문, 출처 줄 "<모드 제목> · Pod". 소리는 Windows 알림 설정을 따른다.
- 누르면: Pod 창을 앞으로 가져오고(최소화면 복원) 알림의 터미널을, 없으면 그 작업의 첫 터미널을 연다.
- 조건: Windows가 Pod를 알아볼 AppUserModelID `Pod.App`이 등록되어 있어야 한다. 설치 프로그램이 `HKCU\Software\Classes\AppUserModelId\Pod.App`을 만들고(제거 시 삭제) 바로 가기에도 같은 id를 넣는다(docs/14). 설치하지 않은 빌드(포터블, `cargo run`)나 Windows 설정에서 Pod 알림을 끈 경우에는 앱이 레지스트리를 쓰지 않고 **조용히 앱 안 알림만** 쓴다(앱 로그에 한 번 `OS 알림을 쓸 수 없어 …`).
- 앱이 꺼져 있으면 알림은 버려진다(§5, 데몬 로그에는 남음).

## 9. 프로토콜 (podd v11, 추가형)

| 메서드 | 매개변수 | 결과 |
|---|---|---|
| `mod.list` | `{}` | `{enabled: bool, root, mods: [ModInfo], dropped_events}` |
| `mod.set_enabled` | `{name, enabled}` | `ModInfo` |
| `mod.grant` | `{name, permission, granted}` | `ModInfo` |
| `mod.log` | `{name, tail?}` (기본 200) | `{name, lines: [string]}` |
| `mod.command` | `{name, command, task?, terminal?}` | `{}` (실행 오류는 RPC 오류) |
| `mod.reload` | `{name?}` | `mod.list` 결과 |

`ModInfo = {name, version, title?, description?, enabled, state: disabled|running|stopped|invalid, stop_reason?: cpu|memory|errors|load, error?, permissions: [{name, requested, granted}], checks: [{name, argv}], commands: [{id, title, detail?}], detectors: [{match}], layouts: [LayoutPreset], log_lines, memory_kib}`. `commands`와 `layouts`는 실행 중일 때만 채운다.

상태 스트림(`status.subscribe`) 알림: `mod.notify {seq, mod, title, body, level, task?, terminal?}`, `mod.changed {seq, mod: ModInfo}` (켜기/끄기/정지/다시 불러오기). 둘 다 `ids` 필터가 없는 구독자에게만 간다.

`StatusInfo.source`에 `"mod"`가 추가된다(문자열이라 와이어 호환). 이전 앱은 출처를 "unknown"으로 보여 줄 뿐이다.

> v10은 PR 연동(`task.pr.*`, docs/15)이 먼저 썼다. 확장은 그 위의 v11이다.

## 10. 구현 위치와 검증

### 10.1 코드

| 곳 | 내용 |
|---|---|
| `crates/pod-mods` | `manifest.rs`(manifest·권한·레이아웃 검증), `runtime.rs`(QuickJS 샌드박스·제한), `prelude.js`(`pod` API), `manager.rs`(모드 스레드·권한/범위 확인·로그·`ModHandle`), `host.rs`(`Host` trait), `types.rs`(와이어 타입). 기능 `runtime`(기본)을 끄면 와이어 타입만 빌드된다(pod-app·pod-cli는 QuickJS를 링크하지 않는다) |
| `crates/pod-status` | `Source::Mod`(hook > osc > **mod** > heuristic > process), `TerminalStatus::on_mod`, confidence 0.75 |
| `crates/podd` | `server/mods.rs`(새 모듈: `PoddHost`, 이벤트 다리, `mod.*` RPC), `mods_protocol.rs`(새 파일). 기존 파일은 몇 줄씩: `status.rs`(HubEvent 2개, `on_mod`/`clear_mod`, 샘플러가 감지기에 화면·타이틀 전달), `server/mod.rs`(start·dispatch 각 1줄), `lifecycle.rs`(헬퍼 2개 `pub(super)`), `client.rs`(`SubscribeEvent::ModNotify/ModChanged`), 프로토콜 **v11** |
| `crates/pod-app` | `mods_ui.rs`(새 모듈: "확장" 페이지, 팔레트 명령·레이아웃, 알림 토스트, 레이아웃 적용). 설정 화면에 "확장 관리" 줄, 팔레트 `mod:`/`layout:`/`extensions`, `SourceEvent::ModNotice/ModChanged`, 시작 옵션 `--extensions` |
| `examples/mods` | `aider-status`(감지기), `group-done-notify`(이벤트·알림·팔레트 명령), `pod-layouts`(프리셋 "리뷰 집중", "4분할 에이전트") |
| `crates/pod-app/tools/mods-demo.ps1` | 스크린샷: 격리 데몬 `podd-mods-demo`, `target/podapp-demo/mods`, 가짜 Aider 화면(`mods-demo/aider.ps1`, 고정 텍스트만), 데모 전용 모드 `demo-notify`. 합성 입력 없음 |

의존성: `rquickjs` 0.14(+`rquickjs-core`, `rquickjs-sys`: MIT, QuickJS-ng C 소스 MIT), `hashbrown`/`foldhash`/`allocator-api2`(MIT/Apache). `cargo deny check`: `advisories ok, bans ok, licenses ok, sources ok`.

### 10.2 테스트 (2026-11-09, master 병합 후 전체 447 통과 / 0 실패 / 3 무시)

| 테스트 | 확인하는 것 |
|---|---|
| `pod-mods` `runtime::tests` (6) | 샌드박스 탈출 시도: `require`·`process`·`std`·`os`·`fetch`·`XMLHttpRequest`·`WebSocket`·`setTimeout`·`print`·`scriptArgs` 등 없음, `import("os")` 거부, `eval`/`new Function`으로도 못 찾음, `pod`·`__pod_dispatch` 덮어쓰기 불가 · 무한 루프(불러오기 중, 콜백 안 `try/catch` 중첩, 끝없는 Promise 사슬) 정지 · 메모리 폭탄(문자열, 배열, `catch`로 OOM 삼키기) 정지 · 예외 메시지 · 호스트 오류가 잡을 수 있는 JS 예외가 됨 |
| `pod-mods` `manifest::tests` (2) | manifest 규칙(API 버전, 이름=폴더, 모르는 권한, `..`·절대 경로, 모르는 필드), 레이아웃 규칙(깊이, 잎 수, index, size) |
| `pod-mods` `tests/manager.rs` (9) | 발견·켜기·권한 허용과 저장(새 매니저도 같은 상태) · 등록한 모드에만 이벤트 · 권한 거부(요청 안 함/허용 안 함) · 범위 밖 터미널·작업 거부, manifest에 없는 검사 거부 · 콜백당 20회 제한 · 루프·예외 3회·메모리로 정지되고 다른 모드는 계속 동작, 정지 시 상태 힌트 지움 · 감지기는 맞는 터미널만, 잘못된 반환값 기록, 권한 취소 시 해제 · 팔레트 명령과 레이아웃만 있는 모드 · 큐가 차면 막지 않고 셈 · **예제 모드 3개**(Aider 화면 4개 → idle/working/blocked:question/done, 그룹 완료 알림 한 번·다음 라운드 다시, 요약 명령) |
| `pod-status` `tests/mod_source.rs` (3) | 우선순위 hook > osc > mod > heuristic, 지우기·`ack`·프로세스 종료 |
| `podd` `tests/mods.rs` (1) | 실제 데몬: 하네스가 `POD_MODS_DIR`를 임시 폴더로(사용자 `%APPDATA%` 안 건드림), `mod.reload/list/set_enabled/log/command/grant`, 감지기 → 출처 `mod`, OSC working이 mod를 이김, OSC clear 뒤 mod로 돌아감, `mod.notify`·`mod.changed` 스트림, 무한 루프 명령이 5초 안에 정지되고 데몬은 계속 응답, 끄면 증거 지워짐, 로그 파일 |
| `pod-app` `mods_ui::tests` (2) | 권한·정지 이유 한국어 라벨, 예제 프리셋 파싱 |
| `pod-mods` 타이머 (2026-11-09, `tests/manager.rs` 4 + 샌드박스 검사) | 순서(지연 순, 같으면 만든 순서)·인자·`setInterval` 최소 100 ms·지운 interval은 다시 안 돎·이벤트 범위 이어받음 · `clearTimeout`(미리, 다른 타이머 안에서, 없는 id) · 33번째 타이머 `RateLimited`, 24시간 초과, 함수 아님 · 무한 루프 타이머 → CPU 정지, 예외 반복 interval → 정지, 정지·끄기·다시 읽기에서 타이머 사라짐, 다른 모드 타이머는 계속 · 문자열 타이머 거부, 타이머 함수 덮어쓰기 불가 |
| `pod-app` `os_notify::tests`, `mods_ui::tests` (2026-11-09) | Windows 알림을 띄울 조건(설정 켬 + 창 비활성 또는 최소화), 가능 여부(등록 안 된 id·Windows에서 끔 → 한국어 이유), 등록 안 된 id는 WinRT를 부르지 않고 조용히 포기, 토스트 XML 이스케이프·한국어, 클릭 시 열 터미널(알림의 터미널 → 작업의 첫 살아 있는 터미널 → 창만), 예전 설정 파일은 켬 |
| `pod-app` `native_qa` `h_*` (2026-11-09) | **실제 사용자 경로**: Ctrl+K → "리뷰 집중" Enter → 가운데가 `h[검토, 터미널 w1 + w2 탭]`, 승인 요청함은 오른쪽 도크 · "4분할 에이전트"(터미널 2개) → 한 줄, 프리셋에 없는 검토 패널은 닫지 않고 탭으로 · 종료 경로(`on_quit`)로 저장 → 새 앱이 같은 배치로 복원 · 사라진 프리셋 → 한국어 오류 "레이아웃을 찾을 수 없습니다" · 잘못된 프리셋 파일의 모드 → 카드에 "레이아웃 프리셋 오류: …", 팔레트에 안 나옴 |

clippy(`pod-mods`, `podd`, `pod-status`, `pod-app`, `pod-ui`, `--all-targets`): 경고 0. release 빌드 통과.

### 10.3 화면 (서브 모니터 150 %, DPI 인식 PrintWindow, `crates/pod-app/qa/mods/`)

| 파일 | 내용 |
|---|---|
| `mods-settings-dark.png` / `mods-settings-light.png` | "확장" 페이지: 모드 폴더·다시 읽기, 모드 카드(이름·버전·상태 칩·켜기/끄기, 설명, 권한 버튼 "상태 감지: 허용", 제공 기능·메모리, 로그 보기) |
| `mods-action-idle-dark.png` | `aider-status`가 가짜 Aider의 "Waiting for …"/스피너를 읽어 사이드바 "작업 중 ≈"(추정 표시, confidence 0.75) |
| `mods-action-question-dark.png` | "(Y)es/(N)o" 질문 → "답변 필요"와 데모 모드의 알림 토스트 "에이전트가 답을 기다립니다 · 데모: 질문 알림". `pod status`: `blocked:question source=mod confidence=0.75` |

접근성: 카드는 `Role::Group` + 이름·상태 라벨, 오류는 `Role::Alert`, 로그는 `Role::Document`, 모든 버튼에 동작과 결과를 읽어 주는 접근성 이름("Aider 상태 감지 권한 상태 감지: 허용. 누르면 거부"). 키보드: 페이지는 팔레트("확장 관리")나 설정의 버튼으로 열린다. 버튼은 표준 Button이라 Tab 순서에 들어가지만, 실제 키 입력으로는 확인하지 않았다.

### 10.4 실제 앱 확인 (2026-11-09, `crates/pod-app/tools/mods-qa.ps1`, 서브 모니터, `crates/pod-app/qa/mods/qa-*`)

입력은 사용자 키보드·마우스를 쓰지 않고 Pod 창에 메시지로 보낸다(Ctrl+K는 Pod 입력 스레드의 키 상태만 잠깐 바꿈).

| 확인 | 결과 |
|---|---|
| `Qa-Layout`: 터미널 4개 격자 → 팔레트 "리뷰 집중" → 창 닫고 다시 열기 | 첫 터미널 + 나머지 탭, 승인 요청함은 오른쪽 도크(`qa-layout-2`). 다시 연 앱이 같은 배치(`qa-layout-3`, 저장된 `layout.json` 가운데 `h[tabs(t1,t2,t3,t4)]`) |
| `Qa-InvalidPreset`: 자식 1개짜리 나누기를 가진 프리셋 | 카드에 "설정 오류", "레이아웃 프리셋 오류: layout layouts/bad.json: a split needs 2-6 children"(`qa-invalid-preset`) |
| `Qa-ToastFallback`: 등록 안 된 `Pod.App`, 창 최소화 | 앱 안 알림만, 앱 로그 한 줄 "OS 알림을 쓸 수 없어 … AppUserModelID가 등록되지 않음", 앱 계속 동작 |
| `Qa-Toast`: 임시 HKCU id `Pod.QA.Toast`(끝나면 지움), 창 최소화 | Windows 알림 기록에 1건: 제목 "에이전트가 답을 기다립니다", 본문 "t1: blocked:question (확장 감지)", 출처 "데모: 질문 알림 · Pod"(`qa-toast-history.xml`). 주 모니터 화면은 찍지 않는다(사용자 창이 함께 찍힘) |

토스트를 실제로 눌러 보는 것은 하지 않았다(주 모니터에 뜨는 알림을 합성 클릭해야 함). 클릭 처리는 `mods_ui::tests`(열 터미널 고르기)와 GPUI `activate_window`(최소화 복원)로만 확인했다.

### 10.5 한계와 남은 일

- 감지기는 터미널을 띄운 argv로만 맞춘다(셸 안에서 나중에 실행한 프로그램은 모름).
- 알림은 앱이 켜져 있을 때만 보인다(데몬 로그에는 남음). Windows 알림은 설치한 Pod에서만(§8.2). 알림 화면 위치는 Windows가 정한다(주 모니터).
- 레이아웃 `size`는 창 크기 비율 근사다. `gates`/`agents`는 가운데에 넣지 않고 오른쪽 도크에 연다.
- 설치는 폴더 복사뿐(서명·업데이트·마켓 없음). 앱에서 모드 폴더 열기 버튼, 모드별 설정값, `pod mod` CLI는 후속.
- 권한 버튼 클릭은 실제 클릭으로 확인하지 않았다(합성 입력을 쓰지 않음). 레이아웃 적용은 GPUI 키 입력 경로(팔레트)로 확인한다(§10.2 `h_*`, §10.4).

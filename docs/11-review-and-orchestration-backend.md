# P4·P5 백엔드: 리뷰 코멘트 → 에이전트, Fan-out 그룹, 게이트

작성일: 2026-11-08 · 브랜치 `SJin-Choi/review-orch-backend` · 코드: `crates/podd/src/orch_protocol.rs`, `crates/podd/src/server/{review,groups,gates,orch_store}.rs`, `crates/pod-cli/src/orch.rs` · 상위 기획: [01 §4.2 (3)(4), §6 P4·P5, §8](01-plan.md) · 기반: [03](03-podd-p0.md), [07 §8](07-pod-status.md), [08](08-tasks.md), [05 §2 코멘트 JSON](05-diff-spike.md)

P4(리뷰)와 P5(오케스트레이션)의 **데몬 API와 `pod` CLI**만 만들었다. UI는 pod-app에서 나중에 붙인다. 다른 제품의 코드를 보지 않은 클린룸 구현이다. 새 의존성은 없다(`cargo deny check` 통과).

---

## 1. 결정

| 항목 | 결정 | 이유 |
|---|---|---|
| 저장 | **JSON 파일 하나** `<state_dir>\orch\store.json` (코멘트·전달 기록·그룹·게이트·id 카운터) | 08의 레지스트리와 같은 방식: 임시 파일 + `rename`, 깨진 파일은 `store.corrupt-<ms>.json`으로 옮김. 데몬 재시작 뒤에도 남는다. |
| 프로토콜 | **v5** (v4 + `review.*`, `group.*`, `gate.*`). 버전은 한 번만 올렸다. | 모든 변경은 추가형이다. 기존 결과 타입에는 `#[serde(default)]` 필드만 붙였고(`TaskInfo.review/gates/group`, `StatusSubscribeResult.gates`), 구독 스트림에는 새 알림 `gate.changed`만 더했다. 예전 클라이언트는 모르는 알림을 무시한다. |
| 코멘트의 코드 인용 | **추가할 때** 캡처한다. `new` = worktree 파일, `old` = `merge-base(base, HEAD)`의 파일. UI가 `code`를 보내면 그것을 쓴다. | 리뷰어가 본 코드가 프롬프트에 그대로 들어간다. 에이전트가 그사이 파일을 고쳐도 인용이 어긋나지 않는다. |
| 프롬프트 전달 | **bracketed paste**(`ESC[200~ … ESC[201~`, 줄바꿈은 `\r`) → 프로그램이 붙여넣기를 **다 받은 뒤** 별도의 Enter(`\r`) → **제출 확인**(v8, e2e-fix1) | 여러 줄이 한 메시지로 들어간다. Claude Code는 bracketed paste를 쓴다. Codex(Windows)는 콘솔 키 이벤트를 읽는다. 번들 ConPTY는 붙여넣기 마커를 지우고 내용을 키 이벤트로 넘기므로 Codex는 타이밍으로 붙여넣기를 감지한다(8 ms 안에 이어진 글자 = burst, 60 ms 조용하면 표시, burst 마지막 글자 120 ms 안의 Enter는 **줄바꿈**). 예전의 250 ms 고정 대기는 Codex가 입력을 늦게 처리하면 이 창 안에 들어가 프롬프트가 입력창에 남았다(docs/12 B1). 이제 Enter는 §3.1 "제출 확인" 규칙으로 누른다. 마커는 항상 보낸다. 프로그램이 DECSET 2004를 켰는지는 진단용으로 `app_bracketed_paste`에 남긴다. |
| 전달 시점 | 상태 엔진(07 §8)의 **표시 상태**로 판단: `working` → 큐, `blocked` → 거절(`AGENT_BLOCKED`), `exited` → 거절(`TERMINAL_EXITED`), 그 밖(`idle` `done` `unknown` `error`) → 즉시. 권한을 승인한 뒤 명령이 도는 동안에는 이제 `working`(07 §3.1-6)이라 거절되지 않고 큐에 든다(docs/12 B2, 실제 Codex로 확인) | `unknown`도 즉시 보낸다. 상태를 보고하지 않는 프로그램(셸 등)에도 보낼 수 있어야 하기 때문이다. |
| 그룹 멤버의 프롬프트 | claude/codex는 08처럼 CLI 첫 인자, shell/argv는 **시작 직후 입력으로 타이핑 + Enter** | 08의 `task.create`는 shell에 prompt를 받지 않는다. 그룹에서는 셸 에이전트도 같은 프롬프트로 시작해야 테스트·스크립트 에이전트를 쓸 수 있다. |
| 게이트 인증 | 따로 없음. 파이프 DACL(사용자 전용)이 경계다. | `gate.create`는 어차피 같은 사용자만 부를 수 있다. 터미널 안의 `pod`는 `POD_TERMINAL_ID`/`POD_TASK_ID`를 붙여 보낼 뿐이다. |

## 2. 데이터 모델

```
store.json
{ "version": 1,
  "counters": {"c": 4, "d": 2, "g": 2},
  "comments":   [ReviewComment],
  "deliveries": [{info: DeliveryInfo, turn_at_delivery?}],
  "groups":     [{id, repo, name, base, base_commit, prompt, check?, created_ms,
                  picked?, picked_ms?, members: [{task, agent, check?: CheckResult}]}],
  "gates":      [GateInfo] }
```

- id: 코멘트 `c<N>`, 전달 `d<N>`, 게이트 `g<N>`(state dir 안에서 유일, 재시작 뒤에도 이어짐). 그룹 id는 `<repo>/<name>`.
- 코멘트 상태: `open` → (`review.send`) → 에이전트가 working이면 `queued` → 전달되면 `sent` → 리뷰어가 `resolved`. 전달이 실패하거나 취소되면 다시 `open`. `reopen`으로 `resolved` → `open`.
- Task를 지워도 코멘트·전달 기록·게이트는 남는다(기록). 그 Task로 큐에 있던 전달은 `failed`가 된다.

## 3. API (프로토콜 v5, v6 추가분 표시)

전체 스키마는 `pod server schema`. 새 오류 코드는 §3.4.

### 3.1 리뷰 (P4)

| 메서드 | params | result |
|---|---|---|
| `review.comments.add` | `{task, comments: [{file\|path, side?: old\|new (기본 new), line, end_line?, body, code?}]}` | `{comments: ReviewComment[]}` (추가된 것) |
| `review.comments.list` | `{task?, state?: open\|queued\|sent\|resolved}` | `{comments}` |
| `review.comments.resolve` | `{ids, note?, reopen?}` | `{comments}` |
| `review.comments.remove` | `{ids}` | `{comments}` (지운 것) |
| `review.send` | `{task, comment_ids?, all_open?, instructions?, terminal?, dry_run?}` | `DeliveryInfo` |
| `review.deliveries` | `{task?}` | `{deliveries: DeliveryInfo[], submits? (v8): {id: SubmitCheck}}` |
| `review.cancel` | `{id}` (큐에 있는 전달) | `DeliveryInfo` (`cancelled`, 코멘트는 `open`으로) |

- `ReviewComment = {id, task, file, side, line, end_line?, body, code?, state, created_ms, delivery?, resolved_ms?, note?}`. `file`은 worktree 기준 `/` 경로로 정규화한다(`\` 허용, 절대 경로·`..`·드라이브 문자는 `INVALID_PARAMS`). 리뷰 UI 내보내기 형식(05 §2)의 `path`를 `file`의 별칭으로 받는다. 인용은 최대 40줄이고, 넘으면 `…` 줄을 붙인다. 바이너리·없는 줄이면 `code`가 없다.
- `DeliveryInfo = {id, task, terminal, comment_ids, state: queued|delivered|cancelled|failed|preview, prompt, created_ms, delivered_ms?, agent_state, delivered_state?, app_bracketed_paste?, error?}`. `agent_state`는 요청 시점, `delivered_state`는 전달 시점의 상태 라벨이다. `delivered`는 **입력했다**는 뜻이지 에이전트가 받아들였다는 뜻이 아니다. 그것은 `submit`(아래)이 말한다.
- **제출 확인(v8, e2e-fix1, docs/12 B1)**: `SubmitCheck = {state: pending|confirmed|unconfirmed|unverifiable, enters, signal?: working|blocked|turn, entered_ms?, settled_ms?}`. `DeliveryInfo`에는 필드를 더하지 않았다(pod-app 테스트가 구조체를 직접 만들어 추가형이 아니다). 대신 `review.deliveries` 결과에 `submits: {<delivery id>: SubmitCheck}`, `review.delivery` 이벤트에 `submit?`가 붙는다. 전달 이벤트는 `submit.state = pending`으로 먼저 오고, 확정되면 **같은 전달의 두 번째 이벤트**가 `confirmed`/`unconfirmed`/`unverifiable`로 온다(DeliveryInfo 내용은 같다). 규칙(`server/submit.rs`, `terminal.send`와 공통): 붙여넣기 후 프로그램이 무언가를 그리고 300 ms 조용해지면 Enter(최소 150 ms, 아무것도 그리지 않으면 1 s, 최대 1 s + 글자당 0.5 ms ≤ 5 s). 에이전트 상태가 보이는 터미널(에이전트를 알거나 출처가 hook/osc)이고 바쁘지 않았으면 4 s 안에 working/blocked가 되거나 turn이 늘어야 `confirmed`. 아니면 Enter를 한 번 더 누르고(줄바꿈이 된 Enter라면 이것이 제출한다. 빈 입력창의 Enter는 아무것도 하지 않는다) 다시 4 s를 본다. 그래도 없으면 `unconfirmed`(로그와 이벤트, 프롬프트가 입력창에 남아 있을 수 있음). 셸처럼 상태가 없으면 `unverifiable`(한 번만 누름). 확인이 `pending`인 동안 같은 터미널의 다음 큐 전달은 기다린다. 데몬이 확인 도중 재시작되면 `unconfirmed`로 남긴다. 붙여넣기는 RPC 안에서 하고, Enter와 확인은 백그라운드에서 한다(RPC는 더 이상 250 ms를 기다리지 않는다).
- `review.send` 선택 규칙: `comment_ids`(해결된 것은 `INVALID_STATE`, 다른 Task 것은 `REVIEW_NOT_FOUND`) + `all_open`이면 그 Task의 `open` 코멘트 전부. 보낼 것이 없으면 `INVALID_STATE`. 프롬프트 안에서는 파일, 줄 순으로 정렬한다. `terminal` 기본값은 Task의 첫 터미널(에이전트 터미널)이다. `dry_run`은 프롬프트만 돌려주고 아무것도 저장하지 않는다(`id = "preview"`).

**프롬프트 형식** (실제 테스트 출력의 일부):

~~~~text
Code review feedback on your changes in this worktree (task demo/rev). Please address each of the 3 comment(s) below. File paths are relative to the worktree root; line numbers refer to the current file unless marked as the base version. When you are done, reply with a short summary per comment.

Instructions from the reviewer:
Fix these, then run the tests.
Do not touch README.md beyond the comment.

1. README.md:1
```
   1 | hello
```
Reviewer: Say more

2. src/lib.rs:2-4
```
   2 | fn two() {
   3 |     let y = `tick`;
   4 | }
```
Reviewer: Why rename x to y?
Keep the old name.

3. src/lib.rs:3 (base version, before your changes)
...
~~~~

- 코드 펜스는 인용 안의 가장 긴 백틱 연속보다 길게 만든다.
- 본문·인용·지시문에서 제어 문자(ESC, 탭·줄바꿈 외의 C0, DEL, C1)를 지운다. 코멘트에 `ESC[201~`가 들어 있어도 붙여넣기가 일찍 끝나거나 터미널을 조종하지 못한다.

**전달 규칙**

1. 요청 시점: 터미널이 끝났으면 `TERMINAL_EXITED`. 표시 상태가 `blocked:*`이면 `AGENT_BLOCKED`(`data = {state, msg}`, 메시지에 질문/권한 내용). 아니면 아래 조건을 모두 만족할 때 즉시 전달하고, 아니면 `queued`로 저장한다.
   - 상태가 working/blocked/exited가 아님
   - 같은 터미널에 먼저 큐에 들어간 전달이 없음(순서 보장)
   - 같은 터미널의 직전 전달이 아직 "처리 중"이 아님: 직전 전달 때의 turn 카운터보다 지금 turn이 커졌거나(에이전트가 일하고 멈춤), 30 s가 지났음(최후 수단).
     - (podd-hardening) 상태 출처가 `hook`/`osc`가 아닌 터미널(셸, 휴리스틱으로만 아는 Codex 같은 프로그램)은 turn이 늘지 않을 수 있다. 이런 터미널은 전달 후 1 s(붙여넣기 + Enter + 에코)가 지난 뒤, **전달 이후 상태가 바뀌었거나**(휴리스틱 전이), **출력이 1.5 s 동안 없으면** 처리된 것으로 본다. 상태 전이(`queued` → `delivered`)는 여전히 store 락 안에서 하므로 정확히 한 번이다.
2. 큐 작업자(`review::start`): 상태 허브의 모든 이벤트, `review.send`의 큐 등록, 큐가 있을 때 250 ms(없으면 5 s) 주기에 깨어난다. 터미널마다 가장 오래된 전달 하나만 위 조건으로 검사해 보낸다. 터미널이 사라졌거나 끝났으면 `failed`(코멘트는 `open`).
3. 상태 전이(`queued` → `delivered`)는 store 락 안에서 하고, 타이핑은 그다음에 한다. 그래서 요청 처리기와 작업자가 같은 전달을 두 번 보낼 수 없다(exactly-once). 큐에서 전달 중 blocked가 되면 계속 기다린다(거절은 요청 시점에만).
4. 데몬이 재시작되면 `queued` 전달은 store에서 다시 읽혀 새 작업자가 이어서 보낸다.
5. **큐 타임아웃(podd-gaps2, v7)**: `created_ms`부터 `podd --review-queue-timeout <기간>`(기본 `30m`, 환경 변수 `POD_REVIEW_QUEUE_TIMEOUT`, `off`면 끔)이 지난 `queued` 전달은 작업자가 `failed`로 바꾸고 `error = "expired: queued for 31m (review queue timeout 30m); comments are open again"`를 넣는다. 코멘트는 `open`으로 돌아가고 `review.delivery` 이벤트가 나간다. 데몬이 꺼져 있던 동안 지난 것도 다음 시작 직후 처리한다(`created_ms`가 저장돼 있음). `DeliveryState`에 새 값을 더하지 않은 이유: pod-app이 이 열거형을 빠짐없이 match하므로 새 값은 추가형이 아니다. 만료는 `failed` + `error`가 `expired:`로 시작하는 것으로 구분한다.

### 3.2 그룹 / fan-out (P5)

| 메서드 | params | result |
|---|---|---|
| `group.create` | `{repo, name, base?, prompt, agents: AgentSpec[], check?: argv, parent_terminal?, cols?, rows?}` | `GroupInfo` |
| `group.list` | `{repo?}` | `{groups: GroupInfo[]}` |
| `group.show` | `{group}` | `GroupInfo` |
| `group.compare` | `{group}` | `{group, base, check?, members: CompareMember[]}` |
| `group.check` | `{group, check?: argv, timeout_ms?, tasks?, wait?: bool}` | `{group, results: [{task, result: CheckResult}], all_passed, job?: CheckJob}` |
| `group.check_cancel` (v6) | `{group}` | `group.check`과 같은 결과(job이 끝난 뒤 응답, 최대 15 s) |
| `group.pick` | `{group, task, remove_others?, force?, delete_branch?}` | `{group, picked, removed: TaskRemoveResult[], kept: [{task, code, message}]}` |
| `group.remove` | `{group}` | `{id}` (기록만 지움, Task는 남김) |

- `group.create`: `agents`는 1–16개, `AgentSpec`은 08과 같다(`"claude" | "codex" | "shell" | {"argv": [...]}`). base를 **한 번** 커밋으로 풀고, 모든 멤버를 그 커밋에서 `task.create`로 만든다. 멤버 이름은 `<name>-<agent>-<n>`(agent = `claude` `codex` `shell` `argv`, n은 agent별 1부터). 각 Task의 `base`는 사용자가 준 ref 이름, `base_commit`은 그 커밋, `prompt`는 공통 프롬프트로 기록한다. 멤버 하나라도 실패하면 이미 만든 멤버를 `force + delete_branch`로 지우고 오류를 돌려준다. 같은 그룹 id나 멤버 Task가 이미 있으면 `GROUP_EXISTS`.
- `GroupInfo = {id, repo, name, base, base_commit, prompt, check?, created_ms, picked?, picked_ms?, members: [{task, agent, info?: TaskInfo, removed, picked, check?}], gates}`. `info`는 08의 `TaskInfo`(상태 집계와 git 정보 포함)이고 지워진 멤버는 `removed: true`. `gates`는 그룹 게이트 + 멤버 Task의 게이트.
- `CompareMember = {task, agent, removed, picked, status?, files: DiffFile[], files_changed, added, deleted, ahead?, dirty?, check?, error?}`. diff는 08의 `task.diff`와 같다(커밋 + 미커밋 + 추적 안 된 파일, merge-base 기준). 순위는 매기지 않는다. UI가 check 통과, 상태, 변경량으로 정렬한다.
- `group.check` (v6에서 바뀜, podd-hardening): **백그라운드 작업**으로 시작하고 바로 돌아온다(`job.state = running`, `results` 비어 있음). 예전에는 요청이 끝날 때까지(최대 10분) 그 연결의 RPC가 막혔다. `CheckJob = {id: k<N>, state: running|finished|cancelled|interrupted, argv, tasks, done, timeout_ms, started_ms, finished_ms?, all_passed?}`는 `GroupInfo.check_job`에 저장된다. 멤버 결과는 끝나는 대로 멤버 기록(`group.show`/`compare`의 `check`)에 저장되고, `status.subscribe` 스트림(필터 없는 구독자)에 `group.check {seq, group, job, member?}` 알림으로 온다(시작, 멤버마다, 끝). `wait: true`면 작업이 끝난 뒤 전체 결과로 응답한다(그 연결만 기다리고 데몬은 다른 요청을 계속 처리한다. CLI가 이 방식). 그룹마다 작업은 하나만 돈다(두 번째는 `INVALID_STATE`). `group.check_cancel`은 실행 중인 명령(Job Object 전체)을 죽이고 `cancelled`로 끝낸다(멤버 결과 `error: "cancelled"`). 데몬이 작업 도중 멈추면 다음 시작 때 `interrupted`로 표시한다. 시간 제한은 멤버별 `timeout_ms`(기본 10분)다.
- 실행 방식: 저장된 check argv(또는 이번에 준 argv, 저장됨)를 각 멤버 worktree에서 **동시에** 실행한다. `CREATE_NO_WINDOW`, stdin null, `GIT_DIR` 등 제거, Job Object(닫으면 하위 프로세스까지 종료)로 감싼다. 기본 제한 10분, 넘으면 Job을 종료하고 `timed_out`. `CheckResult = {argv, passed, exit_code?, timed_out, duration_ms, finished_ms, output_tail (stdout+stderr 마지막 4 KiB), error?}`. `passed`는 종료 코드 0이고 시간 초과가 아닐 때뿐이다. 결과는 멤버 기록에 저장되어 `show`/`compare`에 나온다. **실패한 check는 실패로 보고한다**(`all_passed: false`, CLI 종료 코드 1). argv[0]은 `.exe`로 풀리므로 `.cmd`/셸 명령은 `["cmd", "/c", "..."]`이나 `["powershell", "-Command", "..."]`로 준다.
- `group.pick`: 승자를 기록한다. `remove_others`면 다른 멤버를 08의 **`task.remove` 규칙 그대로** 지운다. 커밋 안 된 변경이나 unpushed 커밋이 있으면 그 멤버는 지우지 않고 `kept`에 `TASK_DIRTY`로 남긴다(아무것도 바뀌지 않음). `force`/`delete_branch`는 그대로 전달한다.

### 3.3 게이트 (P5)

| 메서드 | params | result |
|---|---|---|
| `gate.create` | `{task?, group?, terminal?, title?, message, expires_in_s?}` | `GateInfo` |
| `gate.resolve` | `{id, decision: approve\|reject, note?}` | `GateInfo` |
| `gate.list` | `{task?, group?, state?: pending\|approved\|rejected\|expired}` | `{gates}` |
| `gate.show` | `{id}` | `GateInfo` |
| `gate.wait` | `{id, timeout_ms?}` | `{outcome: matched\|timed_out, gate, waited_ms}` |

- `GateInfo = {id, title, message, task?, group?, terminal?, state: pending|approved|rejected|expired, created_ms, resolved_ms?, note?, expires_ms?}`.
- **만료**(v6, podd-hardening): `expires_in_s`(1초–1년)를 주면 그 시간 안에 아무도 결정하지 않을 때 데몬의 만료 작업자가 `expired`로 해결한다(`resolved_ms`, `note = "expired after N s without a decision"`, 저장, `gate.changed` 발행). 데몬이 꺼져 있는 동안 지난 만료는 다음 시작 직후 처리한다. 만료된 게이트를 해결하면 `GATE_RESOLVED`. `gate.wait`는 `matched`(gate.state = expired), `pod wait --gate`는 종료 코드 1. 기본은 여전히 만료 없음. `title` 기본값은 "Approval needed". `terminal`이 있고 `task`가 없으면 그 터미널이 속한 Task가 `task`가 된다. 모르는 터미널·Task·그룹은 각각 `TERMINAL_NOT_FOUND`·`TASK_NOT_FOUND`·`GROUP_NOT_FOUND`.
- 이미 해결된 게이트를 다시 해결하면 `GATE_RESOLVED`(`data` = 현재 게이트).
- **대기와 데몬 재시작(e2e-fix1, docs/12 B6)**: `gate.wait`는 연결 하나에서 막히는 RPC다. 데몬이 재시작되면 `pod gate request --wait`/`pod wait --gate`가 "daemon closed the connection"으로 끝났다. 이제 CLI가 같은 파이프에 백오프로 다시 붙어 **같은 저장된 게이트**를 남은 시간으로 다시 기다린다(데몬은 직접 띄우지 않는다. `POD_RECONNECT_TIMEOUT` 초, 기본 60, 안에 돌아오지 않으면 게이트 id를 담은 오류). 테스트: `tests/orchestration.rs::waits_survive_a_daemon_restart`(podd를 죽이고 2 s 뒤 재시작 → 승인 → 대기 중이던 `pod wait --gate`가 0으로 끝나고 stderr에 reconnected, `pod wait --status done`도 이어짐, 데몬이 돌아오지 않으면 1.5 s 창 뒤 오류).
- **알림**: 만들 때와 해결할 때 상태 허브에 `gate.changed {seq, gate}`를 발행한다. `seq`는 `status.changed`와 같은 데몬 전체 순번이다. `status.subscribe` 응답에 `gates`(현재 pending 게이트)가 붙고, 이후 변화는 같은 스트림의 `gate.changed`로 온다. `ids` 필터를 준 구독자는 그 터미널에서 요청한 게이트만 받는다. 스냅샷과 첫 이벤트 사이에 같은 게이트가 중복될 수 있지만 이벤트가 전체 상태를 담으므로 덮어쓰면 된다.
- Rust 클라이언트: `StatusStream::next_event()`는 예전처럼 상태 이벤트만 돌려준다(게이트는 건너뜀). 게이트까지 받으려면 새 `next_any() -> SubscribeEvent::{Status, Gate}`를 쓴다.
- (P5 UI, 추가형·v6 유지) 리뷰 전달이 기록되거나 상태가 바뀔 때마다(`queued`/`delivered`/`cancelled`/`failed`) 같은 스트림에 `review.delivery {seq, delivery: DeliveryInfo}`가 온다(`ids` 필터는 전달의 터미널 기준). Rust: `SubscribeEvent::Delivery`. pod-app Review 패널은 이것으로 `review.deliveries` 폴링을 없앴다([09 §8.3–8.4](09-pod-app.md)).
- `TaskInfo.gates`(그 Task의 게이트), `GroupInfo.gates`에도 나온다.

### 3.4 새 오류 코드

| 코드 | 이름 | 의미 |
|---|---|---|
| -32014 | `AGENT_BLOCKED` | `review.send`: 에이전트가 사용자 응답을 기다림. `data = {state, msg}` |
| -32015 | `REVIEW_NOT_FOUND` | 없는 코멘트·전달, 또는 다른 Task의 코멘트 |
| -32016 | `GROUP_NOT_FOUND` | 없는 그룹, 또는 이름이 모호함 |
| -32017 | `GROUP_EXISTS` | 같은 그룹이나 멤버 Task가 이미 있음 |
| -32018 | `GATE_NOT_FOUND` | 없는 게이트 |
| -32019 | `GATE_RESOLVED` | 이미 해결된 게이트 |
| -32020 | `INVALID_STATE` | 지금 상태에서 할 수 없음(보낼 코멘트 없음, 해결된 코멘트 전송, 큐에 없는 전달 취소, check 미설정) |

### 3.5 기존 타입에 더한 필드 (모두 선택)

- `TaskInfo.review?: {open, queued, sent, resolved}` (코멘트가 있을 때만), `TaskInfo.gates: GateInfo[]`, `TaskInfo.group?: string`.
- `StatusSubscribeResult.gates: GateInfo[]`.

## 4. CLI

```
pod review add <task> <file> <line> [--end-line N] [--side old|new] --body TEXT
pod review add <task> --from-json comments.json      # 리뷰 UI 내보내기({comments:[...]}) 또는 배열
pod review ls [<task>] [--state open|queued|sent|resolved]
pod review send <task> [<id>...] [--all] [--instructions TEXT] [--terminal T] [--dry-run]
                                                     # id가 없으면 open 전부. --dry-run은 프롬프트 출력
pod review resolve <id>... [--note TEXT] [--reopen]
pod review rm <id>...        pod review deliveries [<task>]        pod review cancel <dN>

pod group new <repo> <name> --prompt TEXT --agent claude|codex|shell|'["prog","arg"]' ... [--base REF] [-- CHECK ARGV]
pod group ls [--repo R]      pod group show <group>      pod group compare <group>
pod group check <group> [--timeout S] [-- CHECK ARGV]    # 하나라도 실패하면 종료 코드 1, "CHECK FAILED" (작업이 끝날 때까지 기다림)
pod group check <group> --no-wait                       # v6: 작업 id만 출력, 결과는 group show
pod group check <group> --cancel                        # v6: 실행 중인 check 취소(종료 코드 1)
pod group pick <group> <task> [--remove-others] [--force] [--delete-branch]   # 남은 멤버가 있으면 종료 코드 1
pod group rm <group>

pod gate request "<message>" [--title T] [--task T | --group G] [--expires S] [--wait [--timeout S]]
pod gate ls [--task T] [--group G] [--pending]
pod gate show <id>           pod gate approve <id> [--note]        pod gate reject <id> [--note]
pod wait --gate <id> [--timeout S]                       # 0 = approved, 1 = rejected 또는 시간 초과
```

- 모두 `--json`을 받는다(결과 구조체 그대로).
- `pod gate request`는 Pod 터미널 안에서(`POD_SOCKET`이 같은 데몬일 때) `POD_TERMINAL_ID`와 `POD_TASK_ID`를 붙인다. 터미널 밖에서 `--task`/`--group`이 없으면 오류(종료 코드 2). 에이전트는 `pod gate request "배포해도 될까요?" --wait`로 사람의 결정을 기다리고 종료 코드로 분기할 수 있다.
- `pod group new`도 Pod 터미널 안이면 호출한 터미널을 멤버의 부모로 기록한다(08 §4.1과 같음).

## 5. 검증

`cargo test --workspace`, `cargo clippy -p podd -p pod-cli --all-targets`(경고 0), `cargo deny check` 통과. 테스트는 번들 ConPTY(`vendor/conpty` 1.25.260930003)로 돌렸다. 실제 Claude/Codex에는 프롬프트를 보내지 않았다(사용량).

**통합 테스트** `crates/podd/tests/orchestration.rs`(실제 podd·pod-holder·pod, 저장소와 worktree는 temp 아래 공백 포함 경로):

1. `review_comments_persist_queue_and_deliver_once` — 에이전트는 `fake-agent`: OSC `working` → 파일 신호를 기다림 → `paste-loop`(raw VT 입력, `ESC[?2004h`, 받은 제출을 로그 파일에 기록하고 working → idle 턴을 한 번 돎).
   - 코멘트 3개(new 범위, `path` 별칭의 old 쪽, `code` 지정) → 인용 캡처 확인(new는 수정된 worktree, old는 base). `../` 경로는 `INVALID_PARAMS`.
   - **데몬 재시작**(터미널 유지) 뒤 코멘트가 그대로(id·내용·상태 동일), `TaskInfo.review.open = 3`, 에이전트는 working(OSC 재생).
   - `review.send all_open + 여러 줄 instructions` → `queued`(agent_state working), 코멘트 `queued`. 1.5 s 동안 아무것도 전달되지 않음.
   - 신호 파일 → 에이전트 idle → 작업자가 전달(`delivered_state = idle`, `app_bracketed_paste = true`). 에이전트가 받은 텍스트가 `prompt`와 **바이트 단위로 같고**(여러 줄, 비 ASCII `—` 포함), bracketed paste 1번, 이후 턴이 끝나고 2 s 더 지나도 기록은 **정확히 1건**. 코멘트 `sent`, `delivery = d1`.
   - idle 상태에서 새 코멘트를 보내면 즉시 `delivered`, 두 번째 기록이 생김.
   - 해결한 코멘트를 보내면 `INVALID_STATE`.
   - OSC `blocked:question` 에이전트 → `AGENT_BLOCKED`(메시지에 `blocked:question`), 코멘트는 `open` 그대로. `pod review send`는 종료 코드 2. `pod review ls --json` 확인.
2. `group_fan_out_compare_check_pick` — shell 에이전트 3개, 공통 프롬프트(각자 `result.txt`에 자기 Task id를 쓰고, `-2` 멤버만 `extra` 줄 추가).
   - 멤버 이름 `fx-shell-1..3`, 모두 같은 `base_commit`(= 저장소 HEAD), `base = main`, `prompt` 기록, `TaskInfo.group = demo/fx`. 같은 그룹 재생성은 `GROUP_EXISTS`.
   - `group.compare`: 멤버마다 파일 1개(`result.txt`, `?`), +1 / +2 / +1, dirty 1.
   - check `cmd /c findstr /c:extra result.txt` → `[실패, 통과, 실패]`, `all_passed = false`, 실패 멤버의 `exit_code = 1`. compare에 결과 반영. `pod group check`는 종료 코드 1과 "CHECK FAILED", `pod group compare --json` 파싱.
   - `group.pick remove_others`(force 없음) → 다른 둘은 `TASK_DIRTY`로 `kept`, 아무것도 지워지지 않음. `force + delete_branch` → 둘의 터미널·worktree·브랜치 삭제, 승자는 그대로. `group.show`에 `removed`/`picked` 반영.
3. `gate_requested_in_terminal_subscribe_and_wait` — PowerShell Task 안에서 `pod gate request 'Deploy to staging?' --title Deploy`.
   - 구독자(`next_any`)가 `gate.changed`(pending, task = `demo/gt`, terminal = 그 터미널)를 받음. `task.show.gates`에 있음. 새 구독자의 스냅샷 `gates`에 있음.
   - 밖에서 띄운 `pod wait --gate g1 --timeout 60`은 1.5 s 동안 끝나지 않다가 `pod gate approve g1 --note "go ahead"` 뒤 종료 코드 0, "g1 approved". 해결 이벤트 수신. 두 번 해결하면 `GATE_RESOLVED`.
   - 터미널 안 `pod gate request ... --wait` → API로 reject → 터미널의 `$LASTEXITCODE`가 1.
   - 터미널 밖에서 `--task` 없이 `pod gate request` → 종료 코드 2.

**단위 테스트**: store 저장/읽기·id·깨진 파일 격리, 경로 정규화, 줄 추출(CRLF, 범위 초과, 40줄 제한), 제어 문자 제거, 프롬프트 모양(긴 펜스, 번호, old 표시)과 paste 바이트, 멤버 이름 규칙, check 실행기(성공, 종료 코드 3, 시간 초과 시 Job 종료, 없는 프로그램, 출력 tail), CLI 파싱(`pod wait --gate`와 터미널 인자 충돌 포함), 스키마에 새 메서드 모두 포함.

### 3.6 v9 수명 이벤트와의 관계 (fu-daemon, 2026-11-09)

프로토콜은 이제 **v9**다([03 §4.3](03-podd-p0.md)). 리뷰·그룹·게이트 알림은 그대로이고, `status.subscribe`에 `lifecycle: true`를 주면 같은 스트림·같은 `seq`로 `terminal.changed`, `task.changed`(created/removed/status/git/terminals/touched), `task.removing`이 더 온다. 리뷰 쪽에 쓸모 있는 것: `task.changed git`/`touched`가 오면 리뷰 패널이 `task.diff`를 다시 부른다(편집기 저장은 `task.touch`로 알림). `group.pick --remove-others`나 `group.create` 롤백이 멤버 태스크를 지우면 멤버마다 `task.removing` → `task.changed removed`가 나간다(같은 `task.remove` 경로). 새 멤버는 `task.changed created`로 온다.

## 6. 한계와 남은 일

1. ~~**실제 에이전트로는 검증하지 않았다.**~~ **실제 Codex로 검증(e2e-fix1, docs/12 B1)**: 250 ms 고정 Enter는 실제 Codex에서 제출되지 않았다(docs/12). 고친 뒤 실제 Codex 0.159에서 큐 전달 1회·즉시 전달 1회가 모두 Enter 1번에 `confirmed`였다. 회귀 테스트는 Codex의 붙여넣기 감지를 흉내 내는 `fake-agent burst-loop`(콘솔 키 이벤트, 키당 0.3 ms 지연): `tests/orchestration.rs::review_delivery_submits_to_a_lagging_paste_burst_tui`, `tests/submit.rs`(send 제출, 지연 TUI, 놓친 Enter 재시도, 끝내 무반응 → unconfirmed, 셸 → unverifiable). Claude Code에는 여전히 보내지 않았다. 아래는 원래 기록이다. 전달 방식(bracketed paste + 250 ms 뒤 Enter)은 fake-agent로만 확인했다. Claude Code(Ink)가 paste 안의 `\r`을 줄바꿈으로, Codex(Windows, crossterm)가 ConPTY를 거친 붙여넣기를 한 메시지로 받는지는 사용량 제약 때문에 보지 않았다. 확인 방법: 신뢰된 scratch 폴더에서 `pod review send --dry-run`으로 짧은 2줄 프롬프트를 만들어 한 번씩 보내 본다. Codex가 붙여넣기를 줄마다 제출하면 Codex 쪽만 마커 없이 보내거나 Enter 지연을 늘리는 옵션이 필요하다.
2. ~~셸처럼 turn을 보고하지 않는 터미널에는 두 번째 전달이 최대 30 s 늦게 나갈 수 있다.~~ 고침(podd-hardening, §3.1): 상태 변화나 1.5 s 무출력이면 보낸다. 테스트 `review_queue_is_prompt_for_terminals_without_turns`(OSC를 보내지 않는 `paste-loop-silent` 에이전트: 큐에 든 두 번째 프롬프트가 10 s 안에, 각 1회, 순서대로). 남은 한계: 출력 없이 오래 생각하는 휴리스틱 에이전트(스피너도 타이틀도 없음)에는 1.5 s 뒤 다음 프롬프트가 들어갈 수 있다. 그 경우 에이전트 쪽 입력 큐에 쌓인다.
3. 큐에 있는 동안 에이전트가 blocked가 되면 계속 기다린다(사용자가 답하면 이어서 전달). `review.cancel`로 취소한다. ~~큐 타임아웃은 없다.~~ **고침(podd-gaps2, §3.1-5)**: 기본 30분 뒤 만료(`failed`, `expired: …`, 이벤트, 코멘트 다시 `open`). 테스트: `tests/orchestration.rs::queued_review_expires_with_an_event`(`--review-queue-timeout 2s`, working 상태 fake-agent: 1 s 뒤엔 아직 `queued`, 2–10 s 사이에 `failed` 이벤트, 오류 문구, 코멘트 `open`), `review::tests::queue_expiry`. 남은 한계: 만료 뒤 자동 재전송은 없다(사람이 다시 보냄).
4. ~~`group.check`는 요청 동안 블록된다.~~ 고침(v6, §3.2): 백그라운드 작업 + `group.check` 알림 + `group.check_cancel`. 테스트: `group_fan_out_compare_check_pick`(120회 ping check가 5 s 안에 `running`으로 돌아옴, 같은 연결의 `group.show`가 동작, 두 번째 시작 거절, 시작·취소 이벤트, 취소 후 모든 결과 `cancelled`, CLI `--no-wait`/`--cancel`). 결과는 여전히 마지막 1회만 저장한다. 진행 중 출력은 스트리밍하지 않는다(끝난 멤버의 마지막 4 KiB만).
5. `group.compare`는 순위를 매기지 않는다. 멤버마다 git을 여러 번 실행하므로 멤버가 많으면 느리다(테스트 기기에서 3명 기준 수 초).
6. 그룹 생성은 멤버를 순서대로 만든다(멤버당 worktree + 터미널 시작 약 0.5–5 s). claude/codex의 `.cmd` shim 프롬프트 제한(08 §4.1)이 그대로 적용되어, `" % 줄바꿈`이 든 프롬프트는 그룹 생성이 롤백된다. 이 경우 shim을 우회하거나, 에이전트를 프롬프트 없이 띄운 뒤 `review.send`와 같은 paste 경로로 첫 프롬프트를 보내는 방식으로 바꿀 수 있다.
7. 게이트는 기본으로 사람이 해결할 때까지 pending이다. v6부터 `expires_in_s`로 만료를 줄 수 있다(§3.3, 테스트 `gates_expire`: 1 s 만료 → 이벤트·`pod wait --gate` 종료 코드 1·늦은 해결 거절, 데몬이 꺼진 동안의 만료는 재시작 직후 처리). 게이트를 요청한 에이전트의 상태(blocked 등)는 바꾸지 않는다. UI가 `gate.changed`로 알림을 띄우는 것을 전제로 한다.
8. 코멘트 인용은 추가 시점의 내용이다. 이후 에이전트가 줄을 옮기면 `line`은 옛 위치를 가리킨다(프롬프트에 인용 코드가 있으므로 에이전트가 찾을 수 있다). 코멘트 위치 추적(diff anchoring)은 UI 단계에서 다룬다.

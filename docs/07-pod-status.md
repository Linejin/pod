# 07. `pod-status` 상태 엔진과 `pod-hook` (P1, v0.1)

작성: 2026-11-08 · 브랜치 `SJin-Choi/pod-status` · 크레이트 `crates/pod-status`(라이브러리), `crates/pod-hook`(바이너리)
선행 문서: [04 상태 감지 리서치](04-status-detection-research.md) §1.2–§1.4, §3, §4 · [01 §4.2(2), §8](01-plan.md)

이 문서는 podd가 호출할 API, 상태 머신, 어댑터, 그리고 아직 검증하지 못한 항목을 정리한다. 구현은 docs/04에 적힌 동작을 보고 새로 작성했다(클린룸). 다른 제품의 코드는 복사하지 않았다. 라이선스는 Apache-2.0이고, 의존성은 `serde`, `serde_json`, `base64`(이상 MIT/Apache)와 테스트 전용 `tempfile`, `windows-sys`(MIT/Apache)뿐이다.

---

## 1. 설계 원칙

- **순수·동기 라이브러리.** PTY, 스레드, IPC, 시계가 없다. podd가 터미널마다 `TerminalStatus`를 하나 두고, 출력 바이트·타이틀·화면 텍스트·포그라운드 프로세스·훅 메시지를 넣는다. 시간은 모든 호출에 `now: Millis`(podd가 정한 단조 시계의 ms)로 넘긴다. 그래서 테스트가 결정적이다.
- **바이트 스트림을 바꾸지 않는다.** OSC 7501 파서는 관찰만 한다. podd는 원시 바이트를 그대로 클라이언트에 넘기고, 질의(`ESC]7501;?`)가 보이면 `QUERY_REPLY`를 PTY 입력에 써 준다.
- **증거 우선순위** `hook(4) > osc(3) > mod(2) > heuristic(1) > process(0)` (docs/04 §4.1). `mod`는 확장의 상태 감지기·힌트다([16 §7.1](16-extensions.md), P6).

## 2. podd가 호출할 API

```rust
use pod_status::*;

let mut st = TerminalStatus::new(now);                  // 터미널마다 하나
// 또는 TerminalStatus::with_config(StatusConfig { hook_stale_ms: 120_000, quiet_idle_ms: 3_000 }, now)

// 1) PTY 출력 (청크 경계 무관)
let r: OutputResult = st.feed_output(&bytes, now);
for _ in 0..r.queries { pty.write_input(QUERY_REPLY); } // ESC]7501;?:states=…:kinds=… ESC\
if let Some(t) = r.transition { /* 이벤트 발행: t.from → t.to, t.turn, t.source */ }

// 2) VT 계층이 해석한 타이틀(OSC 0/2) 변경
st.on_title(&title, now);
// 3) 보이는 화면 텍스트(마지막 N줄 정도, podd가 쓰로틀)
st.on_screen(&visible_text, now);
// 4) 포그라운드 프로세스 변경 / 종료
st.on_foreground(Some(&ProcessInfo { pid, name, argv }), now);  // shell로 돌아오면 exited
st.on_process_exit(now);
// 5) pod-hook이 named pipe로 보낸 한 줄
let msg = HookMessage::from_line(&line)?;   // msg.terminal_id 로 라우팅, msg.token 검증은 podd 몫
st.on_hook(&msg, now);
// 6) 사용자가 패널을 봄 → done 확인
st.ack(now);
// 7) 주기 타이머(≈1s): 무출력 idle, hook staleness
st.tick(now);

// 읽기
let snap: Snapshot = st.snapshot();      // serde 직렬화 가능
snap.state; snap.source; snap.confidence; snap.since; snap.turn;
snap.display_state();                    // 자식 blocked 반영
snap.badge();                            // "done (+2 running)", "blocked:permission"
aggregate_snapshots(&snaps);             // 패널/탭/태스크 집계
```

모든 `on_*`/`tick`/`ack`는 루트 상태가 바뀌었을 때 `Some(Transition { from, to, source, turn, at })`를 돌려준다. `feed_output`은 `OutputResult { queries, transition, changed, invalid }`를 돌려준다. `changed`는 레코드·msg·progress 변화도 포함한다.

기타 공개 API:

| 모듈 | 항목 |
|---|---|
| `osc` | `Osc7501Parser::{new, feed, feed_with, is_mid_sequence}`, `parse_payload`, `encode_report`, `QUERY_REPLY`/`query_reply()`, 한계 상수(`MAX_PAYLOAD=4087` 등), `Report`, `ReportState`, `ReportKind`, `OscEvent::{Query, Report, Invalid}` |
| `types` | `AgentState`, `BlockedKind`, `Source`, `AgentKind`, `Outcome`, `aggregate()` |
| `heuristic` | `classify_title`, `match_screen`, `detect_agent`, `HeuristicTracker`, 데이터 표 `TITLE_RULES`, `SCREEN_PATTERNS`, `AGENT_EXES`, `WRAPPER_EXES`, `AGENT_PACKAGES` |
| `hook` | `HookMessage`(와이어 v1), `HookAgent`, `map_hook`/`map_message` → `HookEffect`, `CLAUDE_EVENTS`, `CODEX_EVENTS`, `PAYLOAD_FIELDS`(검증 여부 표), env 이름 상수 |
| `install` | `InstallScope::{Global, Profile}`, `HookCommandSpec`, `claude_hooks_block`, `codex_hooks_block`, `merge_hooks`, `remove_pod_hooks`, `plan_install`/`plan_uninstall` → `InstallPlan`, `apply_plan(plan, ApplyOptions)` |

## 3. 상태 머신

상태: `unknown`, `idle`, `working`, `blocked{permission|question|auth|other}`, `done`, `error`, `exited`. JSON으로는 `{"state":"blocked","kind":"permission"}` 형태다.

```
             launch / 첫 출력
 unknown ─────────────────────────┐
   │ osc idle / hook SessionStart  │ heuristic(스피너)
   ▼                               ▼
  idle ──── UserPromptSubmit / osc working ───▶ working ◀──┐
   ▲                                             │  │  │   │ PostToolUse / ElicitationResult / osc working
   │ ack (사용자가 패널을 봄)                     │  │  └─▶ blocked{kind} ──┘
  done ◀──── Stop / osc done ────────────────────┘  │        │ Esc / Interrupt / osc idle
   │ 새 프롬프트 → working       StopFailure / osc error    ▼
   ▼                                                ▼      idle
 (idle)                                           error ── 새 프롬프트 ──▶ working
 어느 상태든: 프로세스 종료 → exited(final_state 보존) · osc state=clear → 낮은 출처로 강등
 working + interrupt(Stop is_interrupt, Codex Interrupt) → idle (outcome=cancelled)
```

### 3.1 출처 결합 규칙 (`resolve`)

1. 출처별 증거 슬롯(hook, osc, heuristic)마다 `state`와 `at`(마지막 확인 시각)을 둔다. process는 상태를 만들지 않고 에이전트 식별과 `exited`만 담당한다.
2. 우선순위 순서로 보고 **신선한** 첫 증거를 고른다. 신선도: hook은 `hook_stale_ms`(기본 120 s) 동안, osc는 프로세스 종료 전까지 무기한, heuristic은 무기한이다. 예외(podd-gaps2): hook `blocked`는 그 뒤에 root OSC 상태가 **바뀌어** `idle`/`done`/`error`(턴 끝)가 되면 곧바로 stale로 본다. Esc로 권한·질문 프롬프트를 취소하면 Claude는 훅을 보내지 않지만 OSC 보고는 idle로 바뀐다. 같은 상태의 재보고와 OSC `working`은 풀지 않는다(보고가 늦게 올 수 있음). "그 뒤"는 시각이 아니라 이벤트 순서로 판단한다(데몬 재시작 때 podd가 OSC를 먼저 재생하고 저장된 훅을 원래 시각으로 다시 넣기 때문에, 시각으로 비교하면 복원된 훅 blocked가 바로 풀린다). OSC 보고가 없는 에이전트에는 120 s 규칙이 그대로 대비책이다(Codex는 `Interrupt` 훅으로 이미 풀린다).
3. 높은 출처가 stale이면, **그보다 나중에 갱신된** 낮은 출처만 상태를 정할 수 있다. 그런 증거가 없으면 stale 상태를 유지하고 confidence를 절반으로 낮춘다.
4. 낮은 출처의 `idle`은 stale한 높은 출처의 `done`을 덮지 못한다. `done → idle`은 `ack()`로만 일어난다(사용자가 결과를 보지 않았는데 확인 처리되는 것을 막기 위함).
5. confidence: hook 0.95, osc 0.9, 화면 패턴 0.7, 타이틀 0.6, 응답 추론 0.6, 무출력 idle 0.4, process(상태 unknown) 0.2.
6. **응답 추론(e2e-fix1, docs/12 B2)**: 권한 요청을 승인했을 때 오는 훅은 없다. Codex와 Claude Code 모두 승인된 도구가 **끝난 뒤** `PostToolUse`를 보낸다(Claude Code의 훅 목록에도 '승인됨' 이벤트가 없다. 문서 기준이며 실제 Claude 턴으로는 확인하지 않았다). 그래서 승인한 명령이 오래 돌면 그동안 `blocked:permission`이 남았다. 이제 hook이 `blocked:permission`/`blocked:question`이고 그것이 현재 상태일 때 터미널에 **응답 키**(`is_answer_input`: Enter, 또는 한 글자 선택 키 `y a n p 1–9`)가 입력되고 그 뒤 3 s 안에 출력이 오면, 프롬프트가 답해졌다고 보고 hook 슬롯을 blocked 직전 상태(보통 `working`)로 바꾼다. 출처는 `heuristic`, confidence 0.6. 다음 실제 훅(PostToolUse, Stop, Interrupt…)이 다시 정한다. 응답이 아닌 입력: 단독 Esc(취소는 에이전트가 Interrupt/Stop 훅이나 OSC로 알린다. Esc 동작은 그대로), 화살표 등 이스케이프 시퀀스, 포커스 보고, 터미널 질의 응답(OSC 10/11 등), 붙여넣기, 긴 텍스트. 거절(`n`)도 응답으로 친다. Codex는 거절 뒤 턴을 계속하거나 Interrupt로 끝낸다. 한계: 프롬프트가 떠 있는 동안 주기적으로 다시 그리는 TUI에 `y`/Enter를 의미 없이 치면 잠깐 working이 될 수 있다(다음 훅이나 120 s staleness가 바로잡는다). 입력은 podd를 거치는 것(attach 입력, `pod keys`, `pod send`)만 보인다.

### 3.2 그 밖의 의미

- `turn`: 루트가 `working`을 떠날 때마다 1 증가한다. `exited`로 바뀌는 경우도 포함한다. 재시작해도 유지된다. 예외: 세션 시작 훅(`SessionStart`)이 일으킨 전이는 세지 않는다. 에이전트가 시작하며 바빠 보이다가(Codex의 MCP 서버 로딩) 훅이 idle을 알리면, 프롬프트가 돌기도 전에 턴이 끝난 것으로 셌다(docs/19 §17).
- `since`: 루트 상태(blocked는 kind 포함)가 바뀐 시각이다.
- `ack`: 현재 `done`일 때만 동작한다. done 증거를 idle로 바꾼다.
- `exited`: `on_process_exit` 또는 포그라운드가 에이전트에서 셸로 돌아오면 일어난다. 최종 상태는 `final_state`에 남는다. hook·osc·heuristic 증거는 버리고, 레코드는 보존한다. 다음 실행(에이전트 포그라운드 재등장, 새 OSC 보고, hook `SessionStart`)에서 초기화된다.
- `osc state=clear`(id 없음): osc 증거를 지우고 낮은 출처로 강등한다. `clear:id=X`는 레코드 X를 지운다.
- 하위 레코드: OSC `id=` 레코드와 hook 서브에이전트(`hook/<agent_id>`)다. 루트 상태에는 영향을 주지 않는다. `running_children`이 있으면 배지가 `done (+N running)`이 된다(held-open). 자식이 blocked면 `display_state()`가 blocked가 된다.
- `outcome`: hook에서만 알 수 있다. `completed`(Stop), `cancelled`(Stop is_interrupt, Codex Interrupt), `failed`(StopFailure). 새 턴(working)에서 지운다.
- 집계 우선순위: `blocked > error > done > working > idle > unknown > exited`(`AgentState::priority`).

## 4. 어댑터

### 4.1 L2 OSC 7501 파서 (docs/04 §1.2, §4.4)

- 상태 기계 `Ground → ESC → ']' → "7501;" 접두 매칭 → Payload → (BEL | ESC \)`. 바이트 하나씩 처리하므로 시퀀스가 청크 사이에서 쪼개져도 된다. 다른 OSC(타이틀 등)는 무시한다. 중간의 `ESC`(뒤에 `\`가 아님)는 시퀀스를 끊고 새 시퀀스의 시작으로 다시 처리한다. CAN/SUB도 끊는다.
- 검증: 출력 가능한 ASCII만 허용, key `[a-z]{1,16}`, value `[A-Za-z0-9_.,+/=-]*`. 필드 하나라도 잘못되면 레코드 전체를 버린다(`OscEvent::Invalid`). 알 수 없는 키는 무시한다(`x…` Pod 확장 포함). `state`는 필수다. `app` ≤32 토큰. `id`는 `/` 세그먼트 ≤8개, 각 ≤32자, 전체 ≤128. `kind`는 permission/question/auth만 허용하고 blocked가 아니면 버린다. `progress`는 `0*\d{1,3}`이고 ≤100이며 working/blocked에서만 유지한다. `title`은 base64 인코딩 ≤256, 디코딩 ≤192. `msg`는 인코딩 ≤2732, 디코딩 ≤2048. 둘 다 UTF-8이어야 하고 제어문자가 없어야 하며 패딩은 있어도 없어도 된다. 페이로드가 4087바이트를 넘으면 `TooLong`.
- 같은 키가 두 번 나오면 마지막 값을 쓴다(규격에 없는 부분, 임의로 정함).
- `?:states=…` 형태의 응답이 출력에 보이면(중첩 터미널) 질의로도 보고로도 처리하지 않는다.

### 4.2 L1 hook 매핑 (docs/04 §3.3, §4.3)

| Claude 이벤트 | Pod 효과 |
|---|---|
| SessionStart (`source`≠compact) | idle + 세션 경계(hook 자식 정리) · compact면 무시 |
| UserPromptSubmit | working |
| PreToolUse | `tool_name`=AskUserQuestion → blocked{question}, 아니면 working |
| PostToolUse, PostToolUseFailure | working (권한 승인 후 복귀) |
| PermissionRequest | blocked{permission}, msg=`tool_name` |
| Elicitation | blocked{question}, msg=`message` |
| ElicitationResult | 대기 전 상태로 복원 |
| Stop | done(completed) · `is_interrupt`=true면 idle(cancelled) |
| StopFailure | error(failed), msg=`error` |
| SubagentStart / SubagentStop | 자식 레코드 `hook/<agent_id>` 추가/삭제 |
| SessionEnd | hook 증거와 hook 자식 제거(상태는 낮은 출처나 process 종료가 정함) |
| `agent_id`가 있는 Pre/PostToolUse | 리드가 아니라 자식 레코드만 갱신 |

| Codex 이벤트 | Pod 효과 |
|---|---|
| SessionStart | idle + 세션 경계 |
| UserPromptSubmit, PostToolUse | working |
| PreToolUse | `tool_name`=request_user_input/AskUserQuestion → blocked{question}, 아니면 working |
| PermissionRequest | blocked{permission} |
| Stop | done(completed) |
| Interrupt | idle(cancelled) |
| SubagentStart / SubagentStop | 자식 레코드 |

훅 증거는 어떤 이벤트가 오든(효과가 없는 이벤트 포함) 신선도가 갱신된다.

### 4.3 L3 휴리스틱 (데이터 표, `heuristic.rs`)

- **타이틀**(`TITLE_RULES`, 첫 글자 기준): Claude `✳`(U+2733) = idle 표시. Braille U+2800–U+28FF = working(Claude·Codex). 쿼터서클 ◐◑◒◓ = working(미관찰). Codex는 "스피너 없음"이 idle 신호이고(`NO_GLYPH_IDLE_AGENTS`), 출력이 `quiet_idle_ms`(기본 3 s) 동안 없을 때 `tick`에서 확정한다. working을 거친 뒤의 idle 신호는 `done`, 처음부터 idle이면 `idle`이다. 에이전트를 모르면 타이틀을 해석하지 않는다(셸 프로그램의 스피너 오인 방지). 단 `✳ … Claude`는 Claude로 식별한다.
- **화면 패턴**(`SCREEN_PATTERNS`, 소문자 부분 문자열): Claude 신뢰 대화상자 `is this a project you trust`(관찰됨), `quick safety check`, `trust this folder`, 권한 프롬프트 `do you want to proceed?` 등. Codex `trust this folder?`, `would you like to run the following command` 등. 일치하는 동안 blocked{kind}(confidence 0.7)이고, 화면에서 사라지면 해제한다. OSC 7501이 오면 OSC가 우선한다.
- **process**(`detect_agent`): `claude.exe`/`codex.exe`, npm shim `cmd.exe /c claude.CMD`, `node …/@anthropic-ai/claude-code/cli.js`, `node …/@openai/codex/…`. 셸·기타는 `None`이다. 에이전트가 포그라운드였다가 셸로 돌아오면 `exited`가 된다.

## 5. `pod-hook` 바이너리

```
pod-hook claude|codex <Event>      # 에이전트 훅이 실행, stdin = 훅 JSON
pod-hook install --dry-run [--agent claude|codex|all] [--home DIR | --claude-config-dir DIR --codex-home DIR]
                 [--exe PATH] [--uninstall] [--block-only]
```

- **항상** stdout에 `{}`를 먼저 출력하고 exit 0으로 끝난다. panic도 잡는다. 에이전트의 결정에는 개입하지 않는다.
- `POD_TERMINAL_ID` 또는 `POD_SOCKET`이 없으면 stdin을 읽지 않고 바로 끝난다(Pod 밖에서 무해, 측정 수 ms). 이것이 env 가드다. 전역에 설치해도 Pod 밖에서는 아무 일도 하지 않는다.
- env가 있으면 stdin을 최대 0.5 s, 4 MiB까지 읽는다(JSON이 아니면 `payload: null`, 4 MiB를 넘으면 `payload: {"pod_truncated": {...}}`). 보내는 한 줄 전체는 podd의 줄 한도(4 MiB)를 넘지 않는다: 넘으면 payload에서 작은 스칼라 필드(`session_id`, `hook_event_name`, `tool_name` 등, 각 4 KiB 이하)만 남기고 `pod_truncated: {payload_bytes}` 표시를 붙인다. 이벤트 자체는 버리지 않는다(daemon-05). 파이프는 `SECURITY_IDENTIFICATION` SQOS로 열어 서버가 hook을 impersonate할 수 없다(security-01). 그다음 `POD_SOCKET`의 named pipe(`\\.\pipe\…`, Unix는 소켓 경로)에 JSON 한 줄을 쓴다. 전체 예산은 약 1 s다. `ERROR_PIPE_BUSY`면 예산 안에서 재시도한다. 예산이 끝나면 `process::exit(0)`로 멈춘 스레드를 버린다.
- 와이어 메시지(`HookMessage`, v1):
  ```json
  {"v":1,"terminal_id":"term-42","token":"…","agent":"claude","event":"PermissionRequest",
   "payload":{…훅 stdin 원문…},"sent_at_unix_ms":1762560000000}
  ```
  `token`은 `POD_HOOK_TOKEN`이 있을 때만 들어간다. 검증은 podd가 한다.
- 창: Windows에서는 **GUI 서브시스템** 실행 파일이다(§8.6). OS가 콘솔을 만들지 않으므로 어떤 방식으로 실행돼도 콘솔 창이 뜨지 않는다. 표준 입출력은 에이전트가 넘긴 파이프로 그대로 동작한다. `install`/`--help`는 stdout이 리디렉션되지 않았으면 부모 콘솔에 붙어 출력한다.

## 6. 훅 설치기 (`install` 모듈)

- `InstallScope::Global { home }` → `<home>/.claude/settings.json`, `<home>/.codex/hooks.json`(전역 + env 가드).
- `InstallScope::Profile { claude_config_dir, codex_home }` → `<dir>/settings.json`, `<codex_home>/hooks.json`. `launch_env()`가 podd가 주입할 `CLAUDE_CONFIG_DIR`, `CODEX_HOME`을 돌려준다.
- 두 방식 중 무엇을 쓸지는 **사용자 결정 사항**이다(docs/04 §6). API는 둘 다 지원한다.
- 병합은 멱등이다. `pod-hook` 문자열이 들어간 그룹만 교체·삭제하고, 다른 도구의 항목과 다른 설정 키는 보존한다. 설치 후 제거하면 원래 JSON으로 돌아간다(테스트로 확인). 기존 파일이 잘못된 JSON이면 오류를 내고 덮어쓰지 않는다.
- Claude 블록: 13개 이벤트, 도구 이벤트(Pre/PostToolUse, PostToolUseFailure, PermissionRequest, Elicitation, ElicitationResult)는 `"matcher": "*"`, `timeout: 10`. 명령은 `"C:/…/pod-hook.exe" claude <Event>`(슬래시로 바꾸고 공백이 있으면 따옴표). Codex 블록: 9개 이벤트, Interrupt만 `timeout: 3`.
- **안전장치**: `apply_plan`은 실제 사용자 홈의 `.claude`/`.codex` 아래 경로를 `ApplyOptions { allow_user_config: true }` 없이는 쓰지 않는다. CLI는 `--dry-run`만 지원하고, 그 밖의 경우 exit 2로 끝난다. 이번 작업에서 실제 사용자 설정은 쓰지 않았다(테스트는 임시 디렉터리만 사용).
- Codex는 새 훅에 대해 신뢰 확인(`config.toml`의 `[hooks.state]`)을 요구할 수 있다. 설치기는 `config.toml`을 건드리지 않는다.

## 7. 검증 상태와 남은 일

**테스트**(`cargo test`): pod-status 59개(OSC 파서 15, 상태 머신 25, 휴리스틱 9, 훅·설치기 10), pod-hook 8개.
- docs/04 §1.3 A–E 원시 샘플을 바이트 그대로 재생한다. 모든 바이트 경계 분할, 바이트 단위, 짧은 샘플은 모든 3분할까지 검사한다.
- §4.2의 모든 화살표 전이, 우선순위·staleness·ack·turn, 하위 레코드·held-open, 집계를 검사한다.
- Claude/Codex 타이틀 샘플, 화면 패턴, process shim을 검사한다.
- 대표 훅 페이로드 매핑, 와이어 왕복, 설치기(임시 디렉터리: 전역/프로필, 멱등, 제거, 사용자 설정 쓰기 거부)를 검사한다.
- pod-hook: env 없음 → `{}`/0 즉시, stdin을 닫지 않아도 즉시. 잘못된 인자. env 있음 + 파이프 없음 → `{}`/0 <2 s. stdin을 닫지 않음 + env → 예산 안에 종료. **실제 named pipe 서버로 한 줄 전달 후 엔진이 blocked{permission}으로 해석**하는 것까지 확인한다. install dry-run이 아무것도 만들지 않는지도 검사한다.
- (podd-hardening) 전체 워크스페이스 테스트 중 한 번 시간 예산을 넘긴 일이 있었다. 원인은 부하 상태의 프로세스 시작 지연으로 보이며 재현되지 않았다. 바이너리: `{}`를 쓰고 flush한 뒤 `std::process::exit` 대신 `TerminateProcess`로 바로 끝낸다(stdin `ReadFile`에 막힌 스레드가 종료 경로를 늦출 수 없게). 테스트: 예산 검사는 느린 시작 한 번을 허용하고(다시 실행해 그 실행이 예산을 지켜야 함, 매번 `{}`/0 확인, 10 s 상한), 12개 동시 실행(절반은 stdin을 닫지 않음)을 추가했다.

**미검증 (실측 필요)**
- Claude 훅 필드 `Stop.is_interrupt`, `StopFailure.error`, `Elicitation.message`, `SessionEnd.reason`. Codex 훅 페이로드 전체(`session_id`, `turn_id`, `agent_id`, `tool_name`, 질문 도구 이름 `request_user_input`). `hook::PAYLOAD_FIELDS`에 `verified=false`로 표시했다.
- Claude 권한 프롬프트·Codex 승인 프롬프트의 화면 문구, Codex 신뢰 대화상자 정확한 문구, 쿼터서클 스피너. 표에 `verified=false`로 표시했다.
- Claude가 OSC 7501을 보낼 때 타이틀 스피너가 Braille인지, Codex 승인 대기 중 타이틀 변화.
- Claude/Codex가 실제로 `pod-hook`을 실행할 때의 동작: 창 깜빡임, 실행 셸(Claude on Windows는 Git Bash 경유 가능성), 따옴표 처리. 실제 설정에 설치하지 않았으므로 확인하지 못했다.
- 훅 Blocked 후 사용자가 Esc로 취소했을 때 Claude가 어떤 훅을 보내는지(실측 안 함). ~~아무 훅도 오지 않으면 OSC가 idle을 보내도 hook blocked가 최대 120 s 남는다.~~ **고침(podd-gaps2, §3.1-2)**: OSC 보고가 턴 끝(idle/done/error)으로 바뀌면 hook blocked가 바로 풀린다. 120 s는 OSC가 없는 경우의 대비책으로만 남는다. 테스트: `crates/podd/tests/status.rs::hooks_authenticated_and_outrank_osc`(실제 pod-hook으로 blocked → 늦은 OSC working으로는 안 풀림 → OSC idle로 바뀌면 바로 idle, 예전에는 이 단계에서 blocked가 남는 것을 확인했다), `turn_and_hook_evidence_survive_daemon_restart`(재시작 뒤에도 복원된 훅 blocked 유지), `tests/state_machine.rs::hook_blocked_clears_on_osc_turn_end_after_interrupt`(OSC blocked 재보고·늦은 working으로는 안 풀림, idle로 바뀌면 그 청크에서 idle, 새 PermissionRequest는 예전 idle 재보고로 안 풀림, done도 풀림, Codex `Interrupt`, OSC가 없으면 60 s 뒤에도 blocked). 남은 한계: Claude가 Esc 뒤 OSC를 보내지 않거나(실측 필요) 상태 보고가 꺼진 inbox ConPTY에서는 여전히 최대 120 s다.
- Codex 실패 턴(Stop 없음)은 `transcript_path` rollout 폴링으로 보완해야 한다(docs/04 §3.3). 이번 범위에서는 구현하지 않았고, 휴리스틱(무출력)으로 done이 된다.

**podd 연동**: 완료(§8). 설치 정책(전역 vs 프로필)은 여전히 사용자 결정 사항이다.

---

## 8. podd 연동 (P1 완료, 브랜치 `SJin-Choi/status-integration`)

코드: `crates/podd/src/server/status.rs` + `status/{hub,ingest,stream}.rs`(엔진 연결·훅 수신·구독 스트림), `crates/pod-protocol/src/status_protocol.rs`(와이어 타입, podd가 옛 경로로 재수출), `src/osc_replay.rs`(스냅샷 재생), `crates/pod-cli/src/status.rs`(CLI). 공유 파일(`protocol.rs`, `server/mod.rs`, `server/tasks.rs`, `terminal.rs`, `vt.rs`, CLI `main.rs`)에는 몇 줄씩만 추가했다. 프로토콜 버전은 **4**다(v3 = P3 태스크, v4 = v3 + 상태 메서드).

### 8.1 입력 배선

| 엔진 입력 | podd에서 오는 곳 |
|---|---|
| `feed_output` | holder가 보내는 Output 프레임(attach 클라이언트가 받는 원시 바이트와 같다). `queries`는 무시한다. OSC 7501 질의 응답은 holder만 한다(docs/03 §3, 중복 방지). |
| `on_title` | 같은 바이트에서 `status.rs`의 `TitleScanner`가 OSC 0/2를 찾는다(청크 경계 무관, 1 KiB 초과는 무시). VT 미러(alacritty)는 타이틀을 버리므로 따로 스캔한다. |
| `on_screen` | 250 ms 주기 티커. 마지막 샘플 뒤 출력이 있었고 250 ms가 지났을 때만 데몬 VT 미러의 보이는 화면(`read_lines(Screen)`)을 읽고, 바뀌었으면 넣는다. 실행 중인 터미널만 대상이다. |
| `tick` | 같은 티커에서 1 s마다(무출력 idle, hook staleness). |
| `on_foreground` | 터미널 생성·재연결 때 argv로 한 번(`ProcessInfo { pid, name: argv[0] 파일명, argv }`). 셸 안에서 나중에 띄운 에이전트를 찾는 포그라운드 프로세스 추적은 아직 없다(§8.7). |
| `on_process_exit` | holder의 Exited 프레임, 또는 holder 연결이 끊긴 경우(lost). |
| `on_hook` | 데몬 파이프로 들어온 pod-hook 줄(§8.2). |
| `ack` | `terminal.ack`(UI가 패널을 보여줄 때, `pod ack`). |
| `on_input` | 터미널로 가는 모든 입력(`Terminal::input`: attach 입력, `terminal.keys`, `terminal.send`, 리뷰 전달). §3.1-6 응답 추론에만 쓴다. |

엔진 시계는 데몬 시작 시각(unix ms)에 단조 경과 시간을 더한 값이다. 그래서 `since`를 벽시계 시각처럼 읽을 수 있고, 엔진 안에서는 단조 증가한다. 락 순서는 터미널 스트림 상태 → `TermStatus` → 허브 발행 락이다.

### 8.2 훅 수신과 인증

- `terminal.run`이 터미널마다 128비트 난수 토큰(`BCryptGenRandom`)을 만든다. 이 토큰을 `<state dir>/<id>.token`에 쓰고 터미널 env에 `POD_HOOK_TOKEN`으로 넣는다. `run`의 `env` 덮어쓰기보다 **나중에** 넣으므로 호출자가 바꿀 수 없다. state dir은 사용자 전용(`%LOCALAPPDATA%`)이다.
- 데몬 파이프의 연결 처리기는 JSON-RPC로 해석하기 전에 줄을 확인한다. 최상위 객체에 `jsonrpc`가 없고 `v`와 `terminal_id`가 있으면 훅 메시지로 본다. 응답은 쓰지 않는다(pod-hook은 한 줄 쓰고 닫는다).
- `v != 1`, 형식 오류, 모르는 터미널은 로그만 남기고 버린다(세지 않음). 터미널이 있으면 그 터미널의 `HookGate`(`status/ingest.rs`)가 토큰(상수 시간 비교) → 메시지 형태 → flood 제한 순으로 본다. 하나라도 걸리면 거부하고 `hooks_rejected`를 올린다(상태는 그대로). 통과하면 `hooks_accepted`를 올리고 `on_hook`에 넣는다. 자세한 규칙은 §8.8.
- 토큰 파일은 데몬 재시작 뒤에도 남는다. 그래서 재연결한 (실행 중인) 터미널의 훅도 인증된다. 터미널 프로세스가 끝나면, 그리고 `terminal.rm`과 오래된 기록 정리에서 지운다(§8.8).

### 8.3 노출

- `TerminalInfo.status`: `terminal.list`, `terminal.show`, `terminal.run`, wait 결과 등 `TerminalInfo`가 나오는 모든 곳에 들어간다. 형태는 `StatusInfo { state, kind?, source?, confidence, since, turn, agent?, app?, title?, msg?, progress?, outcome?, final_state?, badge, records, running_children, blocked_children, hooks_accepted, hooks_rejected }`이다. `running_children`이 held-open 하위 레코드 수다. `badge`는 `display_state()` 기준이다(`done (+1 running)`, `blocked:question`).
- `terminal.show {id|name}` → `TerminalInfo`. `terminal.ack {id|name}` → `TerminalInfo`(done이면 idle).
- `status.wait {id|name, status?, turn_end?, timeout_ms?}` → `{outcome: matched|timed_out|exited, status, terminal, waited_ms}`.
  - `status`: 상태 이름이면 그 상태가 될 때까지 기다린다(이미 그 상태면 즉시 일치). `any_change`면 다음 상태 변화를 기다린다(blocked kind 변화 포함).
  - `turn_end`: turn 카운터가 호출 시점보다 커지고 working이 아닐 때 일치한다. `status`와 함께 주면 둘 다 만족해야 한다.
  - 터미널이 끝나면 `exited`를 돌려준다. 단 `status: "exited"`를 기다린 경우는 matched다.
- `status.subscribe {ids?, lifecycle? (v9)}`: 응답 `{epoch, seq, terminals: [{id, seq, status}]}`를 보낸 뒤 연결이 스트림이 된다. 알림은 `status.changed {seq, id, status, from?, removed}`다. 같은 스트림에 v5 `gate.changed`, v6 `group.check`, `review.delivery`가 오고, `lifecycle: true`면 v9 `terminal.changed`·`task.changed`·`task.removing`도 온다(같은 `seq`, [03 §4.3](03-podd-p0.md)). 태스크 집계 상태가 바뀌면 `task.changed {change: "status", status}`가 나간다(아래 태스크 항목).
  - `seq`는 데몬 전체에서 단조 증가한다. 스냅샷에 이미 반영된 이벤트(터미널별 `seq` 이하)는 보내지 않는다.
  - 보이는 값이 바뀔 때마다 발행한다(confidence와 훅 카운터 포함, 창 타이틀 제외).
  - 느린 구독자는 `status.lagged {missed}`를 받고, `terminal.list`로 다시 맞추면 된다(버퍼 4096).
  - `epoch`(데몬 시작 시각)가 바뀌면 seq도 처음부터 다시 시작한 것이다. `terminal.rm`은 마지막으로 `removed: true` 이벤트를 보낸다.
- 태스크(P3): `TaskInfo.status = { state, kind?, terminal, badge, terminals }`. 태스크 터미널들의 표시 상태를 `blocked > error > done > working > idle > unknown > exited` 순서로 집계하고, 결정한 터미널과 그 배지를 함께 준다(동률이면 먼저 만든 터미널). `pod task ls`에 STATUS 열이, `pod task show`에 status 줄이 있다.

### 8.4 CLI

```
pod ls                                   # STATUS 열: "idle (osc)", "blocked:permission (hook)"
pod status <t> [--json]                  # 상태 상세
pod ack <t>                              # done → idle
pod wait <t> --status idle|working|done|blocked|error|exited|any-change [--timeout S]
pod wait <t> --turn-end [--timeout S]    # 현재(또는 다음) 턴이 끝날 때까지
pod hooks install|uninstall|status ...      # §9.1
```

- `pod wait`는 일치하면 0, 시간 초과나 종료면 1로 끝난다.
- `--turn-end` 주의: `pod send` 직후 에이전트가 wait 호출 전에 턴을 이미 끝냈다면 다음 턴까지 기다린다.
- (e2e-fix1, docs/12 B6) `pod wait`와 `pod gate request --wait`는 데몬 재시작을 견딘다: 연결이 끊기면 같은 파이프에 백오프로 다시 붙어 남은 시간으로 다시 묻는다(데몬을 직접 띄우지는 않는다). `POD_RECONNECT_TIMEOUT`(초, 기본 60) 안에 돌아오지 않으면 오류. `--turn-end`는 재시작 뒤 다시 물을 때 그 시점의 턴 기준이 된다(복원된 turn 카운터). 테스트: `tests/orchestration.rs::waits_survive_a_daemon_restart`.
- `pod hooks install|uninstall|status`: §9.1 (처음에는 `--dry-run`만 있었으나, 사용자 결정 후 실제 설치를 구현했다).

### 8.5 데몬 재시작과 스냅샷 재생 (선택한 동작)

엔진 상태는 메모리에 있다. 데몬이 holder에 (재)연결할 때 받는 VT 스냅샷 끝에 **마지막 OSC 7501 보고와 마지막 창 타이틀**을 붙인다. 그리고 (podd-hardening부터) **turn 카운터, 마지막으로 받아들인 훅 메시지, 마지막 ack 시각**을 `<state dir>\<id>.status.json`에 저장한다(turn이 바뀔 때, 훅을 받을 때, ack 때. 토큰은 빼고, 4 KiB가 넘는 payload 필드(도구 입력·출력)도 뺀다). 보고는 레코드별로 최신 1개씩, 최대 64개다(`osc_replay.rs`, holder와 데몬 VT 모두 적용). 새 `TermStatus`는 argv를 먼저, 그다음 스냅샷을 엔진에 넣는다. 결과는 다음과 같다.

- 재시작 뒤 상태는 **재유도**된다. Claude처럼 OSC로 보고하는 프로그램은 마지막 보고 상태(예: idle/working, source `osc`)로 돌아온다. 보고하지 않던 프로그램은 `unknown`에서 시작해 화면·타이틀 휴리스틱으로 다시 정해진다. 끝난 터미널은 `exited`다.
- **복원하는 것**: `turn` 카운터(저장값과 재유도값 중 큰 쪽), 마지막 훅 메시지를 **원래 받은 시각으로** 새 엔진에 다시 넣는다. 그래서 hook 증거·`outcome`·세션 id·도구 이름이 돌아오고, staleness 규칙(120 s)도 원래 시각 기준으로 그대로 적용된다(오래된 훅은 재생된 OSC보다 약하다). 훅 뒤에 ack가 있었으면 재생 뒤 ack도 다시 한다. 테스트: `tests/status.rs::turn_and_hook_evidence_survive_daemon_restart`(훅 blocked:permission + turn 1 → 재시작 → 같은 상태·turn, rm이 파일을 지움).
- (e2e-fix1) 저장된 훅이 blocked이고 그 뒤 응답 추론(§3.1-6)이 있었으면 그 시각(`answered_ms`)도 저장하고, 재시작 때 훅 재생 뒤 다시 적용한다(`restore_answer`). 그렇지 않으면 승인 뒤 일하던 에이전트가 재시작 후 다시 blocked로 보인다. 테스트: `tests/orchestration.rs::approved_permission_is_working_and_reviews_queue`.
- **복원하지 않는 것**: 휴리스틱이 기억하는 "working을 거쳤는지", 훅 하위 레코드(서브에이전트), `hooks_accepted`/`rejected` 카운터(0부터), `since`(재연결 또는 훅 시각).
- 이 재생은 시작 직후의 경쟁도 해결한다. 데몬이 holder에 붙기 전에 프로그램이 보낸 보고는 예전에는 스냅샷에만 남았다가 사라졌다. 번들 ConPTY에서 테스트가 실제로 실패해서 이 문제를 찾았다.
- attach 클라이언트도 같은 스냅샷을 받으므로 창 타이틀이 복원된다. OSC 7501은 일반 터미널이 무시한다.

### 8.6 pod-hook과 창 깜빡임

- Claude Code 2.1.319 바이너리(번들 JS)를 읽기 전용으로 확인했다. 훅은 `spawn(cmd, [], {shell: <Git Bash 또는 true>, detached: !windows, windowsHide: true})`로 실행된다. Windows에서는 `detached:false`와 `windowsHide:true`(libuv `CREATE_NO_WINDOW`)이므로 셸이 숨은 콘솔을 받고, pod-hook은 그 콘솔을 물려받는다. 따라서 콘솔 exe여도 Claude에서는 창이 뜨지 않는다.
- Codex가 훅을 어떤 플래그로 실행하는지는 확인하지 못했다(Rust 바이너리). 실행기가 `DETACHED_PROCESS`로 띄우면 콘솔 exe는 **새 콘솔 창을 만든다**(깜빡임). 그래서 pod-hook을 Windows에서 GUI 서브시스템(`windows_subsystem = "windows"`)으로 빌드했다. 콘솔을 아예 만들지 않으므로 실행 방식과 상관없이 창이 생기지 않는다. stdin/stdout은 상속된 파이프 핸들로 동작한다.
- 테스트로 확인한 것: PE 헤더의 Subsystem이 2(GUI)다. `DETACHED_PROCESS`로 띄워도 `{}`를 출력하고, 데몬이 없을 때 1.5 s 안에 끝난다(전체 예산은 1 s, §5).
- 대가: 대화형 콘솔에서 `pod-hook install`을 직접 치면 cmd가 GUI 프로그램을 기다리지 않아 프롬프트가 먼저 나올 수 있다. 사람이 쓸 때는 `pod hooks install --dry-run`을 쓴다.
- 실제 Claude/Codex 설정에 훅을 설치해서 실행해 보지는 않았다(사용자 설정을 쓰지 않는다는 제약).

### 8.7 검증

`crates/podd/tests/status.rs`는 실제 podd·pod-holder·pod-hook·pod 바이너리와 예제 `fake-agent`(스크립트로 OSC·타이틀·훅을 내는 테스트 프로그램)를 쓴다. 번들 ConPTY와 inbox ConPTY(`POD_CONPTY=inbox`) 모두에서 통과한다.

1. OSC working → blocked(question, msg) → done이 `terminal.show`·`terminal.list`를 바꾸고, `status.subscribe` 이벤트가 그 순서대로(seq 단조 증가) 온다. `terminal.ack`로 idle이 된다.
2. `codex.exe`로 복사한 fake-agent의 Braille 타이틀 스피너 → working(heuristic). 스피너가 사라지고 3 s 동안 출력이 없으면 → done. 화면에 "Would you like to run the following command"가 나오면 → blocked:permission(heuristic).
3. 터미널 안에서 실제 pod-hook으로 보낸 PermissionRequest → blocked:permission(source hook, msg Bash). 이후 새 OSC idle이 와도 hook이 우선한다. Stop → done(outcome completed). 잘못된 토큰과 토큰 없는 위조 메시지는 거부된다(`hooks_rejected=2`, 상태 변화 없음).
4. `pod wait --status done`과 `--turn-end`가 done 도착 시 반환한다. `pod status --json`, `pod ls` STATUS 열, 시간 초과 시 exit 1, `pod hooks install --dry-run`이 아무것도 만들지 않음을 확인한다.
5. 데몬 재시작 뒤 working(osc, 재생), turn 0. 이후 OSC done이 반영되고, 재시작 뒤에도 훅 토큰이 유효하다. rm이 토큰 파일을 지운다.

단위 테스트: 타이틀 스캐너(모든 분할 지점), 토큰, 재생 버퍼(모든 분할 지점), 태스크 집계, CLI 파싱. P3 태스크 통합 테스트에도 태스크 상태 확인을 추가했다.

**실제 에이전트 증거**(Claude Code 2.1.319, 이미 신뢰된 scratch 폴더, 입력 없음, release 빌드, 번들 ConPTY):
- `pod run --name c -- claude` 직후 `pod wait c --status idle`이 `idle source=heuristic`(타이틀 ✳, confidence 0.6)으로 먼저 반환됐다.
- 약 1 s 뒤 `pod status c` → `idle source=osc confidence=0.90 agent=claude app=claude-code`, `conpty: bundled:…\conpty.dll`.
- 데몬 `server stop` 뒤 다시 `pod status c` → `idle source=osc`(스냅샷 재생). 이후 kill·rm으로 정리했다.

### 8.8 훅 신뢰와 출처 (TM-22, rm-o4)

[22 위협 모델](22-threat-model.md) TM-22의 O4 카드. 훅은 **상태 증거일 뿐**이다. Gate 결정, 도구 권한 승인, 완료 보고(`pod orch done`)로 바뀌는 경로가 없고, 깨우기는 기존 규칙(busy/blocked/draft면 미룸)을 그대로 따른다.

- **형태 검사**(`HookMessage::check`): `terminal_id` ≤64자 출력 가능 ASCII, 이벤트 이름은 영문·숫자·`_` ≤64자(비어 있으면 payload의 `hook_event_name`, 그것도 같은 규칙), payload는 객체나 `null`. 상태로 보이는 훅 문자열(도구 이름, 질문, 오류, 하위 에이전트 종류)은 제어·bidi 문자를 빼고 256자로 자른다.
- **flood 제한**: 터미널마다 버스트 120, 이후 초당 60. 넘는 줄은 거부로 센다. 거부 로그와 즉시 발행은 터미널마다 초당 10건이고, 나머지는 세기만 한다(250 ms 티커가 카운터를 발행, 다음 로그 줄에 "N more rejected"). 버리는 줄의 로그는 데몬 전체 초당 10줄. 로그에는 serde 오류 원문(필드 값, 예컨대 잘못 놓인 토큰이 들어갈 수 있음)을 쓰지 않고, 보낸 쪽 문자열은 escape해 64자로 자른다.
- **토큰 수명**: `terminal.run`마다 새 토큰(재사용 없음). 터미널 프로세스가 끝나면 즉시 폐기한다(파일 삭제, 이후 같은 토큰은 "terminal exited" 거부) — 남은 자식 프로세스가 SessionStart로 끝난 터미널을 되살리던 경로를 막는다. `rm`, 포기한 unreachable, 오래된 기록 정리는 기존대로 지운다. 시작할 때(재발견 뒤) 목록에도 unreachable에도 없는 `t*.token`을 지운다. **실행 중인 에이전트의 토큰은 데몬 재시작에서 바꾸지 않는다**: 에이전트 환경 변수는 바꿀 수 없어서, 바꾸면 정상 훅만 끊긴다.
- **비노출**: 토큰은 상태 파일(`slim_hook`, payload 32 KiB 이하 객체만), 로그, 터미널 출력, brief에 쓰지 않는다. pod-hook의 응답은 정확히 `{}`다.
- **한계**: 토큰을 가진 같은 사용자 프로세스의 위조(가짜 idle/Stop)는 인증으로 막지 못한다. 데몬이 살아 있는 holder를 잃은 드문 경우(호환되지 않는 holder, 끊긴 holder 파이프)도 종료로 보고 토큰을 폐기하므로, 나중에 다시 붙으면 그 에이전트의 훅은 거부된다(상태는 OSC·휴리스틱으로 계속). 상태 `unknown`일 때도 깨우기를 허용하는 `pod_orch::wake::may_wake` 규칙은 이 카드에서 바꾸지 않았다(O1 이후 후속).
- 테스트: `crates/podd/tests/hook_trust.rs`(틀린·이전·없는 토큰, 깨진·큰 payload, 잘못된 형태, flood, 종료 시 폐기, 재시작 정리, 대기 Gate가 있는 워커에 위조 PermissionRequest/Stop → Gate 전이·입력·done 0, busy/draft 코디네이터 깨우기 0과 대조군, 로그·상태 파일·워크트리에 토큰 0), `status/ingest.rs` 단위 시험(토큰 순서, 버스트·초당 한도, 보고 한도), `pod-status` `hook.rs`(형태·문자열 정리).

**남은 일·한계**
- 포그라운드 프로세스 추적이 없다. 셸 터미널에서 나중에 띄운 claude/codex는 OSC `app=`(Claude)이나 훅으로만 식별된다. 그전까지는 에이전트를 모르므로 타이틀 휴리스틱이 꺼져 있다. Job Object 프로세스 목록으로 보완할 수 있다.
- ~~hook blocked 뒤 Esc로 취소했을 때 훅이 오지 않으면, 최대 120 s 동안 hook blocked가 OSC idle보다 우선한다.~~ 고침(podd-gaps2, §3.1-2·§7): OSC가 턴 끝으로 바뀌면 바로 풀린다. OSC 보고가 없을 때만 120 s 규칙.
- ~~재시작 때 turn과 hook 증거를 복원하지 않는다.~~ 고침(podd-hardening, §8.5): turn·마지막 훅·ack를 state dir에 저장해 복원한다. 하위 레코드와 훅 카운터는 복원하지 않는다.
- 이번 변경 전 데몬이 만든 터미널은 토큰 파일이 없어서 훅이 모두 거부된다(실패 시 닫힘).
- 출력 청크마다 상태 스냅샷을 계산해 비교한다. 출력이 아주 많은 터미널에서의 비용은 측정하지 않았다.

---

## 9. 전역 훅 설치와 Codex 폴더 신뢰 (사용자 결정 2026-11-08)

### 9.1 `pod hooks install|uninstall|status`

사용자 결정: 훅은 **전역**에 설치한다(Claude `%USERPROFILE%\.claude\settings.json`, Codex `%USERPROFILE%\.codex\hooks.json`). env 가드 덕분에 Pod 밖에서는 아무 일도 하지 않는다(pod-hook은 `POD_TERMINAL_ID`/`POD_SOCKET`이 없으면 `{}`를 출력하고 바로 끝난다). 코드는 `crates/pod-cli/src/hooks_install.rs`다.

```
pod hooks install   [--dry-run] [--no-codex-trust] [--confirm DIGEST] [공통 옵션]
pod hooks uninstall [--dry-run] [공통 옵션]
pod hooks status    [--check-codex] [공통 옵션]
공통: [--scope global|profile] [--agent claude|codex|all] [--home D | --claude-config-dir D --codex-home D]
      [--bin-dir D] [--source pod-hook.exe] [--json]
```

- **안정된 바이너리 위치**: `install`은 `pod-hook.exe`(기본은 `pod.exe` 옆)를 `%LOCALAPPDATA%\Pod\bin\pod-hook.exe`로 복사하고 훅이 그 경로를 가리키게 한다. 저장소를 옮기거나 다시 빌드해도 에이전트가 깨지지 않는다. 내용이 같으면 복사하지 않는다. 에이전트가 훅을 실행하는 중이라 덮어쓸 수 없으면 기존 파일을 `pod-hook.exe.old-<시각>`으로 옮긴 뒤 바꾼다(실행 중인 exe도 이름은 바꿀 수 있다).
- **확인 (TM-22, rm-o4)**: 바이너리나 설정 파일 중 하나라도 바뀌는 실제 설치는 사용자 확인이 있어야 쓴다. 콘솔이면 바뀔 내용(절대 pod-hook 경로, 파일별 동작)을 보여 주고 y/N을 묻는다. 콘솔이 아니면 `--dry-run`이 출력한 digest(`confirm`, 16자, 경로·바이너리 해시·파일별 새 내용·Codex 신뢰 여부의 SHA-256)를 `--confirm`으로 넘긴다. 그 사이 바이너리나 설정이 바뀌면 digest가 달라 거부되고 다시 확인해야 한다. 바뀔 것이 없으면 묻지 않는다. 앱은 미리보기의 digest를 그대로 넘긴다. 훅 실행 파일은 절대 로컬 경로(UNC 불가)의 `pod-hook[.exe]`이고 셸이 확장하는 문자(`"` `%` `$` `` ` `` `!`, 제어 문자)는 거부한다. 영문·숫자·`-_./:` 밖의 문자가 있으면 따옴표로 감싼다.
- **병합**: `pod_status::install`의 `merge_hooks`/`remove_pod_hooks`를 쓴다. Pod 명령 모양(`<…/pod-hook[.exe]> claude|codex <Event>`, `is_pod_command`)인 그룹만 추가·교체·삭제한다. `pod-hook`이라는 글자만 들어간 다른 도구의 훅은 남긴다. 다른 키(`model`, `permissions`, `statusLine` …)와 다른 도구의 훅은 그대로 둔다. 다시 실행해도 중복되지 않는다(Pod 그룹을 지운 뒤 끝에 다시 붙이므로 위치도 같다). 키 순서를 보존하도록 `serde_json`의 `preserve_order`를 켰다. 들여쓰기는 2칸 pretty JSON으로 바뀐다.
- **백업**: 바꿀 파일마다 쓰기 전에 `<file>.pod-backup-<UTC YYYYMMDD-HHMMSS>`로 복사한다(같은 이름이 있으면 `-N`). 바뀌지 않는 파일은 백업하지 않는다.
- **제거**: `uninstall`은 Pod 그룹만 지운다. 결과는 설치 전 JSON과 같다(테스트로 확인). 없는 파일은 만들지 않는다. `pod-hook.exe`는 남겨 둔다(실행 중일 수 있다).
- **상태**: 설치된 바이너리가 있는지, 이 빌드와 같은지, 에이전트별 Pod 이벤트 수·다른 도구 그룹 수·훅이 실행하는 exe 존재 여부를 보여준다. `--check-codex`는 Codex에 신뢰 상태도 물어본다.
- **Codex 훅 신뢰**: Codex는 사용자가 신뢰한 훅만 실행한다. 신뢰 정보는 `config.toml`의 `[hooks.state.'<hooks.json 경로>:<event>:<그룹 번호>:<훅 번호>'] trusted_hash = "sha256:…"`에 있다. 해시는 Codex가 계산한다(명령 문자열의 단순 SHA-256이 아님, 확인함). 그래서 Pod는 Codex에 직접 묻는다. `codex -c approval_policy=never -s read-only -a never app-server`(stdio JSON-RPC, `CREATE_NO_WINDOW`)를 띄우고 `initialize` → `hooks/list`로 각 훅의 `key`, `currentHash`, `trustStatus`를 받는다. Pod가 방금 쓴 명령과 정확히 같고 Pod가 쓴 hooks.json에 있는 훅 중 신뢰되지 않은 것만(rm-o4: 다른 파일이나 다른 실행 파일의 Pod 모양 명령은 사용자가 직접 승인) `config/batchWrite {keyPath: "hooks.state", mergeStrategy: "upsert"}`로 기록한다. 쓰기 전에 `config.toml`을 백업하고, 다시 `hooks/list`로 모두 trusted인지 확인한다. `CODEX_HOME`을 명시적으로 넘기므로 `--home`으로 설치하면 실제 설정은 절대 건드리지 않는다. Codex 0.159.0에서 로그인 없이 동작하고, 모델 턴은 없다. codex가 없거나 실패하면 설치는 계속하고 note로 알린다(Codex 안에서 직접 승인해야 한다).
- **제거 시 Codex 신뢰 항목**: 남겨 둔다. 해시가 이전 훅 내용에 고정되어 있어서 아무 효과가 없다.
- **이 머신에서 실제 설치는 하지 않았다.** 검토 후 코디네이터가 실행할 명령:
  ```
  cargo build --release -p pod-cli -p pod-hook
  target\release\pod.exe hooks install --dry-run     # 확인
  target\release\pod.exe hooks install
  target\release\pod.exe hooks status --check-codex
  # 되돌리기: target\release\pod.exe hooks uninstall   (또는 *.pod-backup-* 복원)
  ```

**pod-hook 실행 경로 확인**(release, podd 없음, `POD_*` env 있음, stdin JSON): 직접 실행, `cmd /d /s /c`, `powershell -Command`, Git Bash `bash -c`를 각각 `CREATE_NO_WINDOW`·`DETACHED_PROCESS`·기본 플래그로 띄웠다. 모두 `{}`를 출력하고 exit 0으로 13–392 ms 안에 끝났다. GUI 서브시스템이라 어느 경우에도 pod-hook 자신의 콘솔은 생기지 않는다(§8.6). 예외는 PowerShell을 `DETACHED_PROCESS`로 띄운 경우다. 이때는 PowerShell 자신이 콘솔을 만들고 출력을 잃는다. 이는 실행기 쪽 문제다. Claude의 실행 방식은 §8.6에서 확인했다. Codex의 실제 훅 실행 플래그는 여전히 확인하지 못했다(실행하려면 인증된 Codex 세션이 필요하다). GUI 서브시스템이라 결과는 같다.

**테스트**(`crates/pod-cli/tests/hooks.rs`, 임시 HOME만 사용): ① 다른 도구의 훅과 무관한 키가 있는 settings.json·hooks.json에 dry run(아무것도 쓰지 않음) → install(바이너리 복사, 원본 그대로인 백업, 키 순서 유지, 다른 도구 그룹이 먼저, Pod 그룹이 뒤, 다른 도구만 있는 이벤트 유지, matcher) → 재설치(unchanged, 새 백업 없음, 중복 없음) → status(Claude 13 / Codex 9 이벤트, 다른 그룹 3, exe 존재) → uninstall(원본 JSON과 동일) ② 파일이 없을 때: uninstall은 아무것도 만들지 않고 install은 새로 만든다 ③ 실제 `codex app-server`로 임시 `CODEX_HOME`에 신뢰 기록: Pod 훅 9개 모두 trusted, 다른 도구 그룹(`session_start:0:0`, `stop:0:0`)은 신뢰하지 않음, 재실행 시 새로 쓸 항목 0. 테스트 전후 실제 `%USERPROFILE%\.codex\config.toml`이 같은지 비교한다(codex가 없으면 건너뜀). podd `status.rs`의 CLI 테스트도 임시 프로필 디렉터리로 실제 설치를 한다.

참고: 테스트 중 실제 `~/.codex/config.toml`의 해시가 한 번 바뀌었다. 원인은 같은 시각(21:57:36)에 같은 폴더에서 실행 중이던 다른 Codex 세션(PID 54556)이 `[projects."C:\\ai\\ide"]`를 쓴 것이다. Pod 테스트가 쓴 것이 아니다. 다만 그런 동시 쓰기가 있으면 ③의 실제 설정 비교 검사가 거짓 실패할 수 있다.


**fu-daemon(2026-11-09) 추가**:
- `pod hooks uninstall`은 settings.json/hooks.json에서 Pod 그룹만 지우는 것(기존)에 더해, Codex `config.toml`의 `[hooks.state]`에서 **Pod 훅의 신뢰 항목**을 지운다. 신뢰 키는 `<hooks.json>:<event_snake>:<그룹>:<훅>` 위치 기반이므로 지우기 전 hooks.json에서 Pod 그룹의 위치를 계산한다. Pod 그룹 뒤에 있던 다른 도구의 그룹은 앞으로 당겨지므로 그 신뢰 항목도 새 위치 키로 옮긴다(해시는 그대로; 옮기지 않으면 Codex가 그 훅을 다시 승인해 달라고 한다). 다른 파일의 항목, 다른 키·주석·순서는 그대로 둔다(`toml_edit`, MIT/Apache-2.0). 쓰기 전에 `config.toml.pod-backup-<시각>`. `--dry-run`은 개수만 보고. 결과 JSON: `codex_untrust {config, removed, moved, backup?}`. 실패하면 note만 남기고 제거는 계속(남은 항목은 옛 해시라 무해).
- `pod-hook --version`(`pod-hook 0.1.0`). `pod hooks status --json`에 `bin`(설치 경로), `bin_version`, `source`(이 `pod.exe` 옆의 `pod-hook.exe`, 설치기가 비교할 원본), `source_version`이 더해졌다(`bin_current`는 바이트 비교 그대로). `--version` 이전 빌드는 `{}`를 출력하므로 `bin_version`이 없다 → 설치기·앱은 "오래된 훅"으로 보면 된다. 2 s 제한, `POD_TERMINAL_ID`/`POD_SOCKET`를 지우고 실행.
- 테스트(임시 홈만, 실제 설정은 건드리지 않음): `hooks_install::tests::codex_trust_entries_pruned_and_rekeyed`(앞 도구–Pod–나중 도구 순서: Pod 2개 삭제, 나중 도구 항목 2→1 이동, 다른 파일·`[projects]`·주석 유지, 두 번째 실행은 무변경), `tests/hooks.rs::uninstall_removes_pod_codex_trust_entries_from_temp_config`(codex 없이: dry-run 무변경, Pod 항목 3개 삭제, 다른 도구 항목 유지, 백업 = 원본), `codex_hooks_trusted_via_app_server_in_temp_home`(실제 `codex app-server`로 신뢰한 9개를 uninstall이 9개 지움), `status_reports_binary_paths_and_versions`.

### 9.2 Codex 폴더 신뢰 자동화 (Pod가 만든 워크트리만)

`agent: codex` 태스크를 만들면 podd가 Codex를 이렇게 띄운다(`crates/podd/src/server/agent_launch.rs`).

```
codex --no-daemon -c check_for_update_on_startup=false -c "projects={'<워크트리 절대 경로>'={trust_level='trusted'}}" [prompt]
```

- **방법을 고른 이유**(Codex 0.159.0 실측, 새 임시 폴더, 시작 화면만): 점 표기 `-c projects.'<path>'.trust_level="trusted"`는 원래 대소문자, 소문자, 슬래시 표기 어느 것으로도 신뢰 대화상자를 막지 못했다(docs/04 §6의 미해결 질문). **인라인 테이블** `-c projects={'<path>'={trust_level="trusted"}}`은 원래 대소문자와 소문자 모두에서 대화상자 없이 바로 시작 화면이 나왔다. 이 오버라이드는 그 프로세스의 `projects` 테이블만 바꾼다. 그래서 사용자의 `config.toml`에 쓰지 않고, 태스크를 지울 때 정리할 것도 없으며, Pod가 만든 워크트리 경로 외에는 신뢰하지 않는다(사용자가 명시적 argv로 띄운 codex에는 적용하지 않는다).
- 경로는 TOML 리터럴 문자열(작은따옴표)로 넣는다. 따옴표가 없으므로 `cmd /c` shim을 거쳐도 깨지지 않는다. 경로에 `'`가 있으면 기본 문자열로 이스케이프한다. `/`는 `\`로 바꾼다.
- `--no-daemon`: docs/04 §4.3 권장 사항이다(작업이 PTY 안에서 돈다). 실측에서 데몬 모드는 "start the Windows daemon from a non-elevated terminal" 오류로 시작하지 않았다.
- `check_for_update_on_startup=false`: "Update available" 선택 화면도 신뢰 대화상자처럼 무인 태스크의 시작을 막는다(0.159.0 → 0.167.0 업데이트 안내가 실제로 먼저 떴다). Pod 태스크에서만 끈다. **코디네이터 확인 필요**: 요청 범위 밖의 판단이다.
- **증거**(release, 별도 파이프, 임시 git 저장소, 입력 없음): `pod task new ev cx --agent codex` → 워크트리 `C:\Users\User\pod\worktrees\ev\cx`, 명령 `codex --no-daemon -c check_for_update_on_startup=false -c projects={'C:\Users\User\pod\worktrees\ev\cx'={trust_level='trusted'}}` → 10 s 뒤 화면 `>_ OpenAI Codex (v0.159.0)` / `› Ask Codex to do anything`, "trust this folder" 0줄. `~/.codex/config.toml`에 해당 경로 항목 0개(애초에 쓰지 않음). `pod task rm ev/cx --delete-branch` → "worktree removed; branch deleted". 이후에도 설정 파일에 남은 항목이 없다.
- 관찰: 이 Codex 시작 화면은 `done (heuristic)`으로 표시됐다. 시작 중 타이틀 스피너를 working으로 보고, 스피너가 사라진 뒤 3 s 동안 출력이 없어 done이 됐다. 실제 턴이 아닌데 done이 되는 것은 휴리스틱의 한계다. 훅을 설치하면 SessionStart(idle)가 우선한다.

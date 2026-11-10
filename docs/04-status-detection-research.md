# 04. 에이전트 상태 감지 리서치 — 측정과 Pod 상태 엔진 설계 (v0.1)

작성: 2026-11-08 · 대상: Claude Code 2.1.319, Codex CLI 0.159.0 (Windows 11)
선행 문서: [01 §4.2(2), §7](01-plan.md)

표기: **(관찰)** Pod가 에이전트의 터미널 출력·공개 훅 인터페이스를 통해 직접 측정한 것 · **(추론)** 근거에서 끌어낸 판단 · **(미확인)** 확인 못 함

이 문서는 Pod가 공개 인터페이스(에이전트가 터미널에 쓰는 바이트, 문서화된 훅)로 측정한 사실과, 그 위에 세운 Pod 상태 엔진 설계를 담는다. 다른 제품의 내부 구현은 다루지 않는다.

---

## 0. 요약 (TL;DR)

1. **Claude Code는 OSC 7501 상태 시퀀스를 직접 내보낸다**(관찰). 시작할 때 `ESC]7501;?` 질의를 보내고, 터미널이 응답하면 그때부터 상태를 보고한다. 응답이 없으면 보고하지 않는다.
2. **형식**(관찰): `ESC ] 7501 ; key=value(:key=value)* (BEL | ESC \)`. 필드는 `state`, `app`, `id`, `kind`, `progress`, `title`(base64), `msg`(base64)이다. 상태 단어는 `idle|working|done|blocked|error`(+`clear`)이고 kind는 `permission|question|auth`이다. 터미널의 질의 응답은 `ESC]7501;?:states=idle,working,done,blocked,error:kinds=permission,question,auth ESC\`이다.
3. **Codex 0.159.0은 OSC 7501을 내보내지 않는다**(관찰). Codex에서 쓸 수 있는 신호는 ① 공개된 hooks 기능(`~/.codex/hooks.json`) ② 작업 중 터미널 타이틀의 Braille 스피너(U+2800–28FF) ③ OSC 9 알림(미확인)이다.
4. **훅**: Claude Code(`~/.claude/settings.json`의 `hooks`)와 Codex(`hooks.json`)는 둘 다 공개 문서화된 훅을 지원한다. 훅 명령은 stdin으로 JSON 이벤트를 받는다. Pod는 `pod-hook.exe`를 등록하고, 환경 변수 가드로 Pod 밖에서는 아무 일도 하지 않게 한다.
5. **Pod 권장안**: 우선순위는 `hook > osc(7501) > title/heuristic > process`이다. 단일 상태 머신 `idle/working/blocked/done/error/unknown`을 두고, **Pod OSC 규격 v1은 OSC 7501 문법을 그대로 채택**한다(새 번호를 만들지 않음). 그래야 Claude Code와 같은 규격을 쓰는 다른 터미널 도구와 즉시 호환된다.

---

## 1. Q1 — OSC 7501: 바이트 형식, 필드, 누가 내보내는가

### 1.1 송신자 = 에이전트 자신 (Claude Code 네이티브)

- **(관찰)** ConPTY로 Claude Code를 직접 실행해 원시 출력을 캡처하면, 시작 직후 `ESC]7501;?` 질의가 나온다(§1.3 B). 질의에 응답하는 터미널 안에서는 턴마다 `state=working` → `state=done` 같은 보고가 나온다(§1.3 C). 래퍼나 훅 없이 Claude Code 프로세스 자신이 쓰는 바이트다.
- **(관찰)** 질의에 응답이 없으면 보고는 나오지 않는다. 즉 터미널이 7501을 지원한다고 응답해야 켜진다.
- **(관찰)** 보고는 BEL(`\x07`)로 끝났다. 수신 측은 ST(`ESC \`)도 받아야 한다(§1.2).
- **(관찰)** 하위 작업(서브에이전트 등)은 `id=<작업id>`, `title=<설명>`이 붙은 별도 레코드로 보고될 수 있다(합성 송신기로 수신 처리를 확인, Claude 실측은 §6 미확인).
- **(관찰)** Codex 0.159.0은 같은 조건에서 7501 질의도 보고도 보내지 않았다. 터미널 타이틀(OSC 0/2)만 바뀌었다.

### 1.2 바이트 형식 (규격 정리)

```
보고:   ESC ] 7501 ; <field> ( : <field> )*  ( BEL | ESC \ )
field:  key=value      key = [a-z]+ (≤16자),  value = [A-Za-z0-9_.,+/=-]*
질의:   ESC ] 7501 ; ?  ( BEL | ESC \ )
응답:   ESC ] 7501 ; ?:states=idle,working,done,blocked,error:kinds=permission,question,auth ESC \
```

| 필드 | 값 | Pod 수신 제한 |
|---|---|---|
| `state` | `idle` `working` `done` `blocked` `error` `clear` | **필수**. 그 외 값이면 레코드 전체 무시 |
| `app` | `[A-Za-z0-9_.+-]+`, ≤32 | 송신 프로그램 식별자. Claude=`claude-code` |
| `id` | `/`로 구분된 최대 8개 세그먼트, 각 `[A-Za-z0-9_.+-]{1,32}`, 전체 ≤128 | 없으면 루트 레코드. 있으면 하위 작업 레코드 |
| `kind` | `permission` `question` `auth` | `state=blocked`일 때만 의미 있음 |
| `progress` | `0*\d{1,3}`, ≤100 | `working`/`blocked`일 때만 |
| `title` | base64(UTF-8), 인코딩 ≤256 / 디코딩 ≤192 | 제어문자 금지 |
| `msg` | base64(UTF-8), 인코딩 ≤2732 / 디코딩 ≤2048 | 제어문자 금지 |
| (기타 키) | — | 무시(전방 호환) |
| 페이로드 전체 | ≤4087자 | 초과 시 무시 |

- 종결자: Claude는 BEL(`\x07`)을 썼다. Pod의 질의 응답과 attach 재생은 ST를 쓴다. **수신 측은 둘 다 받아야 한다.**
- `state=clear`(+`id`)는 해당 레코드 삭제, `id` 없이 보내면 루트 상태 해제(프로그램 종료/모드 복원 시).

### 1.3 실측 샘플 (raw)

**A. 터미널의 질의 응답** — 합성 송신기(`emit7501.py`)가 stdin으로 받은 바이트(관찰):
```
'\x1b]7501;?:states=idle,working,done,blocked,error:kinds=permission,question,auth\x1b\\'
```

**B. Claude Code 질의** — Windows 기본 ConPTY(pywinpty)에서 캡처한 Claude 시작 출력(관찰, `raw_claude.log`):
```
1.192 '\x1b[>0q'                      # XTVERSION
1.298 '\x1b[?u\x1b]7501;?\x07'        # kitty keyboard 질의 + OSC 7501 질의 (BEL 종결)
```

**C. Claude Code 상태 보고** — 질의에 응답하는 터미널에 붙어 받은 원시 스트림(관찰, `raw_attach.log`; 프롬프트 "reply with the word ok"):
```
0.110  ...\x1b[=5;1u\x1b]7501;state=idle:app=claude-code\x1b\\\x1b[?1004h...   # attach 시 터미널이 현재 상태를 ST로 재생
6.168  '\x1b]7501;state=working:app=claude-code\x07'                            # Claude 원본 프레임 (BEL)
7.103  ...\x1b[46;3H\x1b[?25h\x1b]7501;state=done:app=claude-code\x07\x1b[m...
```

**D. Claude Code 보고 순서**(관찰, 프롬프트 2회 + 질문 도구 1회 + Esc):
```
idle                         # 신뢰 대화상자 통과 직후 첫 보고
working → done               # 턴 1
working → blocked kind=question msg="answer: Would you like me to keep my replies short ... (Yes · No)"
idle                         # Esc로 중단
```

**E. 합성 송신기 → 파싱 결과**(관찰): 모든 필드가 디코딩되어 반영된다.
```
working:  {"app":"podtest","msg":"compiling","progress":40,"state":"working"}
blocked:  {"app":"podtest","kind":"permission","msg":"Allow rm?","state":"blocked"}
id=sub1:  records {"sub1":{"app":"podtest","state":"working","title":"child task"}}  (루트 상태는 그대로)
done / error("boom") / idle 순서대로 반영
```

### 1.4 관찰된 함정

- **Claude 신뢰 대화상자("Is this a project you trust?") 동안에는 7501 보고가 없다**(관찰). 첫 보고는 대화상자를 넘긴 뒤 `idle`로 온다. → 이 구간은 휴리스틱(화면 패턴)이나 Pod 자체 `launch` 상태로 메워야 한다.
- **Windows 기본 ConPTY + win32-input-mode(`ESC[?9001h`)에서는 응답을 써 넣어도 Claude의 7501이 켜지지 않았다**(관찰: 질의 후 보고 0건). Microsoft가 배포하는 최신 ConPTY(`conpty.dll` + `OpenConsole.exe`)를 번들한 터미널 안에서는 정상 동작했다. 출력 방향의 OSC 7501은 기본 ConPTY도 그대로 통과시켰다(합성 송신기로 관찰). → **Pod는 번들 OpenConsole/conpty.dll을 쓰거나, 질의 응답이 입력 경로에서 깨지지 않게 해야 한다.** 원인(입력 인코딩인지 DA1 flush인지)은 미확인. 해결은 [03 §5](03-podd-p0.md).

---

## 2. Q2 — 프로세스와 실행 방식 (Windows)

- **포그라운드 프로세스**(관찰): npm shim 때문에 Claude는 `cmd.exe /c claude.CMD …`, Codex는 `codex.EXE`로 보였다(Windows에서는 래퍼 프로세스가 포그라운드로 잡힘). 에이전트 식별은 ① 7501 `app` 값 ② 없으면 포그라운드 프로세스 이름/argv 순서로 한다. `node`·`python` 같은 인터프리터는 건너뛰고 실제 스크립트 이름을 본다.
- **Codex의 공유 백그라운드 서버**(관찰): Codex 0.159는 공유 백그라운드 서버(app-server)를 쓸 수 있어, 그대로 두면 명령이 Pod 터미널 밖에서 실행될 수 있다. Pod가 Codex를 띄울 때는 `--no-daemon`을 기본으로 한다(§4.3, [07 §8](07-pod-status.md)).
- Codex는 7501을 보내지 않으므로 Pod 안에서도 OSC만으로는 상태가 `unknown`이다(관찰). 훅 또는 타이틀 휴리스틱이 필요하다.

---

## 3. Q3 — 훅: 공개 인터페이스와 Pod 매핑

### 3.1 쓸 수 있는 훅 이벤트

- **Claude Code**(공개 문서): `settings.json`의 `hooks`에 이벤트별 명령을 등록한다. Pod가 쓰는 이벤트: `SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, PostToolUseFailure, PermissionRequest, Elicitation, ElicitationResult, Stop, StopFailure, SubagentStart, SubagentStop, SessionEnd`. 이벤트는 Claude Code 버전에 따라 추가되므로 오래된 버전에서는 일부가 오지 않을 수 있다.
- **Codex**(공개 기능): `hooks.json`에 등록한다. Pod가 쓰는 이벤트: `SessionStart, UserPromptSubmit, PreToolUse, PermissionRequest, PostToolUse, SubagentStart, SubagentStop, Stop, Interrupt`. Codex는 새 훅에 대해 `config.toml`의 신뢰 기록을 요구한다(Pod의 처리 방식은 [07](07-pod-status.md)).
- 훅 명령은 stdin으로 이벤트 JSON을 받는다. Pod 훅은 stdout에 `{}`만 바로 출력해서 에이전트의 결정에 개입하지 않는다.

### 3.2 전송 원칙

- **훅은 전역 설정에 한 번 설치하고, 패널 라우팅은 Pod가 PTY에 주입한 환경 변수로 한다.** Pod 밖에서 실행되면 env가 없으므로 훅은 아무것도 하지 않는다. 구체적인 전송(named pipe)은 §4.3.

### 3.3 이벤트 → 상태 매핑

Pod의 매핑은 [07 §4.2](07-pod-status.md)에 표로 정리했다. 요점:

| Claude 훅 | Pod 상태 |
|---|---|
| `SessionStart`(compact 제외) | idle + 세션 경계 |
| `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `PostToolUseFailure` | working |
| `PreToolUse`(AskUserQuestion), `PermissionRequest`, `Elicitation` | blocked{question/permission} |
| `ElicitationResult` | 대기 전 상태 복원 |
| `Stop` | done (`is_interrupt`면 idle/cancelled) |
| `StopFailure` | error |
| `SubagentStart/Stop`, `agent_id`가 있는 이벤트 | 자식 레코드 갱신(리드 상태는 유지) |

| Codex 훅 | Pod 상태 |
|---|---|
| `SessionStart` | idle + 세션 경계 |
| `UserPromptSubmit`, `PostToolUse` | working |
| `PreToolUse` | 질문 도구면 blocked{question}, 아니면 working |
| `PermissionRequest` | blocked{permission} |
| `Stop` | done |
| `Interrupt` | idle(cancelled) |
| `SubagentStart/Stop` | 자식 레코드 |

- 페이로드 필드 이름(`tool_name`, `agent_id`, `is_interrupt`, `transcript_path` 등)은 공개 문서와 실측 페이로드를 기준으로 한다. 실측하지 못한 필드는 `hook::PAYLOAD_FIELDS`에 `verified=false`로 표시한다.
- (미확인) Codex는 실패한 턴에 `Stop`을 보내지 않을 수 있다. 그렇다면 `transcript_path`(rollout)를 읽어 실패 턴 종료를 보완해야 한다.

### 3.4 터미널 타이틀 규칙 (가장 약한 증거)

- `✳ `로 시작하는 타이틀 = Claude idle 표시.
- Braille `[⠀-⣿]` 스피너 = working(Codex 관찰: `⠋ work`). 쿼터서클 `[◐-◓]` 스피너도 working으로 본다(미관찰).
- OSC 7501 보고가 있으면 타이틀보다 우선한다. 타이틀은 프로그램 보고가 없을 때만 해석한다.

---

## 4. Q4 — Pod 권장 설계

### 4.1 원칙

1. **증거 우선순위**: `hook(3) > osc(2) > heuristic(1) > process(0)`. 같은 패널에서 높은 출처의 신선한 증거가 있으면 낮은 출처는 상태를 바꾸지 못하고 부가 정보(타이틀, 진행률)만 채운다. 높은 출처가 N초(권장 hook 120s, osc 무기한, 단 프로세스 종료 시 폐기) 동안 조용하면 낮은 출처가 다시 상태를 결정할 수 있다.
2. **에이전트 설정은 건드리지 않는 쪽을 기본으로 한다.** Claude는 OSC 7501만으로도 idle/working/done/blocked(kind)/error를 정확히 준다(관찰). 훅은 **선택적 정밀도 향상**(도구명, 질문 본문, 서브에이전트 트리, 세션 id)용으로 쓰고, 설치는 사용자 동의 후 별도 프로필(`CLAUDE_CONFIG_DIR`)이나 env 가드 방식으로 한다.
3. 모든 상태에 `source`, `confidence`, `since`, `turn`을 함께 기록한다(01 문서 §4.2(2)).

### 4.2 통합 상태 머신

상태: `unknown`(초기·무보고), `idle`, `working`, `blocked{kind: permission|question|auth|other}`, `done`(턴 완료, 미확인), `error`, (터미널 레벨) `exited`.

```
             launch / 첫 출력
 unknown ─────────────────────────┐
   │ osc idle / hook SessionStart  │ heuristic(스피너) 
   ▼                               ▼
  idle ──── prompt/UserPromptSubmit/osc working ───▶ working ◀──┐
   ▲                                                  │  │  │     │ approve / answer /
   │ user focuses pane (ack)                          │  │  │     │ PostToolUse / ElicitationResult
   │                                                  │  │  └──▶ blocked{kind} ──┘
  done ◀──── Stop / osc done ─────────────────────────┘  │          │ Esc/interrupt
   │                                                     │          ▼
   │ new prompt → working                 StopFailure / osc error   idle
   ▼                                                     ▼
 (idle)                                                error ── new prompt ──▶ working
 어느 상태든: 프로세스 종료 → exited(최종 상태 보존) · osc state=clear → 낮은 출처로 강등
 working + interrupt(Esc, hook Interrupt, is_interrupt) → idle(outcome=cancelled)
```
- `done → idle`: Pod UI에서 사용자가 해당 패널을 보면 확인(ack)한 것으로 처리한다. `done`은 "사용자가 안 보는 사이에 끝남"이라는 앱 레이어 개념이다. 집계 우선순위는 `blocked > error > done > working > idle > unknown`이다.
- `turn` 카운터: `working`을 떠날 때마다 증가한다(세션 시작 훅이 일으킨 전이는 제외, docs/07). `pod wait`은 이 값으로 "새 턴 종료"를 판단한다.
- 자식 작업(서브에이전트/백그라운드): `records[id]`로 따로 둔다. 리드가 done이어도 자식이 working이면 패널 배지는 `done(+N running)`으로 표시한다(held-open).

### 4.3 에이전트별 어댑터

| 계층 | Claude Code | Codex CLI |
|---|---|---|
| **L1 hook** (선택) | settings hooks: `SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, PostToolUseFailure, PermissionRequest, Elicitation, ElicitationResult, Stop, StopFailure, SubagentStart, SubagentStop, SessionEnd` → `pod-hook.exe claude`. 매핑은 §3.3 Claude 표를 따름 | `hooks.json`: `SessionStart, UserPromptSubmit, PreToolUse, PermissionRequest, PostToolUse, Stop, Interrupt, SubagentStart, SubagentStop` → `pod-hook.exe codex`. **Codex는 이게 사실상 유일한 정확한 신호.** 실패 턴은 Stop이 없을 수 있으므로 `transcript_path` rollout 읽기로 보완 |
| **L2 osc** | OSC 7501 네이티브(추가 작업 없음). 질의 응답 필수, BEL/ST 둘 다 수신 | 없음(0.159 기준). Codex가 7501을 지원하면 자동으로 L2가 켜짐 |
| **L3 heuristic** | 타이틀 `✳ …`=idle, 스피너 글리프=working. 화면 패턴: 신뢰 대화상자("Quick safety check", "trust this folder"), 권한 프롬프트 → blocked | 타이틀 Braille 스피너(관찰: `⠋ work` → `⠋ Reply ok \| work` → 종료 시 `Reply ok \| work`) = working, 스피너 없음 = idle. 화면 패턴: `Trust this folder?`, 승인 프롬프트 → blocked. 출력 정지 + 스피너 없음 N초 → idle |
| **L0 process** | 포그라운드 `claude.exe`/`cmd.exe … claude.CMD`(shim 주의). 종료 = exited | `codex.exe`. `--no-daemon`이 아니면 실제 작업이 PTY 밖에서 돌 수 있음 → Pod가 띄울 때는 `--no-daemon`을 기본으로 |

**Pod 훅 전송(Windows)**: **named pipe `\\.\pipe\podd-<user>`에 JSON 한 줄**을 보낸다(로컬 HTTP 포트를 열지 않음). 라우팅은 PTY에 주입한 env(`POD_TERMINAL_ID`, `POD_HOOK_TOKEN`)로 한다. env가 없으면 즉시 `{}` 출력 후 종료한다(Pod 밖 실행에 무해). 훅 바이너리는 stdout에 `{}`만 출력해서 에이전트의 결정에 개입하지 않는다(승인 대행은 v2 이후).

### 4.4 Pod OSC 상태 규격 v1 (제안)

**결정: 새 번호를 만들지 않고 OSC 7501 문법을 그대로 채택한다.** Claude Code가 이미 이 형식을 보내므로, Pod가 같은 규격을 쓰면 같은 규격을 쓰는 도구들과 즉시 호환된다.

```
Report  := ESC "]7501;" Field *( ":" Field ) Term
Query   := ESC "]7501;?" Term
Reply   := ESC "]7501;?:states=idle,working,done,blocked,error:kinds=permission,question,auth" ESC "\"
Term    := BEL / ESC "\"            ; 수신은 둘 다, 송신은 BEL 권장(kitty류는 ST)
Field   := key "=" value            ; key=[a-z]{1,16}, value=[A-Za-z0-9_.,+/=-]*
```
| key | 필수 | 의미 |
|---|---|---|
| `state` | ✔ | `idle working done blocked error clear` |
| `app` | 권장 | 송신 도구 id(`[A-Za-z0-9_.+-]{1,32}`), 아이콘/어댑터 매칭에 사용 |
| `id` | | 하위 작업 레코드 경로(`a/b/c`, 세그먼트 ≤32자 × 최대 8). 없으면 루트 |
| `kind` | | blocked일 때 `permission question auth` |
| `progress` | | 0–100, working/blocked일 때 |
| `title` | | base64 UTF-8 ≤192B, 작업 제목 |
| `msg` | | base64 UTF-8 ≤2048B, 사람용 설명(질문 본문, 오류 이유, 완료 요약) |

수신 규칙(Pod 구현 체크리스트):
1. 질의에 응답하기 전에는 보고가 없을 수 있다. **PTY 입력 경로로 응답이 손상 없이 전달되어야 한다**(Windows: 번들 OpenConsole 권장, §1.4).
2. 잘못된 필드 하나 → 레코드 전체를 버린다. 알 수 없는 키는 무시한다.
3. 페이로드 ≤4087자. 청크 경계에서 쪼개진 시퀀스를 이어 붙인다.
4. 프로세스 종료 시 루트/레코드 상태는 **보존하되** `exited`로 표시한다. 다음 실행 시작 시 초기화한다.
5. Pod 확장 키는 `x` 접두어로 예약한다(`xpod...`는 key 문법상 소문자만 허용하므로 `xturn`, `xsession` 등). 다른 수신기는 무시한다.

다른 도구가 Pod에 상태를 보내는 최소 예:
```sh
printf '\033]7501;state=working:app=mytool:progress=30\007'
printf '\033]7501;state=blocked:app=mytool:kind=question:msg=%s\007' "$(printf 'Deploy to prod?' | base64)"
printf '\033]7501;state=done:app=mytool\007'
```

---

## 5. 측정 방법

- `rawcap.py`(pywinpty ConPTY)로 Claude Code·Codex를 직접 실행해 원시 출력 바이트와 타이밍을 캡처했다. 같은 방법으로 합성 송신기(`emit7501.py`)를 실행해 질의 응답과 필드 디코딩을 확인했다.
- 질의에 응답하는 터미널(번들 ConPTY) 안에서 Claude를 실행하고 attach 스트림을 캡처했다(§1.3 C·D). Claude 프롬프트 총 3회, 파일 편집 없음.
- Codex: 임시 `CODEX_HOME`(최소 config + auth.json 사본, 작업 후 삭제)으로 `-s read-only -a never` 실행(Codex 프롬프트 1회). 사용자 config는 바꾸지 않았다.
- 훅: 공개 문서의 이벤트 이름과 페이로드 필드를 기준으로 설계하고, 사용자 설정은 읽기만 했다.
- 캡처 파일(`raw_attach.log`, `raw_claude.log`, `raw_emit.log`, `emit7501.py`, `rawcap.py`)은 저장소 밖 임시 폴더에 있었다. 핵심 샘플은 §1.3에 옮겨 두었고, `crates/pod-status/tests/osc_parser.rs`가 바이트 그대로 재생한다.

---

## 6. 미확인 / 열린 질문

- [미확인] **기본 ConPTY에서 Claude 7501이 켜지지 않은 정확한 원인**: win32-input-mode가 응답 바이트를 바꿨는지, DA1(`ESC[c`) flush 응답이 없어 probe가 끝나지 않았는지. (해결: 번들 ConPTY, [03 §5](03-podd-p0.md))
- [미확인] Claude의 `kind=permission`(도구 권한 프롬프트)과 `kind=auth`, `error`, `progress`, `title`, 그리고 `id` 하위 레코드의 실측. 이번에는 auto mode라 권한 프롬프트가 뜨지 않았고 `question`만 관찰했다.
- [미확인] Codex의 OSC 9 알림 실제 송신 조건(`notification_condition` 기본값)과 바이트. 승인 대기 중 타이틀 변화(blocked를 타이틀로 구분할 수 있는지)도 미확인. Codex에서는 권한 프롬프트를 일으키지 않았다.
- [미확인] Codex 훅 페이로드의 실제 필드(`turn_id`, `transcript_path`, `agent_id`)는 실측하지 않았다.
- [미확인] Codex 신뢰 대화상자를 `-c projects.'<path>'.trust_level` 오버라이드로 넘기지 못한 이유(키 정규화 방식). → 해결: [07 §8](07-pod-status.md)의 인라인 테이블 방식.
- [열림] Pod가 Claude/Codex 전역 설정에 훅을 설치할지, `CLAUDE_CONFIG_DIR`/`CODEX_HOME` 별도 프로필을 쓸지. → 결정: 전역 설치 + env 가드([01 §7](01-plan.md)).

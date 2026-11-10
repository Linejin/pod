# 세션 찾기: 이 PC의 에이전트 세션을 Pod에 등록

작성일: 2026-11-10 · 코드: `crates/pod-discover`, `crates/podd/src/server/discovery.rs`·`discovery_protocol.rs`, `crates/pod-cli/src/sessions.rs`, `crates/pod-app/src/sessions_ui.rs` · 관련: [03 (podd 프로토콜)](03-podd-p0.md), [08 (저장소·작업)](08-tasks.md), [04](04-status-detection-research.md)·[07 (상태)](07-pod-status.md)

Pod 밖에서 시작한 Claude Code·Codex 세션(지난 세션과 지금 도는 세션)을 찾아 "발견된 세션" 목록에 보여 주고, Pod 저장소·작업에 연결하고, Pod 터미널에서 이어서 실행(재개)할 수 있게 한다.

---

## 1. 결정

| 항목 | 결정 | 이유 |
|---|---|---|
| 기본값 | **꺼짐**. "발견된 세션" 화면의 설명과 "세션 찾기 켜기" 버튼, 또는 `pod sessions scan --enable`로 켠다 | 다른 앱의 대화 기록(개인 데이터)을 읽는 기능이다. 사용자가 무엇을 읽는지 보고 직접 켜야 한다. 꺼져 있으면 디스크도 프로세스도 읽지 않는다 |
| 첫 프롬프트 미리보기 | 켜면 기본 **켜짐**, 화면·`pod sessions settings --preview off`로 끈다 | 세션을 알아보는 데 가장 쓸모 있는 정보다. 끄면 저장된 요약도 바로 지운다(화면 공유 대비) |
| 접근 | **읽기 전용**. 에이전트 파일을 고치거나 옮기거나 지우지 않고, 에이전트 설정도 쓰지 않는다. 네트워크 없음 | 남의 데이터 저장소다 |
| 저장 | `<state_dir>\sessions\sessions.json`(설정·숨김·연결)과 `cache.json`(스캔 캐시). 끄면 `cache.json`과 메모리의 결과를 지우고 숨김·연결만 남긴다. 요약은 이 PC의 podd 상태에만 있다. 요약이 들어갈 수 있는 파일은 `cache.json`, `cache.json.tmp`, 옛 버전의 `cache.corrupt-*.json`뿐이고, 끄면 셋 다 지운다. 쓰기에 실패한 임시 파일은 바로 지우고, 깨진 `cache.json`은 옮기지 않고 지운다(다음 스캔이 다시 만든다). 디스크 반영(설정 저장·캐시 삭제)이 실패하면 메모리는 이미 꺼짐/비공개로 바뀐 채 `-32043 PERSIST_FAILED`(`data = {paths, error}`)로 알리고, 다음 `sessions.*` 호출과 podd 시작 때 다시 시도한다(security-04) | 다른 상태 파일과 같은 규칙(임시 파일 → rename, 깨진 `sessions.json`은 `.corrupt-<ms>`로 옮김) |
| 범위(v1) | Claude Code, Codex CLI. `Source` 트레이트로 추가 가능 | 나중: Gemini CLI, Aider, OpenCode 등 |
| 재개 | 에이전트 자신의 재개 명령을 **세션의 cwd에서 새 Pod 터미널**로 실행 | 세션 기록은 에이전트가 소유한다. Pod는 대화 내용을 옮기지 않는다 |

## 2. 출처 (관측한 로컬 형식, 2026-11 기준)

형식은 공개 문서가 아니므로 언제든 바뀐다. 파서는 모르는 줄·필드·깨진 줄을 건너뛰고, 아무것도 못 읽으면 그 파일을 빼며 **절대 패닉하지 않는다**.

| 에이전트 | 지난 세션 | 실행 중 |
|---|---|---|
| Claude Code (`CLAUDE_CONFIG_DIR`, 기본 `%USERPROFILE%\.claude`) | `projects\<인코딩된 cwd>\<세션 UUID>.jsonl`. 줄마다 JSON. `sessionId`·`cwd`·`gitBranch`·`version`·`timestamp`, `type:"user"`의 첫 사용자 메시지, `message.model`, `type:"ai-title"`/`"summary"`(제목). 하위 폴더(서브에이전트 기록)는 보지 않는다 | `sessions\<pid>.json`(`pid`·`sessionId`·`cwd`). pid가 살아 있고 프로세스 이름이 맞을 때만 믿는다 |
| Codex CLI (`CODEX_HOME`, 기본 `%USERPROFILE%\.codex`) | `sessions\YYYY\MM\DD\rollout-*.jsonl`. 첫 줄 `session_meta`(id·cwd·timestamp·cli_version·git.branch·source), `turn_context.model`, 첫 `user_message`. 제목은 `session_index.jsonl`(`id`·`thread_name`) | 명령줄에 `resume <UUID>`가 있으면 그 세션. 없으면 같은 cwd에서 프로세스 시작 뒤 수정된 가장 최근 세션(추정, `guessed`) |

시험용으로 `POD_DISCOVER_CLAUDE_DIR`·`POD_DISCOVER_CODEX_DIR`가 위치를 덮어쓰고, `POD_DISCOVER_PROCESSES=off`가 프로세스 스캔을 끈다.

**실행 중인 프로세스**: Toolhelp 스냅샷으로 pid·부모·실행 파일 이름을 얻고, 후보(`claude.exe`, `codex.exe`, 명령줄이 에이전트인 `node.exe`)만 명령줄(`NtQueryInformationProcess`)과 cwd(PEB 읽기, 같은 사용자 프로세스만)를 읽는다. `codex app-server`·`mcp` 같은 비대화형 하위 명령은 뺀다. 같은 에이전트의 부모·자식 프로세스(셰임 → 실제 바이너리)는 가장 바깥 하나로 합친다. 실행 파일 이름이 같은 데스크톱 앱(Store 설치 `WindowsApps` 경로, Electron 보조 프로세스의 `--type=`)은 세션이 아니다. 부모 pid가 자식보다 늦게 만들어졌으면 재사용된 pid로 보고 사슬을 끊는다.

**Pod 터미널과 중복 제거**: 조상 사슬에 Pod 터미널의 자식 pid나 holder pid가 있으면 그 Pod 터미널 안의 세션이다(`pod_terminal`). 목록에는 "실행 중 (Pod)"으로 보이고 해당 터미널로 이동만 한다. 그 밖의 실행 중 세션은 "실행 중 (다른 터미널)"이다. 그 PTY는 다른 앱 소유라 Pod가 넘겨받을 수 없다. 읽기 전용으로 보여 주고, 끝난 뒤에만 재개를 연다.

## 3. 상태

| 상태 | 조건 | 동작 |
|---|---|---|
| 실행 중 (Pod) | Pod 터미널 안에서 실행 중 | 터미널 열기 |
| 실행 중 (다른 터미널) | Pod 밖에서 실행 중 | 재개 막음(설명 표시) |
| 재개 가능 | cwd가 있고 마지막 활동이 30일 이내 | 재개·작업으로 등록·숨기기 |
| 오래됨 | 30일 넘음, 또는 cwd 폴더가 없음 | 재개 가능(폴더 없으면 막음)·숨기기 |

## 4. 등록·재개

- **자동 연결**: cwd를 등록된 작업 worktree(가장 긴 일치)와 저장소 경로에 맞춰 본다(대소문자·구분자·끝 `\` 무시, 경로 요소 경계). 결과는 `match = {repo?, task?}`.
- **작업으로 등록** (`sessions.register`): cwd가 작업 worktree면 그 작업에, 저장소면 그 저장소에 세션을 연결한다. 저장소를 모르면 화면이 "저장소 등록"을 제안하고(`add_repo: true`) git 최상위(linked worktree면 주 저장소)를 `repo.add`로 등록한 뒤 연결한다. 저장소 이름은 폴더 이름에서 허용되지 않는 글자를 `-`로 바꾸고, 이미 있으면 `-2`, `-3`…을 붙인다. 새 worktree를 만들지는 않는다: 에이전트는 세션을 **원래 cwd**에서만 재개하기 때문이다.
- **재개** (`sessions.resume`): 설치된 CLI의 `--help`로 확인한 명령만 쓴다(Claude Code 2.1.319, codex-cli 0.159.0).
  - Claude: `claude --resume <UUID>`
  - Codex: `codex --no-daemon -c check_for_update_on_startup=false resume <UUID>` (작업 에이전트와 같은 플래그. `--no-daemon`: 작업이 PTY 안에서 돌게. 폴더 신뢰는 주지 않는다)
  - **세션 cwd가 Pod 작업 worktree의 루트**면 `task.resume {task, session, agent}`([19](19-worktree-sessions-orchestration.md))로 넘긴다: 그 작업의 에이전트 터미널이 되고, 작업에 `agent_session`(출처 `discovery`)이 기록되며, Codex는 그 worktree에 한해 신뢰를 받는다. 하위 폴더에서 돈 세션은 에이전트가 cwd별로 세션을 찾으므로 아래처럼 그 폴더에서 실행한다.
  - 세션 id는 UUID 문자(16진수와 `-`)만 받는다. 실행 파일은 `POD_SESSIONS_CLAUDE_EXE`/`POD_SESSIONS_CODEX_EXE`로 바꿀 수 있다(시험용 가짜 실행 파일).
  - 그 밖에는 연결된(없으면 일치한) 작업이 있으면 그 작업의 터미널로(`terminal.run{task}`), cwd는 세션 cwd. 실행 중이면 `SESSION_RUNNING`, cwd가 없으면 `INVALID_STATE`.
- **숨기기** (`sessions.hide`): 목록에서만 뺀다(`include_hidden`으로 보임). 파일은 그대로.

## 5. 한계 (스캔은 낮은 우선순위·증분)

- 출처마다 mtime 최신순 **2000개** 파일까지(목록은 5만 항목까지만 훑음), 파일마다 앞 **256 KiB/400줄**과 끝 **128 KiB**만 읽는다.
- 스캔은 두 단계다. (1) 메타데이터: 출처마다 전체 예산의 같은 몫(절반을 나눔)을 써서 큰 저장소 하나가 다른 출처를 굶기지 않는다. (2) 메시지 수(대략): 남은 절반으로 최신 파일부터 파일 전체를 센다. **16 MiB** 넘는 파일은 세지 않는다. 못 센 파일은 다음 스캔에서 이어서 센다.
- 스캔 한 번에 **10초**, **128 MiB** 읽기 예산. 넘으면 남은 파일은 다음 스캔으로 미루고(이전 결과는 유지) `truncated`를 표시한다. 이 PC 실측(Claude 36개, Codex 34개, 수백 MB): 첫 스캔 약 5초, 이후 증분 스캔은 바뀐 파일만 읽는다.
- (경로, 크기, mtime)이 같은 파일은 캐시를 쓴다. 스캔 스레드는 Windows 백그라운드 모드(CPU·I/O 낮은 우선순위)로 돈다. 프로세스 목록은 약 600개 프로세스에 30 ms 남짓.
- 켜져 있으면 데몬 시작 5초 뒤 한 번, 그 뒤 lifecycle 구독자가 있을 때 2분마다 다시 스캔한다. 목록·실행 수가 바뀌면 `sessions.changed`를 보낸다.

## 6. 프로토콜 (v15, `sessions.*`, 추가형)

| 메서드 | params | result |
|---|---|---|
| `sessions.list` | `{include_hidden?}` | `SessionsList = {enabled, preview, scanned_ms?, truncated, sessions: DiscoveredSession[], hidden, unknown_running: [{agent, pid, cwd?, pod_terminal?}]}` |
| `sessions.scan` | `{enable?}` | `SessionsList`. 꺼져 있고 `enable`이 없으면 `DISCOVERY_OFF` |
| `sessions.settings` | `{enabled?, preview?}` | `SessionsList` |
| `sessions.register` | `{session, task?, add_repo?}` | `DiscoveredSession` |
| `sessions.resume` | `{session, name?}` | `TerminalInfo` |
| `sessions.hide` | `{session, hidden?=true}` | `DiscoveredSession` |

`DiscoveredSession = {key, agent, id, cwd, title?, preview?, model?, branch?, version?, origin?, started_ms?, last_ms?, messages?, state, running?: {pid, pod_terminal?, guessed}, match?: {repo?, task?}, link?: {repo?, task?}, hidden, cwd_exists, resume_argv}`. `key = "<agent>:<id>"`. `state`: `running_pod | running_other | resumable | stale`.

알림 `sessions.changed {seq, enabled, count, running}`(`status.subscribe`, `lifecycle: true`). 새 오류 코드: `-32040 SESSION_NOT_FOUND`, `-32041 SESSION_RUNNING`, `-32042 DISCOVERY_OFF`, `-32043 PERSIST_FAILED`(변경은 적용됐지만 디스크 반영 실패, 위 §1 저장).

CLI: `pod sessions list [--all] [--json]`, `pod sessions scan [--enable]`, `pod sessions settings [--enabled on|off] [--preview on|off]`, `pod sessions register <key> [--task T] [--add-repo]`, `pod sessions resume <key>`, `pod sessions hide <key> [--show]`. `<key>`는 `claude:<id>`, `codex:<id>` 또는 유일한 id 앞부분.

## 7. 앱

명령 팔레트 "발견된 세션"이 가운데 독에 페이지를 연다(`sessions_ui.rs`, 기존 pod-ui 컴포넌트와 토큰만 사용). 꺼져 있으면 설명과 "세션 찾기 켜기", 켜져 있으면 미리보기 토글·다시 찾기·세션 카드(제목/미리보기, 에이전트·cwd·브랜치·마지막 활동·메시지 수, 상태 칩, 연결된 작업)와 "이 세션 재개"·"작업으로 등록"(또는 "저장소 등록")·"숨기기". 다른 터미널에서 실행 중이면 버튼 대신 "다른 앱의 터미널에서 실행 중입니다. 그 세션이 끝나면 여기서 재개할 수 있습니다."를 보여 준다. 버튼마다 접근성 이름(세션 제목 포함)을 붙인다.

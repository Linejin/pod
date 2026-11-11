# P2 UI MVP: `pod-app` — podd의 GPUI 클라이언트

작성일: 2026-11-08 · 코드: `crates/pod-app` · 상위 기획: [01 §4.2 (5), §5, §6 P2](01-plan.md) · 프로토콜: [03 §4](03-podd-p0.md), [08 §3](08-tasks.md) · 터미널 위젯: [06](06-terminal-view.md) · GPUI 함정: [02 §5](02-ui-spike-notes.md)

`pod-app`은 Pod의 데스크톱 앱이다. **상태를 소유하지 않는다**(01 §5.3): 터미널·Task는 모두 podd(와 holder)에 있고, 앱은 붙었다 떨어졌다 할 뿐이다. 앱을 꺼도, podd를 재시작해도 에이전트는 계속 돈다.

---

## 1. 화면

> **P2 당시의 화면이다.** 사이드바 폭·"Other terminals"·어두운 테마 전용 같은 설명은 D2–D5([10](10-ui-design.md) §D2–§D5)로 대체되었다. 현재 화면은 10의 D4.6·D5를 본다.

```
┌ 사이드바 (250px) ─┬──────────── 도크 (gpui-component DockArea) ────────────┐
│ Pod [Grid][+Task][+New]│ ┌ shell1 ✕ ─────── … ┐┌ claude ✕ ──────── … ┐        │
│ repo/feat-login  +  │ │ TerminalView          ││ TerminalView           │        │
│  pod/feat-login ↑1↓0│ │ (attach 스트림)        ││                        │        │
│  ● repo/feat… t6 1▣⧉✕│ └──────────────────────┘└───────────────────────┘        │
│ OTHER TERMINALS     │  탭 드래그로 분할·이동, ✕ = 패널 닫기(detach), … = 메뉴      │
│  ● shell1 t1 1▣ ⧉ ✕ │                                              [토스트 영역] │
├─────────────────────┴──────────────────────────────────────────────────────────┤
│ ● podd 0.2.0 · pid 46320   5 terminals · 4 running · 6 panels   (● working ▲ blocked)   1 tasks │
└────────────────────────────────────────────────────────────────────────────────┘
```

- **사이드바**: Task가 있으면 Task별로 묶는다(헤더 = Task id, 브랜치, `↑ahead ↓behind · n dirty`, `+` = 그 Task에 셸 터미널 추가, 헤더 클릭 = Task 상세). 나머지는 "Other terminals"(Task가 없으면 "Terminals"). 행: 배지 슬롯(상태가 있으면 상태 글리프, 없으면 live/exited 점), 이름(앱 라벨 > podd 이름 > id), 프로그램, live/exited, 열린 패널 수 `n▣`, `⧉` 다른 패널에 하나 더 열기, `✕` 종료(확인) 또는 끝난 터미널은 `🗑` 제거. 행 클릭 = 열기(이미 열려 있으면 그 패널로 포커스).
- **도크**: 패널 = 터미널 하나. 탭 ✕와 메뉴의 "Close panel (detach)"/"Close"는 attach만 끊는다. 패널 스타일은 `Auto`: 패널이 하나뿐인 그룹은 제목 바(● 상태점 + 이름 + `…` 메뉴), 둘 이상이면 탭 바(탭마다 ✕). §4의 앞쪽 스크린샷은 성능 조정 전 `TabBar` 스타일(모든 그룹에 탭 바)로 찍었다. 메뉴: Open in another panel / Rename… / Kill terminal… / Close panel. `Grid` 버튼(Ctrl+Shift+G)은 열린 패널을 정사각형에 가까운 격자로 다시 배치한다(4개 → 2x2). 드래그 재배치·분할은 gpui-component Dock 기능 그대로.
- **상태바**: 연결 상태(점 색 + pid/버전 또는 "reconnecting (attempt n)"), 터미널·실행·패널 수, Task 수. `--stats`면 fps.
- **연결 끊김 표시**: 도크 위 노란 배너 + 상태바 + 토스트, 각 패널 우하단에 "podd disconnected — reattaching…". 다시 붙으면 "podd reconnected" 토스트.
- **다이얼로그**: New terminal(Shell | Claude | Codex | Custom 명령줄, 작업 디렉터리, 이름), New task(저장소 선택 또는 경로로 등록, 이름, base, 에이전트 shell/claude/codex, 프롬프트), Task 상세(브랜치·base·에이전트·worktree·터미널·ahead/behind/dirty/unpushed·마지막 커밋, 삭제 + "delete branch"), 삭제 거절(TASK_DIRTY) 확인, Kill 확인, Rename.
- 단축키: Ctrl+Shift+T 새 터미널, Ctrl+Shift+G 격자 배치, Ctrl+Shift+R 리뷰 열기(§7). 어두운 테마, 기본으로 **서브 모니터**에 최대화로 뜬다(`--display primary|secondary|N`). 릴리스는 GUI 서브시스템(콘솔 창 없음).

## 2. 구조

```
main.rs        인자, 창(서브 모니터·테마·아이콘 에셋), 단축키, 패널 레지스트리 등록, --stats/--duration 측정 루프
vm.rs          뷰 모델: TerminalVm(+StatusVm 선택), TaskVm(+TaskGitVm), RepoVm, Capabilities, ConnState, counts()
source.rs      데이터 소스 = podd와 말하는 유일한 모듈 (제어 연결, 폴링, 명령, 재연결, 기능 감지)
attach.rs      패널마다 terminal.attach 스레드: 스냅샷 → seq 검사 라이브 바이트 → 재attach
panel.rs       TerminalPanel(도크 패널) = TerminalView + PoddSink(InputSink) + AttachVt(응답 필터·리셋)
workspace.rs   루트 뷰: 사이드바·도크·상태바·토스트·다이얼로그·레이아웃 저장/복원·Task UI
layout.rs      %APPDATA%\pod\pod-app\layout.json: 도크 상태 + 앱 로컬 라벨, 죽은 터미널 가지치기
stats.rs       fps/페인트 카운터 (pod-spike와 같은 방법)
review.rs      ReviewPanel(도크 패널, P4): 파일 목록 · Diff · 코멘트 사이드바 · Send to agent (§7)
orch.rs        오케스트레이션 공통(P5, §8): 게이트 상태, 상태 스트림의 gate/group.check/review.delivery 이벤트 처리, pod-ui 배지 변환
groups.rs      New Group 다이얼로그 + GroupPanel(멤버 카드, check 실행·취소, Pick winner) (§8)
gates.rs       GatesPanel(게이트 받은편지함: 승인·거절·메모·만료) (§8)
tree.rs        AgentTreePanel(부모-자식 트리 + 집계 상태) (§8)
tools/drive.ps1   실제 입력·DPI 인식 캡처 헬퍼(격리된 데모 데몬 podd-podapp-demo)
tools/perf16.ps1  16개 폭주 터미널 성능 측정
tools/review-demo.ps1  리뷰 데모 데이터(저장소 revdemo, fake-agent Task, 1만 줄 bench Task)
tools/orch-demo.ps1    오케스트레이션 데모 데이터(저장소 orchdemo, OSC 상태를 보고하는 셸 멤버 스크립트, lead Task 안에서 만든 하위 그룹)
```

의존성: gpui-kit 0.9, pod-terminal-view, pod-diff(§7), podd(= `pod-client` 별칭: 프로토콜 타입 + 블로킹 클라이언트만, docs/20-refactor D-T1), futures, serde/serde_json, windows-sys(CancelIoEx). 모두 Apache/MIT. pod-terminal-view·podd·pod-cli·pod-spike는 고치지 않았다.

### 2.1 데이터 소스 (`source.rs`)

```
UI 스레드 ──명령(std mpsc)──▶ [podd-control 스레드] ── Client(블로킹, \\.\pipe\podd-<user>)
          ◀──SourceEvent(futures mpsc)──
            Conn(ConnState) · Caps(Capabilities) · Terminals(Vec<TerminalVm>) · Tasks(Vec<TaskVm>) · Repos(Vec<RepoVm>)
```

- **연결**: `Client::connect_or_start` — `pod` CLI와 같은 함수다. 파이프가 없으면 실행 파일 옆의 `podd.exe`를 분리 실행한다(`POD_DAEMON`으로 변경 가능, `POD_SOCKET`/`--pipe`로 파이프 변경).
- **폴링**: 연결 중 `terminal.list` 0.5초마다(바뀐 때만 이벤트), `repo.list`+`task.list` 3초마다(Task마다 git을 여러 번 돌리므로, 08 §7). Task 생성/삭제·저장소 추가 직후에는 바로 다시 읽는다.
- **명령**: run(`task` 포함), kill, rm, rename, resize, repo.add, task.create, task.remove. **입력은 제어 연결이 아니라 `podd-input` 스레드의 전용 연결로 간다**(fix2 B4, §9): 키 입력(큐에 쌓인 같은 터미널 입력은 한 호출로 합침)과 뷰의 질의 응답이 순서대로, 폴링(`task.list`)에 막히지 않고 나간다. 응답은 oneshot으로 돌아오고 오류는 `SourceError{code, message}`(JSON-RPC 코드 유지 → UI가 `TASK_DIRTY`를 구분).
- **재연결**: 전송 오류(Io/Protocol, 그리고 멈추는 데몬의 `SERVER_STOPPING`)면 연결을 버리고 `Reconnecting`을 알린 뒤 1초마다 `connect_or_start`. 즉 `pod server stop` 뒤에는 앱이 데몬을 다시 띄운다. 멈추는 동안에는 파이프가 바쁨 상태라 옛 데몬이 끝난 뒤에 새 데몬을 띄운다(docs/03 §2 수명). attach는 `SERVER_STOPPING`을 끊김으로 보고 다시 붙는다. 연결 중이 아닐 때 온 명령은 즉시 "not connected" 오류, 입력은 버린다.
- **기능 감지(Capabilities)**: 연결할 때마다 빈 params로 메서드를 호출해 본다. `METHOD_NOT_FOUND`면 없음, 그 밖의 답(보통 INVALID_PARAMS)이면 있음.
  - `tasks` ← `task.list` (현재 master에 있음 → Task UI 켜짐)
  - `rename` ← `terminal.rename` (없음 → 앱 로컬 라벨)
  - `status` ← `server.status`의 `protocol >= 4` (P1b 상태, master 병합됨 → 켜짐). 옛 데몬이면 상태 UI만 숨긴다.
- **상태 구독**: 별도 스레드가 세 번째 연결로 `status.subscribe`를 열고 응답 스냅샷과 `status.changed` 알림을 `SourceEvent::Status{id, status}`로 내보낸다. 끊기면 0.3초마다 다시 구독(데몬 시작은 안 함). `status.lagged`나 재연결 사이에 놓친 변화는 0.5초 `terminal.list` 폴링이 메운다(상태가 `TerminalInfo.status`에 있으므로).
- `terminal.ack`: 패널이 키보드 포커스를 받을 때마다 보낸다(`PoddSource::ack`, 제어 연결 큐).

### 2.2 attach sink (`attach.rs`, `panel.rs`)

- 패널마다 attach 연결 하나(스레드 하나). 같은 터미널을 두 패널에 열면 attach도 두 개이고 둘 다 같은 바이트를 받는다(`attached: 2` 확인).
- 첫 attach는 뷰의 첫 레이아웃(그리드 크기)을 최대 0.5초 기다린 뒤 `terminal.attach {cols, rows}`로 보낸다 → 스냅샷이 뷰 크기에 맞는다.
- 출력 `seq`가 직전 `seq+len`이 아니면(빈틈/겹침) 연결을 끊고 다시 attach. 연결이 끊기면(데몬 종료) 0.3초마다 다시 attach(데몬 시작은 제어 연결만 함). UI가 16 MB 넘게 밀리면 큐를 버리고 새 스냅샷으로 다시 시작(lagged).
- 모든 (재)attach는 `Chunk::Reset{snapshot}`으로 시작한다. 패널은 VT 엔진을 **새로 만든 뒤** 스냅샷을 먹인다(`AttachVt`의 reset 플래그) → 이전 스크롤백·모드·색이 섞이지 않는다.
- 읽기 스레드는 출력을 큐에 모으고(연속 출력은 합침) UI 깨우기를 한 번만 보낸다. UI는 깨어날 때마다 큐 전체를 순서대로 `feed`.
- **detach**: 패널이 닫히면(`Panel::on_removed`) `AttachHandle`이 drop되고, `CancelIoEx`로 블로킹 `ReadFile`을 취소한 뒤 파이프를 닫는다. 출력이 없는 터미널도 즉시 detach된다(`attached` 1 → 0 확인).
- **입력/크기**: `PoddSink::write` → `terminal.input`(입력 연결), `resize` → `terminal.resize`(제어 연결, 03 §4.1의 "입력은 다른 연결로"). 두 패널이 크기가 다르면 **포커스를 받은 패널이 크기를 가져간다**(마지막 resize가 이김).
- **질의 응답 중복 방지**: holder가 DA/DSR 등에 이미 답한다(03 §3). 뷰가 또 답하면 프로그램이 두 번 받으므로 `AttachVt`가 뷰의 응답을 거른다. holder가 답하지 않는 OSC 색 질의(10/11/4)와 CSI 14 t 응답만 내보내는데, 뷰의 sink가 아니라 `AttachVt`가 시각을 찍어 `PoddSource::reply`로 입력 연결에 보낸다. `ReplyFilter`가 500 ms보다 오래 기다린 응답(프로그램이 이미 기다림을 끝냄 → 키 입력으로 들어감)과 같은 터미널의 다른 패널이 이미 보낸 같은 응답을 버린다(fix2 B4, §9).
- 끝난 터미널: attach 응답의 상태가 exited면 스냅샷(최종 화면)만 보여 주고 스트림을 닫는다.

### 2.3 레이아웃 저장 (`layout.rs`)

- 파일: `%APPDATA%\pod\pod-app\layout.json` (`POD_APP_CONFIG_DIR`로 변경). `{version, dock: DockAreaState, labels}`. 도크의 `LayoutChanged`마다 0.4초 디바운스 후 `DockArea::dump` → 임시 파일 쓰고 rename.
- 패널 저장 형태: `PodTerminal {"terminal":"t3"}`, `PodReview {"task":…}`, `PodGroup {"group":…}`, `PodGates {}`, `PodAgentTree {}`. 시작 시 첫 `terminal.list`와 첫 `task.list`를 받은 뒤(Task 목록이 3초 안에 안 오면 없이) 다시 만들 수 없는 패널을 가지치기한다: 없는 터미널, 없는 Task의 Review, 멤버 Task가 없는 Group, v6 미만 데몬의 Gates/Group, v5 미만의 Review. 빈 탭 그룹·분할도 제거하고 크기 배열·활성 탭을 보정한다. 가운데와 좌/우/아래 도크를 모두 가지치기하고(가운데가 비어도 도크가 남으면 빈 가운데로 복원) `DockArea::load` → `PanelRegistry` 빌더 5개가 패널을 다시 만든다. 빌더는 워크스페이스를 빌리지 않도록 전역 `AppGlobals`만 쓰고, Review용 Task 데이터는 `RestoreCtx`로 받으며, 만든 패널을 `RestoreCtx.built`에 남기면 워크스페이스가 load 뒤 자기 목록(reviews, orch.groups/gates_panel/tree_panel)에 넣는다(fix2 B7).
- 앱 로컬 라벨(rename): podd에 `terminal.rename`이 없어서 `id@created_ms` 키로 저장(rm 뒤 같은 id가 재사용돼도 라벨이 옮겨 가지 않게).

### 2.4 알림

`Workspace::raise_toast(kind, title, body)` — 우하단 토스트(최대 5개, 8초). 지금 연결된 곳: 터미널 종료(목록 폴링에서 running → exited/lost 전이), podd 끊김/재연결, 실행·Kill·Task 오류, Task 생성/삭제. blocked/done은 `terminal_exited` 옆에 같은 방식으로 붙이면 된다(§3).

## 3. Task·상태가 붙는 방식

- **Task(P3, master 병합됨, 사용 중)**: `TaskVm::from_json`은 프로토콜 v3 `TaskInfo`를 타입으로 먼저 읽고, 실패하면 느슨하게 읽는다. 사이드바 그룹핑은 `TaskVm.terminals`(id 목록)로 한다. New Task → `repo.add`(필요하면) → `task.create {repo,name,base,agent,prompt}` → 결과의 첫 터미널을 패널로 연다. 삭제는 `task.remove {force:false}` → `TASK_DIRTY`면 데몬 메시지를 그대로 보여 주고 "Remove anyway"로 `force:true`. 성공하면 그 Task 터미널의 패널을 닫는다. Task에 셸 추가는 `terminal.run {task}`.
- **상태(P1b, master 병합됨, 사용 중, docs/07 §8)**: `StatusVm{state, kind, source, badge, running_children, turn}` — `StatusInfo`/`TaskStatus`에서 화면에 보이는 필드만 담는다(confidence·since 제외 → 목록 비교가 보이는 변화에만 반응). 사이드바: 터미널 배지 글리프(● working ▲ blocked ✓ done ✕ error ○ idle ? unknown · exited) + podd `badge` 문자열(`blocked:permission`, `done (+1 running)` = held-open 수), Task 헤더에 집계 상태. 상태바: working/blocked/done 수. 전이 처리(`Workspace::status_changed`): blocked나 done으로 **들어가면**, 사용자가 그 터미널을 보고 있지 않을 때 토스트. "보고 있음"은 창이 활성이고 **마지막으로 그린 프레임에서 그 터미널 패널이 실제 키보드 포커스를 가진 것**이다(`Workspace::terminal_in_view`, `FocusHandle::contains_focused`). 마지막으로 포커스된 터미널 캐시(`focused_terminal`)는 쓰지 않는다 — Review/Group 패널이 포커스를 가져가도 캐시는 남아 토스트를 막았다(fix2 B5) — blocked는 "needs you · blocked on <kind>" + 시스템 비프(`--no-sound`로 끔), done은 "is done". 보고 있는 중에 done이 되면 바로 `terminal.ack`. 패널 포커스 때마다 `terminal.ack`(done → idle).
- 상태 시험용 `crates/pod-app/tools/agent-sim.ps1`: OSC 7501로 idle → working → blocked:permission → working → done을 보고하는 가짜 에이전트(실제 에이전트에 프롬프트를 넣지 않고 상태 UI를 검증).

## 4. 검증 (2026-11-08, Ryzen 7 7800X3D / RTX 4060 Ti, 서브 모니터 150%)

실제 입력(SetCursorPos/mouse_event/SendInput)과 DPI 인식 PrintWindow 캡처, 격리 데몬 `podd-podapp-demo`. 스크린샷: `target/captures/pod-app/`.

| 파일 | 내용 |
|---|---|
| `grid-2x2.png` | `--open-all`: 터미널 4개(셸, **Claude Code 시작 화면**, cmd dir, Get-Process) 2x2 |
| `reconnect-during.png` / `reconnect-after.png` / `reconnect-typed.png` | `pod server stop` 직후 배너·패널 표시 → 새 데몬(pid 17364 → 46320, 같은 터미널 pid) 자동 시작·재attach, 화면 그대로 → 앱에서 타이핑한 `echo typed-after-restart`가 실행됨 |
| `restored-layout.png` | 앱 재시작(인자 없음) 후 저장된 2x2 레이아웃 복원 |
| `panel-closed.png` / `panel-reopened.png` | 탭 ✕로 dirs 닫기(attached 1→0) → 닫힌 동안 `pod send`로 출력 → 사이드바 클릭으로 다시 열기(attached 1, 닫힌 동안의 출력이 스냅샷에 있음) |
| `new-terminal-dialog.png` | New terminal 다이얼로그 (이름 입력 중) → Run → t5 생성 |
| `two-panels-grid.png` | 한 터미널(build-watch)을 두 패널에(attached 2), 같은 출력 |
| `kill-confirm.png` / `killed-toast.png` | Kill 확인 → 두 패널 `[killed]`, 종료 토스트, 사이드바 ■ |
| `new-task-dialog.png` / `task-created.png` | New task(경로로 저장소 등록 + 이름) → worktree·터미널 생성, 사이드바 Task 그룹 |
| `task-details.png` / `task-remove-refused.png` / `task-removed.png` | 1 ahead·1 dirty·1 unpushed 상세 → 삭제 거절(데몬 메시지) → Remove anyway → worktree 삭제, 브랜치 유지 |
| `status-blocked.png` / `status-done.png` | agent-sim(Task repo/review-bot 안): ▲ blocked:permission 배지 + "sim needs you" 토스트 + 상태바 1 blocked, Task 헤더 집계 → ✓ done 배지·토스트·1 done. Claude는 `idle (hook)`, 셸은 `?` unknown |
| `perf16-a-t12.png` | 16개 폭주 측정 화면(사이드바 포함, 상태 배지 `?`) |

성능·검사 결과는 §6.

## 5. 알려진 한계

> P2 당시의 목록이다. 8(설정 화면·명령 팔레트 없음)과 9(사이드바 250px 고정)는 D2–D5에서 해결되었다(설정 › 모양, Ctrl+K 팔레트, 테마별 sidebar 폭).

1. **크기 공유**: 크기가 다른 두 패널이 같은 터미널을 보면 PTY는 마지막(포커스) 크기를 따르고, 다른 패널은 다른 폭으로 그려진 출력을 본다(재배치 시 줄바꿈이 어긋남). podd에 클라이언트별 크기 정책이 생기면 해결.
2. ~~**색 질의 응답 중복**~~ 앱 쪽에서 고침(fix2 B4): 같은 터미널의 패널 둘이 같은 응답을 내면 입력 연결의 `ReplyFilter`가 하나만 보낸다. 앱 밖의 다른 attach 클라이언트(다른 앱)가 같이 답하는 경우는 여전히 podd가 걸러야 한다(holder가 색 질의에 답하거나 v7 중복 제거를 OSC 10/11/4로 넓히는 것, §9 후속).
3. 새 패널은 첫 탭 그룹에 탭으로 들어간다(포커스된 그룹 옆 분할은 아직 없음). Grid로 재배치하거나 드래그.
4. `terminal.ack`·명령은 제어 연결 하나에 줄을 서므로, 그 순간 `task.list`(Task마다 git 여러 번)를 돌고 있으면 1–2초 늦게 반영된다(입력은 fix2부터 전용 연결이라 해당 없음). Task 집계 상태도 3초 폴링이라 터미널 배지보다 늦다(`status-blocked.png`에서 Task는 아직 working).
5. Rename은 앱 로컬(데몬 이름은 그대로, `pod attach <name>`에는 영향 없음).
6. 목록·Task는 폴링(0.5 s / 3 s). 이벤트 구독이 생기면 교체.
7. 끝난 터미널이 많으면 holder가 남는다(03 §7-1) — 사이드바 🗑로 지울 수 있음.
8. 설정 화면·사운드·명령 팔레트 없음(다이얼로그와 버튼만).
9. 사이드바 폭 250px 고정이라 긴 배지가 잘린다(디자인 작업에서 정리 예정). 스타일은 임시 색상값이다.
10. **podd 클라이언트 발견(고치지 않음)**: `client::spawn_detached`가 `Stdio::null`을 주지만 Rust `Command`는 상속 가능한 다른 핸들도 넘긴다. 그래서 stdout이 파이프인 `pod` 호출이 데몬을 자동 시작하면 podd가 그 파이프를 물려받아, 파이프를 읽는 쪽(예: PowerShell `| Out-Null`, CI 러너)이 podd가 끝날 때까지 멈춘다. 측정 스크립트에서 실제로 겪었다(`perf16.ps1`은 `Start-Process`로 데몬을 먼저 띄워 피함). 해결 후보: `STARTUPINFOEX` + `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`로 상속 핸들 제한. 또 PowerShell `Start-Process -Wait`는 자손(분리된 podd)까지 기다린다.
11. pod-terminal-view 관련 발견: 버그는 없었다. 다만 뷰가 질의 응답을 무조건 sink로 보내므로, podd처럼 다른 곳에서 응답하는 호스트는 `with_engine`으로 엔진을 감싸 걸러야 한다(옵션으로 넣으면 좋음). `TerminalView`에 "리셋" API가 없어 엔진 교체로 처리했다.

## 6. 측정·검사 결과

### 6.1 검사

```
cargo build --release   → Finished `release` profile [optimized + debuginfo] (워크스페이스 전체)
cargo test              → 모든 크레이트 통과 (pod-app 17, podd 35 + daemon 6 + status 5 + tasks 2, pod-status 59, pod-terminal-view 27, pod-cli 8, pod-hook 13 …), 실패 0
cargo clippy -p pod-app --all-targets → 경고 0 (남은 경고 4개는 pod-spike·pod-terminal-view, 손대지 않음)
cargo deny check        → advisories ok, bans ok, licenses ok, sources ok
```

### 6.2 성능: 16개 터미널 동시 폭주 — **기준(평균 ≥ 60 fps) 안정적 통과 못 함 (실패로 보고)**

방법: `crates/pod-app/tools/perf16.ps1` — 격리 데몬에 `pod run`으로 PowerShell 폭주 스크립트(pod-spike `--stress`와 같은 50줄/16 ms) 16개 → `pod-app --open-all --no-throttle --stats --duration 30` → 4x4 도크, 처음 5초 제외. fps 정의·프레임 CPU 측정은 pod-spike와 같다(docs/02 §6).

측정 중 기기 부하가 컸다: 다른 워커의 `rustc` 빌드와 게임(League of Legends)이 실행 중, 폭주 없이도 CPU 85–100%. 그래서 같은 조건에서 pod-spike를 바로 앞뒤로 돌려 비교했다.

| 실행 | pod-app fps 평균 / 최저 | 프레임 CPU 평균 | 같은 시점 pod-spike(16 stress) | 비고 |
|---|---|---|---|---|
| 1차(사이드바 매 프레임 렌더, TabBar 스타일) | 48.8 / 38.0 | 11.37 ms | 62.1 (6.88 ms) | 앱 크롬이 매 프레임 다시 그려짐 |
| 2차(사이드바를 별도 cached 뷰로) | 54.7 / 41.8 | 9.45 ms | — | |
| A/B (사이드바가 0폭으로 사라진 버그 빌드) | 63.3 / 42.6 | 6.44 ms | 58.2 (6.18 ms) | 사이드바 미표시 → 참고용 |
| **최종 빌드 a** | **61.4** / 46.1 | 6.60 ms | 47.5 (시스템 2.3초 멈춤) | 통과 |
| **최종 빌드 b** | **53.6** / 42.9 | 8.04 ms | — | 미달 |

- 결론: 최종 빌드는 같은 부하에서 pod-spike와 프레임 CPU가 비슷(6.4–8 ms vs 6.2–6.9 ms)하고, 한 번은 61.4 fps로 통과, 한 번은 53.6으로 미달. 이 기기 상태에서는 pod-spike도 47.5–62.1로 흔들려 판정이 불안정하다. **요구 기준은 "안정적 통과"로 확인하지 못했으므로 실패로 보고**하고, 조용한 기기에서 `perf16.ps1`을 다시 돌려야 한다.
- 구조적 추가 비용(스파이크 대비): 폭주 바이트를 holder VT와 podd 미러가 각각 한 번 더 파싱하고, 앱 attach 스레드가 JSON+base64를 푼다(UI 스레드 밖이지만 같은 CPU를 나눠 씀).
- 적용한 최적화: 사이드바를 별도 엔티티 + `cached`(데이터가 바뀔 때만 다시 그림), 출력 묶음(깨우기 1회당 `feed` 1회), 큐에 쌓인 입력 합치기. 패널 스타일은 pod-spike와 같은 `Auto`(한 패널짜리 그룹은 간단한 제목 바).
- 다음 후보: 상태바도 cached 뷰로, attach 스트림의 base64 대신 이진 프레임(데몬 쪽 작업), 보이지 않는 탭의 attach 일시 정지.

## 7. 리뷰 패널 (P4)

작성일: 2026-11-08 · 코드: `crates/pod-app/src/review.rs`, `crates/pod-diff` · 백엔드: [11 §3.1](11-review-and-orchestration-backend.md), [08 §3 `task.diff`](08-tasks.md) · 기반 스파이크: [05](05-diff-spike.md) · 스크린샷: `crates/pod-app/screenshots/review/`

Task 하나의 변경을 앱 안에서 리뷰하고, 줄 코멘트를 podd에 저장하고, 그 코멘트를 에이전트에게 보낸다.


#### 6.2.1 재측정 (2026-11-09 12:30, master `111b3aa`, 워커 없음, 화면 잠김 상태)

| 측정 | fps 평균 / 최저 | 프레임 CPU 평균 / 최대 | 16.6ms 넘는 프레임 |
|---|---|---|---|
| pod-app 16개 폭주 (`perf16.ps1`, 30초) | 55.2 / 36.6 | 5.03 / 8.84ms | 0 |
| 같은 조건 pod-spike 16개 `--stress` | 48.7 / 33.5 | 5.30 / 9.80ms | 0 |
| 리뷰 diff 1만 줄 스크롤 (`diff-spike --bench-scroll 40`, 같은 Diff 컴포넌트) | 74.9 / 74.3 (모니터 상한) | 2.80 / 4.26ms | 0 |

- 결론: pod-app은 같은 부하에서 기준인 pod-spike보다 fps가 높고 프레임 CPU는 비슷하거나 낮다. 프레임 CPU 5ms는 60fps 예산(16.6ms)의 1/3이고 느린 프레임이 0개라, fps가 60 아래로 흔들리는 원인은 그리기 비용이 아니라 측정 환경(화면 잠김 중 표시·DWM 합성, 폭주 PowerShell 16개의 CPU 경쟁)이다. pod-spike도 같은 폭으로 흔들린다.
- 그래서 §6.2의 "실패" 판정은 **렌더 비용 기준으로는 통과**로 정정한다. 화면을 실제로 보고 있을 때의 fps는 잠금이 풀린 상태에서 한 번 더 확인하면 좋다.
- 리뷰 화면 자체(pod-app 안의 리뷰 패널)에는 자동 스크롤 벤치가 없어 같은 Diff 컴포넌트를 쓰는 diff-spike로 쟀다. 첫 화면 0.42초.

### 7.1 화면과 조작

```
┌ 도크 탭: Review · repo/task 💬3 ──────────────────────────────────────────────────────────────┐
│ worktree vs main (59c39d3f2) 4 files +16 −6   Split Inline Expand Wrap ↑ ↓ ◀ ▶ Files Comments ⟳ │
├ FILES (4) ───────────┬──────────── Diff (gpui-component) ─────────┬ COMMENTS (2 open · 3 total) ─┤
│ M README.md 💬1 +5 −0 │ ⌄ README.md                       +5 −0     │ ☐ c1 · src/lib.rs:4 (new) open│
│ D src/legacy.rs +0 −3 │  4  4                                      │   본문 · Resolve · Delete     │
│ M src/lib.rs 💬2 …    │     5 + ## Ports                           │ …                             │
│ A src/net.rs  +6 −0   │   ┌ 💬 c3 · README.md:7 (new)   open ┐     ├ SEND TO AGENT ────────────────┤
│                       │   └ 본문 · Resolve · Delete           ┘     │ [지시문 textarea]             │
│                       │                                            │ [Send 2 open to agent]        │
│                       │                                            │ agent t1: working — will queue│
│                       │                                            │ d1 · 3 comment(s) · queued…   │
└───────────────────────┴────────────────────────────────────────────┴───────────────────────────────┘
```

- **열기**: 사이드바 Task 헤더의 `±` 버튼(열린 코멘트가 있으면 `± 3`), Task 헤더 우클릭 메뉴 "Review changes", Task 상세 다이얼로그의 "Review changes", 단축키 **Ctrl+Shift+R**(포커스된 터미널의 Task, 없으면 첫 Task), 명령줄 `pod-app --review <task>`. Task마다 패널은 하나(이미 열려 있으면 그 탭으로 이동). 도크 탭으로 열리고, 앱 재시작 때 Task가 남아 있으면 같은 자리에 복원된다(fix2 B7, §2.3).
- **diff**: 기본은 **Inline**. `Split`(좌우)은 두 칼럼이 들어갈 폭이 필요해서 켤 때 줄바꿈도 같이 켠다. 패널이 좁으면 `Files`/`Comments`로 양옆 패널을 숨긴다. `Expand`/`Collapse`는 변경 없는 줄 접기(문맥 3줄). 구문 강조·단어 단위 강조·접기·가상화는 gpui-component `Diff` 그대로다(05).
- **코멘트**: 코드 줄을 클릭하면 그 줄(new/old 쪽 그대로)에 입력창이 열린다. 줄 번호를 **드래그**해 여러 줄을 고르면 드래그가 끝날 때 범위 입력창이 열린다. Enter = 저장(`review.comments.add`, 리뷰어가 본 코드를 `code`로 같이 보냄), Shift+Enter = 줄바꿈. 저장된 코멘트는 diff 안(해당 줄 아래 카드)과 오른쪽 목록에 나오고, 상태(`open`/`queued`/`sent`/`resolved`)를 색으로 표시한다. Resolve/Reopen(`review.comments.resolve`), Delete(`review.comments.remove`). 목록 카드를 누르면 그 줄로 스크롤(접힌 구간이면 펼침). 해결된 코멘트는 "Show N resolved"로 본다. 파일 목록에 파일별 미해결 코멘트 수 `💬n`. 줄바꿈(CRLF/LF)만 바뀐 파일에는 "줄바꿈만 바뀜"이 붙는다(05).
- **Send to agent**: ☐로 고른 코멘트를, 고른 것이 없으면 열린 코멘트 전부(`all_open`)를 지시문과 함께 `review.send`로 보낸다. 결과:
  - `delivered` → 성공 토스트, 코멘트 `sent`, 지시문 칸 비움.
  - `queued`(에이전트 working) → 안내 토스트, 전달 목록에 노란 `queued until the agent is idle` + Cancel(`review.cancel`). 전달되면 "Review delivered" 토스트와 초록 `delivered to t1 (idle)`.
  - 거절(`AGENT_BLOCKED`, `TERMINAL_EXITED`, 보낼 것 없음 `INVALID_STATE`) → 경고 토스트와 빨간 설명 줄. 코멘트는 `open` 그대로, 전달 기록 없음.
  - 버튼 아래에 에이전트 상태와 예고("working — will queue until idle", "blocked:question — send will be refused", "idle — delivers now").
- **단축키**(Review 패널 안에서만. 글자 키는 입력창과 겹치므로 쓰지 않음): F7 / Shift+F7 또는 Alt+↓ / Alt+↑ 다음·이전 변경, Alt+PgDn / Alt+PgUp 다음·이전 파일, **Ctrl+Shift+C** 코멘트 추가(선택한 줄, 없으면 마우스가 가리키는 줄), **Ctrl+Shift+Enter** 보내기, F5 새로고침. diff 본문의 ↑↓·PgUp/PgDn·Shift+↑↓(줄 선택)·Ctrl+C는 컴포넌트 기본.

### 7.2 구조

```
pod-diff (새 라이브러리 크레이트, pod-app과 diff-spike가 같이 씀)
  model.rs   SourceFile{path, old_path, old, new, binary} · full_context_patch(rename·binary 헤더 포함)
             load_changed(worktree, merge_base, files): 옛 버전은 `git cat-file --batch` 한 번, 새 버전은 worktree 파일
             from_git / from_two_files / bench_texts (spike·벤치용), git은 CREATE_NO_WINDOW
  lib.rs     parse(sources) -> (Vec<DiffFile>, Vec<FileRow>, error): gpui-component diff API를 감싸는 얇은 어댑터(05 §7 권고)
             gpui-kit tree-sitter 문법 feature(rust/js/ts/python/toml/markdown)도 여기서 켠다
  examples/gen-bench.rs  1만 줄 벤치 파일 쌍 생성

pod-app
  source.rs     PoddSource::request / call::<M>() — 새 **요청 연결**(스레드 podd-request, 자기 파이프 연결).
                리뷰 호출(task.diff, review.*)이 제어 연결의 task.list(git 여러 번) 뒤에 줄 서지 않게 분리.
                Capabilities.review = protocol >= 5
  review.rs     ReviewPanel(도크 패널) + 순수 함수(send_selection, finished_deliveries, turn_ended, delivery_text, refusal_text)와 테스트
  workspace.rs  open_review / Ctrl+Shift+R / 사이드바 ± 버튼·우클릭 메뉴 / 상태·Task 이벤트를 열린 Review 패널에 전달
  vm.rs         TaskVm.review_open (TaskInfo.review.open)
```

**pod-diff를 따로 만든 이유**: diff-spike의 입력 계층(전체 문맥 패치, git 읽기)은 GUI와 무관하고 테스트할 수 있는 코드라, pod-app에 복사하면 같은 코드가 두 벌 생긴다. 반대로 spike의 화면 코드(`review.rs`)는 스파이크 전용(JSON 내보내기, 벤치 구동)이라 옮기지 않았다. 그래서 "모델 + gpui-component 어댑터"만 작은 라이브러리로 뺐고 diff-spike도 그것을 쓴다(동작 같음, `--git`은 이제 `cat-file --batch`). tree-sitter feature를 pod-diff에 두어, 의존하는 쪽은 구문 강조가 자동으로 켜진다.

**데이터 흐름**

1. 패널 생성 → `task.diff`(요청 연결) → `{merge_base, files}` → 백그라운드 executor에서 `load_changed` + 패치 + `DiffFile::parse` → UI 스레드에서 `DiffState`를 **새로 만든다**.
   - `set_files`를 쓰지 않는 이유: 아직 그려지지 않은 상태에 `set_files`를 하면 다음 상호작용 전까지 목록이 비어 보였다. 새 상태도 1만 줄에서 1ms 안팎이고, `set_files`도 어차피 펼침·스크롤을 초기화한다.
   - 도크는 패널을 `cached` 뷰로 그리므로 `DiffState`의 notify(백그라운드 구문 강조 완료, 접기)만으로는 패널이 다시 그려지지 않는다. `cx.observe(&state)`로 패널을 notify한다(이것이 없으면 강조가 클릭 전까지 안 보였다).
2. 새로고침: (a) ⟳ / F5(diff + 코멘트 + 전달 기록), (b) 에이전트 터미널의 **턴 종료**(상태 이벤트에서 `turn` 증가 또는 working에서 벗어남), (c) 3초 Task 폴링의 git 요약(ahead·dirty·마지막 커밋)이 바뀜. (b)(c)는 0.4초 디바운스. 패치 해시가 같으면 아무것도 바꾸지 않고, 바뀌면 보던 파일로 다시 스크롤한다. diff를 타이머로 폴링하지 않는다.
3. 코멘트·전달: 바꿀 때마다 응답으로 목록을 고친다. (P5에서 바뀜, §8.4) podd가 상태 스트림에 `review.delivery` 이벤트를 보내므로 전달 기록은 이벤트로 갱신한다. 이 이벤트를 보내지 않는 옛 데몬에서만 `queued` 전달이 있을 때 (a) 에이전트 상태 이벤트 0.6초 뒤, (b) 2초마다 `review.deliveries`를 다시 읽는다. `queued`에서 다른 상태로 바뀐 전달마다 토스트 하나.
4. 레이아웃: Split 켜기와 옆 패널 토글 때 같은 파일로 `DiffState`를 다시 만든다. 컴포넌트가 콘텐츠 폭을 늘리기만 해서(05 §5-1), 칼럼이 좁아진 뒤에는 오른쪽 칼럼이 잘린 채 남기 때문이다.
5. 색은 `review.rs`의 `mod color` 한 곳에 모았다(pod-ui가 나중에 바꿈, 10 §5).

### 7.3 검증 (2026-11-08, 서브 모니터 150%, 격리 데몬 `podd-review-demo`)

`crates/pod-app/tools/review-demo.ps1`(drive.ps1 다음에 dot-source): 저장소 `revdemo`, Task `revdemo/feat`의 에이전트는 podd의 **fake-agent**(`osc:state=working` → 신호 파일 대기 → `paste-loop:<log>`, 실제 Claude/Codex는 쓰지 않음). worktree에 수정·추가·삭제·문서 변경 4파일. 입력은 **창에 PostMessage**(WM_MOUSEMOVE/LBUTTONDOWN/UP, WM_CHAR, WM_KEYDOWN)로 넣었다. 실제 커서를 움직이는 `mouse_event`는 사용자가 쓰고 있던 다른 창에 떨어져 클릭이 사라졌다. 캡처는 DPI 인식 PrintWindow.

| 파일 | 내용 |
|---|---|
| `01-review-panel.png` | 4파일(M/D/M/A, +16 −6) Inline, 파일별 💬 수, 코멘트 카드 inline + 목록. 앱을 다시 켠 뒤에도 코멘트가 podd에서 그대로 |
| `02-comment-adding.png` | Split + wrap에서 새 버전 줄 클릭 → 입력창(`New comment on src/lib.rs new:4`) |
| `02-range-comment-adding.png` | 줄 번호 1–3 드래그 → 범위 입력창 → `c4 README.md:1-3` 저장 |
| `03-comments-sidebar.png` | new·old(`src/lib.rs:4 (old)`)·한 줄 코멘트의 inline 카드와 목록 |
| `04-send-queued.png`, `04b-send-queued-row.png` | 에이전트 working 중 Send → `d1 queued`, 코멘트 3개 `queued`(해결한 c2 제외), 토스트, 전달 목록의 queued + Cancel |
| `05-delivered.png` | `Release-Agent`(신호 파일) → idle → podd 작업자가 전달 → "Review delivered" 토스트, `d1 delivered to t1 (idle)`, 코멘트 `sent`. fake-agent 로그: 지시문 + 코멘트 3개 프롬프트가 **paste 1번**으로 도착 |
| `06-delivered-and-refreshed.png` | 에이전트 idle에서 Send → 즉시 `d2 delivered`. 그 전에 worktree의 `src/net.rs`를 바꿔 두었고, fake-agent가 받은 뒤 턴을 끝내자 diff가 자동 갱신(`net.rs +6 → +10`, 합계 +16 → +20) |
| `07-send-refused-blocked.png` | `blocked:question` fake-agent Task(사이드바 `± 1`로 연 두 번째 Review 탭) → Send → 거절 토스트(podd 메시지 "waiting for you (blocked:question: Proceed?)"), 전달 기록 없음, 코멘트 open 유지 |
| `08-bench-10k.png` | 1만 줄 파일 1개, 약 950곳 변경(+761 −761) Task |
| `09-keyboard-f7.png` | 패널 포커스 후 F7 3번 → 세 번째 변경으로 이동 |
| `10-split-wide.png` | Files·Comments를 숨기고 Split + wrap(폭 충분) |

**여는 시간**(`[review] … loaded` 로그, 릴리스, 패널 생성부터 diff 준비까지):

| 입력 | task.diff (podd, git) | 파일 읽기(cat-file + worktree) | 패치 + 파싱 | **준비** |
|---|---|---|---|---|
| 4파일 33줄 | 218–260 ms | 61–74 ms | <1 ms | 286–328 ms |
| 1만 줄 1파일, +761 −761 | 253 ms | 123 ms | 28 ms | **405 ms** |

- 대부분이 podd의 `task.diff`(git 여러 번)다. 1만 줄 diff 자체(읽기·패치·파싱)는 150 ms 안팎이고 UI 스레드 밖에서 돈다. 스파이크(05 §4.1)와 같은 경향.
- 대기 상태에서 패널 다시 그리기 0회/초(Inline, Split+wrap, `POD_APP_DEBUG` 로그로 셈).
- **스크롤 fps는 재지 않았다**: 코디네이터 지시(기기 CPU 100% 근처, 성능 측정 금지)에 따라 미측정으로 남긴다. diff 본문은 스파이크와 같은 컴포넌트·같은 가상화라 비슷할 것으로 보지만 확인하지 않았다. 측정하려면 `New-BenchTask` 후 `pod-app --review revdemo/bench --no-throttle --stats`.

### 7.4 한계와 남은 일

1. **Split + 줄바꿈 없음**에서는 패널이 계속 다시 그려진다(약 10회/초, 컴포넌트가 칼럼 폭을 다시 재며 notify). Split을 켜면 줄바꿈도 켜서 피하지만, 사용자가 Wrap을 끄면 생긴다. 좁은 폭에서는 오른쪽 칼럼이 잘린다(05 §5-1, 칼럼별 가로 스크롤 없음). 그래서 기본은 Inline.
2. 새로고침은 `DiffState`를 새로 만들므로 펼친 구간과 스크롤 위치가 초기화된다(보던 파일 머리로 돌아감). 코멘트 위치는 추가 시점의 줄 번호 그대로라(11 §6-8), 에이전트가 줄을 옮기면 카드가 다른 줄 아래에 붙는다. 프롬프트에는 인용 코드가 들어가므로 전달 내용은 정확하다.
3. 접힌 구간 안의 코멘트는 inline 카드가 보이지 않는다(목록 카드를 누르면 펼쳐짐). 해결된 코멘트는 inline에 표시하지 않는다. inline 카드가 diff 폭을 넘으면 오른쪽이 잘린다.
4. 줄 번호 드래그 뒤에 나오는 컴포넌트의 `+` 버튼은 한 줄만 넘겨서, 대신 드래그가 끝나면 바로 범위 입력창을 연다. 줄 클릭은 곧 코멘트 입력이라, 코드 복사는 줄 번호 선택 + Ctrl+C로 한다.
5. 수정자 키 단축키(Ctrl+Shift+C/Enter/R, Alt+PgUp/PgDn)는 PostMessage로 수정자 상태를 줄 수 없어서 **실제 입력으로 확인하지 않았다**(F7은 확인). 바인딩·액션 연결은 F7과 같은 경로다.
6. ~~Review 패널은 앱 재시작 때 복원하지 않는다.~~ fix2부터 복원한다(§2.3). 코멘트·전달 기록은 podd에 있다. CLI(`pod review …`)로 바꾼 코멘트는 F5나 상태 이벤트 때 다시 읽힌다.
7. 코멘트 본문 편집·스레드·답글, "viewed" 표시, 파일 트리, staged/커밋 범위 선택은 없다. 바이너리 파일은 목록에만 나온다.
8. ~~상태 스트림에 전달 알림이 없어서 queued 동안 2초 폴링을 한다.~~ 고침(P5, §8.4): `review.delivery` 이벤트. 옛 데몬에서만 폴링한다.
9. 실제 Claude Code/Codex에는 보내지 않았다(사용량). 붙여넣기 방식의 실제 에이전트 호환은 11 §6-1의 남은 확인 그대로다.

### 7.5 검사 (이 worktree의 `target`, sccache, jobs 4)

처음 검증은 공유 target(`C:\ai\pod-target`)에서 했다. 공유 target은 다른 worktree에 있는 같은 경로 크레이트의 산출물을 그대로 쓸 수 있어서, 자기 `target`에서 전부 다시 빌드·검사했다. 실제 창 흐름(코멘트 → working 중 Send = queued → idle 뒤 delivered, paste 1번)과 1만 줄 열기(준비 371 ms: task.diff 248, 읽기 98, 패치+파싱 23)도 그 바이너리로 다시 확인했다(`target/captures/review2/`).

```
cargo build --release   → Finished `release` profile [optimized + debuginfo] (워크스페이스 전체), exit 0
cargo test --workspace  → 실패 0 (pod-app 21, pod-diff 7, podd 42 + daemon 6 + orchestration 3 + status 5 + tasks 2, pod-terminal-view 27, pod-status …)
cargo clippy -p pod-app -p pod-diff -p diff-spike --all-targets → 이 세 크레이트 경고 0 (pod-terminal-view의 기존 경고 3개는 손대지 않음)
cargo deny check        → advisories ok, bans ok, licenses ok, sources ok
```

새 의존성 없음(pod-diff는 기존 gpui-kit·similar·anyhow만 씀). podd·pod-cli·pod-terminal-view는 고치지 않았다.

## 8. 오케스트레이션 (P5)

작성일: 2026-11-09 · 브랜치 `SJin-Choi/orch-ui` · 코드: `crates/pod-app/src/{orch,groups,gates,tree}.rs` · 백엔드: [11 §3.2–3.3](11-review-and-orchestration-backend.md) · 상위 기획: [01 §4.2 (3), §6 P5](01-plan.md) · 디자인: [10 §4.4, §5](10-ui-design.md) · 스크린샷: `crates/pod-app/screenshots/orch/`

하나의 프롬프트를 여러 에이전트에 나눠 주고(fan-out), 결과를 비교하고, 승자를 고르고, 사람 승인(게이트)을 처리하고, 부모-자식 에이전트 트리를 보는 화면이다. 모두 podd 프로토콜 v6의 `group.*`·`gate.*`를 쓰며, v6 미만 데몬이면 버튼이 보이지 않는다(`Capabilities.orch`). 새 UI는 pod-ui 디자인 시스템(StatusBadge, StatusSummary, TreeRow, Frame, semantic_button, 토큰 색)으로 그렸다. 기존 화면의 스타일은 건드리지 않았다(D2 작업과 병합하기 쉽게).

### 8.1 화면과 조작

**New Group 다이얼로그** — 사이드바의 `+ Group` 또는 **Ctrl+Shift+N**.

```
┌ New group: fan out one prompt to several agents ───────────────┐
│ Repository  [repo] [orchdemo] [Add repository…]                │
│ Name [fx            ]   Base [main (기본: 저장소 기본 브랜치)]  │
│ Prompt [모든 에이전트가 받는 프롬프트 (여러 줄)              ] │
│ Agents  Claude [−] 0 [+]  prompt as CLI argument               │
│         Codex  [−] 0 [+]                                       │
│         Shell  [−] 3 [+]  PowerShell; prompt typed as a command│
│         Custom [−] 0 [+]  any program (argv 입력칸)            │
│ Check (optional) [findstr /c:PASS result.txt]  → cmd /c 로 실행 │
│ 3 member task(s), one worktree each: fx-shell-1, -2, -3        │
│                                         [Cancel] [Create group]│
└────────────────────────────────────────────────────────────────┘
```

- 에이전트마다 개수를 정한다(합계 1–16). 멤버 이름 미리보기는 podd 규칙(`<name>-<agent>-<n>`)과 같다. 검사 실패(저장소 없음, 이름 문자, 프롬프트 없음, 멤버 0/17개, Custom 명령줄 없음)는 다이얼로그 안에 빨간 줄로 보여 주고 닫지 않는다.
- Check 명령은 `["cmd", "/c", <입력>]`으로 보낸다. podd는 argv[0]을 `.exe`로 풀기 때문에 `npm test` 같은 `.cmd` shim이나 셸 문법도 그대로 쓰려면 cmd를 거쳐야 한다(11 §3.2).
- `Create group` → (필요하면 `repo.add`) → `group.create`(요청 연결) → 성공하면 Group 패널이 열리고 Task 목록을 바로 다시 읽는다(`PoddSource::refresh_tasks`). 실패하면 데몬 메시지를 이름과 함께 토스트(`GROUP_EXISTS: …`). 멤버 생성이 실패하면 podd가 이미 만든 멤버를 지운다(11 §3.2).

**Group 패널** (도크 가운데 탭 `Group · <repo>/<name>`)

```
┌ Group orchdemo/fx ───────────────────────────────────────────────┐
│ base main (6bf08208c) · 3 member(s) · 상태 응답 1 · 완료 2       │
│ [⚠ 확인 필요] [★ Winner: …]                                      │
│ Prompt: …                                                        │
│ Check [findstr /c:PASS result.txt] [Run check] [Cancel check] ⟳ Compare │
│ Check k1 FAILED: `findstr …` · 2 failed · 1 passed   (빨강)       │
│ Picked … · removed …                                             │
├ fx-shell-1 ──────── shell ┬ fx-shell-2 ─── shell ★ ┬ fx-shell-3 … ┤
│ [◌ 작업 중]               │ [✓ 완료]               │ [⛨ 승인 필요]  │
│ ⎇ pod/fx-shell-1          │ …                      │                │
│ +2 −0 2 file(s) ↑0 2 dirty│ +5 −0 3 file(s)        │                │
│ ? result.txt     +1 −0    │ ? src/extra.rs  +2 −0  │                │
│ ✕ check FAILED · exit 1 · 0.1 s  show output       │                │
│ [Terminal] [Review]   [Pick winner]                │                │
└──────────────────────────────────────────────────────────────────┘
```

- 멤버 카드: pod-ui `StatusBadge`(에이전트 터미널의 실시간 상태, 상태 스트림 이벤트로 즉시 바뀜), 브랜치, `+추가 −삭제 · 파일 수 · ahead · dirty`, 바뀐 파일 6개까지(`… N more`), check 결과(통과 초록 / **실패는 빨강 `check FAILED · exit N`**, 시간 초과·오류도 실패로 표시), `show output`으로 출력 끝 14줄. 버튼: `Terminal`(멤버의 에이전트 터미널 열기), `Review`(§7 Review 패널), `Pick winner`. 지워진 멤버는 흐리게 `removed`, 승자는 초록 테두리와 `★ winner`.
- 헤더: 그룹 전체 `StatusSummary`(이름 붙인 숫자, 10 §4.4)와 대표 배지, 프롬프트, check 입력칸 + `Run check` / `Cancel check`, `⟳ Compare`, check 작업 줄(`Check k2 running · 1/3 done …` → `PASSED` 초록 / `FAILED` 빨강 / `cancelled` 노랑 / `interrupted`), 마지막 pick 결과.
- **Run check**: 입력칸이 저장된 명령과 다르면 그 명령을 저장하며 시작한다(`group.check {check, wait: false}`). 결과는 응답이 아니라 상태 스트림의 `group.check` 이벤트(시작, 멤버마다, 끝)로 카드에 들어온다. 끝나면 워크스페이스가 토스트(`Check passed` / `Check FAILED`)를 띄운다 — 패널을 닫아 두어도 알 수 있다. **Cancel check** = `group.check_cancel`(멤버 결과 `check error: cancelled`).
- **Pick winner** 다이얼로그: `Remove the other members`(기본 켬, 안전 삭제 설명 포함), `Also delete their branches`. 확인 → `group.pick {remove_others, force: false}`. podd가 커밋 안 된 변경·unpushed 커밋 때문에 멤버를 남기면(`kept`, `TASK_DIRTY`) **데몬 메시지를 멤버별로 그대로 보여 주는** "podd kept some members" 다이얼로그가 뜨고, `Remove anyway`를 누를 때만 `force: true`로 다시 부른다. 지워진 멤버의 터미널 패널은 닫는다.
- 갱신: `group.show`는 열 때·pick 뒤·check 작업이 끝날 때, `group.compare`(멤버마다 git 여러 번)는 열 때·⟳·**멤버의 git 요약이 바뀔 때**(3초 Task 폴링의 ahead/dirty/마지막 커밋, 0.8초 디바운스). 상태 배지는 워크스페이스의 상태 이벤트로 바로 바뀐다. 타이머 폴링은 없다.

**게이트 받은편지함** — 상태바의 `⚑ N gates pending`(대기 게이트가 있으면 노란 배경) 또는 **Ctrl+Shift+I**. 오른쪽 도크 탭 `Gates (N)`.

- 대기 게이트 카드(pod-ui `Frame`): `g3 · 제목`, 메시지, 출처(`task … · group … · terminal t14`), `requested 40s ago`, 만료가 있으면 `expires in 14m 19s`(1초마다 줄어듦, 만료 게이트가 있을 때만 다시 그림) / 없으면 `no expiry`, `open terminal`(요청한 터미널, 없으면 Task의 첫 터미널), 메모 입력칸, `Approve` / `Reject`(pod-ui `semantic_button`, 처리 중에는 Loading). → `gate.resolve {decision, note}`. 이미 다른 곳(CLI·만료)에서 해결됐으면 `GATE_RESOLVED` 메시지를 카드에 보여 주고 목록을 다시 읽는다.
- 아래 `RECENT DECISIONS`: 해결된 게이트(최근 12개, approved 초록 / rejected 빨강 / expired 노랑, 메모).
- **새 게이트 토스트**: 상태 스트림의 `gate.changed`로 새 pending 게이트가 오면 `Approval requested · <제목>` 토스트와 비프(`--no-sound`로 끔). 시작할 때 이미 대기 중인 게이트는 하나의 요약 토스트(`N gate(s) waiting`). 다른 곳에서 해결되면 `Gate g1 approved` 토스트.
- 게이트 목록은 받은편지함을 열지 않아도 워크스페이스가 가진다(구독 스냅샷 `gates` + `gate.changed`). 재연결 스냅샷에 없는 옛 pending 게이트는 지우고, 받은편지함을 열 때 `gate.list`로 결과를 채운다.

**에이전트 트리** — 사이드바의 `Agents` 또는 **Ctrl+Shift+E**. 오른쪽 도크 탭 `Agents`.

```
▾ ⧉ group orchdemo/fx                 [✓ 완료]
      완료 1
  ▾ ▣ orchdemo/fx-shell-2             [✓ 완료]
    › orchdemo/fx-shell-2  powershell · t10
▾ ▣ orchdemo/lead                     [⚠ 확인 필요]   shell · pod/lead · 응답 1 · 미확인 1 · 완료 2
  ▾ › orchdemo/lead (t5)              [? 상태 미확인 · 추정]   하위 응답 1 · 완료 2
    ▾ ⧉ group orchdemo/sub            [⚠ 확인 필요]   응답 1 · 완료 2
      ▾ ▣ orchdemo/sub-shell-3        [⚠ 확인 필요]
        › orchdemo/sub-shell-3 (t14)  [⛨ 승인 필요]
```

- 관계는 podd 기록에서 온다: Pod 터미널 안에서 `pod task new`/`pod group new`로 만든 Task는 그 터미널을 `parent_terminal`로(그 Task를 `parent_task`로) 가진다(08 §4.1, 11 §4). 그룹 멤버는 그룹 아래, 그룹은 멤버들의 공통 부모 터미널 아래(공통 부모가 없으면 루트), Task 아래에 그 터미널, 터미널 아래에 거기서 시작된 Task/그룹.
- 행은 pod-ui `TreeRow`(들여쓰기, 선택 표시, 배지). 터미널은 자기 `StatusBadge`, Task·그룹은 하위 터미널 전체의 `StatusSummary`(터미널마다 한 번, 우선순위 오류 > 응답 > 작업 > 미확인 > 대기, 10 §4.4) 배지와 이름 붙인 숫자. `▾/▸`로 접기(접어도 집계는 유지). 행 클릭: 터미널 → 패널 열기, Task → Task 상세, 그룹 → Group 패널.
- Task에 속하지 않은 터미널(Pod 터미널 안의 `pod run`)은 protocol v7 `TerminalInfo.parent_terminal` 아래에, 그 터미널이 podd 목록에서 사라졌으면 `parent_task` 아래에 놓인다. podd가 두 필드를 종료 뒤에도 유지하므로 자식·부모가 끝나도 자리가 그대로다(fix2 B3).
- 순환 기록(서로를 부모로 가리킴)은 무한 루프 없이 각각 한 번 루트에 보인다. 알 수 없는 부모는 루트로 간다.

### 8.2 구조

```
orch.rs    OrchState(워크스페이스가 가짐): gates(BTreeMap), 열린 Group 패널들, Gates/Agents 패널
           Workspace::on_orch_event  — Gates 스냅샷 / Gate / GroupCheck / Delivery 이벤트 → 상태 갱신, 토스트, 패널에 전달
           Workspace::orch_data_changed — Task·터미널·상태가 바뀌면 Group 패널(git 요약 비교)과 트리를 다시 그림
           open_group / open_gates / open_agent_tree, 상태 → pod-ui 배지 변환(status_badge, summary_of …)
groups.rs  AgentCounts·validate·check_argv(순수 함수, 테스트), New Group 다이얼로그, GroupPanel
gates.rs   GatesPanel, duration_text·expiry_text
tree.rs    build(tasks, terminals, collapsed) -> Vec<Node> (순수 함수, 테스트), AgentTreePanel
source.rs  상태 스트림이 gate.changed / group.check / review.delivery를 SourceEvent::{Gate, GroupCheck, Delivery}로,
           구독 응답의 gates를 SourceEvent::Gates로 내보냄. Capabilities.orch = protocol >= 6. refresh_tasks()
vm.rs      TaskVm.parent_task / parent_terminal / group
```

공유 파일 수정은 작게 했다: `workspace.rs`(필드 몇 개 `pub(crate)`, `orch` 필드 하나, 이벤트 분기 한 줄, 사이드바 버튼 줄·상태바 항목·액션 3개, 빈 화면 조건), `main.rs`(모듈 4개, 키 바인딩), `Cargo.toml`(pod-ui·pod-status 의존성), `review.rs`(§8.4). pod-ui·pod-terminal-view는 고치지 않았다.

주의(GPUI): 워크스페이스가 `update` 중일 때 다른 엔티티가 워크스페이스를 `read`하면 패닉한다. 그래서 워크스페이스가 부르는 패널 메서드(`data_changed`)는 필요한 데이터를 인자로 받고, 패널의 버튼 동작은 `cx.defer`로 워크스페이스를 부른다. 처음 데모에서 이 패닉을 실제로 겪었다.

### 8.3 podd 쪽: `review.delivery` 이벤트 (추가형, 프로토콜 v6 유지)

- 리뷰 전달이 기록되거나 상태가 바뀔 때마다(요청 시 `queued`/`delivered`, 큐 작업자의 `delivered`, `review.cancel`의 `cancelled`, `failed`) 허브가 `review.delivery {seq, delivery: DeliveryInfo}`를 발행한다. `seq`는 다른 이벤트와 같은 데몬 전체 순번이다. `ids` 필터를 준 구독자는 그 터미널의 전달만 받는다.
- 예전 클라이언트는 모르는 알림을 무시하므로 프로토콜 번호는 그대로 v6이다. Rust 클라이언트: `SubscribeEvent::Delivery`(열거형이 `non_exhaustive`). `pod server schema`의 notifications에 포함.
- 테스트(`crates/podd/tests/orchestration.rs` `review_queue_is_prompt_for_terminals_without_turns`): 구독자가 `d1 delivered → d2 queued → d2 delivered` 순서로, `seq`가 늘어나며 받는다. 다른 터미널로 필터한 구독자는 받지 않는다.

### 8.4 Review 패널의 폴링 제거

`review.delivery` 이벤트를 받으면 워크스페이스가 같은 Task의 Review 패널에 넘기고(`ReviewPanel::delivery_event`), 패널은 기록을 바꾸고 `queued` → 다른 상태 전이마다 토스트를 띄우고 코멘트를 다시 읽는다. 첫 이벤트가 오면(`source::delivery_events()`) 2초 폴링과 상태 이벤트 뒤 0.6초 재읽기를 하지 않는다. 구독할 때마다 이 표시를 지우므로, 이벤트를 보내지 않는 옛 데몬에서는 예전처럼 폴링한다. 덤으로 CLI(`pod review send`)로 만든 전달도 패널에 바로 나타난다(예전에는 F5나 상태 이벤트 때).

### 8.5 검증 (2026-11-09, 서브 모니터 150%, 격리 데몬 `podd-podapp-demo`)

`crates/pod-app/tools/orch-demo.ps1`(drive.ps1·review-demo.ps1 다음에 dot-source): 저장소 `orchdemo`의 `tools/member.ps1`이 에이전트 역할을 한다. 그룹 프롬프트가 이 스크립트를 실행하고, 스크립트는 OSC 7501로 `working`/`blocked:permission`/`done`을 보고하며 worktree 파일을 바꾸고(멤버 2만 `PASS` 줄), 멤버 3은 **자기 터미널 안에서** `pod gate request … --expires 900 --wait`로 승인을 기다린다. 실제 Claude/Codex 턴은 쓰지 않았다(사용량). 입력은 창에 PostMessage(사용자의 실제 커서를 움직이지 않음), 캡처는 DPI 인식 PrintWindow.

| 파일 | 내용 |
|---|---|
| `01-new-group-dialog.png` | New Group: 저장소 orchdemo, 이름 fx, 프롬프트, Shell 3, check `findstr /c:PASS result.txt`, 멤버 미리보기 |
| `02-group-working.png`, `02b-group-progress.png` | 생성 직후 Group 패널: 멤버별 `작업 중` → 하나씩 `완료`, 헤더 집계 `작업 2 · 완료 1`, 토스트 |
| `03-gate-requested.png` | 멤버별 +/−·파일 목록·dirty, 멤버 3 `승인 필요`, 상태바 `⚑ 1 gate pending` |
| `04-check-result.png` | Run check → `Check k1 FAILED … 2 failed · 1 passed`(빨강), 카드별 `check FAILED · exit 1` / `check passed · exit 0`, 실패 토스트 |
| `05-gates-inbox.png` | 받은편지함: g2(task fx-shell-3, terminal t11, 37초 전, `expires in 14m 22s`, open terminal) |
| `06-gate-approved.png` | 메모 "go ahead, staging only"로 Approve → pending 0, RECENT DECISIONS에 approved + 메모. 터미널 안의 `--wait`가 종료 코드 0으로 끝나 멤버 3이 이어서 `완료` |
| `07-pick-dialog.png`, `08-pick-refused.png`, `09-picked.png` | fx-shell-2(check 통과)를 Pick → 다른 두 멤버는 podd가 거절(`TASK_DIRTY`: "2 uncommitted change(s) …", 멤버별 메시지) → Remove anyway → 둘 다 removed(흐림), `★ Winner`, 사이드바에서 사라짐 |
| `10-agent-tree.png` | lead Task의 터미널 t5 안에서 `pod group new orchdemo sub …` → 트리: lead ▸ t5 ▸ group sub ▸ 멤버 3 ▸ 각 터미널, 집계 `작업 3 · 미확인 1` |
| `11-gate-toast.png` | 새 게이트 토스트 `Approval requested · …`, 트리의 `승인 필요`가 lead까지 `확인 필요`로 올라감 |
| `13-inbox-resolved.png` | 대기 2개(g3: 터미널 t14에서 요청, g4: CLI) → g4 Reject(메모 "not needed"), g3 Approve → 결정 4건 기록 |
| `14-sub-from-tree.png` | 트리에서 group sub 클릭 → Group 패널(CLI가 저장한 check `cmd /c findstr …`) |
| `15-check-running.png`, `16-check-cancelled.png` | `ping -n 60 127.0.0.1` check 실행 중(`0/3 done`, 카드 `check running…`, Cancel check) → 취소 → `Check k2 cancelled`, 카드 `check error: cancelled`, 토스트 |
| `17-review-queued.png`, `18-review-delivered-event.png` | fake-agent(working) Task에 CLI로 `pod review send` → 열린 Review 패널에 `d1 queued`가 이벤트로 나타남 → 신호 파일로 에이전트 idle → `delivered to t15 (idle)` + "Review delivered" 토스트(폴링 없음) |

검사 (워크트리 자기 target, sccache, jobs 4):

```
cargo build --release   → Finished `release` profile (워크스페이스 전체), exit 0
cargo test --workspace  → 테스트 바이너리 29개 모두 ok, 실패 0 (pod-app 35, podd 47 + orchestration 5 + daemon 6 + status 6 + tasks 2 + retention 1 + detach 1, pod-ui 9 …)
cargo clippy -p pod-app -p podd -p pod-cli --all-targets → 이 크레이트들 경고 0 (pod-terminal-view의 기존 경고 3개는 손대지 않음)
cargo deny check        → advisories ok, bans ok, licenses ok, sources ok
```

새 단위 테스트: 게이트 스냅샷 병합·정렬·출처(`orch`), 상태 → pod-ui 배지(추정 표시 포함), 멤버 이름·검증·check 명령 왕복·실패 요약(`groups`), 만료 문구(`gates`), 트리 구성·집계·접기·순환(`tree`), 스트림 이벤트 파싱과 폴링 끄기(`source`). 새 의존성은 없다(pod-ui·pod-status는 워크스페이스 크레이트).

### 8.6 한계와 남은 일

1. ~~**일반 `pod run` 터미널의 부모는 기록되지 않는다.**~~ podd v7이 기록하고 앱이 fix2부터 트리에 쓴다(§8.1, B3).
2. ~~Group 패널은 앱 재시작 때 복원하지 않는다.~~ fix2부터 Group·Gates·Agents 패널과 오른쪽 도크를 복원한다(§2.3, B7). 사이드바에는 그룹 목록이 없다(트리에 있음).
3. `group.compare`는 멤버마다 git을 여러 번 돌려(11 §6-5) 멤버가 많으면 수 초 걸린다. 그동안 카드는 `comparing…`이고 요청 연결의 다른 요청(리뷰)이 그 뒤에 줄 선다.
4. check 출력은 끝난 멤버의 마지막 4 KiB만 있다(podd가 스트리밍하지 않음, 11 §6-4).
5. 게이트 토스트는 기존 토스트처럼 8초 뒤 사라진다. 10 §5.4의 "응답 필요는 자동으로 사라지지 않음 + 알림 센터"는 D2/D3 토스트 작업으로 넘긴다(받은편지함과 상태바 개수는 남는다).
6. 트리는 펼침 상태를 저장하지 않고, 키보드 탐색(↑↓←→)이 없다(pod-ui `TreeRow`는 Enter만). 10 §5.1의 키보드 규칙은 D2 범위.
7. 수정자 키 단축키(Ctrl+Shift+N/I/E)는 PostMessage로 수정자를 줄 수 없어 실제 입력으로 확인하지 않았다(버튼 경로와 같은 액션).
8. 실제 Claude/Codex 멤버로는 돌려 보지 않았다(사용량). claude/codex 멤버의 프롬프트 제한(`.cmd` shim, 11 §6-6)은 그대로다. 다이얼로그는 그 제한을 미리 검사하지 않고 podd 오류를 보여 준다.
9. 상태 배지는 pod-ui의 한국어 라벨(`작업 중`, `승인 필요`)이고 나머지 앱 문구는 아직 영어다. D2 적용 때 정리.
## 9. E2E 수정 fix2 (2026-11-09, B3·B4·B5·B7)

docs/12의 실제 Codex E2E에서 나온 앱 쪽 결함 네 개. 검증 방법과 근거 사진은 docs/12 각 버그 항목과 `docs/e2e/fix2/`.

| 버그 | 원인 | 수정 | 테스트 |
|---|---|---|---|
| B3 `pod run` 자식이 트리 루트 | `TerminalVm`에 v7 부모 필드 없음, 트리는 Task 소속만 봄 | `TerminalVm.parent_terminal/parent_task`, `tree::parent_of`가 터미널 → 부모 터미널(없으면 부모 Task) | `vm::terminal_parent_fields_v7`, `tree::pod_run_child_terminal_nests_under_parent_terminal_and_survives_exit`, `tree::loose_terminal_chain_and_self_parent` |
| B4 OSC 색 응답이 Codex 입력칸에 | 응답이 제어 연결로 가서 Task 생성 직후 `poll()`(git 상태를 도는 `task.list`) 뒤에 줄 섬 → Codex 질의 대기 시간이 지나 키 입력으로 들어감(Windows Codex는 콘솔 키 이벤트를 읽어 ESC는 사라지고 `]10;rgb:…\`만 남음) | 입력 전용 연결(`podd-input`), 응답은 `AttachVt`가 시각을 찍어 `PoddSource::reply` → `ReplyFilter`(500 ms 넘은 응답·다른 패널의 같은 응답 버림) | `source::color_reply_filter_drops_stale_and_duplicate_replies` |
| B5 숨은 터미널의 needs-you 토스트가 안 뜸 | 억제 조건이 마지막 포커스 캐시 `focused_terminal` | 창 활성 + 그린 프레임에서 그 터미널 패널이 포커스(`terminal_in_view`) | `workspace::needs_you_toast_suppressed_only_for_the_terminal_in_view` |
| B7 터미널 외 패널·오른쪽 도크 유실 | `layout::prune`가 터미널 잎만 남기고 도크를 버림, 빌더는 터미널만 등록 | 패널별 유지 규칙(`layout::Alive`), 도크 가지치기, 빌더 4개 + `RestoreCtx`, 첫 Task 목록 뒤 복원 | `layout::restores_review_group_gates_agents_and_right_dock`(E2E가 저장한 실제 layout-before.json), `layout::review_and_group_need_their_target` |

**v8 제출 확인(e2e-fix1 후속)**: `review.delivery` 이벤트의 `submit`과 `review.deliveries`의 `submits`를 Review 패널이 전달별로 보관하고 전달 행 아래에 보여 준다(`제출 확인 중` / `에이전트가 받음` / `제출 확인 안 됨: 입력칸에 남아 있을 수 있음`, 셸의 `unverifiable`은 표시 안 함). `unconfirmed`가 되면 경고 토스트 한 번과 행의 `Enter 누르기` 버튼(그 터미널에 Enter 한 번). 테스트 `review::submit_check_lines_and_unconfirmed_toast_once`. 실제 미확인 전달로 화면 확인은 하지 않았다.

**podd 후속(이 작업은 podd를 건드리지 않음)**: (1) 다른 attach 클라이언트(다른 앱·`pod attach`)가 같은 색 질의에 답하면 여전히 중복된다. v7 중복 제거를 OSC 10/11/4 응답으로 넓히거나 holder가 색 질의에 답하게 해야 한다(팔레트 결정 필요). (2) 늦은 응답을 막는 근본 대책은 holder/ConPTY 쪽이다: 프로그램이 질의 대기를 끝낸 뒤 도착한 응답은 키 입력이 되므로, podd가 "질의 뒤 N ms 안의 응답만 통과"를 적용하면 모든 클라이언트에 효과가 있다. (3) Codex의 질의 대기 시간은 측정하지 않았다. 500 ms 상한은 앱 큐 지연 대비 안전장치이고, 정상 경로의 지연은 0 ms로 기록됐다.

# D2 appearance and host behavior (2026-11-09)

`theme.rs` owns persisted appearance, density, motion and jump-shortcut preferences.
Settings live beside the layout in `settings.json` (`POD_APP_CONFIG_DIR` overrides
the directory for isolated QA). `pod_ui::apply_theme` supplies the component theme;
the host maps its terminal defaults into `pod_terminal_view::TerminalPalette` on
construction, appearance changes and reconnect reset. OSC overrides and truecolor
remain terminal-owned. A failed settings write leaves the active settings intact.

**손상된 설정(2026-11-10)**: `settings.json`을 읽지 못하면 원본을 `settings.json.bad-<유닉스 초>`로 복사해 두고, 읽히는 항목(값 하나만 틀린 객체)은 살린 채 나머지를 기본값으로 시작한다. 시작할 때 경고 알림으로 백업 경로를 알린다. 다음 저장이 원본을 덮어도 백업은 남는다.

**테마(2026-11-10, docs/10 §D5)**: `settings.json`에 `style`(스타일 테마 id, 기본 `quiet` = 04 조용한 편집기), `theme`(다크/라이트/시스템), `density`(`theme`·`dense`·`compact`·`comfortable`·`accessible`; 예전 `comfortable: true`는 여유롭게), `accent`, `layout`(레이아웃 모드 id)이 추가됐다. 바꾸면 `pod_ui::themes::activate` → GPUI 테마 → 터미널 팔레트·편집기에 즉시 적용(재시작 없음). 사용자 테마는 `%APPDATA%\Pod\themes\*.json`(`POD_THEMES_DIR`, 없으면 `POD_APP_CONFIG_DIR\themes`), 명령 팔레트 “테마: 사용자 테마 다시 읽기”로 다시 읽는다. 창 본문은 `layout_mode.rs`의 등록부가 고른 모듈이 그린다(처음에는 작업대만, 지금은 7개 모드 모두).

`chrome.rs` owns the custom titlebar, command registry, jump palette, repository
dialog, settings, notification history and status inspector. Ctrl+K opens jump,
Ctrl+N creates a task, Ctrl+, opens settings, Ctrl+Shift+B opens notifications and
Ctrl+Shift+S inspects the current terminal. The alternate Ctrl+Shift+K shortcut applies immediately; the registry can send
Ctrl+K through to the current terminal.
Palette Enter checks the input's actual IME marked range before activation.

The sidebar uses a virtual list of repo/task/terminal rows and persists
collapsed groups. Up/Down selects, Left/Right collapses/expands and Enter opens.
Status view models retain source, confidence, timestamps and final state; a
connection loss marks the presentation stale rather than changing AgentState.
New Task retains inputs on server errors and guards duplicate submissions.
Toasts retain up to 100 history entries, show at most three, pause on hover or focus within the notification area, and
keep warnings/errors until dismissed. Closing a notification does not acknowledge
or resolve its blocked request. Existing terminal focus acknowledgement remains.

Review, Groups, Gates and Agents consume the current theme rather than fixed dark
colors. Attach reconnects after protocol-v7 `terminal.lagged`. Existing dock close
still detaches the view; killing a process remains a separate confirmed command.

For UI-only scale QA, `pod-app --ui-fixture --display secondary` supplies 100 tasks
and 300 terminals without connecting to or starting a daemon. It is explicitly
labelled read-only. Screenshots and validation limitations are recorded in
`crates/pod-app/qa/README.md` and docs/10's D2 results; the full D2 gate is not
implied by this implementation summary.

## D2 part 2: 한국어 작업 화면과 탐색

`SJin-Choi/d2b-polish`는 `18b4129` 위에서 적용한다. 제품 문구는
`strings.rs`의 상수와 `tr!` 포맷 템플릿으로 관리한다. 저장소·브랜치·경로·명령,
사용자가 작성한 요청/의견 및 진단용 서버 오류 원문은 그대로 둔다.
Review·Group·Gates·Agents, 생성/삭제 확인, 알림, 상태 표시줄도 같은 한국어 문구를 쓴다.

- 사이드바는 304px, 작업·터미널은 이름과 보조 정보를 분리한 두 줄이다.
  작업 보조행에 브랜치와 상태 집계를 표시한다. 검색 중에는 저장된 접힘 상태를
  바꾸지 않고 일치한 자식과 그 부모를 표시한다. 검색을 지우면 원래 접힘 상태로 돌아간다.
- 설정은 중앙 도크 탭, 상태는 304px 오른쪽 도크 패널이다. 상태 패널은 선택한
  터미널과 연결 상태 변경을 따라간다. 이 두 보조 패널의 재시작 복원은 별도 과제다.
- UI 글꼴·크기와 터미널 글꼴·크기를 따로 저장한다. 기존 터미널에도 새 글꼴을
  적용하며 다음 레이아웃에서 새 셀 크기를 보고한다. 실패한 저장은 활성 설정을 바꾸지 않는다.
- 검색 단축키 변경은 바로 적용된다. 기본 Ctrl+K를 해제하면 현재 터미널에
  Ctrl+K를 전달한다. 팔레트는 Unicode 문자 순서 검색, 굵게/밑줄 강조,
  최근 선택 20개 저장, 빈 결과의 검색 지우기를 제공한다.
- 알림은 사이드바 아래의 전용 영역에 최대 세 개를 쌓는다. 작업 영역 위에 겹치지
  않아 리뷰 전송 버튼을 가리지 않는다. 개별 hover 또는 알림 영역 내부 키보드 focus가
  유지되는 동안 일반 알림의 타이머를 멈춘다. 오류·응답 필요 알림은 계속 남는다.
- 종료 집계는 모두 같은 알려진 종료 코드일 때 그 코드를 전달한다. 서로 다르거나
  모르는 코드를 성공 코드 0으로 만들지 않는다.

검증 결과와 화면별 비교는 `crates/pod-app/qa/README.md`의 D2b 기록을 따른다.
설정/상태 패널 배치 변경은 레이아웃 복원 로직을 바꾸지 않는다. Agents 부모 관계,
OSC 응답 전달, 보고 있는 터미널의 알림 억제 로직은 병렬 버그 수정 작업의 범위다.

UI 라이브러리의 도크/입력 메뉴와 diff 접근성 문구도 같은 카탈로그를 사용한다.
이미 잠금 파일에 있던 MIT `rust-i18n 4.2.4`를 직접 의존성으로 선언하고
공개 번역 확장 hook을 한 번 등록한다. 라이브러리 소스/생성 자산은 수정하지 않는다.

D2b integration note: master through a35648d is merged locally (7a63fdb), including
the parallel worker's parent tree, OSC reply, focused-terminal toast and panel
restoration fixes. Those changes are inherited, not reimplemented by this task.
Review's empty/error/loading content now has an explicit full-width flex column.

### D2b 최종 검증 (2026-11-09)

최종 소스 8d0f633: workspace release 빌드, 전체 release-profile 테스트 270개(실패 0, 기존 doctest 1개 제외), pod-app/pod-ui/pod-terminal-view strict clippy 경고 0, cargo deny 4개 검사 통과. 알려진 lagged attach 테스트도 이번 전체 실행에서 통과했습니다. Review 빈 파일 목록을 숨겨 오른쪽 인스펙터와 함께 열어도 빈 상태 문구 너비를 확보했습니다.

화면별 다크/라이트 전후 비교, 보조 검증, 명령 마지막 줄과 남은 D3 항목은 [D2b 보고서](../crates/pod-app/qa/d2b-report.md)에 기록했습니다. 네이티브 Tab 입력은 Windows의 window_not_focused로 차단되어 키보드 전용 흐름·IME·Narrator·모달 포커스 복귀는 통과로 주장하지 않습니다. **전체 D2 접근성 게이트는 미완료**입니다.


## 10. P3 편집 통합: 파일 · 퀵오픈 · 에디터 · LSP (2026-11-09)

브랜치 `SJin-Choi/p3-integrate`. `crates/pod-editor`(docs/13)를 작업 공간에 붙였다. 자세한 API·LSP 수명은 [docs/13 §8](13-editor-and-lsp.md#8-pod-app-통합-2026-11-09).

### 10.1 화면과 조작

- **파일 패널** `파일 · <작업 id>`: 작업(워크트리)마다 Dock 패널 하나(`editor_panel.rs`). 안에 파일 트리 | 에디터 탭 · 배너 · 코드 에디터 · 문제/참조 | 상태 표시줄이 있고, 터미널·리뷰 패널과 같은 Dock에 탭/분할로 놓인다. 저장하지 않은 파일이 있으면 패널 제목에 `● n`이 붙고 Dock의 닫기 버튼이 사라진다(먼저 저장하거나 탭을 닫는다 — 탭 닫기는 저장 여부를 묻는다).
- **여는 길**: Ctrl+P(현재 작업 폴더 퀵오픈 — 파일 패널 안에서는 패널 자신의 Ctrl+P), 팔레트 `파일 열기` / `파일 탐색기 열기` / `모든 파일 저장`, 사이드바 작업 우클릭 `파일 열기`, 리뷰에서 파일 헤더의 `↗ 편집` 버튼(그 파일의 첫 변경 줄, 또는 가리킨 줄), 리뷰 파일 목록 더블클릭/우클릭 `편집기에서 열기`, 리뷰 안 Ctrl+Shift+O(가리킨 줄).
- **현재 작업**: 포커스를 가진 파일/리뷰 패널의 작업 → 포커스된 터미널의 작업 → 마지막으로 쓴 파일 패널 → 워크트리가 있는 첫 작업.
- **상태 표시줄**: 마지막으로 쓴 파일 패널의 활성 파일 서버를 짧게 표시 — `rust-analyzer 실행 중 / 색인 중 / 설치 안 됨 / 일시 중지됨`(+ 다른 서버 수 `외 n개`), 점 색은 실행 초록·색인/시작 파랑·재시작 주황·오류 빨강.
- **에이전트 편집**: 깨끗한 버퍼는 자동으로 다시 불러오고 에디터 상태 표시줄에 알림. 저장하지 않은 버퍼는 에디터 배너(`디스크 내용 불러오기 / 내 변경 유지`)와 함께 경고 토스트 `에이전트가 열린 파일을 바꿨습니다`. 파일을 저장하면 그 작업의 리뷰 diff를 새로 읽는다.
- **닫기 확인**: 저장 안 한 탭 닫기(Ctrl+W, ×, 가운데 클릭) → `저장 후 닫기 / 저장하지 않고 닫기 / 취소`. 창 닫기에 저장 안 한 파일이 있으면 `모두 저장하고 닫기 / 저장하지 않고 닫기 / 취소`(창은 그대로). 작업 삭제 때 그 작업 편집기에 저장 안 한 파일이 있으면 먼저 묻는다.
- **트리 파일 조작**: 우클릭 `새 파일… / 새 폴더… / 이름 바꾸기… / 삭제…`, 트리에서 Ctrl+N · F2 · Delete. 이름은 워크트리 밖·`..`·`.git` 금지. 삭제는 확인 대화. 열린 탭은 이름 바꾸기를 따라가고 삭제되면 닫힌다.
- **테마**: 다크/라이트/고대비 미리보기를 바꾸면 모든 편집기에 `set_mode`. 에디터 크롬은 pod-ui 토큰, 구문 색은 gpui-component 기본(docs/13 §7).

### 10.2 LSP 수명 (작업 공간 쪽)

- 서버는 **지원 파일을 처음 열어 활성화할 때** 시작한다(파일 패널을 열기만 하거나 트리를 보는 것으로는 안 뜸). 레이아웃 복원으로 되살아난 탭은 그 패널이 처음 포커스를 받을 때까지 서버를 띄우지 않는다.
- **동시 실행 상한 2개**(`POD_APP_LSP_MAX`로 바꿈): 서버 상태가 바뀔 때마다 모든 파일 패널의 실행 중 서버를 모아 넘치면 가장 오래 쓰지 않은 것을 `suspend`(정중한 shutdown, 2초 뒤 Job Object 트리 종료)하고 정보 토스트를 띄운다. 그 서버의 파일로 돌아가면(탭 활성화/열기) 다시 시작한다.
- 작업 삭제(또는 작업 목록에서 사라짐)·파일 패널 닫기 → 그 워크트리의 서버 종료. 앱 종료(`on_app_quit`) → 레이아웃 저장 후 모든 서버 종료. 비정상 종료여도 Job Object(KILL_ON_JOB_CLOSE)가 트리를 죽인다.

### 10.3 레이아웃 저장

`PodEditor {"task": id, "session": {"tabs": [{"path", "line", "col"}], "active"}}` — 경로는 워크트리 기준 `/` 상대 경로, 줄·열은 1부터. 복원 때 작업이 있고 **워크트리 폴더가 디스크에 있을 때만** 살린다(`layout::Alive::worktrees`). 탭 열기/닫기/전환 때 저장 예약, 커서는 다음 저장(또는 종료 시 저장)에 반영.

### 10.4 검증 (2026-11-09, 서브 모니터 150%, `--ui-fixture`)

픽스처의 `sample/task-000`이 생성된 작은 git 저장소를 워크트리로 받는 장면(`fixture_scenes.rs`, `--fixture-scene editor*`/`review-editor*`)으로 확인했다. 데몬·실제 에이전트 없음, rust-analyzer는 그 작은 저장소에서만.

| 항목 | 결과 |
|---|---|
| `cargo build --workspace --release` | 통과 |
| `cargo clippy -p pod-app -p pod-editor --all-targets -- -D warnings` | 경고 0 |
| `cargo test --workspace --release -- --test-threads=4` (한 번) | 326 통과, 실패 0, 기존 ignored 2 |
| `cargo deny check` | `advisories ok, bans ok, licenses ok, sources ok` (기존 ISC 미사용 경고만) |
| 시각 회귀 `tools/visual/capture.ps1` + compare | 28/28 통과(기존 26 + 다크/라이트 `editor`), 허용치 8 초과 픽셀 0 |
| LSP 상한(가짜 서버, 상한 1) | 동시 프로세스 최대 1, LRU 일시 중지 → 돌아가면 재시작, 앱 종료 후 0 |
| 창 닫기 + 저장 안 한 파일 | WM_CLOSE 뒤 창 유지, 확인 대화 |
| 복원 | 탭 2개·활성 탭·커서(5:1) 되살아남 |

기준 이미지 변경: `palette`(결과 수 41→44, 명령 3개 추가)와 `shortcuts`(편집 단축키 6줄 추가)는 의도된 변경이라 다크/라이트 모두 새 이미지로 바꿨다(차이 이미지 확인). 리뷰 화면은 버튼을 툴바가 아니라 파일 헤더에 두어 기준 그대로 통과한다. 스크린샷·UIA 트리·로그: `crates/pod-app/qa/editor/`(실행: `run-editor-qa.ps1`).

### 10.5 남은 일

- ~~터미널 출력의 `path:line` Ctrl+클릭으로 열기~~ → §12.1.
- 파일 패널 하나 = 워크트리 하나(트리 + 내부 탭) 구조다. 파일 하나하나를 별도 Dock 탭으로 두는 방식은 pod-editor를 버퍼 소유자/뷰로 나눠야 해서 남겨 둠. 설계 메모: [docs/13 §7.2](13-editor-and-lsp.md#72-설계-메모-파일마다-별도-dock-탭-보류).
- ~~상태 표시줄 LSP 항목은 표시만 한다~~ → §12.3 (다시 시작 · 끄기 · 로그 보기).
- 키보드만으로의 흐름·포커스 링·Narrator는 백그라운드 픽스처 창에서 확인할 수 없음(D3와 같은 이유) — 대화형 데스크톱에서 수동 확인 필요.
- ~~Ctrl+P는 전역이라 터미널에 ^P를 보내지 못한다~~ → 팔레트 `현재 터미널에 Ctrl+P 보내기`(§12.3).
- ~~데몬 쪽 후속: 작업 삭제 전 편집기 알림, 저장 이벤트~~ → protocol v9 `task.removing` · `task.touch`로 처리(§12.4).

## 11. 네이티브 QA 자동화 (2026-11-09)

`crates/pod-app/src/native_qa.rs`: GPUI 테스트 플랫폼에서 실제 `Workspace`를 띄우고
앱과 같은 키 바인딩(`main.rs`의 `install()`), 실제 DirectWrite 글꼴 측정, 테스트 스레드에서만
응답하는 podd 대역(`source.rs`의 `TestBackend`)으로 키보드 전용 흐름, Esc 후 포커스 복귀,
Tab 순서, 한국어 IME 조합(입력기 핸들러 경로), 배율 1.0–1.75 레이아웃을 검사한다.
`cargo test -p pod-app native_qa -- --nocapture`가 단계마다 포커스 대상을 출력한다.
UIA 트리 덤프(`qa/native/uia-dump.ps1`)는 릴리스 앱의 접근성 이름·역할을 검사한다.
결과와 한계: [qa/native/README.md](../crates/pod-app/qa/native/README.md),
[D3 체크리스트](10-ui-design.md#d3-checklist-and-evidence).

## 12. 후속 fu-app (2026-11-09)

브랜치 `SJin-Choi/fu-app`(master `bf21c27` 위, protocol v9 master `683f0bc` 병합). podd/pod-cli/pod-status/pod-hook, build.rs·assets·설치 프로그램은 건드리지 않았다.

### 12.1 터미널 링크

- **감지**(`pod-terminal-view/src/links.rs`, 순수 함수): `http(s)://…`(끝 문장부호·짝 없는 `)` 제외), `path:line[:col]`(rustc `--> src/a.rs:12:5`, gcc, `grep -n`의 `a.rs:3:본문`은 `a.rs:3`까지), MSVC `file.rs(12,5)` / `file.rs(12)`. Windows(`C:\a\b.rs`, `src\b.rs`)와 POSIX(`src/b.rs`), Git Bash `/c/Users/…` 경로. 경로처럼 보여야 한다(구분자가 있거나 확장자 있는 파일 이름): `12:30:45`, `localhost:8080`, `v1.2:3`은 링크가 아니다. 줄 바꿈으로 잘린 링크는 논리 줄(소프트 랩 행 이어 붙임) 단위로 찾는다. 전각 문자 칸 위치도 맞춘다.
- **확인**: 앱이 `set_link_filter`로 거른다. 주소는 항상, 파일은 터미널 cwd → 그 작업의 워크트리 순으로 풀어서 **존재할 때만** 밑줄·클릭·목록 대상이다.
- **마우스**: Ctrl을 누른 채 링크 위에 있으면 밑줄 + 손 모양 커서(Ctrl을 누르거나 떼기만 해도 바뀜). Ctrl+클릭: 파일은 그 파일을 가진 작업(터미널의 작업 우선)의 파일 패널에서 그 줄·열로 연다(`open_editor`), 주소는 기본 브라우저(`ShellExecuteW "open"`, http/https만 — 링크로 프로그램을 실행하지 않는다). 프로그램이 마우스를 보고받는 중(vim 등)에도 Ctrl+클릭은 링크가 가져간다. 작업 폴더 밖 파일은 알림만.
- **키보드**: 팔레트 `터미널에서 링크 열기` → 현재 터미널 화면의 링크 목록(파일/주소 표시) → Enter.

### 12.2 훅 설치 안내 (docs/14 §11.5)

`hooks_prompt.rs`. 시작할 때 앱 옆 `pod.exe hooks status --json`을 백그라운드로 실행(`CREATE_NO_WINDOW`). 판정: 사용하는 에이전트(설정 폴더가 있는 것)에 Pod 훅이 없으면 **없음**, 훅이 다른 `pod-hook.exe`를 가리키거나 그 파일이 없거나, 설치본 버전(`bin_version`)이 이 Pod의 `source_version`과 다르거나(버전이 없으면 이전 빌드), 바이트가 다르면(`bin_current=false`) **다름**.

- 작업 영역 위 배너(역할 Alert, 포커스를 빼앗지 않음): `훅 설치` / `나중에`. 팔레트 `에이전트 훅 설치`로도 연다.
- `훅 설치` → 먼저 `pod hooks install --dry-run --json`으로 바뀔 것(pod-hook.exe 복사 위치, Claude `settings.json`, Codex `hooks.json`, Codex `config.toml` 신뢰 기록)과 "쓰기 전에 `<파일>.pod-backup-<시각>` 백업"을 대화 상자에 보여 준다. **설치** 단추를 눌러야만 실제 `pod hooks install --json`을 실행하고, 결과 알림에 백업 경로를 보여 준다. 조용히 설치하는 경로는 없다. `pod hooks uninstall`은 앱 어디에도 없다(설치 프로그램·CLI만).
- `나중에` → `settings.json`의 `hooks_dismissed`에 `<앱 버전>/<pod-hook.exe 크기+FNV 해시>` 저장. 같은 빌드에서는 다시 묻지 않고, 새 pod-hook 빌드면 다시 묻는다.
- 테스트는 `MockHookCli`만 쓴다. 이 PC에서 실제 `pod hooks install`은 실행하지 않았다.

### 12.3 팔레트·상태 표시줄

- `현재 터미널에 Ctrl+P 보내기`(^P), 기존 `Ctrl+K 보내기`와 같은 방식.
- 상태 표시줄 LSP 항목이 단추가 됐다(Tab으로 도달, Enter/클릭으로 메뉴): **다시 시작**(그 서버 재시작, 꺼져 있으면 켬), **끄기**(마지막으로 쓴 작업 편집기의 언어 서버 전부 종료, `언어 서버 꺼짐` 표시가 남아 다시 켤 수 있음), **로그 보기**(상태·루트·서버 stderr 마지막 200줄·설정 오류 대화 상자).
- 도움말 단축키에 `Ctrl+.`(코드 작업)과 `Ctrl+클릭`(터미널 링크) 추가.

### 12.4 protocol v9 (docs/03 §4.3)

- 상태 연결이 `status.subscribe {lifecycle: true}`. `terminal.changed`·`task.changed`·`status.lagged`가 오면 제어 연결이 목록을 즉시 다시 읽고, 수명 스트림이 살아 있는 동안 목록 폴링은 0.5 s/3 s → 15 s/45 s 안전망으로 줄인다(v8 이하 데몬은 예전대로).
- 편집기 저장 → `task.touch {task, paths}`. `task.changed` `touched`/`git` → 그 작업의 Review가 diff를 다시 읽는다.
- `task.removing`(앱 또는 `pod task remove`): 그 작업 편집기에 저장 안 한 내용이 있으면 **저장**하고 경고 알림(데몬이 유예 후 다시 검사해 `TASK_DIRTY`로 멈추므로 입력이 사라지지 않는다), 깨끗하면 편집기를 닫는다(LSP 종료). 앱에서 "저장하지 않고 삭제"를 고른 삭제는 닫기만 한다.
- `terminal.lagged`(또는 앱이 밀림): 패널 오른쪽 아래에 4초간 `출력이 밀려 화면을 다시 받았습니다`.

### 12.5 검증

`crates/pod-app/qa/followups/`(README에 명령·결과). 네이티브 QA(`native_qa.rs` f_*, g_*): 링크 목록·키보드 선택기로 편집기 2번째 줄, Ctrl 누름/뗌 밑줄, 일반 클릭은 무시·Ctrl+클릭은 주소 열기(테스트에서는 기록만), 프로그램에 입력 안 감; ^P/^K 전달; 훅 배너 → 취소는 dry-run만 → 설치 확인 뒤에만 실제 설치 1회 → 배너 사라짐; 나중에 기억(같은 빌드 무시, 새 빌드 다시 표시); LSP 끄기/로그/다시 시작; 저장 → `task.touch`; `task.removing` 저장/닫기. 단위: 링크 감지 7, 경로 해석 4, 훅 판정 6, v9 매핑 1.

## 13. 작은 후속 small-fixes (2026-11-09)

브랜치 `SJin-Choi/small-fixes`. podd/pod-cli/pod-status/pod-hook은 건드리지 않았다. 증거: `crates/pod-app/qa/small-fixes/`(README).

### 13.1 터미널 넓은 글자(한글·한자·가나·이모지)

- **원인**: 넓은 글자는 2칸을 차지하는데, 글꼴 대체(Cascadia Mono에 한글이 없어 맑은 고딕)로 그린 음절의 자연 폭은 약 0.92 em이고 2칸은 약 1.17 em이다. 글자를 칸 왼쪽에 자연 크기로 그려서 음절마다 0.25 em(150 %에서 약 6 px) 빈칸이 생겼다("문 서 :"). 같은 줄을 Windows Terminal(같은 글꼴, 같은 대체 글꼴)에 띄워 비교하면 한글이 2칸을 거의 꽉 채워 붙어 보인다.
- **수정**(`pod-terminal-view/src/element.rs`): 넓은 글자는 따로 자연 폭으로 모양을 잡고 `wide_fit`으로 배율을 정한다 — 2칸에 맞게 키우거나(최대 1.35배, 줄 높이 1.3 em 안) 넓은 이모지는 줄이고, 남는 폭은 가운데 정렬(장치 픽셀에 맞춤). 블록 커서 안에 다시 그리는 글자와 IME 조합 글자도 같은 함수를 쓴다.
- **선택**: 넓은 글자의 앞 절반에서 끝난 선택이 글자 반쪽만 칠하던 것을 뒤 칸까지 넓힌다(`selection.rs`, 복사 텍스트는 같음).
- **대체 글꼴**: 맑은 고딕을 명시하지 않았다. 시스템 대체가 이미 한글에 맑은 고딕을 고르고, 명시하면 일본어(가나·한자)까지 맑은 고딕으로 바뀐다. D2Coding은 이 PC에 없고 묶지 않았다(글꼴 추가 없음 → 라이선스 변경 없음).
- 테스트: `element.rs` 3(채움·최대 배율·가운데 정렬·장치 픽셀, 축소, 넓은 글자 분리), `selection.rs` `wide_chars` 확장. 화면: 새 장면 `--fixture-scene editor-terminal-wide`(한글·한자·가나·전각·이모지, `영역` 선택, `다` 위 블록 커서).

### 13.1a 가나·이모지 다시 측정 (kana, 2026-11-09)

브랜치 `SJin-Choi/kana`. 장면 `--fixture-scene editor-terminal-cjk`(히라가나·가타카나·반각 가타카나·한자·간체/번체·한글·이모지·상자 선, 숫자 눈금 줄)와 같은 줄을 Windows Terminal에 띄워 150 %(보조 모니터, DPI 인식 캡처)에서 같은 칸 크기(Pod 16 px = WT 기본 12 pt, 칸 14.0 px)로 비교했다. 도구: `tools/visual/cjk-compare.ps1`(캡처, 결과는 추적 안 함), `tools/visual/cjk-measure.py`(글자마다 잉크 상자 / 2칸 폭).

- **가나**: 대체 글꼴에서 가나·한자·한글의 진행 폭은 모두 1 em이라 같은 배율(×1.172)로 그려진다. Windows Terminal은 자연 크기로 칸 왼쪽에 그린다. 그래서 Pod의 가나가 약간 크고 가나 사이 빈칸은 오히려 3–4 px **좁다**(예: ひ 잉크 0.75 대 0.61 칸). 가나만 넓어 보이는 원인은 없어서 가나·한자·한글 배치는 바꾸지 않았다(한글 모양 유지). 가나가 한자보다 작은 것은 글꼴 설계이고, 잉크 기준으로 글자마다 키우면 한 줄 안에서 크기가 들쭉날쭉해진다(테스트 `one_scale_per_font_whatever_the_ink`).
- **이모지(고침)**: 컬러 이모지는 진행 폭이 약 1.37 em(2칸보다 넓음)인데 잉크는 약 1.0–1.1 em이다. 진행 폭으로 맞춰 0.85배로 줄어서 WT보다 작고 양옆이 비었다. 이제 진행 폭이 2칸을 넘으면 그 글꼴의 잉크 상자(`typographic_bounds`)로 맞춘다: 잉크가 들어가면 자연 크기, 잉크를 가운데. 잉크도 넘칠 때만 줄인다. 측정: 😀 잉크 0.75 → 0.89 칸, 🎉 0.71 → 0.89 칸(WT 0.86/0.82).
- 상자 선·반각 가나·선택·커서는 칸 단위라 바뀌지 않는다. 100 %는 보조 모니터가 150 % 고정이라 단위 테스트(배율 1.0/1.25/1.5/2.0)로만 확인했다.

### 13.2 `cargo test --release`에서 네이티브 QA

- **원인**: GPUI는 `debug_selector` 경계를 `debug_assertions`일 때만 기록한다(GPUI 자신의 cfg). release 프로필은 꺼져 있어 `Harness::bounds`가 늘 `None` → 훅 배너·상태 표시줄·DPI 테스트가 실패했다. `cargo test --release`는 `release` 프로필이라(별도 test 프로필 아님) 프로필 덮어쓰기로 켜면 배포 앱에도 켜진다.
- **수정**: pod-app `qa_marks.rs`의 `qa_id(..)` — 테스트 빌드에서만 칠하지 않는 절대 위치 `canvas`를 자식으로 붙여 부모 경계를 기록(Taffy는 절대 위치 자식을 부모 기준으로 놓음), 테스트가 아니면 아무것도 하지 않는다. 하네스가 전체 새로 그리기 직전에 기록을 비워 사라진 요소는 없게 나온다. 툴킷 대화 상자는 vendor 패치 `dialog::set_dialog_bounds_observer`(설정했을 때만 같은 방식, `POD-PATCHES.md`)로 `dialog-<층>`을 기록한다. `debug_selector` 13곳을 `qa_id`로 바꿨다.
- 결과: `cargo test --release -p pod-app native_qa` 19/19 통과(이전에는 경계를 쓰는 테스트 실패).

### 13.3 `cargo test --workspace`가 doctest 단계에서 255로 끝난 일

- **재현 안 됨**: 이 워크트리에서 debug 백그라운드 1회, debug 전경(PowerShell `2>&1 | Tee-Object`, fu-app과 같은 방식) 1회, release 1회 모두 모든 `Doc-tests` 단계까지 가서 종료 코드 0.
- **원인 판단**: fu-app 로그는 `Doc-tests pod_editor` 요약 뒤에서 끊기고(다음 `pod_status` 단계 줄이 없음) 실패 줄이 없다. 255는 프로세스 종료 코드 -1(Git Bash/cmd에서 255)이고, `Stop-Process`/`TerminateProcess(-1)`로 강제 종료하면 이 값이 나온다(확인함: `Stop-Process -Force` → -1, `taskkill /F` → 1). 저장소 안의 종료 경로(podd Job Object, holder, LSP Job Object, pod-hook)는 모두 종료 코드 1 또는 0을 쓴다 → 테스트 코드가 cargo를 죽인 것이 아니라 **바깥에서 명령이 강제 종료**된 것(긴 처음 빌드 + 테스트를 도구 호출 하나로 돌리다 시간 제한·취소)으로 판단한다.
- **권장**: 처음 빌드는 `cargo test --workspace --no-run`으로 따로, 긴 묶음은 백그라운드로 돌리고 로그 끝의 마지막 `Doc-tests podd` 요약과 종료 코드를 같이 확인한다.

### 13.4 편집기 (docs/13 §7.1)

스니펫 탭 정지 세션(Tab / Shift+Tab / Esc), 완성의 `additionalTextEdits`(지연 해석 포함), `workspace/applyEdit`(편집기가 적용한 뒤 결과를 서버에 답함), 열린 버퍼 편집을 실행 취소 가능한 범위 편집으로. 실제 rust-analyzer로 import 빠른 수정·자동 import 완성·스니펫 정지를 고정 저장소에서 확인했다. 파일마다 별도 Dock 탭은 설계 메모만(docs/13 §7.2).

### 13.5 검증 (master `6a61c3d` 병합 뒤)

| 항목 | 결과 |
|---|---|
| `cargo test --workspace -- --test-threads=4` | 424 통과, 실패 0, ignored 3, 종료 0 (doctest 7단계 모두, 207 s) |
| `cargo test --workspace --release -- --test-threads=4` | 424 통과, 실패 0, ignored 3, 종료 0 (네이티브 QA 포함, 253 s) |
| `cargo clippy -p pod-app -p pod-terminal-view -p pod-editor --all-targets -- -D warnings` | 경고 0 (vendor `gpui-component --lib`도 0) |
| `cargo deny check` | `advisories ok, bans ok, licenses ok, sources ok` (기존 ISC 미사용 경고만) |
| 시각 회귀(28장면, 병합 전 빌드) | 28/28 통과, 바뀐 픽셀 0, 기준 이미지 변경 없음 |
| rust-analyzer 실제 확인 | `rust_analyzer_code_actions` 통과, 끝난 뒤 rust-analyzer 프로세스 0 |

병합으로 들어온 PR 패널(`pr_panel.rs`)과 리뷰 사이드바 탭(`review.rs`)의 `debug_selector` 9곳도 `qa_id`로 바꿨다(`pr_qa` 테스트가 release에서도 경계를 찾도록).

## 14. 확장 (P6 part 2, 2026-11-09)

설정의 "확장 관리"(또는 팔레트 "확장 관리", 시작 옵션 `--extensions`)가 "확장" 페이지를 연다: 모드 목록, 켜기/끄기, 권한 허용/거부, 로그. 실행 중인 모드의 명령과 레이아웃 프리셋은 팔레트에 `모드 이름: 명령`, `레이아웃: 제목`으로 나온다. 모드 알림(`mod.notify`)은 토스트다. 코드 `src/mods_ui.rs`, 설계·검증 [docs/16](16-extensions.md).

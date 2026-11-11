# P0 코어: `podd` 데몬 + `pod-holder` + `pod` CLI

작성일: 2026-11-08 (v2: holder·attach·Job Object·OSC 7501) · 코드: `crates/podd`, `crates/pod-cli` · 상위 기획: [01 §4.2 (1)(3), §5.3, §6 P0](01-plan.md) · 상태 프로토콜: [04 §1, §4.4](04-status-detection-research.md)

P0 완료 기준 "앱 없이 CLI만으로 Claude 실행·재attach"를 만족한다. 터미널은 **holder 프로세스**가 소유한다. 따라서 데몬을 멈추거나 죽였다 다시 켜도 프로그램이 계속 돌고, 새 데몬이 holder에 다시 붙는다. 사람은 `pod attach`로, 에이전트는 `pod run/send/wait/read`로 터미널을 조작한다.
다른 제품의 코드를 쓰지 않은 클린룸 구현이다.

---

## 1. 크레이트 구성

```
crates/podd/                 lib + bin "podd" + bin "pod-holder"
  src/protocol.rs            JSON-RPC 타입(메서드·알림·오류 코드), schema()
  src/client.rs              블로킹 클라이언트, 데몬 자동 시작(시작 락), attach 스트림, v12 hello, spawn_detached
  src/transport.rs           (v12) 줄 단위 duplex 전송: named pipe·자식 stdio·메모리 파이프, 줄 상한
  src/remote_protocol.rs     (v12) `connection.hello` 타입·버전 협상
  src/endpoint.rs            파이프 이름·상태 디렉터리 규칙, POD_SOCKET / POD_TERMINAL_ID
  src/env.rs                 터미널 환경 정리(런처 표식 제거)
  src/keys.rs                키 이름 → 바이트
  ── feature "server" ──
  src/server/mod.rs          데몬 상태(레지스트리)와 하위 모듈 목록
  src/server/runtime.rs      서버 시작·종료, 파이프 수락
  src/server/connection.rs   연결 하나: 요청 읽기, attach·구독 스트리밍
  src/server/rpc.rs          메서드 → 처리기 연결(dispatch)
  src/server/terminal_ops.rs 터미널 run/rm/send 등
  src/server/rediscover.rs   holder 재발견
  src/server/terminal.rs     데몬 쪽 터미널 = holder 연결 + VT 미러
  src/orch_run_protocol.rs   (v16) `orch.*` 타입(기록은 `crates/pod-orch`)
  src/server/orch_runs.rs    (v16) 오케스트레이션: 워커 시작·우편·깨우기·감독 → docs/19 §13
  src/server/remote.rs       (v12) machine_id(상태 디렉터리에 영속)·boot_id·`connection.hello` 처리
  src/holder.rs              pod-holder 본체 (ConPTY·Job Object 소유, 프레임 서버)
  src/holder_proto.rs        데몬↔holder 바이너리 프레임, HolderSpec/HolderRecord
  src/vt.rs                  alacritty_terminal VT + seq 카운터 + ANSI 스냅샷 + OSC 7501 질의 감지
  src/pty.rs                 ConPTY 직접 생성(번들 conpty.dll 우선), Job Object, PATHEXT 해석
  src/winsec.rs              현재 사용자 전용 DACL, 레지스트리 기준 사용자 환경
  build.rs                   vendor/conpty → target/<profile>/ 복사
  examples/tap.rs            디버그: attach해서 원시 스트림(또는 OSC만) 출력
  src/server/tasks.rs        (P3) 저장소 레지스트리·Task 모델 → docs/08
  src/git.rs                 (P3) git CLI 래퍼(porcelain/-z 파서)
  tests/common/mod.rs        통합 테스트 공용 하네스(테스트 전용 파이프·상태·worktree 루트)
  tests/daemon.rs            실제 podd·pod-holder 바이너리로 하는 통합 테스트 6개
  tests/tasks.rs             (P3) Task 통합 테스트 2개
crates/pod-cli/              bin "pod" (clap), src/attach.rs = 대화형 콘솔 클라이언트
tools/fetch-conpty.ps1       Microsoft ConPTY NuGet 패키지 받기(버전·SHA-256 고정)
deny.toml                    cargo-deny 라이선스/출처 정책
```

CLI가 별도 크레이트인 이유는 v1과 같다. `pod-cli`는 `podd = { package = "pod-client" }`로 프로토콜 타입과 블로킹 클라이언트만 쓴다(tokio·alacritty·데몬 코드 의존 없음, docs/20-refactor D-T1).

## 2. 프로세스 모델

```
pod / pod attach / (P2) pod-app ──┐  \\.\pipe\podd-<USER>  (NDJSON JSON-RPC 2.0, 현재 사용자 DACL)
에이전트 안의 pod (POD_SOCKET) ────┘
                                   podd  (분리 실행, 콘솔·창 없음, 단일 인스턴스)
                                    │  터미널마다 \\.\pipe\pod-holder-<podd-USER>-<id> (바이너리 프레임)
              ┌─────────────────────┼──────────────────────┐
         pod-holder t1         pod-holder t2            ...   (분리 실행, 데몬과 수명 독립)
          ConPTY(OpenConsole)    ConPTY
          Job Object ─ claude    Job Object ─ powershell ─ (자식들)
```

- **holder**: 터미널 하나당 프로세스 하나. ConPTY와 자식 프로세스 트리(Job Object)를 소유한다. 출력을 읽어 자기 VT에 먹이고(질의 응답·스냅샷용), 연결된 데몬에 원시 바이트로 흘려보낸다. 데몬이 없어도 계속 돈다.
- **데몬**: holder를 띄우고 연결한다. holder 스트림으로 **VT 미러**를 유지해 `read`/`wait`/attach 스냅샷을 처리한다. attach 클라이언트에는 원시 바이트를 중계한다.
- **시작·재발견**: holder는 `%LOCALAPPDATA%\pod\state\<pipe key>\<id>.json`(HolderRecord: id, name, pipe, holder_pid, child_pid)을 쓴다. 데몬은 시작할 때 이 디렉터리를 훑어 각 holder 파이프에 연결한다. holder들에 동시에 연결하며 각각 최대 5초(바쁜 파이프 대기·handshake 포함)라 멈춘 holder 하나가 시작을 막지 않는다. 연결되면 터미널로 복원하고, 파이프가 없으면(NotFound) 기록을 지운다. 그 밖의 실패(시간 초과·접근 오류·잘못된 프레임)와 읽을 수 없는 기록은 기록을 남기고 그 id를 예약한다. 다음 id는 기록 파일(`t<n>.json`, 읽을 수 없는 것 포함)과 복원한 터미널 중 최대 id + 1이다(daemon-03/04).
  - **연결 못 한 holder(v19)**: 숨기지 않고 `terminal.list`에 `state: unreachable` + `unreachable`(사유)로 둔다. `pod ls`는 기본 목록에도 보이고 COMMAND 칸에 사유를 쓴다. 데몬은 뒤에서 다시 연결한다(기다림 2·4·8·16·32·60초, 6회. `POD_HOLDER_RETRY_MS`·`POD_HOLDER_RETRIES`로 조정). 연결되면 보통 터미널로 목록에 들어가고 `terminal.changed created`를 낸다. 파이프나 기록이 없어지면 기록을 지운다. 재시도 한 번(round)은 터미널 시작으로 센다: `server.stop {kill:true}`는 그 round가 끝날 때까지 기다린 뒤 kill 대상을 모으고, stop 뒤에는 새 round를 시작하지 않는다. 그래도 stop 뒤에 붙은 holder는 직접 kill·release한다(daemon-security-04). 다 실패하면 사유에 "gave up"을 붙여 계속 보이고 id도 예약된 채다. `pod kill <id>`(`terminal.kill`/`terminal.rm`)가 포기한다: 목록에서 빼고 기록·로그·토큰을 지워 id 예약을 푼다. holder 프로세스는 건드리지 않는다(hello로 확인한 pid가 아니라 재사용됐을 수 있음). 그 밖의 요청은 `TERMINAL_NOT_FOUND`(사유 포함)다. `connection.hello`로 19 미만을 고른 연결에는 `exited` + `lost`로 보낸다(옛 클라이언트가 목록 전체를 못 읽는 일 방지). list만이 아니라 그 연결로 나가는 모든 응답·알림(`terminal.kill`·run·resize·wait·attach·read·task 응답, `status.subscribe`의 `terminal.changed` 등)을 한곳(`write_json` → `downgrade_json`)에서 바꾼다(ui-editor-03). hello를 안 보내는 로컬 클라이언트는 같은 빌드로 본다. 테스트: `tests/unreachable.rs`, `tests/rediscover.rs`.
- **터미널 생성**: 데몬이 `<id>.spec.json`(argv, cwd, **완성된 env**, 크기, 파이프 이름)을 쓰고 `pod-holder --spec <file>`을 분리 실행한다. 그다음 holder 파이프가 열릴 때까지 연결을 재시도한다(전체 최대 15초. 연결 한 번은 바쁜 파이프 대기와 handshake를 합쳐 최대 10초). holder가 그 전에 끝나면 `<id>.err`의 메시지를 `SPAWN_FAILED`로 돌려준다.
- **분리 실행**: `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW`와 `CREATE_BREAKAWAY_FROM_JOB`을 함께 쓴다. 잡이 breakaway를 막으면 breakaway 없이 다시 시도한다. 데몬→holder와 CLI·pod-app→데몬에 같은 함수(`client::spawn_detached` = `detach.rs`)를 쓴다.
  - **상속 핸들 없음**(podd-hardening): `std::process::Command`는 `bInheritHandles = TRUE`로 호출자의 상속 가능한 핸들을 **전부** 넘긴다. 그래서 `pod ls | ...`가 데몬을 자동 시작하면 podd가 호출자의 stdout 파이프를 물려받아, 읽는 쪽이 podd가 끝날 때까지 EOF를 받지 못했다(잡이 breakaway를 막는 환경, 즉 터미널·에이전트·CI 안에서 재현). 이제 `CreateProcessW`를 직접 부르고 `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`로 상속 집합을 새로 연 `NUL` 핸들 하나로 제한한다. 그 핸들이 stdin/stdout/stderr다(`STARTF_USESTDHANDLES`). 회귀 테스트: `tests/detach.rs`(breakaway를 막는 잡 안에서 stdout을 파이프로 받은 `pod ls`가 데몬이 살아 있는 동안 EOF에 도달), `detach::tests::child_does_not_inherit_our_pipe`(같은 조건에서 예전 `Command` 방식은 실패함을 확인했다).
- **수명**:
  - `server.stop`(기본): 데몬만 끝나고 holder는 계속 돈다.
  - `server.stop {kill:true}`: 모든 터미널을 kill한 뒤 release하고 끝난다. 아직 시작 중인 `terminal.run`도 포함한다: kill 대상을 모으기 전에 시작 중인 run을 기다리고(최대 20초), stop을 본 run은 자기 holder를 직접 끝낸 뒤 `SERVER_STOPPING`으로 답한다(daemon-02, `tests/stop_race.rs`).
  - **멈추는 동안**(stop-race, docs/15 §9): 데몬은 응답을 보낸 뒤 더는 연결을 받지 않지만, 남은 파이프 인스턴스를 스스로 붙잡아 **프로세스가 끝날 때까지 파이프 이름을 유지**한다. 그래서 그사이 접속하는 클라이언트는 아무도 처리하지 않는 인스턴스에 붙지 않고(예전: `daemon closed the connection`) 모든 인스턴스가 바쁜 동안 기다리다(최대 15초) 파이프가 사라지면 새 데몬을 띄운다. 새 데몬도 옛 데몬이 holder와 끊기기 전에는 뜰 수 없다. stop 전에 연결해 둔 클라이언트의 새 요청은 `SERVER_STOPPING`(-32050, "podd is stopping …; nothing was done") 오류를 받는다. 자동 시작 클라이언트(`pod` CLI, 앱)는 이 오류면 옛 데몬이 끝나길 기다려 새 데몬에 같은 요청을 다시 보낸다(최대 30초). 이미 처리 중이던 요청은 최대 3초 끝내고 나간다(긴 `wait`는 끊기고 재연결한다).
  - `pod server stop`은 데몬 pid의 프로세스 핸들을 먼저 연 뒤 stop을 보내고, **프로세스가 끝날 때까지**(파이프·상태 파일 해제) 기다렸다가 `podd stopped …`를 출력한다. `POD_STOP_TIMEOUT`(초, 기본 20) 안에 끝나지 않으면 stderr에 그렇게 쓰고 종료 코드 1. `--json`에 `stopped`, `pid`가 더해졌다. 회귀 테스트: `tests/stop_race.rs`.
  - holder는 **release**를 받아야만 끝난다. release는 `terminal.rm`이 보내며, 자식이 이미 종료된 경우에만 받아들인다.
  - 자식이 끝난 뒤에도 holder는 남아 최종 화면과 종료 코드를 보관하므로, 끝난 터미널도 데몬 재시작 뒤에 그대로 보인다.
  - holder가 죽으면 Job Object(`KILL_ON_JOB_CLOSE`)가 프로세스 트리를 정리한다. 데몬은 이 터미널을 `lost`로 표시한다.
- **단일 데몬 락**: 첫 파이프 인스턴스를 `FILE_FLAG_FIRST_PIPE_INSTANCE`로 만든다. 이미 있으면 "lock held"를 남기고 종료 코드 3으로 끝난다. holder 파이프도 같은 방식이다.
- **보안**: 데몬·holder 파이프 모두 SDDL `D:P(A;;GA;;;<현재 사용자 SID>)`로 만든다. 보호된 DACL이라 Everyone/Anonymous 읽기 권한이 없다. `reject_remote_clients`도 켠다.
- **Kill = 프로세스 트리 전체**: 자식을 `CREATE_SUSPENDED`로 만들고, Job Object에 넣은 뒤에 `ResumeThread`한다. 따라서 자식이 만드는 모든 손자도 잡에 속한다. kill은 `TerminateJobObject`다. 콘솔에 붙지 않은 손자(숨은 별도 콘솔의 `ping`)도 함께 끝나는 것을 테스트로 확인했다.

### 2.1 터미널 환경 정리 (`env.rs`)

`pod`는 아무 셸(에이전트 도구 셸, 다른 IDE의 터미널, VS Code…)에서나 데몬을 자동 시작한다. 그래서 런처의 변수가 터미널로 새면 안 된다. 실제로 겪은 사례는 두 가지다. `NO_COLOR=1`이면 Claude 색이 사라진다. `CLAUDECODE`/`CLAUDE_CODE_*` 표식이 있으면 Claude가 자신을 자식 세션으로 착각한다(트랜스크립트를 끄고 경고를 띄운다). 환경은 다음 순서로 만든다.

1. **바탕**: `CreateEnvironmentBlock(token, FALSE)`. 레지스트리의 시스템·사용자 변수만 쓰므로 새로 로그온한 것과 같다. 실패하면 데몬 자신의 환경을 쓴다.
2. **제거**: `NO_COLOR`, `FORCE_COLOR`, `CLICOLOR_FORCE`, `CLAUDECODE`, `CLAUDE_PID`, `CLAUDE_EFFORT`, `TERM`, `TERM_PROGRAM(_VERSION)`, `TERM_SESSION_ID`, `COLORTERM`, `WT_*`, `ConEmu*`, `POD_*`, `CLAUDE_CODE_*`, `CODEX_*`, `VSCODE_*`, 부모 IDE·터미널 멀티플렉서가 넣는 접두어 변수(목록은 `crates/podd/src/env.rs`). 사용자 설정은 남긴다: `CODEX_HOME`, `CLAUDE_CODE_USE_BEDROCK/VERTEX`, `CLAUDE_CODE_SKIP_*_AUTH`, `CLAUDE_CODE_GIT_BASH_PATH`, `CLAUDE_CODE_MAX_OUTPUT_TOKENS`, `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC`.
3. **추가**: `TERM=xterm-256color`, `COLORTERM=truecolor`, `POD_TERMINAL_ID=<id>`, `POD_SOCKET=<데몬 파이프>`. 데몬 옆에 `pod.exe`가 있으면 데몬 폴더를 `PATH` 맨 앞에 넣는다(Pod가 PATH에 설치되지 않은 개발·포터블·격리 실행에서도 에이전트가 같은 버전의 `pod`를 쓰게, docs/19 §17).
4. **덮어쓰기**: `terminal.run`의 `env`(무엇이든 설정 가능). `pod run --inherit-env`는 CLI 셸의 환경을 (2)로 걸러 `env`로 보낸다(venv 같은 셸 상태가 필요할 때).

`argv[0]`은 자식 환경의 `PATH`/`PATHEXT`로 찾는다. `.cmd`/`.bat`(npm의 `claude.cmd` 같은 shim)은 `cmd.exe /d /s /c "..."`로 실행한다.

## 3. 데몬 ↔ holder 채널 (`holder_proto.rs`)

holder의 전용 파이프 위에서 바이너리 프레임을 주고받는다: `u32 len(LE, 이후 바이트 수) | u8 kind | body`. 원시 PTY 바이트는 이 구간에서 base64 없이 그대로 간다. 연결이 하나 새로 오면 이전 연결을 대체한다(재시작한 데몬이 넘겨받는 방식).

| kind | 방향 | body | 의미 |
|---|---|---|---|
| 0x01 Hello | H→D | JSON `{version,id,name,argv,cwd,created_ms,holder_pid,child_pid,cols,rows,conpty}` | 연결 직후 첫 프레임 |
| 0x02 Snapshot | H→D | `u64 seq` + ANSI | holder VT의 현재 상태(스크롤백 전체). 출력 `seq`바이트 직후 상태 |
| 0x03 Output | H→D | `u64 seq` + 바이트 | PTY 출력 `[seq, seq+len)` |
| 0x04 Resized | H→D | `u64 seq, u16 cols, u16 rows` | 이 위치부터 새 크기(스트림 순서 안에서) |
| 0x05 Exited | H→D | JSON `{seq, exit_code, killed, exited_ms}` | 마지막 출력 뒤 |
| 0x06 InputAck (holder 프로토콜 3) | H→D | `u64 written` | 이 연결에서 받은 입력 중 holder가 소비한(PTY에 쓴, 또는 상한 초과로 버린) 누적 바이트. 데몬이 `DaemonHello` ≥ 3을 보낸 연결에만 |
| 0x81 Input | D→H | 바이트 | 자식 입력 |
| 0x82 Resize | D→H | `u16 cols, u16 rows` | ConPTY + holder VT 크기 변경 → Resized로 확인 응답 |
| 0x83 Kill | D→H | – | `TerminateJobObject` |
| 0x84 Release | D→H | – | 종료된 터미널만: 기록 삭제 후 holder 종료 |
| 0x85 DaemonHello (holder 프로토콜 2) | D→H | JSON `{version}` | Hello를 받은 직후. holder 버전이 2 이상일 때만 보낸다(1은 모르는 kind를 오류로 끊는다) |

**버전 협상(podd-gaps2, holder 프로토콜 2)**: Hello의 `version`을 데몬이 확인한다. 지원 범위는 `HOLDER_PROTOCOL_MIN(1)..=HOLDER_PROTOCOL(3)`이다(3: InputAck). 범위 밖이거나 Hello가 이 빌드의 구조로 읽히지 않으면(`ForeignHello`: JSON은 읽히지만 필드가 다름) 연결을 닫고, 그 터미널을 `incompatible_holder` 사유와 함께 **종료된 터미널**로 목록에 둔다(id·이름·argv는 Hello에서 가능한 만큼). 화면에는 `[pod: incompatible holder: holder protocol 99, this podd supports 1..=2. …]` 설명을 쓴다. 입력·리사이즈는 받지 않고, `terminal.rm`은 `force`일 때만 holder 프로세스를 `TerminateProcess`로 끝낸다(Job Object가 트리를 정리). 프로세스가 살아 있으므로 종료 터미널 보존 정책(§7-1)으로는 지우지 않는다. `pod ls`는 기본 목록에도 "incompatible holder"로 보인다. 프로토콜 1 holder는 그대로 동작하며 `DaemonHello`를 받지 않는다. holder 2는 받은 데몬 버전을 로그에 남긴다(지금은 동작이 바뀌지 않음). Hello와 spec에는 선택 필드 `parent_terminal`/`parent_task`가 더해졌다(holder가 들고 있으므로 데몬 재시작 뒤에도 부모가 남는다).

**바이트 예산(daemon-01, holder 프로토콜 3)**: 어느 방향도 큐가 무제한으로 자라지 않는다.
- holder→데몬 출력: 연결별로 아직 쓰지 못한 바이트가 8 MiB를 넘으면 PTY reader가 자리가 날 때까지 기다린다(자식이 자기 출력에서 막히는 backpressure). Hello·Snapshot·Resized·Exited·InputAck 같은 제어 프레임은 기다리지 않는다.
- 데몬→holder 입력: holder가 아직 소비하지 않은 입력(holder 3은 InputAck 기준, 1·2는 파이프에 쓴 기준)이 4 MiB면 `terminal.send`/`keys`/`input`은 아무것도 보내지 않고 `INPUT_BACKLOG`(-32052)로 거절한다. 나중에 다시 보내면 된다. 비어 있을 때는 4 MiB보다 큰 입력 하나도 간다.
- Resize·Kill·Release·DaemonHello는 입력과 다른 큐로 가며 writer가 항상 먼저 보낸다. 입력이 밀려 있어도 kill은 막히지 않는다.
- holder의 PTY 입력 큐는 16 MiB를 넘으면 데몬 입력을 버리고 로그를 남긴다(옛 데몬 대비; 버린 바이트도 InputAck에 넣는다). 질의 응답(holder VT의 DA/DSR·OSC 7501 응답)은 따로 예산을 둔다: 쓰지 못하고 쌓인 응답이 256 KiB 또는 4096개를 넘으면 새 응답을 버리고 holder 로그에 한 번(연속 구간마다) 남긴다(`REPLY_BYTE_CAP`·`REPLY_COUNT_CAP`, daemon-security-02). 입력을 읽지 않는 자식이 질의를 계속 내도 holder 메모리가 늘지 않는다. 테스트: `holder::tests::replies_have_their_own_budget`, `tests/reply_budget.rs`.

holder는 연결이 오면 `Hello → Snapshot(seq) → (종료됐으면 Exited)`를 보내고 구독을 등록한다. 이 전체를 출력 처리와 **같은 락 안에서** 하므로, 첫 Output의 `seq`는 정확히 스냅샷의 `seq`다. 데몬은 Snapshot으로 새 VT 미러를 만들고 Output을 이어서 먹인다. holder VT에 스크롤백 전체가 있으므로 데몬 재시작 뒤 `read --history`도 그대로다.

**질의 응답은 holder만 한다.** holder VT의 DA/DSR 응답과 OSC 7501 응답만 PTY 입력으로 간다. 데몬 미러는 응답하지 않는다. attach 클라이언트가 붙은 실제 터미널이 같은 질의에 답하면 데몬이 그 응답을 attach 입력에서 걸러낸다(§4.1, v7).

## 4. 클라이언트 프로토콜 v2

> v3(P3)에서 `repo.*`·`task.*` 메서드와 `terminal.run`의 `task` 파라미터, 오류 코드 -32006..-32013이 추가됐다. 자세한 내용은 [08-tasks.md](08-tasks.md).
>
> **현재 프로토콜 번호는 19**이다(`PROTOCOL_VERSION`, `server.status.protocol`). v12 = v11 + `connection.hello`(§7.4). v13 = v12 + `mobile.*`(§7.5). v14 = v13 + 작업 세션(`task.resume` 등, [19](19-worktree-sessions-orchestration.md)). v15 = v14 + `sessions.*`(세션 찾기, [18](18-session-discovery.md)). v16 = v15 + 오케스트레이션 `orch.*`(run·item·worker·mail·done, `orch.changed`/`orch.mail` 이벤트, 오류 코드 -32080..-32088, [19 §13](19-worktree-sessions-orchestration.md)). v17 = v16 + 자원 대기열 `orch.queue`, 게이트 선택지 `orch.gate.request`/`orch.gate.decide`와 `gate.*`·`mobile.gate.*`의 추가 필드([19 §14](19-worktree-sessions-orchestration.md)). v18 = v17 + 자원 한도 설정 `orch.limits.get`/`orch.limits.set`([19 §14.4](19-worktree-sessions-orchestration.md)). v19 = v18 + `TerminalState::Unreachable`와 `TerminalInfo.unreachable`(§2 시작·재발견). v11 클라이언트는 그대로 동작한다(`PROTOCOL_MIN = 11`). 아래 v9 설명은 기록으로 둔다.
>
> v9 기록: v9 = v8 + 수명 이벤트(`status.subscribe`의 `lifecycle`, `terminal.changed`·`task.changed`·`task.removing`), `task.touch`, 색 질의 응답 중복 제거와 늦은 응답 차단(`SendResult.dropped_replies`). 모두 추가형(§4.1-6, §4.3, §7.3).

전송: 한 줄에 JSON 하나, JSON-RPC 2.0. 연결 하나에서 요청은 순서대로 처리된다. 전체 스키마는 `pod server schema` 또는 `podd schema`로 볼 수 있다.

| 메서드 | params | result |
|---|---|---|
| `server.status` | `{}` | `{version, protocol, pid, pipe, started_ms, uptime_ms, terminals, running, state_dir}` |
| `server.stop` | `{kill?: bool}` | `{stopping, killed, detached}`. 기본은 터미널을 holder에 남긴다. 이후 요청은 `SERVER_STOPPING`(-32050) |
| `terminal.run` | `{argv, cwd?, name?, env?, cols?, rows?, task?, parent_terminal? (v7)}` | `TerminalInfo` |
| `terminal.list` | `{}` | `{terminals: TerminalInfo[]}` |
| `terminal.send` | `ref + {text, enter?, confirm? (v8, 기본 true)}` | `{id, bytes, submit? (v8)}`. Enter는 별도로 쓴다. v8(e2e-fix1, docs/12 B1)부터 30 ms 고정 대기가 아니라 **프로그램이 텍스트를 그리고 300 ms 조용해진 뒤**(최소 150 ms, 출력이 없으면 1 s, 최대 1 s + 글자당 0.5 ms ≤ 5 s) 누른다. 에이전트 상태가 보이는 터미널이면 4 s 안에 턴 시작(working/blocked/turn 증가)을 확인하고 없으면 Enter를 한 번 더 누른다. 결과 `submit = {state: confirmed\|unconfirmed\|unverifiable, enters, signal?, entered_ms?, settled_ms?}`([11 §3.1](11-review-and-orchestration-backend.md)). `pod send`는 unconfirmed면 경고를 stderr에 쓴다. `--no-confirm`은 확인·재시도 없이 한 번만 |
| `terminal.keys` | `ref + {keys: string[]}` | `{id, bytes}`. `enter tab shift+tab esc backspace space up down left right home end pageup pagedown insert delete f1–f12 ctrl+<a-z@[\]^_> alt+<키>`, 한 글자. 화살표는 DECCKM(앱 커서 모드)을 따른다 |
| `terminal.input` | `ref + {data: base64}` | `{id, bytes, dropped_replies? (v9)}`. 원시 입력(attach 클라이언트가 사용). 중복·늦은 질의 응답은 지운다(§4.1-5, 6) |
| `terminal.read` | `ref + {mode?: screen\|history, lines?}` | `{id, mode, lines, cursor, state}` |
| `terminal.wait` | `ref + {until: "exit" \| {output_contains}, timeout_ms?}` | `{outcome: matched\|timed_out\|exited, terminal, waited_ms}`. `pod wait`(그리고 `--status`/`--turn-end`/`--gate`)는 데몬이 재시작되어 연결이 끊기면 같은 파이프에 백오프(100 ms → 2 s)로 다시 붙어 남은 시간으로 같은 조건을 다시 기다린다(docs/12 B6). 데몬을 직접 띄우지는 않는다. `POD_RECONNECT_TIMEOUT`(초, 기본 60) 안에 돌아오지 않으면 분명한 오류로 끝난다 |
| `terminal.resize` | `ref + {cols, rows}` | `TerminalInfo`. holder가 Resized로 확인한 뒤에 돌아온다 |
| `terminal.kill` | `ref` | `TerminalInfo`. 프로세스 트리 전체 |
| `terminal.rm` | `ref + {force?}` | `{id}`. 종료된 터미널만 지운다(`force`면 kill 후 지움). holder가 끝난다 |
| `terminal.keep` (v6) | `ref + {keep?: bool = true}` | `{id, kept, expires_ms?}`. 종료 터미널 보존 정책에서 제외(`false`면 다시 포함). `<state dir>\<id>.keep`에 저장 |
| `terminal.attach` | `ref + {scrollback?, cols?, rows?}` | `{terminal, seq, snapshot: base64}`. 이후 연결은 스트림이 된다 |
| `connection.hello` (v12) | `{min_protocol, max_protocol, require?: string[], client?}` | `{protocol, min_protocol, max_protocol, capabilities, machine_id, boot_id, version, os, limits}`. 공통 버전이 없거나 `require`가 없으면 `PROTOCOL_MISMATCH`(-32051, `data`에 데몬 범위·`missing`). 원격 연결 전용(§7.4), 로컬 클라이언트는 보내지 않는다 |

`TerminalInfo = {id, name?, argv, cwd, state, exit_code?, pid?, holder_pid?, cols, rows, created_ms, exited_ms?, killed, lost, attached, conpty?, status?, parent_terminal? (v7), parent_task? (v7), incompatible_holder? (v7)}`

v7 추가(모두 선택 필드, 프로토콜 번호 7): `parent_terminal`은 이 터미널을 띄운 Pod 터미널이다. `pod run`이 Pod 터미널 안에서(같은 데몬의 `POD_SOCKET`) 실행되면 호출자의 `POD_TERMINAL_ID`를 `RunParams.parent_terminal`로 보낸다. 데몬이 아는 터미널일 때만 기록하고, 그 터미널이 속한 Task가 `parent_task`가 된다. 모르는 id는 로그만 남기고 무시한다. Task의 에이전트 터미널은 Task의 부모(`task.create`/`group.create` 호출자)를 부모로 받는다. Rust에서 `TerminalInfo`를 직접 만드는 코드는 `TerminalInfo::new(...)` 또는 `..Default::default()`를 쓴다(pod-app의 테스트 하나가 그렇게 바뀌었다).

오류 코드: 표준 코드 + `-32001` 없음, `-32002` 이름 중복, `-32003` 실행 실패, `-32004` 이미 종료, `-32005` 아직 실행 중.

### 4.1 attach (결정 A: 원시 바이트 + 클라이언트 VT)

1. `terminal.attach` 응답에는 `snapshot`이 들어 있다. 같은 크기의 빈 터미널에 쓰면 현재 화면과 스크롤백 N줄(기본 1000줄)이 재현되는 ANSI다. 함께 오는 `seq`는 그 스냅샷이 대응하는 출력 오프셋이다.
2. 그다음 같은 연결로 알림만 온다.
   - `terminal.output {id, seq, data}`: 원시 바이트 `[seq, seq+len)`. 클라이언트는 `seq == 직전 seq + 직전 len`인지 확인한다. 데몬 미러 락 안에서 스냅샷을 찍고 구독을 등록하므로 빠지거나 겹치는 바이트가 없다.
   - `terminal.resized {id, seq, cols, rows}`: 스트림 순서 안의 크기 변경.
   - `terminal.exited {id, seq, exit_code, killed}`: 마지막 알림. 이후 데몬이 연결을 닫는다.
   - `terminal.lagged {id, seq, queued_bytes, limit_bytes}` (v7): 이 클라이언트에 쌓인 출력이 데몬의 클라이언트별 상한(`podd --attach-queue-limit <bytes>`, 환경 변수 `POD_ATTACH_QUEUE_LIMIT`, 기본 16 MiB)을 넘었다. 남은 큐를 버리고 이 알림을 보낸 뒤 연결을 닫는다. `seq`는 받지 못한 첫 바이트다. 클라이언트는 다시 attach해 새 스냅샷부터 이어 간다(`pod attach`는 자동으로 다시 붙는다). 그래서 읽지 않는 클라이언트가 데몬 메모리를 무한정 늘리지 못한다.
3. 입력과 리사이즈는 **다른 연결로** 보낸다(`terminal.input`/`keys`/`resize`). Windows 동기 파이프 핸들은 I/O를 직렬화하므로, 한 연결에서 읽기를 기다리는 동안 쓰기가 막힌다. 클라이언트가 연결을 닫으면 detach다.
4. **크기 정책**: 마지막 resize가 이긴다. 어떤 클라이언트든 `terminal.resize`나 attach의 `cols/rows`로 크기를 바꿀 수 있고, 모든 attach 클라이언트가 `terminal.resized`를 받는다. `pod attach`는 붙을 때 자기 콘솔 크기로 맞추고, 콘솔 크기가 바뀌면 따라간다(`--no-resize`면 크기를 건드리지 않는다).
5. **질의 응답 중복 제거(v7)**: holder가 DA1(`CSI c`)·DA2(`CSI > c`)·DSR 5/6(`CSI 5 n`/`CSI 6 n`)·DECXCPR(`CSI ? 6 n`)·OSC 7501 질의에 이미 답한다. 데몬은 출력 스트림에서 이 질의를 찾으면 그때 붙어 있는 attach 클라이언트 수만큼 응답을 "기대"한다(5초). `terminal.input`으로 들어온 바이트에서 기대한 종류의 완전한 응답 시퀀스(`CSI ? … c`, `CSI > … c`, `CSI 0 n`, `CSI r;c R`, `CSI ? r;c(;p) R`, `OSC 7501;?:…`)를 하나씩 지우고 나머지만 PTY로 보낸다. 결과 `bytes`는 실제로 보낸 바이트 수다. 기대가 없으면 아무것도 지우지 않는다(수정자 F3 `CSI 1;5R`처럼 CPR과 모양이 같은 키도 질의 직후 5초 안이 아니면 그대로 간다). 한계: 한 `terminal.input` 안에서 완결된 응답만 알아본다(두 번에 나뉘어 오면 통과). ~~색 질의(OSC 10/11)는 holder가 답하지 않으므로 대상이 아니다(09 §5-2).~~ v9에서 색 질의와 늦은 응답도 다룬다(아래 6).
6. **색 질의 응답과 늦은 응답(v9, fu-daemon)**: 출력 스캐너가 OSC 10/11/12 질의(`OSC 10;?`, `OSC 10;?;?`는 10과 11)와 OSC 4 질의(`OSC 4;n;?`, 색 번호마다 하나)도 찾는다. 질의마다 "그때 붙은 클라이언트 수, 지금까지 받은 응답 수, 질의 시각"을 기록하고 `terminal.input`의 응답을 하나씩 판정한다(`server/query_dedup.rs`).
   - holder가 답하는 질의(DA1/DA2/DSR/CPR/7501): 클라이언트 응답은 모두 중복이라 버린다(v7과 같음).
   - 색 질의: **첫 응답만 통과**, 다른 클라이언트의 응답은 중복으로 버린다. **결정: holder가 팔레트로 직접 답하지 않는다.** 색의 정답은 사용자가 보고 있는 UI 테마이고 holder는 그것을 모른다(설정 팔레트는 앱 테마·`pod attach`가 붙은 Windows Terminal의 테마와 어긋나 밝은/어두운 판단을 틀리게 만든다). 붙은 클라이언트가 없으면 아무도 답하지 않는 것은 실제 터미널 없는 PTY와 같다(프로그램은 시간 초과 후 기본값을 쓴다). 테마가 바뀌어도 클라이언트가 계속 맞는 값을 준다.
   - **늦은 응답 차단(모든 클라이언트)**: 질의 뒤 응답 창(기본 500 ms, `podd --reply-window <ms>`, 환경 변수 `POD_REPLY_WINDOW_MS`, `0`이면 끔)보다 늦게 온 응답, 그리고 기다리는 질의가 없는 응답은 버린다. 프로그램은 이미 기다림을 끝냈으므로 그대로 보내면 키 입력이 된다(docs/12 B4: Codex 입력칸의 `]10;rgb:…\`). 시간은 데몬이 출력에서 질의를 본 순간부터 데몬이 입력을 받은 순간까지다(클라이언트 왕복 포함). 질의는 5 s 동안 기억한다.
   - 키와 겹치는 모양: `CSI r;c R`(CPR)은 수정자 F3(`CSI 1;5R`)와 같으므로 CPR 질의가 기다리는 중일 때만 응답으로 본다(그 밖에는 키로 통과). DA(`CSI ? … c`, `CSI > … c`), `CSI 0 n`, `CSI ? r;c R`, OSC 응답은 사람이 입력하는 모양이 아니다. 색 *설정*(`OSC 10;#fff`가 아니라 입력 쪽 `OSC 10;?`)이나 마우스·포커스·win32-input-mode 키는 응답 모양이 아니어서 그대로 간다. 한계: bracketed paste 안에 응답 모양의 바이트열이 있으면 지워진다. 응답이 두 `terminal.input`으로 나뉘면 여전히 통과한다(끝의 단독 ESC는 Esc 키라서 붙잡아 둘 수 없다).
   - 결과: `terminal.input`의 `SendResult.dropped_replies = {duplicate, late}`(뭔가 지웠을 때만, v9). 데몬 로그에도 남는다.
   - 테스트: `query_dedup::tests`(분할 경계 전부에서 색 질의 포함 스캔, 색 응답 첫째만 통과, 늦은/질의 없는 응답 버림, 정확히 500 ms는 통과, 창 0이면 예전 동작, F3·Alt+]·불완전 시퀀스는 키), `tests/gaps.rs::attach_query_replies_are_not_duplicated`(번들 ConPTY의 실제 holder + fake-agent가 DA1과 OSC 11을 묻는다: 붙은 클라이언트의 DA1 응답 = 중복, 첫 OSC 11 응답은 프로그램에 도착, 질의 없는 두 번째 DA1·색 응답 = late로 버려짐, 사이의 글자는 도착).

스냅샷 재현 범위는 그리드의 문자, SGR(색 16/256/24비트, 굵게·흐리게·기울임·밑줄·반전·숨김·취소선), 소프트 랩, 전각 문자, 커서 위치·표시 여부, 입력 관련 모드다(앱 커서·키패드, bracketed paste, 포커스, 마우스 1000/1002/1003/1005/1006, 삽입, autowrap 끔). v7부터 대체 화면이면 **주 화면(스크롤백 포함)과 그 커서를 먼저 쓰고** `?1049h` 뒤에 대체 화면을 쓴다. 그래서 프로그램이 나중에 `?1049l`로 나오면 원래 주 화면이 보인다(alacritty는 비활성 그리드를 숨기므로 스냅샷을 찍는 순간 그리드를 잠깐 바꿔 읽고 되돌린다. `snapshot_ansi`가 `&mut`인 이유). DECSTBM 스크롤 영역과 origin 모드(`?6h`, 커서는 영역 기준으로 계산)도 재현한다(영역은 바이트 스트림에서 따로 추적: `CSI t;b r`, `ESC c`·`CSI ! p`·리사이즈에서 초기화, 잘못된 영역은 무시 — alacritty와 같은 규칙). 창 제목과 OSC 7501 보고는 전부터 `osc_replay`로 다시 보낸다. **여전히 재현하지 않는 것**: 탭 위치, 문자셋(G0–G3, DEC 선 그리기), pending-wrap 상태, `?1049`가 지키는 것 외의 DECSC 저장 커서, 커서 모양, 팔레트(OSC 4/10/11), 하이퍼링크(OSC 8), kitty 키보드 모드.

### 4.2 `output_contains`와 입력 에코

`terminal.send`로 보낸 텍스트(줄 단위, 최근 32개)를 기억한다. 화면·스크롤백의 줄에서 **바늘을 포함한 보낸 텍스트 부분을 지운 뒤에도** 바늘이 남아 있어야 일치로 본다. 예를 들어 `send "echo pod-ok"` 뒤 `wait pod-ok`는 `PS> echo pod-ok` 줄에서는 맞지 않고 출력 줄 `pod-ok`에서 맞는다. 테스트는 wait 직후 read에 출력 줄이 이미 있는지 확인한다. 한계: TUI 입력 상자에 테두리와 함께 여러 줄로 감긴 에코는 걸러내지 못한다. 이전에 이미 출력된 같은 문자열도 일치한다.

### 4.3 수명 이벤트와 `task.touch` (v9)

폴링 대신 이벤트를 받는 길. 목록 호출(`terminal.list`, `task.list`)은 그대로 있다.

- **구독**: `status.subscribe {ids?, lifecycle: true}`. `lifecycle`은 선택(기본 false)이라 예전 클라이언트는 새 알림을 보지 않는다. 순번은 `status.changed`와 같은 데몬 전체 `seq`다. 느린 구독자는 기존처럼 `status.lagged {missed}`를 받고 목록으로 다시 맞춘다. `ids`를 주면 그 터미널의 `terminal.changed`만 오고 task 알림은 오지 않는다.
- `terminal.changed {seq, id, change, terminal?}`: `change` = `created`(`terminal.run`, task 안 포함) · `exited`(프로그램 종료 또는 holder 유실, `terminal`에 종료 코드) · `removed`(`terminal.rm`, task 삭제, 보존 정책. 그 id의 마지막 알림, `terminal` 없음) · `renamed`(예약: 지금 podd에는 이름을 바꾸는 메서드가 없다). 상태 변화는 계속 `status.changed`다.
- `task.changed {seq, id, change, task?, status?, git?, terminals?, paths?}`: 바뀐 것만 담는다(나머지는 `task.show`).
  - `created`: `task.create` 완료, `task`에 전체 `TaskInfo`.
  - `removed`: `task.remove` 완료(마지막 알림).
  - `status`: 집계 상태(`TaskStatus`)가 바뀜. 태스크 터미널의 `status.changed`마다 git 없이 다시 계산한다. `status`가 없으면 상태 있는 터미널이 없어진 것.
  - `git`: ahead/behind/dirty/unpushed/HEAD가 바뀜, `git`에 새 값. 처음 본 값은 기준이라 알림이 없다. 다시 계산하는 때: 태스크 터미널의 상태가 바뀐 뒤(에이전트 턴이 보통 파일을 바꾸거나 커밋한다), `task.touch`, 그리고 lifecycle 구독자가 하나라도 있으면 5 s마다(Pod 밖의 편집). 구독자가 없으면 폴링하지 않는다. git은 250 ms마다 모아서 한 태스크씩 돈다.
  - `terminals`: 태스크의 터미널 목록이 바뀜(새 목록).
  - `touched`: 클라이언트가 파일을 저장했다(`task.touch`). 개수가 같아도 diff 내용은 바뀌었을 수 있으니 리뷰·diff 뷰는 새로 고친다. `paths`는 클라이언트가 준 그대로.
  - `renamed`: 예약(태스크 이름은 브랜치·워크트리 경로를 정하므로 바꾸는 메서드가 없다).
- `task.removing {seq, id, worktree, force, grace_ms}`: `task.remove`가 검사(dirty/unpushed)를 통과하고 터미널을 죽이고 워크트리를 지우기 **직전**. 편집기는 그 워크트리의 파일을 저장하거나 닫는다. lifecycle 구독자가 있으면 데몬이 `grace_ms`(기본 300 ms, `POD_TASK_REMOVE_GRACE_MS`, 최대 10 s, 구독자가 없으면 0) 기다린 뒤, force가 아니면 **다시 검사한다**: 편집기가 그 사이 저장해서 dirty가 되면 `TASK_DIRTY`로 멈추고 워크트리는 남는다(이후 `removed`는 오지 않는다). `TaskRemoveParams`에는 필드를 더하지 않았다(pod-app이 구조체를 직접 만든다).
- `task.touch {task, paths?}` → `{task, git?, git_error?, git_changed}`: 클라이언트가 워크트리에 파일을 저장했다고 알린다. 데몬은 git 사실을 바로 다시 계산하고(바뀌었으면 `task.changed git`), 이어서 `task.changed touched`를 보낸다. CLI: `pod task touch <task> [paths…]`.
- Rust 클라이언트: `podd::client::SubscribeEvent::{Terminal, Task, TaskRemoving}`(enum은 `non_exhaustive`).
- 코드: `server/lifecycle.rs`(태스크별 마지막 상태·git 값과 워커), 발행 지점은 `server/terminal_ops.rs`(run/rm), `server/terminal.rs`(종료), `server/tasks.rs`(create/remove/터미널 목록).
- 테스트: `tests/lifecycle.rs::lifecycle_events_and_task_touch`(실제 podd/holder, 임시 git 저장소: 터미널 created→exited(코드 3)→removed와 seq 증가, task created(TaskInfo 포함), 파일 저장 + `task.touch` → `git_changed`, `git`(dirty 1) 다음 `touched`(paths), 변화 없는 touch는 `touched`만, Pod 밖 커밋은 폴링으로 `git`(ahead 1, dirty 0), 태스크 안 `terminal.run` → `terminals`, `task.removing` 뒤 grace 동안 저장 → `TASK_DIRTY`이고 파일 유지, force 삭제 → `removing`·터미널 `removed`·task `removed`, `lifecycle` 없는 구독자는 새 알림을 하나도 받지 않음), `lifecycle::tests`(git 기준값·변화, 상태 변화 판정).

**pod-app이 폴링을 줄이는 방법(pod-app 후속, 이번 작업에서 pod-app은 고치지 않음)**: 지금 pod-app은 0.5 s마다 `terminal.list`, 3 s마다 `task.list`(git 계산 포함)를 부른다. (1) 상태 연결의 `status.subscribe`에 `lifecycle: true`를 더하고 `SubscribeEvent::Terminal/Task/TaskRemoving`을 처리한다. (2) 시작할 때와 `status.lagged`·재연결·`epoch` 변화 때만 `terminal.list`/`task.list`를 부르고, 이후는 이벤트로 모델을 고친다(`terminal.changed`의 `terminal`, `task.changed created`의 `task`, `status`/`git`/`terminals` 필드). 안전망으로 목록 폴링을 30–60 s로 늘려 남겨도 된다. (3) 편집기가 저장하면 `task.touch {task, paths}`를 부르고, 리뷰·diff 패널은 `touched`/`git`에서 `task.diff`를 다시 부른다. (4) `task.removing`을 받으면 그 워크트리의 편집기 버퍼를 저장/닫고 LSP를 멈춘다(앱 자신의 삭제 대화는 그대로). (5) 앱의 `ReplyFilter`(500 ms)는 데몬 쪽 차단과 겹쳐도 해가 없다. 붙은 패널이 여럿이어도 색 응답은 데몬이 하나만 통과시킨다. (6) `SendResult.dropped_replies`는 진단용(무시해도 됨).

## 5. ConPTY와 OSC 7501 (docs/04 §1.4의 해결)

**결론**: Windows 기본(inbox) ConPTY에서는 OSC 7501 응답이 입력 경로에서 버려진다. Microsoft의 최신 ConPTY(`conpty.dll` + `OpenConsole.exe`)를 쓰면 응답이 통과하고 Claude가 상태 보고를 켠다. Pod는 번들 ConPTY를 기본으로 쓴다.

- **응답 주체**: holder가 출력 스트림에서 `ESC]7501;?`(BEL 또는 ST 종결, 청크 경계를 넘어도 됨)를 찾는다. 찾으면 그 위치까지만 VT에 먹이고 곧바로 `ESC]7501;?:states=idle,working,done,blocked,error:kinds=permission,question,auth ESC\`를 입력으로 쓴다. 그다음 나머지를 먹인다. 그래서 응답이 **같은 청크의 DA1 응답보다 먼저** 나간다(Claude는 DA1 응답을 "응답 없음" 판정 경계로 쓴다). 보고(`state=...`)는 해석하지 않고 원시 스트림 그대로 attach 클라이언트에 전달한다(파서는 pod-status 크레이트 담당).
- **번들 ConPTY**: NuGet `Microsoft.Windows.Console.ConPTY` 1.25.260930003(MIT)을 쓴다. `tools/fetch-conpty.ps1`이 고정 버전·SHA-256으로 받아 `vendor/conpty/`(git 무시)에 둔다. `build.rs`가 이를 `target/<profile>/`로 복사한다(`conpty.dll`, `x64\OpenConsole.exe`, 패키지 배치와 같음). `pty.rs`는 실행 파일 옆의 `conpty.dll`을 `LoadLibraryExW`로 읽어 `ConptyCreatePseudoConsole/Resize/Close`를 쓰고, 없으면 kernel32의 inbox 함수를 쓴다. `POD_CONPTY=inbox`는 inbox를 강제하고, `POD_CONPTY=<dir>`는 디렉터리를 지정한다. 어느 쪽인지는 `TerminalInfo.conpty`에 나온다. 두 바이너리 모두 Microsoft Authenticode 서명이 유효함을 확인했다.
- **증거 1: Claude Code 2.1.319, 신뢰된 폴더, 키 입력 없음.** `pod run --name c -- claude` 직후 `tap c --only-osc`로 원시 스트림을 본 결과:
  ```
  번들:  0.971 OSC \x1b]7501;?\x07
         1.071 OSC \x1b]7501;state=idle:app=claude-code\x07      ← 상태 보고 켜짐
  inbox: 1.691 OSC \x1b]7501;?\x07
         (12초 동안 7501 보고 없음)
  ```
- **증거 2: 원인 프로브.** PowerShell이 `ESC]7501;? BEL` + `ESC[c`(DA1)를 출력한 뒤 콘솔 입력으로 받은 것:
  ```
  번들:  <ESC>]7501;?:states=idle,...:kinds=permission,question,auth<ESC>\<ESC>[?6c     ← holder 응답 둘 다 순서대로 도착
  inbox: <ESC>[?61;6;7;21;22;23;24;28;32;42c                                          ← 7501 응답 없음, DA1은 conhost가 직접 응답
  ```
  inbox conhost는 입력 파이프로 들어온 OSC 응답을 버리고 DA1에는 자기가 직접 응답한다. 그래서 Claude는 7501 응답 없이 DA1 응답만 받고 보고를 끈다. 즉 win32-input-mode나 DA1 누락 문제가 아니라 **inbox conhost의 입력 파서가 OSC를 통과시키지 않는 것**이 원인이다.

## 6. 검증 결과

- 단위 테스트(podd 18, pod-cli 4): 프레임 왕복, 스냅샷 재현(색·랩·전각·스크롤백 제한·모드·대체 화면), OSC 7501 질의 스캐너(청크 분할·ST), 환경 필터, 키 매핑, 인자 인용·`.cmd` 해석, SID·DACL, 레지스트리 환경.
- 통합 테스트(`tests/daemon.rs`): 번들/inbox 두 ConPTY 모두에서 통과.
  1. `cmd /c echo hello` 종료 코드·출력; PowerShell send→wait→read(에코 필터); 환경 정리(`NO_COLOR`·`CLAUDECODE`·부모 IDE 변수를 넣고 띄운 데몬에서 터미널 안 값이 비어 있음, `POD_*`·`TERM` 설정됨); keys; 타임아웃; rm 거부/kill/rm 후 holder 종료; 오류 코드
  2. **데몬 재시작 생존**: stop(기본) → holder·셸 생존 → 새 데몬이 같은 pid로 재발견, `read --history`에 이전 출력, send/wait 동작, 새 id는 t2
  3. **attach**: 스냅샷으로 만든 클라이언트 VT + 라이브 바이트 768청크의 seq 연속성, `live 1..300`이 정확히 한 번씩, 리사이즈 알림이 스트림 순서대로, 최종 화면이 데몬과 같음, kill 시 exited 알림의 seq가 일치
  4. **트리 kill**: 숨은 별도 콘솔로 띄운 손자 `ping`까지 kill 후 사라짐
  5. 두 번째 데몬 거부(락), 6. JSON-RPC 오류 코드(없는 프로그램은 `SPAWN_FAILED`)
- 수동 데모(release, Claude Code): `pod run --name c -- claude` → `read c`로 시작 화면 → `server stop` → 상태 "not running"(holder 생존) → `pod ls`가 새 데몬을 자동 시작해 t1 재발견(같은 Claude pid) → `read c`로 같은 화면 → 다른 pod 터미널 안에서 `pod attach c` 실행: 화면 렌더링, 뷰어 콘솔을 90x24로 바꾸자 c도 따라 바뀌고 Claude가 다시 그린 바이트가 라이브로 반영 → `ctrl+]`로 detach("[pod: detached from t1]") → `kill c`, `rm`, Claude·holder 프로세스 없음 → `server stop`. Claude에는 프롬프트를 한 번도 입력하지 않았다.

### 6.1 cargo-deny (`deny.toml`, cargo-deny 0.20.2)

- 라이선스 허용 목록: Apache-2.0(+LLVM-exception), MIT, MIT-0, BSD-2/3, ISC, Zlib, 0BSD, BSL-1.0, CC0-1.0, Unlicense, Unicode-3.0. GPL·AGPL·LGPL은 목록에 없으므로 거부된다. 출처는 crates.io만 허용하고 git 의존은 금지한다.
- 예외 1건: `option-ext`(MPL-2.0, 약한 copyleft). pod-spike의 UI 스택에서만 빌드 시 쓰인다(`gpui-kit-assets → rust-embed → shellexpand → dirs → dirs-sys`). 수정 없이 쓰므로 Apache-2.0 작품에 포함해도 된다. UI 크레이트를 제품화할 때 다시 검토한다.
- 결과: `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`.
- advisories 정책: 취약점은 깊이와 상관없이 실패로 처리한다. "unmaintained"는 워크스페이스가 직접 의존하는 크레이트일 때만 실패로 처리한다(`unmaintained = "workspace"`). 이 정책 전의 엄격한 실행(`all`)에서는 **advisories FAILED**였다: instant, paste, rustybuzz, ttf-parser의 unmaintained 4건이며, 모두 pod-spike의 GPUI 의존(gpui-kit/gpui-pre)에서만 온다. podd·pod-cli만 대상으로 하면(`--exclude pod-spike`) 엄격 정책으로도 전부 통과한다.

## 7. 알려진 한계

1. ~~holder는 release될 때까지 남는다.~~ **고침(v6, podd-hardening)**: 종료 터미널 보존 정책. 종료 후 `podd --retain-exited <기간>`(기본 `24h`, 환경 변수 `POD_RETAIN_EXITED`, `off`면 끔)이 지나면 데몬이 `terminal.rm`과 같은 경로로 지운다(holder release). 예외: `terminal.keep`/`pod keep <t>`로 보존한 터미널, Task에 속한 터미널(Task 삭제가 지운다). `terminal.list`에 `retention_ms`와 터미널별 `retention: [{id, kept, expires_ms?}]`가 붙는다(`TerminalInfo`는 바꾸지 않았다: 구조체를 리터럴로 만드는 클라이언트가 있다). `pod ls`는 실행 중인 터미널만 보이고 숨긴 수를 알려 준다. `pod ls --all`은 전부. 남은 한계: 데몬이 없으면 아무것도 지워지지 않는다(다음 데몬 시작 때 지난 것을 지운다). 테스트: `tests/retention.rs`.
2. ~~attach 중 질의 응답 중복~~ **고침(v7, §4.1-5)**: attach 입력에서 holder가 이미 답한 질의의 응답을 클라이언트 수만큼 지운다. 테스트: `server::query_dedup` 단위 테스트(모든 2분할 청크 경계, 클라이언트당 1회, 만료, 키는 통과), `tests/gaps.rs::attach_query_replies_are_not_duplicated`(실제 holder + fake-agent가 DA1을 묻고, 붙은 클라이언트의 응답은 버려지고 두 번째 응답·텍스트는 프로그램에 도착).
3. ~~attach 구독 큐에 상한이 없다~~ **고침(v7, §4.1-2)**: 클라이언트별 상한 + `terminal.lagged` + 재접속. 테스트: `tests/gaps.rs::slow_attach_client_is_dropped_and_can_reattach`(16 KiB 상한, 읽지 않는 클라이언트에 fake-agent `flood:6000`으로 약 300 KB 출력 → 받은 출력은 연속이고 전체보다 적음, `lagged` 뒤 스트림 종료, `attached = 0`, 다시 attach한 스냅샷에 끝 줄이 있음. e2e-fix1에서 64 KiB/PowerShell 2만 줄 루프(약 28 s, 부하 시 시간 초과)를 바꿔 약 2 s가 되었고, 스트림은 스레드에서 하나의 마감 시각으로 읽는다).
4. ~~스냅샷은 대체 화면 뒤의 주 화면, 창 제목, 스크롤 영역 등을 재현하지 않는다~~ **대부분 고침(v7, §4.1)**: 주 화면·스크롤 영역·origin 모드 추가(창 제목은 이미 재생). 남은 손실은 §4.1 끝의 목록. 테스트: `vt::tests::alt_screen_snapshot_keeps_main_screen`(스냅샷이 원본을 바꾸지 않음, 같은 스냅샷 재현, `?1049l` 뒤 주 화면·커서·색이 원본과 같음), `vt::tests::scroll_region_survives_snapshot`.
5. 에코 필터 한계(§4.2). VT 미러가 데몬과 holder 양쪽에 있어 파싱이 두 번 일어난다(CPU 비용, 큰 문제는 아님).
6. `.cmd` shim은 `cmd /c`를 거치므로 `& | < > ^`가 든 인자는 cmd가 해석한다.
7. 번들 ConPTY는 개발 빌드에서 `tools/fetch-conpty.ps1` + `build.rs` 복사로 배치한다. 배포 패키지(설치 관리자)에 넣는 단계는 아직 없다. x64만 검증했다.
8. ~~데몬과 holder가 같은 버전이라고 가정한다~~ **고침(holder 프로토콜 2, §3)**: 버전 교환과 `incompatible_holder`. 테스트: `holder_proto::tests::foreign_hello_and_version_check`, `tests/gaps.rs::holder_versions_are_negotiated`(테스트가 연 가짜 holder 파이프 두 개: 버전 99는 incompatible로 목록·화면에 나오고 입력은 `TERMINAL_EXITED`, `rm`은 force가 필요하고 force면 holder 프로세스가 끝나고 기록이 지워짐. 버전 1은 실행 중 터미널로 동작하고 입력을 받으며 `DaemonHello`를 받지 않음. 새 id는 재발견한 id 다음부터). 실제 holder가 데몬 버전을 받는 것은 `pod_run_inside_a_terminal_records_its_parent`가 holder 로그로 확인한다.

### 7.1 podd-hardening (프로토콜 v6) 정리

고친 것: 자동 시작 핸들 상속(§2), 종료 터미널 보존 정책(한계 1), `group.check` 백그라운드 작업·리뷰 큐 지연·게이트 만료([11](11-review-and-orchestration-backend.md) §6), 상태 turn·훅 복원([07](07-pod-status.md) §8.5), pod-hook 종료 경로와 시간 예산 테스트([07](07-pod-status.md) §5). v6 변경은 모두 추가형이다(`TerminalInfo`는 그대로 두고 보존 정보는 `ListResult.retention`에 넣었다).

일부러 남긴 것(위험 대비 이득이 작거나 동작 결정이 필요함):
- 한계 2(attach 질의 응답 중복), 3(attach 구독 큐 상한): 상한을 넘을 때 끊으면 pod-app의 재연결 경로를 함께 봐야 한다(pod-app은 이번 범위 밖).
- 한계 8(holder 버전 협상), 07의 "hook blocked 뒤 Esc로 취소 시 최대 120 s"(엔진 우선순위 규칙 변경), 08 §7-1(unpushed 규칙 완화), 11 §6-3(큐 타임아웃): 동작 정책 결정이 필요하다.
- 09 §5-4(제어 연결 하나에 명령이 줄 섬): 클라이언트 쪽 문제다. 데몬은 이미 연결마다 독립적으로 처리하고, 이번에 `group.check`도 연결을 막지 않게 됐다.

### 7.2 podd-gaps2 (프로토콜 v7) 정리

고친 것(모두 추가형, 프로토콜 번호는 한 번만 올림: 6 → 7):
1. **일반 터미널의 부모** (09 §8.6-1): `RunParams.parent_terminal`, `TerminalInfo.parent_terminal/parent_task`(§4). 테스트: `tests/gaps.rs::pod_run_inside_a_terminal_records_its_parent`(PowerShell 터미널 안에서 실제 `pod run` → 부모 기록, API로 아는/모르는 부모, **데몬 재시작 뒤에도 유지**, JSON에서 없는 선택 필드는 생략), `tests/orchestration.rs::run_from_a_task_terminal_records_parent_task`.
2. **attach 응답 중복 제거와 큐 상한** (한계 2, 3).
3. **스냅샷 충실도** (한계 4).
4. **holder 버전 협상** (한계 8).
5. **리뷰 큐 타임아웃** ([11](11-review-and-orchestration-backend.md) §6-3).
6. **훅 blocked 120 s** ([07](07-pod-status.md) §7): 같은 에이전트의 OSC 보고가 턴 끝으로 바뀌면 바로 풀린다.

검증(2026-11-09, 이 worktree의 `target`, sccache, jobs 4, 번들 ConPTY): `cargo build --release` 통과, `cargo test --workspace` 30개 묶음 244개 통과·실패 0, `cargo clippy -p podd -p pod-cli -p pod-status --all-targets` 경고 0, `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`. 기존 `tests/status.rs::hooks_authenticated_and_outrank_osc`는 예전 규칙(훅 blocked가 새 OSC idle보다 우선)을 확인하던 단계를 새 규칙으로 바꿨다. 실제 Claude/Codex 턴은 돌리지 않았다.

일부러 남긴 것:
- 질의 응답 중복 제거는 응답이 한 `terminal.input` 안에서 완결될 때만 동작한다(Windows 콘솔 입력은 보통 한 번에 온다). 색 질의(OSC 10/11)는 holder가 답하지 않으므로 여전히 pod-app 패널이 둘이면 두 번 답한다(09 §5-2, holder가 색 질의에 답하게 하는 것은 팔레트 결정이 필요).
- `terminal.lagged`를 pod-app은 아직 모른다(pod-app은 이번 범위 밖). 연결이 닫히면 pod-app의 기존 재연결 경로를 탄다. 기본 상한 16 MiB는 보통의 UI에서는 닿지 않는다.
- 스냅샷의 남은 손실(§4.1 끝). 탭 위치·문자셋은 alacritty가 공개 API로 내주지 않아 스트림 추적이 필요하고, TUI는 보통 매번 다시 설정하므로 미뤘다.
- holder 2는 데몬 버전을 기록만 한다. 앞으로 프레임이 바뀌면 이 값으로 holder 쪽 분기를 넣는다. 지원 범위 밖 holder를 "이어 받는"(업그레이드) 경로는 없다(사용자가 그 버전의 podd를 쓰거나 `rm --force`).
- 08 §7-1(unpushed 규칙 완화): 동작 정책 결정이 필요해 그대로 둔다.
- 09 §5-4(제어 연결 하나에 명령이 줄 섬): 클라이언트 쪽 문제(그대로).

### 7.3 fu-daemon (프로토콜 v9) 정리

고친 것(모두 추가형, 프로토콜 번호는 한 번만 올림: 8 → 9):
1. **색 질의 응답 중복과 늦은 응답** (09 §9 "podd 후속" 1–2, 12 B4): §4.1-6. 결정과 이유도 거기에.
2. **폴링 대신 이벤트**: `terminal.changed`, `task.changed`, `task.removing`, `task.touch`(§4.3). pod-app이 폴링을 줄이는 방법은 §4.3 끝(pod-app 후속).
3. **`pod hooks uninstall`/`status`** ([07 §9.1](07-pod-status.md)): 제거는 이미 있었다. 이번에 Codex `config.toml`의 `[hooks.state]`에서 Pod 훅의 신뢰 항목도 지우고(백업 후, 다른 도구 항목은 위치가 바뀌면 새 위치로 옮김), `hooks status --json`이 설치된 `pod-hook.exe` 경로·버전과 이 빌드의 원본 경로·버전을 보인다(`pod-hook --version` 추가).
4. §7의 남은 항목 다시 보기: 색 질의는 1로 해결. 나머지(응답이 두 `terminal.input`으로 나뉘는 경우, 스냅샷의 탭 위치·문자셋, holder 업그레이드 경로, 08 §7-1, 09 §5-4)는 싸지 않거나 정책 결정이 필요해 그대로 둔다. 나뉜 응답은 끝의 단독 ESC가 Esc 키와 구별되지 않아 입력을 붙잡아 두면 Esc가 늦어진다.

pod-app 후속(이 작업은 pod-app을 고치지 않음): §4.3 끝의 (1)–(6), 그리고 `terminal.lagged` 처리(§7.2 그대로).

### 7.4 remote-r12 (프로토콜 v12) 정리

[17](17-remote.md) §6의 R1·R2. 모두 추가형이며 v11 로컬 동작은 같다.

1. **전송 분리(R1)**: `client.rs`가 `File`에 묶이지 않고 `transport::Transport`(줄 reader + writer)를 쓴다. named pipe는 `Transport::open_pipe`(overlapped I/O. 동기 핸들은 같은 파일 객체의 읽기·쓰기를 직렬화해서, 한 스레드가 읽기에 막힌 동안 다른 스레드의 쓰기도 멈췄다. 브리지에서 실제로 교착), 자식 stdio는 `from_child`, 그 밖의 `Read`+`Write` 쌍은 `Transport::new`, `Client::from_transport`로 만든다. 줄 상한은 **버퍼를 늘리기 전에** 검사한다(로컬 pipe 64 MiB, 원격 stdio 8 MiB). EOF로 잘린 줄은 `daemon closed the connection`(재접속 판단과 같은 문구), 상한 초과·UTF-8 아님은 프로토콜 오류다. JSON 객체가 아닌 줄(로그인 배너 등)은 건너뛰지 않고 오류로 끝낸다. 데몬도 요청 줄 4 MiB 상한을 읽는 중에 검사하고(예전에는 줄바꿈이 올 때까지 계속 버퍼링), attach/subscribe 연결에서 받는 입력은 버퍼에 모으지 않고 버린다.
2. **`connection.hello`(R2)**: 클라이언트(`Client::hello`)는 `server.status`로 protocol을 보고 12 미만이면 다른 요청 없이 거절한다. 그 뒤 `{min:12, max:이 빌드}`로 hello를 보내 가장 높은 공통 버전을 받는다. 데몬 범위 밖 답, 없는 capability도 거절(`ClientError::is_incompatible`). capability 이름은 docs/17 §5(`remote.files` 등)를 예약했고 지금 데몬은 하나도 광고하지 않는다. `machine_id`는 `<state dir>\machine_id`(128비트 hex)에 한 번 만들어 재시작 후에도 같고, 잘못된 파일은 새로 만든다. `boot_id`는 시작마다 새로 만든다. `limits = {max_line_bytes: 8 MiB, max_request_bytes: 4 MiB, max_output_chunk_bytes: 64 KiB(현재 holder 읽기 단위; docs/17의 32 KiB 목표는 R4), attach_queue_bytes}`.
3. **`pod remote-stdio`(R2)**: stdin/stdout을 이 사용자의 데몬 endpoint에 잇는 짧은 수명 브리지. 데몬이 없으면 `pod` 옆의(또는 `POD_DAEMON`) podd를 분리 실행한다. stdout에는 데몬이 보낸 JSON 객체 줄만 쓰고, 아닌 줄·8 MiB 초과·UTF-8 아님·잘린 줄이면 쓰지 않고 종료(코드 4). stderr 진단은 최대 16줄·줄당 300자. 한쪽이 닫히면 종료하며(stdin EOF 뒤 이미 오는 답은 1초까지 내보냄), 데몬·holder·프로그램은 남는다(브리지는 부모가 아니다).
4. **자동 시작 락**: `connect_or_start`가 데몬을 띄우기 전에 `%LOCALAPPDATA%\pod\start-<pipe key>.lock`을 공유 없이 연다(로그온 세션이 달라도 같은 파일). 기다린 쪽은 락을 얻은 뒤 다시 접속해 먼저 뜬 데몬을 쓴다. 시작 제한 10초가 지나면 로그 경로와 함께 실패한다. 프로세스가 죽으면 OS가 락을 푼다. 파이프 `first_pipe_instance` 락은 그대로 최종 보장이다.

검증: `crates/podd/tests/remote.rs`(메모리 파이프 가짜 서버·`cmd /c type` 자식 stdio·실제 pipe, 분할 줄/UTF-8, EOF, 큰 줄, 배너, attach 전환, v11 거절/v12 성공/미래 비호환·미래 호환, 데몬 큰 요청 줄), `crates/pod-cli/tests/remote_stdio.rs`(격리 podd에 브리지 두 개 동시 시작 → 데몬 한 번 시작, 브리지로 attach, 브리지 종료·강제 종료 뒤 holder/프로그램 생존, stdout 오염 시 클라이언트 실패, stdout EOF, 시작 시간 초과). pod-cli 테스트는 `pod.exe` 옆의 `podd.exe`·`pod-holder.exe`가 필요하다(워크스페이스 빌드).

### 7.5 mobile-m1 (프로토콜 v13) 정리

[17](17-remote.md) §6의 M1. **네트워크 listener 없음**(소켓·TLS·relay 없음). 모두 추가형이다. 코드: `podd/src/mobile_protocol.rs`, `podd/src/server/mobile/`, `server/gates.rs`(`decide_in`), `pod-cli/src/mobile.rs`.

| 메서드 (PC 전용, pipe) | params | result |
|---|---|---|
| `mobile.devices` | `{}` | `{listener: "off", devices: MobileDeviceInfo[], settings}` |
| `mobile.revoke` | `{device?, all?}` | `{revoked}` — 세션 즉시 종료, 권한 비움, `auth_epoch` 증가 |
| `mobile.policy` | `{device, task, allow?, deny?, clear?}` | `MobileDeviceInfo` — 권한 `status`/`output`/`approve`/`message`, 작업별, 기본 거부 |
| `mobile.audit` | `{limit?, device?}` | `{entries: MobileAuditEntry[]}` |
| `mobile.settings` | `{audit_message_bodies?}` | `{audit_message_bodies}` |

- 새 오류 코드(v12 파일 코드와 겹치지 않는 블록): -32060 `MOBILE_DENIED`, -32061 `MOBILE_AUTH`, -32062 `RATE_LIMITED`(`data.retry_after_ms`), -32063 `OPERATION_CONFLICT`, -32064 `MESSAGE_INVALID`.
- **모바일 API는 pipe 메서드가 아니다.** 채널(M2 이후)이 기기를 인증한 뒤 `Daemon::mobile_call(세션, 이름, params)`만 부른다. 이름은 `tasks`·`task`·`output`·`gate.decide`·`message`·`delivery` 여섯 개뿐이고 나머지(모든 pipe 메서드 포함)는 상태를 건드리기 전에 `MOBILE_DENIED`. DTO에는 prompt·argv·환경·경로·토큰이 없다.
- **gate 결정 경로 하나**: `gate.resolve`(PC)와 모바일 `gate.decide`가 같은 `decide_in`을 쓴다. store 락 안에서 operation id 재생/충돌 → 작업 범위 → pending → 현재 시각의 만료(만료 작업자보다 먼저여도 거절) 순으로 검사하고, Gate 전이와 결정 기록(`orch store.gate_ops`: actor·op·이전/새 상태·시각, 최대 2000건·30일)을 **한 번의 저장**으로 커밋한다. 저장 실패면 메모리도 되돌리고 오류(결정 없음). 만료 작업자도 저장 실패 시 되돌린다. PC의 `gate.resolve`가 만료 시각이 지난 pending Gate를 이제 `GATE_RESOLVED`로 거절한다(예전에는 작업자보다 빠르면 통과).
- **메시지**: 검증(최대 1000자·4 KiB·20줄, 줄바꿈 외 제어 문자·ESC·DEL·C1·bidi 제어 문자는 **거절**, 조용히 지우지 않음) → 고정 머리줄(`Message from the user, sent from their phone through Pod ...`)을 붙여 리뷰 전달과 같은 `enqueue_prompt`로 보낸다(bracketed paste → Enter → 제출 확인). docs/11 규칙 그대로: working이면 queued, blocked면 `AGENT_BLOCKED`, 끝났으면 `TERMINAL_EXITED`. `with_open_comments`면 그 작업의 open 리뷰 코멘트를 text를 지시문으로 해서 보낸다. 전달 기록(`DeliveryRecord.origin = {device, operation_id, digest}`)이 멱등 기록이라 재시도는 같은 결과(`replayed`), 같은 id의 다른 요청은 `OPERATION_CONFLICT`. 감사 기록을 못 쓰면 보내지 않는다.
- **세션**: 메모리에만(재시작 시 모두 무효), 절대 8시간·사용자 요청 없이 15분이면 만료(백그라운드 poll은 연장 안 함), 기기당 4개. 기기 최대 3대. 한도: 읽기 60회/분, 결정 10회/분, 메시지 5회/분(기기별, 거절된 시도 포함). 철회·정책은 결정/전달 커밋 동안 mobile 락으로 다시 확인한다(철회는 커밋 앞이거나 뒤, 사이는 없음).
- **감사**: `<state dir>\mobile\audit.jsonl`, 1 MiB에서 `audit.1.jsonl`로 넘김(최대 약 2 MiB), 재시작 뒤에도 남는다. 메시지는 **길이 + SHA-256만** 기본 기록한다: 본문은 이미 전달 기록에 있고 감사 로그는 더 오래·더 넓게 남으므로 두 번째 사본을 만들지 않는다(`pod mobile settings --audit-bodies on`이면 본문도). 성공한 상태 조회는 기록하지 않는다(long-poll이 로그를 채움); 거절·출력 읽기·결정·메시지·PC의 정책/철회는 기록한다.
- **페어링 stub**: 실제 페어링은 M2/M4. 지금은 `Registry::pair_stub`만 있고, 테스트와 **debug 빌드에서 `POD_MOBILE_TEST_HOOKS=1`일 때만** pipe 메서드 `mobile.test.pair/session/call`이 생긴다(release 빌드에는 없음).

CLI: `pod mobile devices`, `pod mobile revoke <m1>|--all`, `pod mobile policy <m1> <task> --allow status,approve [--deny output] [--clear]`, `pod mobile audit [--device m1] [--limit N]`, `pod mobile settings [--audit-bodies on|off]`.

검증: 단위 — `server/mobile/*`(기본 거부·작업별 권한, 철회·기기 수, 손상 파일이면 기기 없음, fake clock 15분/8시간 경계·poll 비연장·재시작·재키잉, 한도 창, 메시지 검증·UTF-8·머리줄, 감사 순환·재시작·잘린 줄, 라우터가 스키마의 모든 메서드·`mobile.*`·test hook 거부, 출력 100줄/16 KiB·제어 문자 제거), `server/gates.rs`(PC/모바일 스레드 경합 50회 한 건만, 재생/충돌, 만료 경계 ±1 ms, 범위, 저장 실패 롤백(가짜·실제 파일), 만료 롤백, 기록 상한). 통합 — `tests/mobile.rs`(idle → 전달·제출 확인·재시도 1회만 입력, working → queued 뒤 전달, blocked 거절, 권한 없음·너무 김·ESC·작업 삭제·한도 초과, 리뷰 코멘트 전달, 감사에 본문 없음/설정 시 본문, 재시작 뒤 세션 무효·기기·감사·op id 유지; Gate 권한·숨은 작업·출력 off 기본, 재생·PC 늦은 결정 거절·stale·만료, PC/모바일 동시 3회 한 건, 철회 즉시 `MOBILE_AUTH`; hook 변수 없으면 stub 없음).

### 7.6 R3: SSH 전송과 머신 프로필 (프로토콜 변경 없음)

[17](17-remote.md) §2.1·§6 R3. 앱 UI 없이 `pod-remote` crate와 CLI만.

1. **프로필**: `%APPDATA%\Pod\machines.json`(`POD_CONFIG_DIR`로 바꿈) `{version:1, machines:[{name, host, user?, port?, identity_file?, known_hosts?}]}`. 다른 필드는 거부(임의 SSH 옵션·ProxyCommand 없음). name은 `A-Za-z0-9._-`, host는 `A-Za-z0-9.-_:%`, user는 `A-Za-z0-9._-@$`이고 둘 다 `-`로 시작·제어 문자·공백 불가, 키/known_hosts는 절대 경로. 손으로 고친 파일도 쓸 때마다 다시 검사한다. CLI: `pod machine add <name> --host H [--user U] [--port P] [--identity-file F] [--known-hosts F]`, `list`, `remove`, `test`(핸드셰이크 후 machine_id/boot_id 출력, `--json`이면 실패도 `{ok:false, kind, …}`).
2. **ssh 실행**: `POD_SSH`(절대 경로만) → `%SystemRoot%\System32\OpenSSH\ssh.exe` → `%ProgramFiles%\OpenSSH\ssh.exe` → PATH의 절대 경로 디렉터리. argv 배열 `-F <생성 설정> -T -o BatchMode=yes -o StrictHostKeyChecking=yes -o PasswordAuthentication=no -o KbdInteractiveAuthentication=no -o ForwardAgent=no -o ForwardX11=no -o ClearAllForwardings=yes -o RequestTTY=no -o PermitLocalCommand=no -o UpdateHostKeys=no -o ConnectTimeout=10 -o ServerAliveInterval=15 -o ServerAliveCountMax=3 -- pod-remote pod remote-stdio`. 설정 파일(`%LOCALAPPDATA%\pod\ssh\*.conf`)에는 `Host pod-remote` 하나와 프로필 값만 쓰고(경로는 `/`, `%`는 `%%`) 핸드셰이크 뒤 지운다. `-F`라 사용자·시스템 ssh_config는 읽지 않는다. host는 argv에 나오지 않는다. 창 없음, `SSH_ASKPASS`·`DISPLAY` 제거.
3. **핸드셰이크·오류**: `Client::hello`(v12 필수, v11은 server.status만 보내고 버전 안내). 제한 `POD_SSH_TIMEOUT`초(기본 30)가 지나면 ssh를 죽인다. stderr는 20줄·줄당 300자만 보관(나머지는 읽어 버림)하고 메시지에는 8줄까지. 종류(`kind`): `ssh_missing`, `host_key_unknown`, `host_key_changed`, `auth_failed`, `timeout`, `unreachable`, `remote_pod_missing`(command not found·`'pod' is not recognized`·종료 127), `version_mismatch`(v11 또는 remote-stdio 없는 옛 pod), `stdout_polluted`(배너 등 JSON 객체가 아닌 줄), `invalid_profile`, `failed`. 메시지는 한국어 요약+할 일+원인+ssh 진단. Pod는 known_hosts·키·서버를 고치지 않는다.
4. **`pod --machine <name> …`**: 위 연결 위의 같은 명령. 클라이언트 라벨 `ssh:<name>`이라 로컬 `POD_TERMINAL_ID`를 부모로 보내지 않고, `run`의 cwd·`repo add` 경로는 주어진 그대로(로컬 현재 디렉터리를 보내지 않음). `--pipe`와 함께 못 쓰고, `attach`(R4)·`server stop`·`hooks`·`machine`·`remote-stdio`·`run --inherit-env`는 ssh 실행 전에 거절. 긴 wait 중 끊기면 재접속하지 않고 오류(R4).

검증: `pod-remote` 단위 테스트(검증 규칙, 설정 내용, argv, 분류, stderr 상한), `pod-remote/tests/system_ssh.rs`(실제 ssh.exe `-G`가 생성 설정·옵션을 그대로 해석, 닫힌 localhost 포트 → unreachable, 설정 삭제), `pod-cli/tests/remote_ssh.rs`(가짜 ssh `pod-fake-ssh.exe`가 argv·설정을 검사하고 `pod remote-stdio`로 격리 podd에 연결: 정상 라우팅·같은 machine_id, 배너, 미등록/변경 호스트 키, 인증 실패, 시간 초과, 원격 pod 없음, 옛 pod, v11, ssh 없음/상대 경로). **실제 SSH 서버 로그인은 미검증**(sshd 미설치; 설치·서비스·키 변경은 하지 않음).

## 8. 다음 단계

- **UI attach(P2)**: pod-app은 `terminal.attach` 스트림을 그대로 쓴다. 비동기(tokio) 클라이언트를 추가하고, 스냅샷 + 라이브 바이트를 앱의 VT(ghostty-vt 전환 시에도 같은 경계)에 먹인다. 다중 attach와 크기 정책은 §4.1 그대로다.
- ~~**이벤트 구독**: `events.subscribe`(created/exited/resized/title + P1 상태 엔진의 7501 보고 → 상태 변화).~~ 상태는 `status.subscribe`(P1), 수명은 v9 `lifecycle`(§4.3). 남은 것: resized/title 알림(attach 스트림에는 resized가 있다).
- ~~attach 응답 중복 필터(한계 2), 구독 백프레셔(한계 3)~~: v7에서 구현(§7.2). (holder 정리 정책은 v6, §7.1.)
- Task = worktree + 터미널: 완료(P3 백엔드, [08](08-tasks.md)). `task.create`가 호출자의 `POD_TERMINAL_ID`를 부모로 기록한다.
- 부모-자식 트리 조회(`wait --children`, Task 트리), `terminal.label`, stitched read. (부모 기록 자체는 v7: `TerminalInfo.parent_terminal`, `terminal.keep`은 v6.)
- 번들 ConPTY의 배포 패키징·서명 확인 자동화, arm64.

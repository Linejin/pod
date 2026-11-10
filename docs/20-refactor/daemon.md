# 데몬 영역: 동작 보존 리팩터링 설계

기준: 2026-11-10, 로컬 `master = HEAD = 8b835ff3930c5bf08ff1e70596ae02b82614246a`. 이 문서는 평가 결과이며 제품 코드 변경이 아니다. README의 문서 목록, docs/03·08·09·10·19의 관련 계약, 아래 소스와 테스트를 읽었다. 외부 제품 내부 구현은 다루지 않는다. UI 작업자의 진행 중 변경은 측정·소유 범위에서 제외했다.

**결정:** `pod-protocol`을 만든다. 기존 경로는 동일 타입을 재노출하는 shim으로 유지한다. 클라이언트가 데몬 크레이트 의존을 완전히 끊으려면 별도로 `pod-client`도 필요하다. 디스패치는 정적 도메인 등록으로 바꾸고, `server/mod.rs`, `orch_runs.rs`, CLI `main.rs`는 책임별 모듈로 나눈다. 락·프로세스·저장 형식·오류·문구를 바꾸는 작업은 섞지 않는다.

## 1. 현재 구조 지도

### 측정 기준과 의존성

LOC는 `src/**/*.rs`의 물리적 줄 수(주석·빈 줄·인라인 테스트 포함), 테스트 수는 `#[test]`/`#[tokio::test…]` 선언 수다. 실행 성공 수가 아니다. Cargo 그래프는 HEAD manifest의 직접 path dependency이며 optional도 포함하고 dev/build dependency는 제외했다. 모듈 fan-in은 명시적 `crate::<module>`/`podd::<module>` 경로가 나타나는 **서로 다른 소스 파일 수**다. 테스트·examples는 제외하고 인라인 테스트와 문서 주석은 포함한 텍스트 지표이므로 컴파일러의 완전한 호출 그래프와 구별한다.

| 크레이트 | src 파일 / LOC | 내부 crate fan-in / fan-out | 단위 / 통합 테스트 선언 |
|---|---:|---|---:|
| podd | 57 / 32,967 | 3(app, cli, remote) / 5(discover, forge, mods, orch, status) | 153 / 96 |
| pod-cli | 15 / 8,069 | 0 / 3(podd, status, remote) | 26 / 16 |
| pod-hook | 1 / 322 | 0 / 1(status) | 0 / 13 |
| pod-status | 7 / 2,883 | 5(app, cli, hook, ui, podd) / 0 | 3 / 67 |
| pod-remote | 3 / 1,208 | 1(cli) / 1(podd) | 7 / 1 |

현재 흐름:

```text
pod-app / pod-cli / pod-remote -> podd(default-features=false)
                                client + transport + protocol facade
pod-hook -> pod-status::hook     (별도 훅 메시지, JSON-RPC 클라이언트 아님)
podd(server) -> server::{tasks, review, groups, gates, sessions, orch_runs, ...}
            -> pod-status / pod-orch / pod-forge / pod-mods / pod-discover
            -> holder_proto -> pod-holder -> PTY/VT
server/mobile -> 제한된 mobile API·정책·세션·감사 (별도 모바일 crate 없음)
```

| 모듈 지표 | 실측 |
|---|---|
| `protocol` fan-in | 85파일 = podd 42 + app 31 + cli 11 + remote 1 |
| `client` / `transport` fan-in | 16 / 8파일 |
| `git` / `holder_proto` fan-in | 11 / 4파일, 모두 podd 내부 |
| `server/mod.rs` 명시적 루트 모듈 fan-out | 8: client, endpoint, env, holder_proto, keys, protocol, transport, winsec (`now_ms` 함수 제외) |
| `orch_runs.rs` 명시적 루트 모듈 fan-out | 3: env, git, protocol; sibling 직접 경로 12개(agent_launch, codex_allowance, gates, orch_config, orch_limits, orch_meter, orch_store, repo_config, sessions, submit, tasks, terminal), 묶음 import의 status 등은 별도 |
| `holder.rs` / `protocol.rs` 루트 모듈 fan-out | 4(holder_proto, pty, vt, winsec) / 9(protocol 하위 도메인) |
| 공개 표면 | `podd/src/lib.rs:17`부터 `pub mod` 24개, `server/mod.rs:8`부터 25개. wire 10파일에는 줄 시작 `pub struct/enum/trait/type` 259선언 + 매크로 생성 Method marker 99개. re-export와 필드는 이 수에 포함하지 않음 |

wire 파일 전수: `protocol.rs` 1,450줄, `discovery_protocol.rs` 253, `mobile_protocol.rs` 367, `mods_protocol.rs` 111, `orch_protocol.rs` 771, `orch_run_protocol.rs` 920, `pr_protocol.rs` 207, `remote_protocol.rs` 209, `session_protocol.rs` 550, `status_protocol.rs` 441. 합계 **10파일/5,279줄**, `schema()`의 `entry::<…>()` **104개**, wire 인라인 테스트 **15개**. `holder_proto.rs`는 별도 바이너리 계약(394줄)이다.

재측정은 `git show <기준SHA>:<파일>`로 고정해서 한다. 디스패치 수는 `dispatch` 본문에서 `Type::NAME =>`, `m if …::handles`, 기본 arm을 각각 세었고, 중복은 공백 정규화한 연속 8줄 비교 후 실제 역할을 확인했다.

### 확장성을 해치는 10가지 근거

아래 경로는 별도 표시 없으면 `crates/podd/src/` 기준이다.

| 문제와 file:line 근거 | 확장 시 비용 / 판단 |
|---|---|
| ① wire와 실행 코드의 패키지 결합. `lib.rs:17`, `protocol.rs:15`, `Cargo.toml:47`; `crates/pod-remote/src/ssh.rs:33`은 Client·HelloResult·Transport 모두 사용 | wire만 옮겨도 daemon 의존은 남는다. feature=false가 이미 PTY/tokio를 빼므로 “현재 모든 클라이언트가 서버 전체를 링크한다”는 주장은 틀림. 타입과 클라이언트 런타임을 각각 추출해야 의존 방향이 명확해짐 |
| ② 중앙 디스패치. `server/mod.rs:709`은 **54 직접 메서드 + 5 위임 + 기본 1 = 60 arm**. 별도로 `:532`, `:539`에서 attach/subscribe 전환 | 질문의 “약 70-arm”과 다름. 신규 메서드가 중앙 match·도메인 목록·schema에 분산 등록된다. 기존 5개 위임을 정적 등록의 출발점으로 활용 |
| ③ `server/mod.rs` 1,568줄에 구성·수명·연결·터미널 연산 혼재. `:151` Daemon, `:228` serve, `:430` StartGuard, `:811` rediscover, `:1027` run, `:1461` stream_attach | 터미널 기능 하나가 종료 경쟁과 프로토콜 협상 코드 옆을 건드린다. 특히 `:523` 요청 계수와 `:1163` stop 뒤 시작된 holder 정리는 이동 시 한 쌍으로 검증해야 함 |
| ④ `server/orch_runs.rs` 3,161줄. `:95` 공유 상태, `:332` 신원, `:395` 재시도 캐시, `:752` 워커 시작, `:1605` 우편, `:2066` RPC, `:2370` 감독, `:2503` 깨우기, `:2679` 게이트 | 새 정책이 RPC·토큰·큐·프로세스·알림을 함께 탐색하게 함. 기존 `orch_config/limits/meter/store`는 이미 분리돼 있으므로 이를 되합치지 않고 경계 보강 |
| ⑤ `crates/pod-cli/src/main.rs` 1,723줄. `:54` Cmd, `:417` 연결+실행, `:833` repo, `:899` task, `:1191` 출력, `:1304` 테스트 | 서브커맨드 추가가 공용 연결 정책과 출력에 결합. `:378`에는 큰 enum/match로 debug 주 스택이 넘쳐 8 MiB 스레드를 쓰는 근거까지 존재. 분리와 동시에 스택 우회책을 제거하면 안 됨 |
| ⑥ 메서드 선언 중복. `protocol.rs:188`, `discovery_protocol.rs:13`, `mobile_protocol.rs:23`, `mods_protocol.rs:15`, `orch_protocol.rs:14`, `orch_run_protocol.rs:27`, `pr_protocol.rs:16`, `session_protocol.rs:16`의 `method!` 8개; `protocol.rs:1183` schema는 수동 목록 | 타입·schema·서버 등록 누락을 컴파일만으로 잡지 못함. `:1415` schema 시험은 MOD 목록을 순회하지도 않음. 공용 매크로와 집합 동등성 시험으로 보강하되 wire 필드는 그대로 |
| ⑦ `server/status.rs` 1,337줄: `:21` 락 순서, `:484` TermStatus, `:1006` RPC 판별, `:1124` 스트림. `crates/pod-status/src/engine.rs` 909줄의 `:536` hook 입력·`:808` 증거 선택 | 엔진은 이미 동기·명시적 시간 입력 경계(`pod-status/src/lib.rs:3`)가 좋다. 이를 유지하며 daemon의 hub/ingest/stream과 엔진의 입력별 처리만 분리; 새 감지원을 RPC/전역 hub와 엮지 않음 |
| ⑧ holder의 제어·예산·I/O 혼재. `holder.rs` 786줄, `:78` 출력 대기, `:167` reply 예산, `:203` 입력 제한, `:308` 시작, `:575` daemon 연결 | 출력 폭주 처리 변경이 kill/resize 제어 지연으로 번질 수 있다. `transport.rs:107`의 동기 line reader와 `server/mod.rs:568` 비동기 reader는 EOF 계약이 다르므로 “비슷한 코드”라고 합치지 않음 |
| ⑨ git 경계가 넓음. `git.rs` 1,266줄, `:42` 실행 환경, `:66` 일반 실행, `:323` worker commit, `:663` worktree, `:862` facts, `:1004` parser | 일반 실행과 저장소 프로그램을 차단하는 bounded worker 실행의 정책이 다름. 한 범용 execute 함수로 통일하면 안전 계약을 잃기 쉬움. 실행 정책·parser·worktree·조회·worker commit을 구분 |
| ⑩ 테스트 helper 복제. `tests/common/mod.rs:59` 격리 daemon이 있지만 `tests/mobile.rs:30`, `tests/orchestration.rs:26`, `tests/orch_runs.rs:27`, `tests/submit.rs:23`에 workspace_bin 복제; `tests/gaps.rs:55`/`tests/mobile.rs:160`/`tests/retention.rs:60`에 until 복제 | 시험 실행 중 중첩 cargo를 시작하고 각 사본의 격리·timeout 수정이 갈라진다. 공용 fixture 준비/대기 helper를 먼저 정리하고, 빌드와 시험 실행을 분리해야 병렬 worktree 검증도 안전 |

줄이 같은 것 모두가 제거 대상은 아니다. `protocol.rs:1037`/`server/tasks.rs:48`의 Task 필드, `orch_protocol.rs:428`/`server/orch_store.rs:94`의 Group 필드 중복은 **wire와 영속 레코드 경계**다. 이를 동일 struct로 합치지 않는다. `transport.rs:33`와 `pod-hook/src/main.rs:188`의 SQOS 설정도 후자는 write-only·총 deadline·항상 성공 종료 계약이므로 Client로 대체하지 않는다.

## 2. 목표 구조와 배치 규칙

```text
pod-protocol/
  protocol.rs + 기존 9개 *_protocol.rs   # 이름/serde/Method 그대로
  lib.rs                                # 공통 method! + 도메인 exports
pod-client/
  client.rs transport.rs endpoint.rs detach.rs env.rs
  lib.rs                                # now_ms, protocol 재노출(이행용)
podd/
  lib.rs                                # 구 API re-export shim
  server/mod.rs                         # 구성/state와 얇은 facade
  server/{runtime,connection,terminal_ops,rediscover}.rs
  server/rpc.rs + rpc/{registry,domains}.rs
  server/orch_runs.rs + orch_runs/{state,identity,records,launch,workers,
                                  mail,supervisor,wake,gates,rpc}.rs
  server/status.rs + status/{hub,ingest,stream}.rs
  holder.rs + holder/{budget,input,process,connection}.rs
  git.rs + git/{command,parse,worktree,query,worker_commit}.rs
pod-cli/
  main.rs args.rs context.rs output.rs
  commands/{mod,terminal,repo,task,server}.rs + 기존 전문 명령 모듈
pod-status/engine.rs + engine/{evidence,input}.rs
          hook.rs + hook/{claude,codex}.rs
pod-hook/main.rs + {relay,install,console}.rs
pod-remote/ssh.rs + ssh/{config,diagnostics,process}.rs
```

### wire 추출과 점진 이행

1. `pod-protocol`은 envelope, `RpcError`/코드, `Method`, params/result/event, serde 규칙, schema, base64·버전 협상·downgrade 같은 순수 wire 연산을 소유한다. `PROTOCOL_VERSION=19`, `PROTOCOL_MIN=11` 유지. holder binary는 `HOLDER_PROTOCOL=3`, 최소 1 그대로 podd에 남긴다(`holder_proto.rs:31`). 훅 v1 메시지는 `pod-status::hook`에 남긴다.
2. 처음에는 `pod-orch` model/text, `pod-forge`, `pod-mods(default-features=false)`, `pod-discover` 타입의 기존 re-export를 유지한다. 이 네 crate에는 역방향 podd 의존이 없다. 따라서 추출 가능하나 **serde-only SDK가 되는 것은 아니다**. forge/discover의 플랫폼 의존까지 없애는 타입 전용 feature 분리는 후속 별도 범위다. 모바일의 cross-target 지원을 이번 추출만으로 보장하지 않는다.
3. F1에서 새 crate에 타입을 기계적으로 복사한 뒤 F2에서 옛 정의를 제거하고 `podd/src/lib.rs`의 선언을 다음처럼 교체한다. 두 복사본이 동시에 외부 API가 되는 기간을 F1→F2로 제한한다. 래퍼/newtype/변환은 쓰지 않는다.

   ```rust
   pub use pod_protocol::{protocol, discovery_protocol, mobile_protocol,
       mods_protocol, orch_protocol, orch_run_protocol, pr_protocol,
       remote_protocol, session_protocol, status_protocol};
   // podd::protocol::TaskInfo == pod_protocol::protocol::TaskInfo
   ```

4. `pod-client` 추출 전에는 소비자가 wire만 `pod_protocol`로 바꿔도 `podd::client`가 남는다. tail에서 Client/Transport/endpoint/detach와 순수 env 함수·now_ms를 옮기고 podd가 다시 export한다. `env.rs`의 worker guard도 현재 I/O 없는 map 변환이므로 그대로 이동 가능하며, 실제 git 설정 읽기는 daemon에 남는다. 서버/holder/PTY 의존은 역으로 들어오지 못한다.
5. 31개 app 파일과 UI 작업자의 `workspace.rs`를 한꺼번에 수정하지 않는다. 첫 전환은 소비자 manifest의 `podd = { package = "pod-client", path = "../pod-client" }` alias로 한다. `pod-client`는 이행에 필요한 `client`, `transport`, `protocol`, `endpoint`, `env`, `now_ms` 경로를 제공한다. 이때 소스의 `podd::` 철자가 남아도 Cargo의 **패키지** daemon 의존은 사라진다. 이후 소비자 소유자가 직접 `pod_client`/`pod_protocol` import로 점진 변경하며 shim 삭제는 별도 결정이다. 현재 모바일 관리 CLI는 같이 전환하고, 미래 모바일 SDK는 wire/schema만 소비한다.

### 정적 도메인 등록

`RpcRegistry`는 시작 시 한 번 조립하고 읽기 전용으로 사용한다. `Method`는 wire에, `RpcHandler`(Arc<Daemon>·Value → boxed Send Future<Result<Value,RpcError>>)와 `MethodRegistration { name, mode, handler }`는 daemon에 둔다. `mode`는 `Unary`/`Attach`/`Subscribe`를 구분한다. `register::<M>()`는 기존 `M::NAME`을 사용하고 중복 이름을 시작 전에 실패시킨다. handler는 처음에는 기존 params/to_value 호출을 그대로 옮긴 adapter다.

`rpc/domains.rs`가 terminal, task/repo, review/group/gate, PR, status, orch, discovery, mods, mobile, system의 등록 함수를 조립한다. 기존 도메인 dispatch는 등록 adapter 뒤에 유지 가능하다. **거대한 match를 다른 파일로 이동하는 것만으로 완료하지 않는다.** 중앙 목록에는 도메인 조립만, 메서드별 처리는 해당 adapter 또는 도메인에 둔다. 순서 의존 prefix 탐색, 동적 플러그인 로딩, inventory, 새 async-trait dependency는 도입하지 않는다.

연결 계층은 framing·hook ingest·요청 계수·중지 확인·hello 선택·모든 출력의 downgrade·스트림 소유권을 담당한다. 현재 `status.subscribe`는 id 있는 요청에서만 스트림이고 notification은 METHOD_NOT_FOUND, attach notification은 INVALID_REQUEST다. `server/mod.rs:532`의 스트림 분기는 일반 JSON-RPC 버전 검사보다 먼저다. 이 순서와 오류를 그대로 고정하며 등록을 이유로 “수정”하지 않는다. `OrchConfigGet`처럼 params를 검사하지 않는 경로(`orch_runs.rs:2107`)도 자동 typed decode로 엄격해지면 안 된다. `mobile.test.*`는 기존 debug+환경 gate 그대로, 정규 공개 104-method 집합과 별도 검사한다.

### 깨끗한 경계를 유지하는 규칙

- 데이터 타입은 wire/persistence/runtime을 구분한다. wire에 파일·락·tokio·daemon을 넣지 않고 persistence 타입을 wire 타입과 합치지 않는다. 초기 domain crate 재노출의 한계는 위에 명시했다.
- 루트 facade는 옛 공개 이름을 유지한다. 새 내부 항목은 private/`pub(super)` 또는 `pub(in crate::server)`부터 시작한다. 이동 때문에 기존 외부 공개 API를 일괄 축소하지 않는다.
- `orch_runs::state`가 기존 mutex·RAII guard를 소유하고 identity가 caller/fence, records가 run/item, launch가 admission/실행, workers가 읽기/중지/해제, mail이 ask/check/reply/done/wait, supervisor/wake가 관찰/알림, gates가 연동을 맡는다. pod-orch에 있는 DAG·journal 엔진을 복제하지 않는다. 기존 `Meter`, 모바일 `Clock` 주입 경계를 재사용한다.
- 첫 분리는 `impl Daemon`을 유지하고 락 획득·drop·await의 상대 순서를 바꾸지 않는다. 이를 새로운 service/actor 객체로 바꾸는 일은 이번 카드에 없다. helper는 구체적 책임 이름을 쓰며 범용 `utils.rs`를 만들지 않는다.
- CLI `CliContext`는 pipe/remote/json과 **명시적 연결 정책**을 담는다. guide/schema/help는 무연결, local server status/stop은 자동 시작 없음, 일반 명령은 기존 자동 시작, 원격은 SSH를 유지한다. 출력 함수는 output/각 명령에 두고 문구·exit code·clap 구조를 보존한다.
- status 엔진은 시간값을 인자로 받는 순수 동기 계층으로 유지한다. hook relay의 1초 총 예산/500ms stdin/4MiB 제한과 `{}` 선출력·강제 종료는 `pod-hook`에 남긴다. SSH의 고정 인자·격리 config·stderr 상한·handshake timeout은 `pod-remote`에 남긴다.

## 3. 병렬 worktree 작업 카드

**총 16카드.** 각 OWNED는 저장소 상대 경로의 완전한 파일 집합이다. `{a,b}`는 두 파일로 전개하며 glob이 아니다. 삭제도 변경 파일로 센다. 같은 wave의 소유 파일은 겹치지 않는다. 선행 카드 병합 후 새 기준으로 worktree를 만들고, 파일 상한을 넘길 발견은 구현 전에 카드 재분할로 처리한다. wave 0은 foundation을 먼저 합치는 단계이며 F0→F1→F2→F3 순서다. wave 1의 P1–P9는 F3 이후 병렬 가능하다. wave 2는 tail이며 T1→T2→T3 순서다. 이 문서 작성에서는 이 구현 카드들을 실행하지 않았다.

공통 완료 조건 **B**: 이전/이후 동일 fixture의 결과·오류 코드/문구·stdout/stderr/exit·이벤트 순서·영속 파일을 비교하고 관련 기존 시험을 실행한다. **W**: master에서 먼저 얻은 JSON golden과 schema를 비교한다(키 순서만 정규화, 누락/null/default/enum/배열 순서/숫자/문구는 보존). 버전 19/최소11, holder3/최소1, hook1 변경 없음. **V**: UI 전환 시 같은 DPI·폰트·renderer·fixture로 `tools/visual/capture.ps1 -Mode compare` **0 failures**. 이 조건은 계획이며 현재 실행 PASS 보고가 아니다.

### F0 — 계약과 격리 helper 고정 · wave 0 · M · 의존 없음

OWNED (15): `crates/podd/tests/common/{mod.rs,fixtures.rs}`, `crates/podd/tests/{mobile.rs,orchestration.rs,orch_runs.rs,submit.rs,gaps.rs,retention.rs,detach.rs,stop_race.rs,sessions.rs,protocol_wire.rs}`, `crates/podd/tests/fixtures/{wire-v19.json,schema-v19.json}`, `tools/test-daemon-refactor.ps1`.

1. common Daemon을 재사용하고 fixture 바이너리 경로·bounded 대기를 공용화한다. 나열한 기존 아홉 시험 파일의 중첩 cargo를 외부 사전 준비 단계로 옮긴다.
2. master 타입으로 104 메서드의 params/result 표본, 모든 알림과 enum 변형, 생략/default/null, 오류 data, v18 downgrade golden을 만든다. JSON을 Value로만 왕복하지 말고 실제 Rust 타입의 deserialize/serialize에 연결한다. 기존 wire 시험과 schema 전체를 함께 고정한다.
3. runner는 개별 test/filter만 선택하고 미리 준비한 바이너리를 검증한다. temp 아래 HOME/APPDATA/LOCALAPPDATA·state·worktrees·mods·agent store·전용 pipe를 지정한다. 남은 pr/status의 중첩 cargo 제거는 각각 P5/P6 소유이며 완료 전 해당 경로는 실행하지 않는다. 바이너리가 이미 있어도 기존 helper는 cargo를 호출하므로 사전 빌드만으로 해결됐다고 보지 않는다.
4. 증명: 기존 common 사용 시험의 동작·종료 정리 불변, W. `tests/remote.rs:22`의 memory fake server와 `examples/fake-agent.rs` 재사용; 실제 모델 호출 없음.

### F1 — protocol crate 준비 · wave 0 · M · 의존 F0

OWNED (15): `Cargo.toml`, `Cargo.lock`, `crates/podd/Cargo.toml`, `crates/pod-protocol/Cargo.toml`, `crates/pod-protocol/src/{lib.rs,protocol.rs,discovery_protocol.rs,mobile_protocol.rs,mods_protocol.rs,orch_protocol.rs,orch_run_protocol.rs,pr_protocol.rs,remote_protocol.rs,session_protocol.rs,status_protocol.rs}`.

1. 10 wire 파일을 새 crate에 복사하고 기존 모듈명과 도메인 타입 re-export를 보존한다. workspace/default-members/lints를 함께 등록한다.
2. 중복 method 매크로만 내부 공통 선언으로 모으되 Method와 schema의 출력은 유지한다. domain runtime feature가 켜지지 않도록 manifest를 구성한다.
3. 새 lib의 인라인 시험에서 F0 golden을 읽어 old/new 동등성을 확인한다. 아직 podd의 타입 정의 교체는 하지 않는다.
4. 증명: W, 새 crate의 wire 시험과 기존 15개 wire 시험; schema 104개와 알림/오류가 정확히 동일.

### F2 — 옛 wire 경로 shim · wave 0 · S · 의존 F1

OWNED (12): `crates/podd/src/lib.rs`, `crates/podd/src/{protocol.rs,discovery_protocol.rs,mobile_protocol.rs,mods_protocol.rs,orch_protocol.rs,orch_run_protocol.rs,pr_protocol.rs,remote_protocol.rs,session_protocol.rs,status_protocol.rs}`, `crates/podd/tests/protocol_compat.rs`.

1. lib에서 새 crate 모듈을 그대로 re-export하고 옛 10개 정의 파일을 삭제한다.
2. `podd::protocol`과 각각의 `podd::*_protocol` 경로 모두에 타입 동일성/Method bound 컴파일 시험을 넣는다. F0 시험은 옛 import 그대로 둔다.
3. 증명: W, `pod-cli` 명령 파싱 시험, `podd --test remote`의 hello 시험. 구 import와 신 import의 값을 변환 없이 같은 함수에 전달할 수 있어야 한다.

### F3 — server 루트 분해 · wave 0 · L · 의존 F2

OWNED (6): `crates/podd/src/server/{mod.rs,runtime.rs,connection.rs,terminal_ops.rs,rediscover.rs,rpc.rs}`.

1. mod에 구성/state·모듈 선언·호환 re-export를 남기고 bind/serve/stop 수명 처리는 runtime으로 옮긴다.
2. 연결 reader/writer·협상·attach를 connection, terminal CRUD/run/input을 terminal_ops, 재발견을 rediscover로 옮긴다. 기존 helper 경로를 facade로 유지한다.
3. match는 우선 rpc로 그대로 옮겨 P1의 단일 진입점을 만든다. connection은 `rpc::mode(method)`를 사용하고 이 단계의 구현은 기존 두 스트림 이름 판별 그대로 둔다. P1은 rpc 안에서만 등록 조회로 바꿀 수 있다. 필요한 가시성만 `pub(in crate::server)`로 조정하고 락/guard 범위는 유지한다.
4. 증명: B/W, `daemon`, `stop_race`, `rediscover`, `unreachable`, `input_budget`, `reply_budget`, `remote`의 해당 필터. stop kill:false/true, 시작 중 stop, unknown/notification/stream/hello 순서 비교.

### P1 — 도메인 handler 등록 · wave 1 · M · 의존 F3

OWNED (4): `crates/podd/src/server/rpc.rs`, `crates/podd/src/server/rpc/{registry.rs,domains.rs}`, `crates/podd/tests/rpc_routes.rs`.

1. RpcRegistry/MethodRegistration과 도메인별 register 함수를 만든다. 기존 다섯 dispatch는 adapter로 호출해 다른 카드 소유 파일을 건드리지 않는다.
2. 직접 54메서드를 도메인 adapter로 옮기고 중앙 match를 등록 조회로 바꾼다. 스트림은 connection의 기존 경로에 연결되는 등록 metadata로 표현한다.
3. 중복 등록/누락/unknown 오류 및 schema의 104개와 등록 집합 동등성을 검사한다. stream 메서드와 debug mobile test 경로는 구분한다.
4. 증명: B/W, `rpc_routes`, `remote`, `mobile`의 대상 시험. status routing은 rpc_routes에서 검사하고 기존 status suite는 P6 통합 후 실행한다. params 무시 경로와 잘못된 jsonrpc/id 없는 스트림 요청의 기존 결과까지 비교.

### P2 — orchestration 책임 분리 · wave 1 · L · 의존 F3

OWNED (12): `crates/podd/src/server/orch_runs.rs`, `crates/podd/src/server/orch_runs/{state.rs,identity.rs,records.rs,launch.rs,workers.rs,mail.rs,supervisor.rs,wake.rs,gates.rs,rpc.rs}`, `crates/podd/tests/orch_runs.rs`.

1. state/Who/cached/remember·Waiting/Stopping guard와 도메인 구현을 §2 경계대로 이동한다. 공개 상수·OrchRuns·handles/dispatch/start는 기존 facade에서 유지한다.
2. `ops` 직렬화, token/generation fence, request_id 저장 시점, 엔진→waiter 락 순서, wake 예약→입력→확정 순서를 보존한다. 이미 분리된 config/limits/meter/store는 수정하지 않는다.
3. 기존 4개 단위 시험을 해당 모듈로 이동하고 기존 16개 통합 시험은 공용 fake-agent로 유지한다.
4. 증명: B/W, `orch_core_roundtrip_fences_breaker_depth_and_restart`, `wakeups_and_supervision`, `resource_queue_fifo_meter_and_restart`, `gate_options_block_items_and_one_decision_path`, worker git guard·unreachable 재결합 회귀를 개별 실행. sleep 대신 passes/wakes 관측을 유지.

### P3 — CLI 서브커맨드 분리 · wave 1 · M · 의존 F3

OWNED (11): `crates/pod-cli/src/{main.rs,args.rs,context.rs,output.rs}`, `crates/pod-cli/src/commands/{mod.rs,terminal.rs,repo.rs,task.rs,server.rs}`, `crates/pod-cli/tests/command_contract.rs`, `crates/pod-cli/tests/fixtures/commands.json`.

1. clap 정의를 args, 연결 선택/caller_terminal을 context, 출력 helper를 output으로 옮긴다. 기존 다른 명령이 쓰는 루트 helper는 re-export한다.
2. terminal/repo/task/server 실행을 각 모듈로 옮기고 이미 분리된 orch/status/session 등의 모듈은 그대로 호출한다. main은 파싱·최상위 분기·종료만 담당한다.
3. master 바이너리의 help/오류/JSON/표 출력·exit code를 고정한다. 현재 9개 main 시험을 이동하되 8MiB 스레드와 remote 제한은 유지한다.
4. 증명: B/W, command_contract, 기존 hooks/remote_stdio/remote_ssh 대상 시험. 무연결 명령과 status/stop이 데몬을 새로 시작하지 않는지 temp 전용 pipe에서 확인.

### P4 — holder 제어와 예산 분리 · wave 1 · M · 의존 F3

OWNED (6): `crates/podd/src/holder.rs`, `crates/podd/src/holder/{budget.rs,input.rs,process.rs,connection.rs}`, `crates/podd/tests/holder_contract.rs`.

1. 예산·입력 queue·프로세스/기록·daemon connection을 옮기고 `holder::main`·공개 상수 facade를 유지한다.
2. 제어 프레임 우회, input ack, reply 독립 예산, VT snapshot/seq와 release 동작을 그대로 둔다. holder_proto와 transport는 수정하지 않는다.
3. 증명: B, 기존 holder 4개 단위/holder_proto 2개 frame 시험, input_budget/reply_budget/gaps/rediscover 대상 시험과 binary frame golden. 출력 포화 중 kill/resize 제어가 기존처럼 진행돼야 한다.

### P5 — git 실행 정책과 parser 분리 · wave 1 · M · 의존 F3

OWNED (8): `crates/podd/src/git.rs`, `crates/podd/src/git/{command.rs,parse.rs,worktree.rs,query.rs,worker_commit.rs}`, `crates/podd/tests/{worker_commit_filters.rs,pr.rs}`.

1. 현 공개 함수명을 git facade에서 보존하며 §2 모듈로 옮긴다. parse는 프로세스를 실행하지 않는다.
2. 일반 command와 bounded worker command를 구분하고 환경 제거·ref 옵션 방어·filter/hook/fsmonitor 차단·경로 검사를 그대로 유지한다. pr 시험의 바이너리 준비도 F0 helper로 바꿔 중첩 cargo를 없앤다.
3. 증명: B/W, 기존 git 단위 7개와 worker_commit_filters 2개, tasks/sessions 대상 시험. 공백/rename/-z/링크/중첩 repo/필터/dirty 삭제 거절 결과 비교. 네트워크 git 없음.

### P6 — daemon status 접점 분리 · wave 1 · M · 의존 F3

OWNED (5): `crates/podd/src/server/status.rs`, `crates/podd/src/server/status/{hub.rs,ingest.rs,stream.rs}`, `crates/podd/tests/status.rs`.

1. hub·hook ingest·subscribe/wait를 분리하고 TermStatus 및 기존 publish/subscribe/handles/dispatch facade를 유지한다.
2. stream-state→TermStatus→hub 락 순서, sequence·lagged·필터·token 검증·persisted 상태를 유지한다. 전역 hub를 새 per-daemon 구조로 바꾸지 않는다. status 시험의 바이너리 준비는 F0 helper로 바꿔 중첩 cargo를 없앤다.
3. 증명: B/W, status 6개 통합·기존 4개 단위와 lifecycle/submit 대상 시험. 이벤트 seq와 hook 거부 횟수, reconnect snapshot을 비교한다.

### P7 — status 엔진과 hook adapter 분리 · wave 1 · M · 의존 F3

OWNED (9): `crates/pod-status/src/engine.rs`, `crates/pod-status/src/engine/{evidence.rs,input.rs}`, `crates/pod-status/src/hook.rs`, `crates/pod-status/src/hook/{claude.rs,codex.rs}`, `crates/pod-status/tests/{state_machine.rs,hooks.rs,mod_source.rs}`.

1. evidence 우선순위/노후화와 입력 처리, 이벤트별 hook mapping을 분리한다. 기존 public engine/hook 타입과 함수는 facade에서 유지한다.
2. 시간 주입·confidence·turn·ack·blocked 전이·hook wire 제한을 보존한다. 새 provider trait나 상태 enum 통합은 하지 않는다.
3. 증명: B, pod-status의 기존 통합 67개를 파일/필터별로 실행하고 hook wire 3개도 유지. 같은 시간·입력 trace의 Snapshot/Transition 직렬화 비교.

### P8 — hook 실행 경계 분리 · wave 1 · S · 의존 F3

OWNED (5): `crates/pod-hook/src/{main.rs,relay.rs,install.rs,console.rs}`, `crates/pod-hook/tests/cli.rs`.

1. install/console 경로를 relay와 나누고 main은 인자 분기와 `{}` 선출력/종료를 유지한다.
2. relay의 write-only SQOS·stdin/전송 deadline·truncation·panic 처리 순서를 보존한다. 자동 데몬 시작/재연결 Client를 넣지 않는다.
3. 증명: B, 기존 cli 13개 시험(멈춘 stdin/pipe, 큰 payload, impersonation 포함). install은 temp home dry-run fixture만 사용한다.

### P9 — SSH 수명·config·진단 분리 · wave 1 · M · 의존 F3

OWNED (6): `crates/pod-remote/src/ssh.rs`, `crates/pod-remote/src/ssh/{config.rs,diagnostics.rs,process.rs}`, `crates/pod-remote/tests/system_ssh.rs`, `crates/pod-cli/tests/remote_ssh.rs`.

1. ssh facade의 SshConnection/ErrorKind/SshError/connect를 유지하고 config 생성, bounded stderr, 프로세스 종료/timeout을 분리한다.
2. MachineStore·고정 옵션/argv·`ssh:<name>` 라벨·hello·재시도 정책을 바꾸지 않는다.
3. 증명: B/W, ssh 단위 5개와 remote_ssh 7개(fake ssh), system_ssh의 로컬 config 검증. 실제 SSH 접속이나 네트워크 쓰기는 하지 않는다.

### T1 — client crate와 소비자 의존 전환 · wave 2 · L · 의존 P1–P9, UI manifest 변경 가능 시점

OWNED (15): `Cargo.toml`, `Cargo.lock`, `crates/{podd,pod-app,pod-cli,pod-remote}/Cargo.toml`, `crates/podd/src/{lib.rs,env.rs}`, `crates/pod-client/Cargo.toml`, `crates/pod-client/src/{lib.rs,client.rs,transport.rs,endpoint.rs,detach.rs,env.rs}`.

1. pod-client를 추가하고 기존 blocking runtime과 순수 env/now_ms를 이동한다. lib에 소비자 호환 protocol 모듈을 재노출하고 podd의 lib/env는 shim으로 바꾼다. 옛 4개 미참조 소스 삭제는 T2가 소유한다.
2. app/cli/remote manifest를 §2의 dependency alias로 바꾸고 lock을 한 번 갱신한다. 기능 플래그와 Windows resource 빌드는 각 실행 crate에 남긴다.
3. 증명: B/W, Client/Transport 타입 동일성, 기존 remote/remote_stdio/remote_ssh/detach 시험, feature=false 소비자 검증. Cargo resolved graph에서 app/cli/remote→패키지 podd 경로가 없어야 한다(다른 서버 테스트 target은 별도).
4. V: UI 소유자 변경이 안정된 공통 기준에서 fixture compare 0 failures. workspace.rs/pod-ui는 편집하지 않는다. 기존 UI baseline이 이미 실패하면 별도 UI 문제로 처리하고 이 카드의 PASS로 덮지 않는다.

### T2 — 추출 후 잔여 정의 제거 · wave 2 · S · 의존 T1

OWNED (5): `crates/podd/src/{client.rs,transport.rs,endpoint.rs,detach.rs}`, `crates/podd/tests/client_compat.rs`.

1. T1 이후 컴파일되지 않는 옛 4개 파일을 삭제한다. lib shim은 건드리지 않는다.
2. 새/옛 Client·Transport·DetachedChild 경로가 동일 타입임을 시험하고 rustdoc 경로·include 참조를 확인한다.
3. 증명: B/W, client_compat와 remote/detach 대상 시험. 새 crate에서 기존 transport 7개·detach 2개·endpoint 1개·env 4개 단위 시험이 계속 발견돼야 한다.

### T3 — 통합 계약 확인과 기록 · wave 2 · M · 의존 T2

OWNED (3): `crates/podd/tests/refactor_acceptance.rs`, `crates/pod-cli/tests/refactor_acceptance.rs`, `docs/20-refactor/daemon-validation.md`.

1. master golden과 최종 구현의 104-method registry/schema, version·wire·오류·이벤트·영속 레코드를 비교한다. 타입/함수/테스트의 단순 이동 누락도 목록으로 대조한다.
2. 격리 daemon에서 fake-agent를 사용해 create→send/wait→restart/rediscover→mail/gate→stop을 확인한다. 사용하지 않는 주입 지점·새 pub 노출·역방향 crate 의존을 검토한다.
3. B/W/V 결과, 실행한 정확한 filter·비실행 항목·artifact 경로를 기록한다. UI compare **0 failures**와 CLI 출력 차이 0을 최종 gate로 삼는다. 실패를 baseline 갱신으로 숨기지 않는다.

## 4. 위험과 하지 말 것

- **이번 평가의 검증 범위:** 파일/선언/의존/중복 계수와 문서·테스트 코드 검토만 했다. cargo·실제 daemon·실제 모델·UI 비교는 실행하지 않았다. 위 시험 수와 과거 문서의 PASS는 이번 평가의 실행 결과가 아니다.
- **빌드 제한:** 향후 검증에서도 cargo 직전 다음 준비를 적용한다. CPU gate 통과 후 free RAM ≥ 8 GiB인지 확인하고 미달이면 기다린다. cargo는 한 번에 하나, workspace 전체 build/test 금지. 시각 도구 README의 workspace build/jobs=4 예시는 이 작업에 적용하지 않는다.

  ```powershell
  Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
  $env:RUSTC_WRAPPER = 'C:\Users\User\AppData\Local\Microsoft\WinGet\Packages\Mozilla.sccache_Microsoft.Winget.Source_8wekyb3d8bbwe\sccache-v0.19.0-x86_64-pc-windows-msvc\sccache.exe'
  $env:CARGO_BUILD_JOBS = '2'
  & C:\ai\ide\tools\cpu-gate.ps1 -MaxPct 70 -MaxCargo 1
  # (Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory / 1MB >= 8 확인
  # 그 다음 승인 범위의 단일 cargo test -p <crate> <filter> 또는 대상 clippy
  ```

- **격리:** 실제 `%APPDATA%\Pod`, 사용자 agent config·실제 podd에는 접근하지 않는다. 임시 root 안 state/log/start-lock/worktrees/mods/discovery/home을 모두 분리한다. fake forge·fake ssh·fake-agent만 사용하고 실제 모델 프롬프트·네트워크 쓰기·GitHub 쓰기는 없다. `tests/common/mod.rs:100`만으로 HOME까지 전부 격리됐다고 가정하지 않는다. `pod-cli/tests/remote_stdio.rs:50`의 환경 격리도 참고한다.
- **병렬 hotspot:** wave0 Cargo/lock은 F1, lib는 F2, server mod list는 F3만 소유. wave1 각 facade가 자기 하위 mod list를 소유하고 중앙 mod list/Cargo를 수정하지 않는다. wave2 Cargo/lock/lib는 T1만 소유. P1과 P2/P6은 기존 dispatch signature를 경계로 공유하며 서로의 파일을 수정하지 않는다. merge 담당자가 임의로 양쪽 변경을 섞지 않는다.
- **UI 경계:** `pod-app/src/workspace.rs`, pod-ui, `strings.rs`와 번역/MsgText 키는 이 카드들의 OWNED가 아니다. UI 병렬 작업의 in-flight diff를 가져오거나 되돌리지 않는다. T1은 manifest만 바꾸며 UI 의미·표시 문자열 변경을 요구하지 않는다.
- **동작을 바꾸는 정리 금지:** 빅뱅 crate 재편, 모든 impl의 service/trait화, 락 통합/분리, 글로벌 hub 교체, schema 자동 생성기로 전면 교체, wire state String→enum, 에러 문구 정돈, timeout/버퍼/재시도 변경, CLI 옵션 재그룹화는 별도 기능 변경이다. 특히 영속 JSON의 이름/default와 protocol/schema의 문자열을 함부로 고치지 않는다.
- **안전 경계 유지:** holder binary와 JSON-RPC를 합치지 않는다. 동기/비동기 line reader의 EOF 차이, `pod-hook` 성공 종료, SSH 고정 command, git worker 경로의 프로그램 차단은 중복 제거 대상이 아니다. 타입을 옮기며 derive/schema 이름·feature 통합으로 내용이 바뀌면 W가 실패해야 한다.
- **되돌리기:** 카드별 독립 커밋을 유지하고 소비자는 shim으로 머물 수 있게 한다. 실패 카드와 그 의존 카드만 되돌린다. protocol 번호 상승·사용자 데이터 마이그레이션으로 리팩터링 회귀를 우회하지 않는다.

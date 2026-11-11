# daemon 리팩터 통합 검증 (D-T3)

[daemon.md](daemon.md) §3 T3 카드의 결과. 대상은 daemon 카드 16장(D-F0~D-T2)과 libs 카드 전부가 병합된 master `fefab34`이고, 비교 기준(baseline)은 리팩터 전 master `64e9530`이다. 미실행 항목은 PASS로 적지 않는다.

**환경:** Windows 11 Home 10.0.26200, rustc/cargo 1.99.1, sccache, `CARGO_BUILD_JOBS=2`, `CARGO_TARGET_DIR` 없음(worktree 자체 target), 매 Cargo·앱 실행 전 `tools/cpu-gate.ps1 -MaxPct 70 -MaxCargo 1`과 여유 RAM ≥8 GB 확인, Cargo 프로세스는 한 번에 하나. baseline 트리는 `git archive 64e9530`을 `target/b0`에 풀고 `vendor/conpty`를 복사해 따로 빌드했다(별도 target).

## 카드별 병합

| 카드 | 병합 | 카드 자체 증명(병합 기준) |
|---|---|---|
| D-F0 | `3bf2a9e` | 공용 fixture·격리 runner, wire/schema v19 golden(`protocol_wire`) |
| D-F1 | `6d0cbc6` | pod-protocol crate(복사), 새 타입에 golden |
| D-F2 | `e627c60` | podd가 옛 경로로 pod-protocol 재노출(`protocol_compat`) |
| D-F3 | `9abf691` | server 루트 → runtime/connection/rpc/terminal_ops/rediscover |
| D-P1 | `9703c33` | 도메인별 const 등록표, 104-method 탐침(`rpc_routes`) |
| D-P2 | `4e053d7` | orch_runs 10모듈 |
| D-P3 | `0d17c6b` | pod-cli 분리, 238-case 명령 계약(`command_contract`) |
| D-P4 | `74ae3b9` | holder budget/input/process/connection |
| D-P5 | `abe6033` | git command/parse/worktree/query/worker_commit |
| D-P6 | `2058c10` | server status hub/ingest/stream |
| D-P7 | `b173604` | pod-status engine·hook adapter, 고정 trace |
| D-P8 | `ace8324` | pod-hook relay/install/console |
| D-P9 | `006d132` | pod-remote ssh, 고정 argv/config |
| D-T1 | `5e158b5` | pod-client crate, app/cli/remote 의존 전환 |
| D-T2 | `303ea67` | 옛 client 소스 삭제(`client_compat`) |

리팩터가 아닌 사이 커밋: fix-shortpath `4f8fb09`(경로 동일성 규칙, 의도된 수정), `89e462f`(holder 로그 한 줄의 공백), 시험·도구 안정화(`87c5011`, `6fa8e59`).

## 1. 계약 비교 (단계 1, W)

- **D-F0 golden을 baseline에서 실행:** `protocol_wire.rs`와 `fixtures/{wire-v19,schema-v19}.json`을 baseline 트리에 그대로 복사해 실행 → **7 passed**(1 ignored = golden 작성기). 같은 시험이 통합 커밋에서도 7 passed. 즉 64e9530 = golden = 통합 커밋(schema 전체, 104 method·18 notification 타입 왕복, 오류 코드, hello 협상, v18 downgrade, envelope, base64, 상수).
- 통합 커밋의 `protocol_compat` 11 passed, `rpc_routes` 5 passed(P1 전 dispatch 탐침).
- **새 시험 `crates/podd/tests/refactor_acceptance.rs::contract_matches_the_baseline`** (golden은 같은 파일을 baseline에서 `POD_ACCEPTANCE_OUT`으로 기록): 번호(protocol 19/최소 11, holder 3/최소 1, hook 1), `podd schema` 지문 `f70885701e1b00c9`와 104 method·18 notification 이름, 새 daemon에서 104개 method 각각의 `"route-probe"` 응답과 88개의 `{}` 응답(22개 결과 본문, 66개 오류 코드·문구; 막히거나·스트림·중지·프로세스 시작 16개는 `{}` 제외), 모르는 이름 4개. 216줄 일치.

### 단순 이동 누락 대조

baseline과 통합 커밋의 daemon 계열 crate(podd, pod-protocol, pod-client, pod-cli, pod-hook, pod-status, pod-remote) 소스를 스크립트로 대조했다.

| 항목 | baseline → 통합 | 사라진 것 |
|---|---|---|
| `#[test]` 함수 | 382(이름 381) → 453(이름 446) | 0 |
| `pub` fn/struct/enum/trait/type/const/static/mod 이름 | 1264 → 1435 | 0 |
| fn 이름 | 1560 → 1746 | 13, 모두 이동·개명 |

사라진 fn 13개: 시험 helper 9개(`fake_agent`, `git`, `pod_exe`, `pod_cli`, `pod_hook`, `profile_dir`, `until`, `until_some`, `workspace_bin`)는 `tests/common/fixtures.rs`와 pod-test-support로 합쳐졌고(D-F0, L-L9), pod-hook `relay`/`install_cli`는 `relay::run`/`install::run`(D-P8), pod-cli `main.rs`의 `repo_cmd`/`task_cmd`는 `commands::{repo,task}::run`(D-P3)이 되었다(`session.rs`의 같은 이름 함수는 그대로).

## 2. 격리 daemon 흐름 (단계 2, B)

`refactor_acceptance.rs::daemon_flow_matches_the_baseline`: 격리 daemon(임시 pipe·state·worktrees·mods·session store, `POD_ORCH_*` 부하 gate off)과 fake-agent(`orch-loop`)로 repo/task create → `terminal.send`/`terminal.wait` → coordinator가 `pod orch worker start` → worker `pod orch gate request` → coordinator `pod orch check`·`gate decide` → `server.stop {kill:false}` 후 같은 state로 재시작, 두 터미널 rediscover(`terminal.list`, `status.wait`) → worker `pod orch ask`+`done`, coordinator `check`·`reply` → `server.stop {kill:true}`. 모든 RPC 결과, agent가 돌린 `pod`의 exit/stdout/stderr, 깨움(`pod mail:`) 입력, 그리고 끝난 뒤 state의 영속 파일(`orch/runs/{index.json,r1/run.json,r1/mail.jsonl}`, `orch/store.json`, `tasks/registry.json`, `t*.status.json`, brief, 파일 목록)을 기록한다.

- 정규화: 임시 root·pipe·바이너리 경로 표기, 버전, 시각(`*_ms`, `*_at`, `ms`, `since`), pid, uptime, 기계 부하, worker token(hash), status turn 계수, request id 안의 시계 숫자. fixture repo 커밋은 날짜를 고정해 commit id도 비교 대상이다. 객체는 키 정렬, 배열 순서·null/누락·문구는 그대로.
- 깨움은 1.5초 quiet 뒤라 시점이 흔들릴 수 있어, 흐름의 정해진 지점에서 깨움 수와 `wakes_in_flight == 0`을 기다린다. 그래서 mail id·read 이벤트 순서까지 고정된다.
- 결과: baseline 기록(48줄) = 통합 커밋, **2 passed**. 같은 트리에서 두 번 기록해 차이 0(baseline·통합 각각). 변이 확인: golden의 gate 선택값 하나(`postgres`→`sqlite`)를 바꾸면 `podd-flow: differs … at line 10`으로 실패.
- 같은 최종 파일(golden 포함)을 baseline 트리에 복사해도 2 passed: 공개 경로(`podd::client`, `podd::protocol`, `podd::holder_proto`, `pod_status::hook`)만 쓰므로 수정 없이 양쪽에서 컴파일된다.

### CLI 출력 (최종 gate: 차이 0)

- **`command_contract`(D-P3, 238 case)를 baseline 트리에서 그대로 실행 → 1 passed.** P3 전 master에서 뜬 fixture가 64e9530 바이너리와도 같다. 통합 커밋에서도 1 passed. 즉 도움말·사용 오류·텍스트/JSON 출력·exit code·연결 정책 238건이 baseline과 같다.
- **새 시험 `crates/pod-cli/tests/refactor_acceptance.rs`**(golden은 baseline에서 기록): `help_tree_matches_the_baseline` — `--version`, `pod --help`에서 따라간 도움말 121쪽, `guide`, `server schema`의 exit와 stdout/stderr FNV-64 지문(124줄). `terminal_flow_matches_the_baseline` — 사용자 세션: daemon 없음 status → run(fake-agent `paste-loop`) → idle 대기 → send → output 대기 → `--json ls/status` → `server stop`(터미널 유지) → 재시작 후 `--json ls`(rediscover) → send/wait → kill/wait --exit/rm → `server stop --kill`(24줄). 둘 다 **passed**, 차이 0.

### 주입 지점·새 pub 노출·역방향 의존 검토

- **주입 지점:** daemon 계열 소스가 읽는 `POD_*` 환경 이름 집합이 baseline과 같다(추가 0, 삭제 0). RPC 등록은 시작 시 조립하는 const 표이고 동적 로딩·inventory·새 async-trait 의존은 없다. 쓰이지 않는 새 주입 지점은 찾지 못했다.
- **새 pub:** 리팩터로 생긴 하위 모듈은 모두 private `mod`(podd `git/*`, `holder/*`, `server/{connection,rediscover,rpc,runtime,terminal_ops}`, `server/rpc/*`, `server/status/*`, `server/orch_runs/*`; pod-status `engine/*`, `hook/*`; pod-remote `ssh/*`; pod-hook·pod-cli 모듈). 새 `pub mod`는 새 crate(pod-protocol, pod-client)의 것뿐이고 podd는 옛 경로로 재노출한다. facade 파일의 `pub` 이름 중 baseline에 없던 것은 `podd::git::{is_inside, path_key}` 둘뿐이며 리팩터가 아니라 fix-shortpath의 경로 규칙이다.
- **역방향 의존(`cargo tree`):** 패키지 `podd`에 의존하는 workspace crate 없음(normal·build·dev 모두). pod-client의 사용자는 pod-app·pod-cli·pod-remote·podd. pod-protocol 트리에 podd·pod-client·tokio·alacritty_terminal 없음. pod-client 트리(73줄)에 podd·tokio·alacritty_terminal·pod-status·QuickJS 없음. pod-mods·pod-process·pod-storage는 §2의 domain 타입 재노출(`pod-mods` default-features=false 등)로 들어오는 것으로, 설계에 적힌 한계와 같다.

## 3. 시각 (V, 최종 gate: 0 failures)

- 빌드: `cargo build -p pod-app --release --locked`(4분 42초). 같은 통합 커밋의 `--ui-fixture`만 띄움(데몬·실제 에이전트 없음).
- 캡처: `tools/visual/capture.ps1 -Output target/visual-t3` → 보조 모니터(창 `{2584,259,4024,1219}`), 144 DPI, 1440×960 physical, dark/light, Malgun Gothic. 36장 모두 장면별 필수 UIA 노드 확인, exit 0.
- 비교: `capture.ps1 -Mode compare -Output target/visual-t3` → `tools/visual/baseline` 36장 대비 **36/36 통과, 바뀐 픽셀 0**(허용치 8, 누락 없음). baseline은 다시 찍지 않았다. `dark-orch-board`, `light-terminal`을 직접 열어 확인.
- 진행 중인 UI 카드(rf-a-p11, rf-a-p14)는 이 커밋에 없다.

## 4. 전체 검사

| 검사 | 명령 | 결과 |
|---|---|---|
| OWNED | `git diff --stat fefab34` | 카드의 3개 경로만 추가 |
| fmt | `cargo fmt -p podd -p pod-cli -- --check` | 통과 |
| clippy | `cargo clippy -p podd -p pod-cli --all-targets --locked -- -D warnings` | 경고 0 |
| podd 대상 | `tools/test-daemon-refactor.ps1 -Test refactor_acceptance,protocol_wire,protocol_compat` | 2 + 7(1 ignored) + 11 passed |
| podd 경로 | 같은 runner의 `rpc_routes` | 5 passed |
| 8.3 경로 | `tools/test-shortpath.ps1 .\tools\test-daemon-refactor.ps1 -Test refactor_acceptance,protocol_wire` | 2 + 7 passed (TEMP=`…\PODSHO~1`) |
| pod-cli | `cargo test -p pod-cli --test refactor_acceptance --test command_contract` | 2 + 1 passed |
| cargo deny | `cargo deny --locked check` | `advisories ok, bans ok, licenses ok, sources ok`. 경고 1(`deny.toml:24` ISC 허용 미사용)은 이전부터 있음 |

baseline 트리(`target/b0`)에서 실행한 것: `test-daemon-refactor.ps1 -Test refactor_acceptance,protocol_wire`(기록 2회, 차이 0; protocol_wire 7 passed), `cargo test -p pod-cli --test refactor_acceptance --test command_contract`(기록 2회 차이 0; command_contract 1 passed), 최종 파일로 두 acceptance 재실행 passed, 변이 1건 실패 확인.

## 비실행 항목과 주의

- **workspace 전체 시험**: 실행하지 않음(코디네이터가 실행).
- **live 알림 스트림 순서**: 이 카드의 시험은 이벤트 순서를 영속 mail log(`mail.jsonl`의 mail·read 순서)와 결과로 비교하고, 구독 스트림(`status.subscribe`, `terminal.attach`)의 알림 순서는 baseline과 직접 대조하지 않았다. 알림 타입·downgrade는 `protocol_wire`, 스트림 동작은 기존 `status`·`orchestration` 시험이 맡는다.
- 실제 SSH·실제 모델·네트워크 쓰기 없음(fake-agent만).
- **runner 주의:** `tools/test-daemon-refactor.ps1 -SkipBuild`는 `target/debug/deps`에서 이름이 같은 가장 최근 `refactor_acceptance-*.exe`를 고르므로 pod-cli의 같은 이름 시험을 집을 수 있다. podd의 `refactor_acceptance`는 `-SkipBuild` 없이 돌린다(빌드 단계가 cargo 출력에서 정확한 exe를 받는다). runner는 이 카드 OWNED가 아니어서 고치지 않았다.

## artifact

- golden: 두 `refactor_acceptance.rs` 끝의 `*_GOLDEN` 상수(baseline 기록 그대로).
- 기록 파일: `target/d-t3-validation/{baseline-64e9530,head}/*.txt`(worktree의 추적되지 않는 target, 임시).
- 다시 기록하려면 baseline 트리에서만: `POD_ACCEPTANCE_OUT=<dir>`로 시험 exe를 돌리면 `<dir>/{podd-contract,podd-flow,cli-help,cli-flow}.txt`를 쓴다.

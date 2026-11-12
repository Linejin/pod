# libs 리팩터 통합 검증 (L-T1)

[libs.md](libs.md) §3 T1 카드의 결과. 대상은 L-F0·L1–L10과 D-T1/D-T2가 병합된 master `303ea67`, 비교 기준(baseline)은 L-F0 직전 master `64e9530`이다. 미실행 항목은 PASS로 적지 않는다.

**환경:** Windows 11 Home 10.0.26200, rustc/cargo 1.99.1, cargo-deny 0.20.2, sccache, `CARGO_BUILD_JOBS=2`, `CARGO_TARGET_DIR` 없음(worktree 자체 target), 매 Cargo 실행 전 `tools/cpu-gate.ps1 -MaxPct 70 -MaxCargo 1`과 여유 RAM ≥8 GB 확인, Cargo 프로세스는 한 번에 하나.

## 카드별 병합

| 카드 | 병합 | 카드 자체 증명(병합 기준) |
|---|---|---|
| L-F0 | `60ad649` | leaf crate 3종(pod-process/pod-storage/pod-test-support) |
| L-L1 | `2b82209` | bounded 소비자 pod-process 전환, GIT_* 정리 공유 |
| L-L2 | `9a92375` | orch/forge/podd JSON 쓰기 pod-storage 전환 |
| L-L3 | `be01ca0` | orch engine 9모듈, `pod-orch/tests/wire_snapshots.rs` |
| L-L4 | `674da3c` | EditorWorkspace 10모듈 |
| L-L5 | `c473d05` | LspStore 분할, wire golden |
| L-L6 | `15ea855` | pod-diff model git/patch/load |
| L-L7 | `42dab11` | mods manager 분할, `pod-mods/tests/wire_snapshots.rs` |
| L-L8 | `f731166` | discover scan/procs 분할, `pod-discover/tests/wire_snapshots.rs` |
| L-L9 | `bccf4c6` | podd 시험 git helper·wait → pod-test-support |
| L-L10 | `cb62bd3` | tools → pod-test-support, boundary/workspace/docs 검사 |

## 1. 의존 경계 (단계 1·5)

- `crates/pod-editor`의 소스·bin·시험에 `pod_forge` 참조 0 → manifest에서 `pod-forge` 제거. `Cargo.lock` 변화는 pod-editor의 `pod-forge` 한 줄뿐(`cargo update -w --offline`).
- `tools/test-support/require_tools.rs`: `#[path]` include 소비자 0 → 삭제. 제품 API 재수출(`pod-forge::bounded`, `pod-diff::model::git_command` 등)은 유지. 후속: `pod-forge::bounded`는 소비자 0이라 rf2-bounds에서 제거.
- `tools/check-boundaries.ps1`: 제거한 간선(pod-editor → pod-forge)이 어느 의존 표에서든 다시 생기면 실패. selftest 6 cases ok, 실제 workspace `crate boundaries ok (21 members …, removed edges absent)`. baseline 트리에 돌리면 editor 간선을 FAIL로 잡는 것을 확인. 후속(rf2-bounds): app/cli/remote→podd, protocol/client→podd·GPUI·alacritty·tokio 서버 기능 금지 추가, selftest 10 cases.
- `check-docs.ps1` ok(중첩 문서 링크 53), `check-workspace.ps1` ok(21 members, 19 default).
- 프로토콜 `PROTOCOL_VERSION = 19`, orch `RUN_VERSION = 1`: 새 wire 시험이 단언.

## 2. wire 시험 (단계 2): `crates/podd/tests/libs_wire_snapshots.rs`

고정 입력(시각·id·fixture)으로 라이브러리를 돌려 podd가 보내는 protocol 타입으로 감싼 JSON을 비교한다. golden은 **같은 파일을 baseline `64e9530` 트리(`git archive`)에서 `POD_LIBS_WIRE_OUT`으로 기록**한 값이다. 키 순서만 정규화, 배열 순서·null/누락·enum 문자열·오류 kind/메시지는 그대로. 비입력 값 3개만 치환: mods root, QuickJS heap, fake forge merge 시각.

| 시험 | 덮는 것 |
|---|---|
| `orch_runs_workers_and_mail_match_the_baseline` | RunInfo·WorkerInfo(터미널 없는 worker의 `next`)·mail check/list·done 결과·OrchError, 재시작이 읽는 run.json/mail.jsonl/index.json, 재로드 RunInfo 동일, token 필드 없음 |
| `pull_request_results_and_errors_match_the_baseline` | `gh` 어댑터(녹화 runner): open/merged/checks 실패/PR 없음/remote 없음/비GitHub/로그인 안 됨/gh 없음의 TaskPrInfo(PrStatus flatten), 보낸 argv·stdin·timeout, create/push/merge/close, `ForgeError::classify` 9건, fake forge 결과와 상태 파일 |
| `discovered_sessions_match_the_baseline` | fixture scan(세션·running ids), 프로세스 매칭(데몬 순서), cwd → repo/task 매칭 |
| `mod_results_match_the_baseline` | mod.list(ModListResult flatten), set_enabled·grant 결과·오류, 상태 파일 |
| `library_wire_schema_matches_the_baseline` | orch.*·task.pr.*·sessions.*·mod.* 메서드/알림 schema 지문(키 정렬 JSON의 FNV-64), protocol 번호 |

결과: baseline에서 기록 5/5, 통합 `HEAD`에서 비교 **5 passed, 0 failed**. 변이 확인: golden 값 2개(`DepsNotDone`, `rate_limited`)를 바꾸면 해당 2개 시험이 경로(`$.errors.begin_pending.kind`, `$.classify[2].kind`)를 짚으며 실패.

## 3. 교차 재실행 (단계 3)

| 조합 | 명령 | 결과 |
|---|---|---|
| L2+L3 journal 실패/복구 | `cargo test -p pod-orch -- store:: torn_ done_with_a_failing done_recorded` | 9 passed |
| L2 primitive | `cargo test -p pod-storage` | 5 passed |
| L4+L5 | `cargo test -p pod-editor --test tabs_lsp --test save_ui` | 2 + 3 passed |
| L1+podd | fixture 빌드(`cargo build -p podd --example fake-agent -p pod-hook -p pod-cli --bin pod --bin pod-hook`) 뒤 `cargo test -p podd --test worker_commit_filters` | 3 passed |

## 4. 시각·UIA·편집기 native QA (단계 4)

- 빌드: `cargo build -p pod-app --release`(4분 25초). 같은 통합 커밋의 `--ui-fixture`만 띄움(데몬·실제 에이전트 없음).
- 캡처: `tools/visual/capture.ps1 -Output target/visual-t1` → 보조 모니터, 144 DPI, 창 1440×960 physical, dark/light, Malgun Gothic. 36장 모두 장면별 필수 UIA 노드 확인(빠지면 캡처 실패), exit 0.
- 비교: `capture.ps1 -Mode compare -Output target/visual-t1` → 기존 `tools/visual/baseline` 36장 대비 **36/36 통과, 바뀐 픽셀 0**(허용치 8, 누락 이미지 없음). baseline은 다시 찍지 않았다. `dark-editor`, `light-orch-board`를 직접 열어 확인.
- UI 병렬 카드(rf-a-p2, rf-a-p4)는 이 커밋에 없으므로 baseline 변경 없음.

- 편집기 native QA: 아래 전체 시험 안의 `pod-app` `native_qa::editor` 7개(언어 서버 메뉴, 저장→작업 갱신, 작업 제거 시 저장/닫기, 스니펫·자동 import, 탭 키보드·MRU, 트리 삭제 확인, 탭 메뉴·이동 키보드) 모두 통과.

## 5. 전체 검사 (단계 5)

| 검사 | 명령 | 결과 |
|---|---|---|
| OWNED | `git diff --stat 303ea67` | 카드의 6개 경로만 변경(shim 삭제 포함) |
| 공개 경로 | 같은 시험 파일이 baseline과 통합 커밋 양쪽에서 수정 없이 컴파일 | 통과 |
| clippy | `cargo clippy -p podd -p pod-editor --all-targets -- -D warnings` | 경고 0 |
| fmt | `cargo fmt -p podd -p pod-editor -- --check` | 통과 |
| 빌드 | `cargo build --workspace --exclude pod-spike --exclude diff-spike --locked` | 통과(첫 시도의 `lsp-types` 실패는 진단 없이 끝난 일시 오류, 재시도 통과) |
| 전체 시험(CI와 같은 인자) | `POD_REQUIRE_TOOLS=1 cargo test --workspace --exclude pod-spike --exclude diff-spike --locked --no-fail-fast -- --skip codex_hooks_trusted_via_app_server_in_temp_home` | 시험 바이너리 90개, **1109 passed, 0 failed**, 4 ignored(rust-analyzer 실서버 2, protocol golden 작성기, doctest 1), 1 filtered |
| cargo deny | `cargo deny --locked check` | `advisories ok, bans ok, licenses ok, sources ok`. 경고 1(`deny.toml:24` ISC 허용 미사용)은 이번 변경 전부터 있음 |

**남은 것:** `docs/13-editor-and-lsp.md:156`은 편집기 프로브가 `pod_forge::bounded`로 돈다고 적혀 있으나 L-L1 이후 `pod_process::bounded`다(이 카드 OWNED 밖이라 고치지 않음; 후속: docs/13은 이미 고쳐졌고 `pod_forge::bounded` 재수출은 rf2-bounds에서 제거). spike 2개는 기본 시험 범위 밖이라 실행하지 않았다.

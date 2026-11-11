# Pod 확장성 리팩터 재평가

2026-11-11 · **종합 5.4 → 7.7 / 10**. 패키지 경계와 변경 위치는 크게 좋아졌다. 특히 클라이언트의 daemon 의존 제거와 7개 레이아웃의 host 전환은 실질적인 개선이다. 그러나 앱의 상태·명령 결합은 상당 부분 남았고, 새 host 추상화에는 수명·이벤트 순서라는 추가 계약이 생겼다.

평가 대상은 **`64e9530` → `95d70b60b115e984fb9489148468a9af2e2697b3`**(작업 시작 HEAD). Git blob만으로 수치를 고정했으며 병렬 버그 수정 작업은 포함하지 않았다. 원 설계의 `8b835ff` 수치를 복사하지 않았다: 지정 baseline의 app은 54,427줄, editor는 14,527줄로 설계 표와 다르다. 제품 코드 변경·Cargo 실행·앱/daemon 실행은 하지 않았다. 아래 위치는 별도 표시 없으면 평가 HEAD 기준이다.

## 1. 기존 30개 문제 판정

번호는 각 설계 §1 순서다. **해결**은 제기된 구조 문제의 목표 달성, **부분**은 분리 효과는 있으나 핵심 결합/중복 잔존이다. 동작 전체의 무결점을 뜻하지 않는다. `D`=`crates/podd/src`, `A`=`crates/pod-app/src`, 나머지 crate도 `crates/<이름>/src` 기준.

| daemon.md | 판정 | 현재 근거 |
|---|---|---|
| D1 wire·실행 패키지 결합 | 해결 | app/cli/remote의 `podd` dependency는 모두 **package=pod-client**; daemon 패키지의 역의존 3→0. `D/lib.rs:24`는 동일 타입 재수출. |
| D2 중앙 dispatch | 부분 | `D/server/rpc.rs:69`는 registry 조회로 축소, 중복 이름은 `rpc/domains.rs:41`에서 컴파일 검사. 다만 464줄 `domains.rs`에 모든 도메인 표/직접 adapter가 모이고 wire schema·기존 도메인 match는 별도다. |
| D3 server 루트 책임 집중 | 해결 | `D/server/mod.rs` 1,568→206줄; runtime/connection/terminal_ops/rediscover로 분리(`:43`). |
| D4 orch_runs 집중 | 해결 | `D/server/orch_runs.rs` 3,161→196줄, `:28`부터 identity/launch/mail/supervisor 등 10개 private 모듈; 공유 상태·락 소유자는 유지. |
| D5 CLI main 집중 | 해결 | `pod-cli/main.rs` 1,723→105줄, args/context/output/commands 분리; `:48`의 8 MiB 스택 계약도 유지. |
| D6 method 선언·등록 누락 | 해결 | `method!` 정의 8→1(`pod-protocol/lib.rs:17`); `D/server/rpc/domains.rs:378`가 공개 등록 집합과 schema 동등성을 검사. 단일 선언 자동 생성까지 달성한 것은 아니다. |
| D7 status hub·입력·stream 혼재 | 해결 | `D/server/status.rs` 1,337→690줄과 status/{hub,ingest,stream}; `pod-status/engine.rs` 909→326줄과 engine/{input,evidence}. |
| D8 holder 제어·예산·I/O 혼재 | 해결 | `D/holder.rs` 786→76줄; budget/input/process/connection 분리, 동기 client framing과 비동기 daemon framing은 합치지 않음. |
| D9 git 실행·조회·worker 정책 혼재 | 해결 | `D/git.rs:20`의 5개 모듈과 명시 재수출, 1,266→204줄; guarded worker 실행은 command/worker_commit에 남음. |
| D10 시험 helper·중첩 빌드 | 부분 | `workspace_bin` 정의 7→0, 공용 fixtures와 별도 runner 도입. 그러나 새 `podd/tests/refactor_acceptance.rs:73`은 자체 polling이고 `:499`는 별도 git 실행기. |

| libs.md | 판정 | 현재 근거 |
|---|---|---|
| L1 EditorWorkspace 변경 범위 | 부분 | `pod-editor/ui/workspace.rs` 4,945→948줄, `:42`부터 책임별 분리. buffers/files/LSP/render가 여전히 같은 상태의 impl이며 파일 묶음의 소유 경계까지 분리된 것은 아님. |
| L2 LSP 수명·기능 요청 결합 | 해결 | `pod-editor/lsp/store.rs:36`의 lifecycle/documents/diagnostics/requests/edits; lifecycle의 generation 검사(`lifecycle.rs:166,209`)가 수명 담당에 모임. |
| L3 journal 저장 순서 은닉 | 해결 | `pod-orch/engine/completion.rs:1,95`에 mail→메모리→run 순서와 실패 의미 명시, persistence/replay 별도. 실패·재시도 시험도 유지. |
| L4 범용 실행기→forge 결합 | 해결 | `pod-editor/Cargo.toml:36`은 pod-process; `pod-forge/runner.rs:14`도 pod-process 사용. 제거 간선은 boundary 검사로 고정. |
| L5 JSON 저장 중복·정책 혼합 | 해결 | orch/store:24, forge/fake:139, mods/manager/state:39, daemon/discovery:174가 pod-storage primitive 사용; mkdir·임시파일 정리·오류 변환은 각 adapter에 유지. |
| L6 git 복제·정책 차이·diff 순환 | 해결 | 공통 9개 환경 제거는 pod-process, 나머지 정책은 caller 소유. `pod-diff/eol.rs:24`는 model façade 대신 `model::git::git_command` leaf 사용. |
| L7 시험 git 8복사·경로 include | 부분 | 지목한 최상위 git 8개는 모두 제거, require_tools 경로 include도 0. 새 daemon 시험 2개에 git helper 재등장(§2), editor fixture·codex_allowance helper도 남음. |
| L8 준비 sleep·부재 관찰 혼동 | 부분 | editor/lsp_lifecycle:202는 PID 준비 polling, mods/tests/manager:894는 stays로 부재 관찰. sleep 63/11/9→46/7/0이지만 잔여 대기의 안정성 전수 증명은 아님. |
| L9 mods manager·discover 책임 집중 | 해결 | mods/manager:30의 core/handle/host_api/state/timers, discover의 scan·procs/{classify,windows}; 기존 Host/Source 경계를 유지. |
| L10 workspace·도구 설명 불일치 | 해결 | tools/visual/README:3은 `-p pod-app`, jobs=2; check-workspace/boundaries 실행 통과(21 members/19 defaults). |

| app.md | 판정 | 현재 근거 |
|---|---|---|
| A1 Workspace 상태·입력·화면 집중 | 부분 | workspace.rs 4,327→292줄이나 하위 포함 6,124줄, fan-in/out 21/23→20/25. `A/workspace.rs:78`의 source/dock/VM 필드는 여전히 crate 공개. |
| A2 LayoutModule의 Workspace 직접 접근 | 해결 | `A/layout_mode.rs:74`는 renderer 필수, 7개 모드 모두 kit 사용. 런타임의 Workspace 타입 이름은 7파일/68행→0(§2). |
| A3 표현·시계 중복 | 해결 | tc 본문 3→1, 공통 font/clock은 kit; `A/layouts/kit/clock.rs:32`는 podd 시계로 위임하며 fixture freeze·각 모드 치수는 보존. |
| A4 feature 정책·요청·render 혼재 | 부분 | review:43, groups:34에서 model/controller/view 분리. 그래도 두 묶음 fan-out은 각각 8→9이며 Group의 Workspace impl과 feature controller 결합은 유지. |
| A5 source·VM 변환 책임 분산 | 해결 | `A/source.rs:21`의 control/events/input/rpc/mapping, cfg(test) backend 분리; `A/vm.rs:9`는 presentation 분리. control/input/attach 연결 구분 유지. |
| A6 명령 ID·키·실행 목록 분산 | 해결 | `A/chrome/commands.rs:63` CommandSpec이 ID/문구/shortcut/run을 연결; `chrome/tests.rs:136,179`가 키·도움말·팔레트를 대조. |
| A7 문구 경계 불규칙 | 해결 | `A/strings.rs:8`의 도메인 6모듈·명시 재수출, feature-local copy와 orch 구조화 번역은 별도 유지. |
| A8 QA 도구와 시나리오 혼합 | 해결 | `A/native_qa.rs` 2,677→52줄, `:45,51`의 harness/fixture 재수출과 주제별 시나리오. 분할 자체를 새 행동 coverage로 계산하지 않음. |
| A9 pod-ui 내부 책임 집중 | 해결 | `pod-ui/components.rs` 1,286→23줄, icons/buttons/rows/frames/text/focus private 분리; 외부 공개 경로는 유지. |
| A10 terminal 입력·그리기 혼합 | 해결 | terminal-view의 view 922→411, element 1,010→421줄; view/{input,ime,links}, element/{text,cursor,preedit}로 분리. InputSink·PTY 비소유 유지. |

**합계: 해결 23, 부분 7, 미해결 0, 악화 0.** 기존 문제의 판정이며 새 비용은 §3에 별도 기록한다.

## 2. 같은 기준으로 재측정

`git ls-tree -r`로 파일/객체 목록을 얻고 Git blob(`git show <sha>:<path>`와 같은 내용, 일괄 읽기는 `git cat-file --batch`)을 UTF-8 `splitlines()`로 집계했다. LOC는 `crates/*/src/**/*.rs`, 주석·빈 줄·inline 시험·bin 포함, tests/examples 디렉터리 제외. Cargo는 manifest의 직접 일반 path dependency만, optional 포함·dev/build 제외·alias는 package명으로 해석했다. 공개 선언은 줄 시작 텍스트 지표이며 실제 외부 도달 API나 매크로 생성 항목 수가 아니다.

| crate | 파일/LOC 전→후 | bare `pub fn` 전→후 | crate fan-in/out 전→후 |
|---|---|---:|---|
| podd | 57/32,967 → 72/27,485 | 342→263 | 3/5→0/9 |
| pod-protocol | 없음 → 11/5,759 | 0→23 | —→2/4 |
| pod-client | 없음 → 6/2,010 | 0→60 | —→4/1 |
| pod-cli | 15/8,069 → 23/8,349 | 55→56 | 0/3→0/3 |
| pod-hook | 1/322 → 4/350 | 0→0 | 0/1→0/1 |
| pod-status | 7/2,883 → 11/2,940 | 79→79 | 5/0→5/0 |
| pod-remote | 3/1,208 → 6/1,466 | 26→26 | 1/1→1/1 |
| pod-editor | 25/14,527 → 40/15,082 | 306→307 | 1/2→1/2 |
| pod-diff | 3/1,245 → 6/1,600 | 21→21 | 2/0→2/0 |
| pod-orch | 7/4,502 → 16/4,714 | 91→91 | 1/0→2/1 |
| pod-forge | 10/3,837 → 10/3,210 | 56→52 | 3/0→3/2 |
| pod-mods | 6/2,697 → 11/2,805 | 28→28 | 1/0→2/1 |
| pod-discover | 8/2,550 → 11/2,573 | 38→38 | 1/0→2/0 |
| pod-process | 없음 → 2/920 | 0→5 | —→3/0 |
| pod-storage | 없음 → 1/151 | 0→2 | —→4/0 |
| pod-test-support | 없음 → 1/341 | 0→5 | —→0/0 |
| pod-app | 56/54,427 → 166/70,025 | 489→523 | 0/7→0/7 |
| pod-ui | 9/4,384 → 19/4,527 | 113→113 | 2/1→2/1 |
| pod-terminal-view | 13/5,022 → 21/5,223 | 55→55 | 2/0→2/0 |
| pod-spike | 4/859 → 4/859 | 7→7 | 0/1→0/1 |
| diff-spike | 3/1,207 → 3/1,207 | 6→6 | 0/1→0/1 |

총 **227→444파일, 140,706→161,596줄(+14.8%)**. 테스트·주석 증가와 새 경계 비용도 포함하므로 코드 팽창 전부를 제품 복잡도 증가로 해석하지 않는다. 반대로 podd만의 LOC 감소는 protocol/client 이동분을 포함하므로 전체 축소로 해석하면 안 된다.

| 최대 파일 순위 | baseline | 평가 HEAD |
|---|---|---|
| 1 | editor/ui/workspace.rs 4,945 | app/pr_panel.rs 1,615 |
| 2 | app/workspace.rs 4,327 | app/layouts/dense/tests.rs 1,524 |
| 3 | podd/server/orch_runs.rs 3,161 | app/layouts/blocks_model.rs 1,452 |
| 4 | orch/engine.rs 2,910 | protocol/protocol.rs 1,439 |
| 5 | app/review.rs 2,688 | podd/server/discovery.rs 1,254 |

### 모듈 결합과 공개 범위

앱은 설계와 동일하게 파일을 최상위 모듈로 합치고, 주석 전용 줄을 제외한 `crate::<실제 모듈>` 간선을 중복 제거했다(자기 참조 제외, 시험 포함, `super`/별칭 미해석). 아래는 **fan-in/out**.

| 앱 묶음 | 전→후 | 앱 묶음 | 전→후 |
|---|---|---|---|
| workspace | 21/23→20/25 | layouts | 2/22→7/23 |
| layout_mode | 4/3→5/2 | source | 18/7→18/7 |
| chrome | 6/10→7/10 | review | 5/8→5/9 |
| groups | 5/8→5/9 | orch_ui | 5/12→5/12 |
| mods_ui | 4/4→4/5 | sessions_ui | 3/5→3/6 |
| vm | 15/2→16/2 | strings | 18/0→18/0 |
| native_qa | 7/14→11/16 | | |

daemon의 명시적 `crate::<모듈>`(daemon 내부)/`podd::<모듈>`(전체 src) 참조 **파일 수**는 protocol **85→156**, client **16→26**, transport **8→9**, git **11→14**, holder_proto **4→9**다. 파일 분할로 참조 파일 수가 늘고 `podd` alias도 남기 때문에 이것은 패키지 역의존 증가가 아니다. façade 파일의 루트 모듈 fan-out은 server/mod **8→3**, orch_runs **3→0**, holder **4→1**; 하위 파일로 이전한 참조를 삭제된 의존으로 보아서는 안 된다.

라이브러리의 대표 façade도 대조했다. fan-in은 해당 모듈 하위 구현·시험 제외 외부 소스 파일 수다. fan-out은 설계 표의 import 열거를 재현하여 façade의 `use/pub use crate::…/super::…`가 가리키는 모듈만 센다(내부 자식 선언 제외). 설계의 문장상 정의보다 좁으며, 본문의 완전 수식 참조까지 포함한 값과 혼용하지 않는다. 예컨대 baseline LSP store는 import 5개 외에 본문 fuzzy/rpc 참조도 있다.

| 라이브러리 모듈 | fan-in/out 전→후 | 해석 |
|---|---|---|
| editor/ui/workspace | 1/13→1/10 | 소비 경로 유지, 기능 참조는 자식 impl로 이동 |
| editor/lsp/store | 3/5→4/4 | 기존 workspace 소비 1개가 lsp_actions/render_overlays 2개로 분산 |
| orch/engine | 1/4→1/2 | 상태 소유자는 동일 |
| forge/bounded | 1/0→0/0 | 외부 제품 참조도 4→0, 실제 실행기는 pod-process |
| mods/manager | 1/4→1/4 | 외부 도메인 경계 유지 |
| diff/model | 2/1→2/0 | eol의 참조는 이제 model/git leaf; model/load→eol→model/git은 순환 아님 |

| 공개 선언 측정 | 전→후 |
|---|---|
| app bare pub 항목 / 필드 / 제한 공개 전체 | 1,516/439/257 → 1,604/457/901 |
| app의 `pub(crate)`만 | 257→461 (나머지 제한 공개는 pub(super) 등) |
| ui bare pub 항목 / 필드 / 제한 공개 | 189/145/1 → 197/145/2 |
| terminal-view 동일 | 116/90/16 → 116/90/32 |
| podd lib / server의 `pub mod` | 24/25→10/25 (재수출은 별도) |
| wire 10파일의 명시 pub struct/enum/trait/type | 259→259 (method! 본문의 생성 템플릿 제외) |

bare 항목 정규식은 줄 시작 `pub` 뒤 fn/struct/enum/trait/type/mod/use/static/const(async/unsafe 허용), 필드는 `pub 이름:`, 제한 공개는 `pub(...)`다. binary 내부·private 모듈의 선언도 포함하므로 “공개 SDK 901개”라는 뜻이 아니다.

**daemon을 의존하는 crate:** normal은 app/cli/remote **3→0**, after의 dev/build/target 의존 표까지 확인해도 package `podd` 소비자 0. after의 app/cli/remote에는 `podd = { package = "pod-client", ... }` 철자가 남는다. protocol은 forge/orch/mods/discover 네 도메인에 의존하므로 serde-only SDK는 아직 아니다.

### 중복과 Workspace 이름

| 측정 대상 | 전→후 / 남은 의미 |
|---|---|
| 동일 tc 함수 본문 | app 3→1(`orch.rs:46`); 나머지는 tokens/kit 색 함수 import |
| 동일 epoch now_ms 계산 | daemon root·app/orch·blocks 3→pod-client 1; orch/kit은 위임, orch_ui의 Frozen 및 mobile Clock 구현은 별도 계약 |
| `fn write_json` 이름 | 3→3. 파일 저장 adapter 2개와 RPC writer 1개라 이름만으로 중복 판정 불가 |
| 지목한 JSON pretty→tmp→rename 4경로 | 4개 자체 기계 동작→공통 pod-storage primitive 1개, 정책 adapter 4개 유지 |
| podd/tests 최상위 `fn git` | 지목한 8→0, 전체는 **8→2**(새 refactor_acceptance:499, short_paths:42). 전자는 고정 날짜 정책이 있어 무조건 동일 helper로 치환하면 안 됨 |
| 기타 git helper | editor/fixture:41, podd/server/codex_allowance:180 유지; CLI command_contract:322에 새 메서드. 모든 git 함수가 중복은 아님 |
| podd/tests `workspace_bin` | 7→0; common/fixtures에 실행파일 탐색 집중 |
| tests의 thread/time sleep 호출 | podd 63→46, editor 11→7, mods 9→0; helper 내부 대기는 제외한 호출 수, flaky 시험 수 아님 |

`layout_mode.rs`+`layouts/**/*.rs`에서 주석·시험 파일/inline tests·Drawer/RailButton의 동명 enum variant를 제외한 **Workspace 타입 참조 행**은 **7파일/68행→0**. before 분포: layout_mode 9, blocks 5, dense 20, kanban 6, queue 14, rail 8, review 6. release fixture 함수는 런타임에 포함했다. after 단순 문자열 검색에는 rail의 enum variant와 `blocks_qa.rs:619,631`의 시험 타입이 남는다. host adapter(`workspace/layout_host.rs`)의 Workspace 사용은 의도된 경계이며, `Toast/ToastKind/ArrangeGrid`의 workspace 모듈 의존까지 사라졌다는 뜻은 아니다.

## 3. 새 냄새·잔여 위험

확정된 구조 사실과 가능성인 위험을 구분했다. 기존 버그를 새 회귀라고 단정하지 않는다.

1. **확정: 파일 분할이 공개 범위 확대를 동반했다.** app pub(crate) 257→461. 예를 들어 `layouts/rail.rs:390–413`의 dock/rows/tabs/focus와 `rail/model.rs:429,449`의 선택 helper가 crate 범위다. 다수는 자식 view/fixture/tests에서만 필요해 더 좁힐 후보이며, 모든 901개 제한 공개가 과도하다고 판단한 것은 아니다.
2. **확정: 테스트를 위한 무동작이 제품 host 계약에 들어갔다.** `layouts/kit/model.rs:270–352`는 “test sink stays small”을 위해 observe/dock/source/post/resolve_gate 등에 None/false 기본값을 준다. 새 host가 구현을 빠뜨려도 컴파일되므로 런타임 capability 누락을 감춘다. 반면 `source::TestBackend`와 kit의 state/release_window 재수출은 cfg(test)로 제대로 격리됐다(`source.rs:27,51`, `kit.rs:22`).
3. **위험: host가 작은 Workspace 대리자로 커질 수 있다.** kit가 실제 DockArea/TerminalPanel/ReviewPanel/PoddSource를 반환(`kit/model.rs:303–342`)해 레이아웃에서 직접 dock 변경·RPC가 가능하다. Workspace 타입 0은 성과지만 모델·행동 경계의 완전 분리는 아니다. `workspace/layout_host.rs` 1,106줄 중 제품부는 449줄, 나머지는 시험이므로 전체를 새 1,106줄 제품 god object라고 부르지는 않는다.
4. **위험: update 종료와 이벤트 전달 완료가 다르다.** `kit/host.rs:329`의 dispatch_then은 dispatch 직후 콜백을 부를 뿐 notify/dock 이벤트를 기다리지 않는다. rail:816–829가 두 번 window.defer하여 동기화 뒤 rearrange/focus를 보장한다. `workspace/layout_host.rs:432`의 post도 호출자가 순서대로 ApplyGate/Notify를 넣고 재진입을 피해야 한다. 현재 오류를 재현한 것은 아니며, host 계약을 오용하기 쉬운 새 표면이다.
5. **확정: façade 중 일부는 호환 비용만 남는다.** forge/bounded.rs:8은 재수출 한 줄이고 제품 소비자 0; podd/env.rs:8도 재수출 전용이다. 전자는 폐기 정책 후보, 후자는 daemon의 기존 env 경로가 실제 사용되므로 dead code로 단정하지 않는다. ui/components와 app/strings façade는 안정적인 소비 경로를 유지하는 유효한 분할이다. legacy layout hook과 빈 renderer stub은 실제 제거됐다.
6. **확정: 새 검증 코드가 helper 중복을 다시 만든다.** daemon/CLI acceptance의 자체 until·git은 공용 지원 crate와 따로 고쳐야 한다. 단, baseline에서도 같은 시험을 컴파일하고 고정 날짜로 golden을 얻는 이유가 있으므로 “공유 안 함=결함”은 아니다. 정책을 드러낸 adapter와 명시적 예외가 필요하다.
7. **확정: 완료 문서와 중간 설명이 혼재한다.** `kit/model.rs:10–11`은 renderer가 없으면 Workspace fallback이라고 쓰지만 `layout_mode.rs:82`는 renderer 필수다. app.md의 A-T2a 표에는 제거된 Touch/TerminalInput/terminal_focus/mono_family가 남고 뒤 T2 기록에서만 삭제를 설명한다. daemon-validation의 SkipBuild 주의는 현 runner:109의 exe index로 이미 해결, libs-validation의 docs/13 주의도 현 docs/13:156에서는 수정됐다. 당시 기록은 유효하나 현재 주의사항과 구분하는 후속 표시가 필요하다.
8. **확정: 새 핵심 의존 경계는 자동 검사 범위 밖이다.** `tools/check-boundaries.ps1:24`의 제거 간선 표는 editor→forge만 막는다. app/cli/remote→podd 복귀와 protocol/client→daemon·GUI 유입 금지도 추가해야 이번 성과가 회귀하지 않는다. protocol의 네 도메인 의존은 설계가 인정한 잔여 비용이다.

## 4. 시험 근거와 점수

이번에 직접 실행한 것은 **check-boundaries / check-workspace / check-docs**이며 모두 exit 0(21/19 members, 문서 링크 55개). 테스트 구현·golden·비교 절차는 읽었지만 Cargo 시험·clippy·시각 검사를 재실행하지 않았다. 아래 기존 PASS는 해당 문서의 커밋에서 보고된 결과이며 평가 HEAD의 새 실행 결과가 아니다.

- daemon-validation(`fefab34`): 64e9530에 같은 wire/acceptance 시험을 적용, 104-method 탐침·CLI 238-case·격리 restart/worker/mail/gate 흐름 및 golden 변이 실패. 단순 “컴파일 성공”보다 강하다. live attach/status 스트림의 알림 순서 직접 비교는 제외됐다고 명시돼 있다.
- libs-validation(`303ea67`): baseline 기반 wire 5건·변이 2건, journal 실패/복구, save/LSP, worker git 필터; 통합 1,109 passed 기록. 시험을 파일로 옮긴 것과 신규 회귀 검출을 구분한다.
- app.md T2: 391 passed·56장 픽셀 동일 기록. `native_qa/layout_contract.rs:47` golden은 **64e9530이 아니라 aff314e(T2z 직전)** 기준이다. 두 번 순환·두 창 수명·request/panel/dock/focus를 고정하지만 `:97–106`은 modal key 누적을 필터링한다. 보존된 focus/blocks/kanban 한계와 keymap 문제를 이번 리팩터가 해결했다고 점수에 반영하지 않았다.

점수는 측정치를 바탕으로 한 판단이다. 5점은 기능 추가가 가능하지만 수정 지점이 넓은 상태, 8점은 책임별 진입점과 회귀 방어가 있으나 수명/경계 예외가 남은 상태로 보았다.

| 항목 | 가중치 | 전→후 | 판단 이유 |
|---|---:|---:|---|
| 확장성(기능·레이아웃·RPC 추가 비용) | 30% | 4.5→7.5 | renderer/host와 registry로 진입점 개선; enum/ALL/id/label·schema·도메인 등록 등 복수 편집은 남음 |
| 모듈 경계·결합도 | 20% | 5.0→7.0 | daemon 역의존 제거·leaf 추출; Workspace 결합과 넓은 ActionSink 잔존 |
| 코드 구조·가독성 | 15% | 4.5→7.5 | 최대 파일 4,945→1,615, 책임별 탐색 개선; façade/visibility/상태 공유 비용 증가 |
| 테스트 품질(동작 고정·회귀 검출) | 20% | 7.0→8.5 | 기존 시험에 baseline golden·변이·수명 순환 추가; live stream 순서·일부 keymap coverage 공백 |
| 문서 정확성 | 5% | 6.5→8.0 | 커밋·방법·미실행·한계를 명시한 검증 기록; 중간 API/주석 일부 불일치 |
| 빌드·의존성 위생 | 10% | 6.5→8.5 | 3 leaf+client/protocol 분리와 정책 검사; protocol 도메인 의존·새 간선 방어 부족 |
| **가중 종합** | **100%** | **5.4→7.7** | 가중합 5.400→7.725, 소수 첫째 자리 반올림 |

## 5. 다음 작업 5개

1. **host 순서 계약을 고정:** render 중 사용 가능한 read/slot API와 update API를 구분하고, dispatch_then의 “이벤트 완료 아님”을 명시. 실제 host에서 dock 변경→notify→focus, 창 닫힘→늦은 RPC 완료, gate 성공/실패 순서를 회귀 시험으로 고정한다.
2. **visibility와 ActionSink 축소:** rail/queue/dense 등의 내부 필드를 private/pub(super)로 좁히고 테스트용 기본 host 구현은 시험 영역으로 옮긴다. 제품 host의 필수 메서드는 구현 누락을 컴파일에서 잡는다.
3. **의존 경계 자동화 완성:** app/cli/remote→podd, client/protocol→daemon·GUI의 금지 간선을 package alias·target/dev/build 표까지 검사한다. 사용자가 없는 forge bounded shim은 호환 정책을 정한 뒤 제거한다.
4. **검증 공백과 중복 정리:** 격리 daemon의 attach/status 알림 순서, modal key 누적을 제외하지 않는 layout 순환을 추가한다. acceptance의 baseline 이식성·git 날짜 정책을 유지하면서 공용 대기/fixture와의 예외를 명문화한다.
5. **현재 확장 가이드 한 장으로 정리:** A-T2a API 표를 최종 API와 연결하고 오래된 fallback/해결된 주의를 표시한다. 새 레이아웃·RPC 하나를 넣을 때 고쳐야 할 enum/등록/schema/시험 위치를 정확히 나열하고, 다음 큰 파일은 pr_panel/discovery처럼 실제 변경 수요가 있는 곳부터 분리한다.

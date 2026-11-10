# 라이브러리·워크스페이스 리팩터 설계

기준: 2026-11-10, `master = HEAD = 8b835ff3930c5bf08ff1e70596ae02b82614246a`. **평가·설계만 수행**했으며 아래 카드는 후속 구현 작업이다. 제품 코드·프로토콜·사용자 상태는 변경하지 않았다. `README.md` 문서 목록과 docs/03·08·09·10·19, 편집기·forge·확장·탐색 계약인 docs/13·15·16·18 및 해당 소스·시험을 읽었다. 다른 앱 내부 구현은 근거로 사용하지 않는다. 별도 UI 작업자의 `pod-app/workspace.rs`, `pod-ui` 변경은 이 설계의 소유 범위 밖이다.

## 1. 현재 구조 지도

### 측정 방법과 의존성

경로는 저장소 루트 기준이다. 근거의 `editor/`, `orch/`, `forge/`, `mods/`, `discover/`, `diff/`는 각각 `crates/pod-<이름>/`, `podd/`는 `crates/podd/`의 약기다. `src/**/*.rs`의 물리 행 수(빈 줄·주석·내장 시험·bin 포함)를 UTF-8로 집계했다. Cargo 수치는 각 manifest의 직접 일반 의존성 중 workspace member만 센다(외부 패키지·dev 의존성 제외). `pub fn`은 행 시작의 제한 없는 `pub [async/unsafe/const] fn` 선언 수이며 매크로 생성 API·공개 필드는 제외한다. **rustdoc의 도달 가능한 API 개수가 아니다**. 비공개 모듈의 pub도 포함하므로 실제 호환성 경계는 아래 재수출 경로와 함께 판단한다.

| crate | Rust 파일 / 행 | 직접 fan-in ← / fan-out → | `pub fn` | 주 구성 |
|---|---:|---|---:|---|
| pod-editor | 25 / 14,495 | app 1 / ui, forge 2 | 305 | tree/index/fuzzy/tabs/text/watch/git/uri + lsp 8모듈 + ui |
| pod-diff | 3 / 1,245 | app, diff-spike 2 / 0 | 21 | model 809, eol 285, lib 151행; GPUI 어댑터 포함 |
| pod-orch | 7 / 4,502 | podd 1 / 0 | 91 | engine 2,910, model/store/resource/wake/text |
| pod-forge | 10 / 3,837 | app, editor, podd 3 / 0 | 56 | gh 869, bounded 815, fake 541, runner/error/remote/status/time |
| pod-discover | 8 / 2,550 | podd 1 / 0 | 38 | procs 884, lib 536, Source 구현·jsonl/matching/time |
| pod-mods | 6 / 2,697 | podd 1 / 0 | 28 | manager 1,503, runtime 479, manifest/host/types |
| diff-spike | 3 / 1,207 | 0 / diff 1 | 6 | review/main/stats |
| pod-spike | 4 / 859 | 0 / terminal-view 1 | 7 | main/pty/panel/stats |

`app/ui/diff/forge`는 표에서만 `pod-` 접두사를 생략했다. 예컨대 editor의 GPUI·notify·LSP 타입 의존성은 실제 존재하지만 위 내부 crate fan-out에는 포함하지 않았다. crate 간 순환은 이 직접 그래프에 없다.

모듈 수준에서는 **같은 crate의 다른 파일에서 명시적으로 참조·재수출한 파일 수**를 fan-in, 명시적 `crate::`/`super::` 경로의 서로 다른 모듈을 fan-out으로 셌다. 외부 패키지, 시험, 매크로, 재수출 뒤 간접 사용은 제외한다. `lsp` 같은 façade도 하나의 모듈로 센다.

| 모듈 | fan-in / fan-out | 근거·경계 |
|---|---:|---|
| editor/ui/workspace | 1 / 13 | lib:26,31 재수출; workspace:29–44의 quick_open/strings, git/index, lsp/providers/snippet/store, tabs/text/tree/uri/watch |
| editor/lsp/store | 3 / 5 | lsp/mod:24, providers:19, workspace:35 → store; store:27–34 → registry/server/snippet/text/uri |
| orch/engine | 1 / 4 | lib:12 → engine; engine:13–16 → model/resource/store/text |
| forge/bounded | 1 / 0 | runner:14 → bounded; **crate 밖 추가 4파일**: podd/git:224, server/groups:12, server/repo_config:361, editor/lsp/registry:459 |
| mods/manager | 1 / 4 | lib:25 재수출; manager의 host/manifest/runtime/types 참조 |
| diff/model | 2 / 1 | lib:21·58, eol:167·209 → model; model → eol. **model ↔ eol 순환** |

호환성 면적: editor의 workspace는 테스트 모듈 전 4,854행에 공개 함수 64개, LSP store는 1,893행에 51개다. orch engine은 1,852행에 62개이고 나머지 1,058행은 시험 구역이다. 따라서 “2,910행 엔진 전체가 제품 로직”이라고 해석하지 않는다. `pod_editor::{EditorWorkspace,EditorSession,EditorEvent}`와 `pod_editor::lsp::{store::*,…}` 경로, `pod_orch::engine::*`와 루트 재수출, `pod_forge::bounded::*`를 그대로 유지해야 한다. 기존 `Forge`/`Runner`, `Source`, `Host`가 이미 확장점이다(`forge/runner.rs:89`, `discover/lib.rs:213`, `mods/host.rs`).

### 가장 큰 문제 10개

1. **편집기 작업 하나의 변경 범위가 너무 넓다.** `editor/src/ui/workspace.rs:259` Buffer가 인코딩·디스크 충돌·LSP·snippet·구독을 소유하고, 같은 파일에 저장(:1661), 감시(:2128), rename/apply edits(:2440), 트리 명령(:2798), 렌더(:3046–4853)가 있다. 저장 동작 하나를 확장해도 이벤트 순서·포커스·렌더 코드를 함께 검토해야 한다. 이미 분리된 `tabs.rs`의 규칙을 다시 UI 안으로 복제하지 않는다.
2. **LSP 수명과 기능 요청의 결합.** `editor/src/lsp/store.rs:195`가 slots/docs/diagnostics/pending completion을 함께 갖는다. 문서 동기화(:425), 프로세스 시작(:674), generation 검사(:778), 진단(:1120), completion(:1335), workspace edit(:1657)가 한 파일이다. 새 기능을 넣을 때 재시작·stale 결과 차단까지 흔들기 쉽다.
3. **엔진의 저장 순서가 기능 코드 속에 숨어 있다.** `orch/src/engine.rs:281` 복구, :550 DAG, :893 admission, :1042 done, :1485 mail, :1746 wake, :1811 요청 중복 처리가 공존한다. 특히 :1107–1124는 **mail append → 메모리 전이 → run 저장**, :339는 replay다. 분할 시 “일반 save”로 합치면 실패 복구가 달라진다.
4. **범용 프로세스 실행기가 forge에 묶였다.** `editor/Cargo.toml:36` 의존성 및 `editor/src/lsp/registry.rs:459`는 PR 기능 없이 `bounded`만 사용한다. 새 프로세스 소비자가 forge 도메인에 의존하게 된다. `forge/src/bounded.rs:251`의 deadline·Job Object·출력 상한 계약은 독립시킬 만한 실제 5파일 fan-in이 있다.
5. **유사 JSON 저장 코드의 차이가 감춰진다.** `orch/src/store.rs:23`, `forge/src/fake.rs:134`, `mods/src/manager.rs:101`, `podd/src/server/discovery.rs:134` 모두 pretty JSON → `.json.tmp` → rename이다. 그러나 orch/forge/discovery는 부모 디렉터리를 만들고 mods는 만들지 않으며, discovery만 오류 시 임시 파일 삭제·로그를 한다. 중복 수정이 누락되기 쉽지만 정책까지 통일하면 기존 실패 동작을 깨뜨린다. `editor/src/text.rs:414` 인근 고유 임시 파일·저장 충돌 처리는 같은 계약이 아니다.
6. **git 래퍼는 복제와 정책 차이가 섞였다.** `podd/src/git.rs:24`와 `forge/src/runner.rs:102`의 환경 제거 목록은 같은 9항목이다. 그러나 podd는 `POD_GIT`, fsmonitor/필터 가드 등을 사용(:42, :218), editor는 `git -C`와 창 숨김만(:178), diff는 환경 3개 제거(:164)다. 하나의 “안전한 git” 기본값으로 바꾸면 실행 argv·설정·오류·시간 한도가 바뀐다. diff/model과 eol이 서로 git 실행·내용 처리를 참조하는 순환도 재사용을 어렵게 한다.
7. **시험용 git 8복사와 파일 경로 include.** 정확히 `podd/tests/{tasks.rs:54,sessions.rs:66,pr.rs:56,orchestration.rs:107,orch_runs.rs:69,mobile.rs:120,lifecycle.rs:48,discovery.rs:32}`의 최상위 `fn git` 8개다. 앞 4개는 stdout String, 뒤 4개는 `()` 반환이며 오류 문구도 다르다. “여러 crate에 똑같은 8개”는 아니다. 별도로 editor/fixture:36과 podd/server/codex_allowance:180에도 helper가 있다. `tools/test-support/require_tools.rs`는 CLI·remote·editor·podd의 4개 시험에서 `#[path]`로 포함된다. 공통 fixture 수정이 누락되거나 패키징 경로에 종속된다.
8. **sleep 개수는 불안정 시험 개수와 다르다.** `tests/**/*.rs`의 `thread::sleep`/`time::sleep` 호출은 podd 63, editor 11, mods 9개(나머지 위 라이브러리 0; 내장 시험 제외). editor/lsp_lifecycle:202의 200ms는 PID 파일/자손 준비를 추측한다. 반면 podd/pr:379–386의 2.5초와 mods/manager 시험:986·1014는 “더 발생하지 않음”을 관찰한다. 전부 즉시 성공하는 polling으로 바꾸면 오히려 회귀를 놓친다. 느린 장비에서 양성 준비 대기와 부재 관찰을 구분해야 확장이 안정적이다.
9. **확장 관리자가 API·실행 루프·저장을 함께 담당한다.** `mods/src/manager.rs:101` 저장, :174 handle, :299 Core, :493 host dispatch, :924 queue loop, :1377 timer 처리. 새 host 기능이 권한·scope·쿼터 검사를 우회하지 않는지 한 파일 전체를 추적해야 한다. `discover/src/procs.rs:283` 분류, :364 연결, :448 Windows snapshot도 같은 파일이라 OS 변경과 순수 분류 변경이 충돌한다. 기존 Source trait를 교체할 이유는 없다.
10. **워크스페이스 정책은 이미 일부 해결됐지만 도구 설명이 어긋난다.** `Cargo.toml:3–7`은 16 members/14 defaults, 두 spike 제외; `.github/workflows/ci.yml`은 명시적 `--exclude`도 사용한다. 반면 `tools/visual/README.md:3`은 `build --workspace --release`와 4 jobs를 안내한다. tools는 `.ps1/.py/.rs` 39파일/3,668행이며 기능별 디렉터리가 이미 있다. 전부 재배치할 근거는 없다. 불필요한 전체 빌드 지침과 경로 include만 바로잡는다.

## 2. 목표 구조와 배치 규칙

```text
pod-process      bounded::{Options, Keep, End, Outcome, run, tail_str}
                 git::clear_overrides(Command)  # 동일한 9항목만
pod-storage      json::to_pretty_bytes, replace::write_then_rename
pod-test-support git::{git_output, git_ok}, wait::{until, stays}, tools::missing
                 # publish=false, 소비자는 dev-dependencies만

pod-forge        Forge / GhForge / FakeForge, Runner / Invocation, 오류 분류 유지
                 bounded.rs는 pod-process::bounded 호환 재수출
pod-orch         engine.rs: Orch façade, 타입·단일 상태 소유자
                 engine/{persistence,items,attempts,admission,completion,mail,wake_state,requests,tests}.rs
                 model/store/resource/wake/text의 기존 경계 유지
pod-editor       ui/workspace.rs: EditorWorkspace, init/actions, 공개 façade
                 workspace/{session,buffers,files,tabs,tree,lsp_actions,render_tree,render_tabs,render_panels,render_overlays}.rs
                 lsp/store.rs: LspStore façade·상태 소유
                 store/{lifecycle,documents,diagnostics,requests,edits}.rs
pod-diff         model.rs façade → model/{git,patch,load}.rs, eol.rs
pod-mods         manager.rs façade → manager/{state,handle,core,host_api,timers}.rs
pod-discover     lib.rs façade → scan.rs; procs.rs → procs/{classify,windows}.rs
```

- **의존 방향:** 제품 도메인 → 작은 leaf crate. process는 std/Windows API, storage는 std/serde/serde_json만 사용한다. 둘은 서로 또는 podd/GPUI/forge를 참조하지 않는다. test-support는 std/tempfile 수준에서 독립하며 podd를 의존하지 않는다. `podd/tests/common`의 데몬 수명 하네스는 그 자리에 둔다. 새 `common`, `utils`, `pod-core` 잡동사니 crate는 만들지 않는다.
- **프로세스:** 기존 `bounded` 구현·시험을 먼저 그대로 옮긴다. 시간 한도, drain grace, Cap/Tail, 취소, suspended spawn → job assign → resume 순서 그대로. 장수 LSP 서버의 `lsp/job.rs`나 holder까지 단일 실행기로 합치지 않는다. `Runner`/`ForgeError`는 forge에 남긴다. 새 범용 trait 계층은 필요 없다.
- **git:** 공유하는 것은 정확히 일치하는 환경 제거 기계 동작뿐이다. command 생성, argv, cwd, prompt, 필터/훅, 출력 변환과 timeout 정책은 각 도메인 adapter에 둔다. editor/diff를 자동으로 bounded 실행으로 전환하지 않는다. `pod-diff::model::git_command`는 기존 재수출을 유지하고 eol은 내부 git 모듈을 직접 참조해 순환을 끊는다. god git crate와 정책 enum 모음을 만들지 않는다.
- **atomic JSON:** storage의 primitive는 **호출자가 준 목적 경로·임시 경로·바이트를 write→rename**할 뿐이다. pretty 직렬화는 별도 fallible 함수다. 디렉터리 생성 순서, 오류 변환, 실패 임시 파일 삭제, corrupt 격리, mail JSONL 복구는 caller 책임. 고정 tmp를 고유 tmp로 바꾸기, fsync 추가, rename 실패 시 in-place fallback은 이번에 하지 않는다. “원자적 교체”와 “전원 장애 내구성”을 혼동하지 않는다.
- **now_ms 결정:** `podd::now_ms`를 앱/데몬의 기존 시계 입구로 유지한다. orch는 이미 `now: Ms`를 인자로 받으므로 global clock 의존성을 추가하지 않는다. `podd/src/lib.rs:52`와 `pod-app/src/orch.rs:532`는 같은 계산, layouts/blocks:391은 같은 의미다. 앱의 두 중복은 UI 소유자에게 위임한다. forge/time:20의 UTC 문자열, mods/manager:334의 초 단위 로그, 파일 mtime·임시 이름·monotonic deadline은 무리하게 합치지 않는다. 6줄 함수 때문에 `pod-time` crate를 늘리지 않는다. `orch_ui/fixture.rs:60`의 frozen clock은 보존한다.
- **상태 소유:** Orch/EditorWorkspace/LspStore를 그대로 둔 채 private 하위 모듈에 `impl`을 나눈다. 새 파일 간 함수는 가능한 한 private 또는 `pub(super)`; 테스트 편의를 위해 상태 필드를 `pub`으로 넓히지 않는다. `include!`로 파일 크기만 감추지 않는다. completion의 journal 순서, GPUI Subscription/Task의 drop 시점, generation/version 검사, defer_lsp는 façade와 해당 책임 모듈에서 명시한다.
- **공개 타입:** 기존 경로·메서드·serde 이름/default/skip 속성·오류 문자열을 유지한다. 새 모듈은 비공개가 기본이다. `Source`, `Host`, `Forge`, `Runner`를 확장점으로 유지하고 아직 소비자가 없는 추상 Repository/EventBus/Clock trait를 추가하지 않는다. 파일 이름은 담당 동작(items/documents/host_api)으로 정한다.
- **시험 지원:** `git_output`/`git_ok`는 assert와 진단만 공통화하고 기존 git 설정·상속 환경·stdout 의미를 바꾸지 않는다. 제품 fixture(`editor/src/fixture.rs`, `forge::fake`)는 데모에서도 쓰이므로 dev crate로 옮기지 않는다. `until`은 deadline/마지막 관측값을 반환하고, `stays`는 전체 관찰 구간 동안 부재 조건을 검사한다. GPUI event pumping은 해당 시험에 남긴다. 전역 환경 변경용 병렬 unsafe guard는 만들지 않는다.
- **스파이크 결정: workspace에 남긴다.** 현재 fan-in 0, default/CI 제외가 이미 목적을 달성한다. 분리하면 workspace 상속·patch·lock을 따로 관리하고 `cargo -p` 재현 경로가 바뀐다. 현재 분리의 실측 이득은 없다. 보관 디렉터리로 이동/삭제하지 않고, 명시적 spike 빌드는 선택 작업으로 유지한다. 향후 독립 toolchain이 필요한 경우만 별도 변경으로 검토한다.

## 3. 병렬 worktree 작업 카드 — 12개

**운영:** Wave 0의 F0를 먼저 단독 병합하고, 그 커밋에서 Wave 1의 L1–L10을 분기한다. Wave 1 내 OWNED는 서로 겹치지 않는다. `dir/{a,b}.rs`는 정확히 열거한 파일만 뜻하며 wildcard 소유권이 아니다. 기존 파일 삭제·이동도 파일 수에 포함한다. Wave 2의 T1은 모두 병합한 뒤 수행한다. 다른 영역의 root Cargo/lock 소유자와는 F0/T1 통합 창을 예약하며, UI 작업자의 파일은 어떤 카드도 소유하지 않는다.

**공통 완료 조건 G:** 동작 변경 없음, 기존 공개 경로 컴파일 가능, `podd/src/protocol.rs:75`의 **v19 유지**, `orch/engine.rs:19`의 RUN_VERSION=1 유지. 변경 전 같은 입력으로 얻은 JSON을 golden으로 고정하고 변경 후 비교한다(단순 round-trip만으로 대체 금지). 시간/id는 고정 입력, 객체 키 순서만 정규화; 배열 순서, null/누락, enum 문자열, 오류 코드·메시지는 그대로 비교한다. UI 카드 및 최종 통합은 같은 DPI/폰트/1440×960/테마의 기존 baseline으로 compare **0 failures**, 누락 이미지/UIA 필수 노드도 실패. baseline을 재생성해서 통과시키지 않는다. 네이티브 키보드·저장 동작은 별도 기존 QA로 증명한다.

**실행 제한:** 아래 검증은 후속 구현자가 수행할 계획이며 이번 평가에서 실행한 결과가 아니다. 모든 Cargo 실행 직전에 `Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue`, `$env:RUSTC_WRAPPER = 'C:\Users\User\AppData\Local\Microsoft\WinGet\Packages\Mozilla.sccache_Microsoft.Winget.Source_8wekyb3d8bbwe\sccache-v0.19.0-x86_64-pc-windows-msvc\sccache.exe'`, `$env:CARGO_BUILD_JOBS = '2'`, `& C:\ai\ide\tools\cpu-gate.ps1 -MaxPct 70 -MaxCargo 1` 실행 후 free RAM ≥8 GB를 확인하고 부족하면 기다린다. crate/필터별 한 Cargo 프로세스만, 전체 workspace build 금지. 데몬 시험은 `tests/common`의 독립 socket/state/worktrees 및 임시 root만 사용한다. 실제 에이전트 프롬프트·네트워크/GitHub 쓰기·실제 Pod/에이전트 설정 접근 금지. ignored 실서버 시험을 무심코 활성화하지 않는다.

### F0 — leaf crate와 의존 계약 확정

- **Wave 0 · L · 의존 없음 · 15파일.** OWNED: `Cargo.toml`, `Cargo.lock`; `crates/pod-process/{Cargo.toml,src/lib.rs}`, `crates/pod-storage/{Cargo.toml,src/lib.rs}`, `crates/pod-test-support/{Cargo.toml,src/lib.rs}`; `crates/{pod-editor,pod-forge,pod-orch,pod-mods,podd,pod-cli,pod-remote}/Cargo.toml`.

1. 위 배치 규칙의 최소 API·의존성·feature를 확정한다. 세 crate는 publish=false, workspace lint/rust-version 상속.
2. process/lib의 `bounded` 모듈에 기존 bounded 코드·시험을 그대로 복사한다. 원본은 L1 전까지 유지(단기 중복).
3. storage/lib에 직렬화·write/rename primitive와 실패 순서 시험, test-support/lib에 git·until/stays·missing과 자체 시험을 둔다.
4. process는 forge/editor/podd, storage는 forge/orch/mods/podd에 연결한다. podd의 server feature 경계를 유지한다. test-support는 podd/editor/mods/cli/remote의 **dev** 의존성만 추가한다. editor의 forge 제거는 T1까지 유예한다.
5. manifest/lock을 한 번 갱신하고 leaf 시험을 각각 실행한다. 기존 bounded 시험과 신규 primitive 성공·직렬화 실패·rename 실패·timeout 진단·require-tools 시험 통과, G. 새 crate API를 먼저 병합한 후 동결한다.

### L1 — bounded 소비자 이동·동일 git 환경 제거 공유

- **Wave 1 · M · F0 · 8파일.** OWNED: `crates/pod-process/src/{lib.rs,git.rs}`; `crates/pod-forge/src/{bounded.rs,runner.rs}`; `crates/pod-editor/src/lsp/registry.rs`; `crates/podd/src/git.rs`, `crates/podd/src/server/{groups.rs,repo_config.rs}`.

1. 기존 bounded.rs를 호환 재수출로 바꾸고 네 외부 소비자와 runner를 새 실행기로 연결한다.
2. git.rs에 동일 9항목 제거 함수만 추출하여 podd/forge에서 사용한다. executable/argv/env 나머지는 불변.
3. 기존 bounded 시험 10개, runner 시험, registry probe 시험을 필터별 실행한다. job assign 실패·즉시 자손·blocked stdin·deadline/drain·출력 상한을 모두 보존한다.
4. podd의 기존 groups/repo_config 및 `worker_commit_filters` 시험을 격리 실행한다. 명령 구성 시험으로 인자/환경 차이 0, 기존 공개 bounded 경로 컴파일, G.

### L2 — JSON 저장 primitive 적용

- **Wave 1 · M · F0 · 3파일.** OWNED: `crates/pod-orch/src/store.rs`, `crates/pod-forge/src/fake.rs`, `crates/podd/src/server/discovery.rs`.

1. 각 adapter의 부모 생성·직렬화·오류 문자열·cleanup 순서를 고정하고 primitive 호출로 교체한다.
2. 같은 파일의 시험에 pretty 바이트 golden과 rename 실패 후 원본/임시 파일 상태를 기록한다. discovery의 summary 임시 파일 삭제도 검증한다.
3. `store::tests`의 torn-last-line/corrupt-middle/corrupt-json, forge fake, podd discovery 시험을 각각 실행한다. JSONL은 그대로, G.

### L3 — Orch의 기능별 분할

- **Wave 1 · L · F0 · 11파일.** OWNED: `crates/pod-orch/src/engine.rs`, `crates/pod-orch/src/engine/{persistence,items,attempts,admission,completion,mail,wake_state,requests,tests}.rs`, `crates/pod-orch/tests/wire_snapshots.rs`.

1. 기존 시험을 tests.rs로 옮기고 상태/오류/입출력 타입과 공개 상수는 façade에 유지한다.
2. load/save/id 복구는 persistence, DAG는 items, token/generation/worker 수명은 attempts, 자원 대기는 admission으로 분리한다.
3. done/replay는 completion에 함께 둔다. append→전이→save 순서와 실패 시 상태를 고정한다.
4. mail/read receipt, wake 기록, request dedup을 각각 분리한다. 모든 하위 모듈은 같은 Orch 상태를 사용한다.
5. 분할 전 RunRecord/MailLine(메일·read)/Item/Attempt/MsgText를 고정 입력 JSON golden으로 저장한다. optional 누락·late done·wake 상태·기본값도 포함한다.
6. 기존 engine 시험을 실행한다. 특히 `done_with_a_failing_mail_write_changes_nothing`, `done_recorded_in_mail_survives_a_failed_run_write`, `replay_skips_a_reopened_attempt`, FIFO/reload, token fences, waits, wake restart 및 wire golden 통과, G. L2와 합친 뒤 T1에서 복구 시험 재확인.

### L4 — EditorWorkspace 분할

- **Wave 1 · L · F0 · 14파일.** OWNED: `crates/pod-editor/src/ui/workspace.rs`, `crates/pod-editor/src/ui/workspace/{session,buffers,files,tabs,tree,lsp_actions,render_tree,render_tabs,render_panels,render_overlays}.rs`, `crates/pod-editor/tests/{save_ui,tabs_ui,tabs_lsp}.rs`.

1. session 타입·JSON 시험을 이동하고 루트 공개 경로는 재수출로 보존한다. actions 이름·key context·init 순서를 유지한다.
2. buffer 생성/구독은 buffers, 저장/외부 변경은 files, 탭 수명은 tabs, 트리 명령은 tree로 옮긴다. TabSet을 계속 단일 규칙 소유자로 쓴다.
3. rename/reference/snippet 동작을 lsp_actions로 옮기되 LspStore API는 바꾸지 않는다.
4. render 함수만 네 모듈로 이동한다. element id·트리 순서·색·문구·포커스·이벤트 순서는 유지한다.
5. 기존 session JSON golden·save_ui·tabs_ui·tabs_lsp를 실행한다. didOpen/didClose 정확히 한 번, background restore 지연, MRU·close cancel·저장 실패·인코딩을 확인한다. tabs_lsp의 준비 대기만 until로 바꾸되 GPUI pump 유지.
6. editor baseline dark/light compare **0 failures**, 기존 native editor QA와 고대비 확인. host `pod-app` 파일·strings.rs를 수정하지 않고 G를 만족한다.

### L5 — LspStore 분할과 준비 대기

- **Wave 1 · L · F0 · 8파일.** OWNED: `crates/pod-editor/src/lsp/store.rs`, `crates/pod-editor/src/lsp/store/{lifecycle,documents,diagnostics,requests,edits}.rs`, `crates/pod-editor/tests/lsp_lifecycle.rs`, `crates/pod-editor/src/bin/fake_lsp.rs`.

1. 공개 타입·필드·메서드와 event façade는 유지한다. Slot·DocState 등의 소유자를 새 entity로 쪼개지 않는다.
2. 시작/재시작/종료/LRU는 lifecycle, open/change/save/close는 documents, stale 진단은 diagnostics로 이동한다.
3. 요청·completion 추가 편집은 requests, 위치/WorkspaceEdit 변환은 edits로 이동하고 기존 helper 경로를 재수출한다.
4. require_tools include를 dev crate로 바꾼다. PID 파일은 exists만 보지 말고 parse+alive+active_processes 조건으로 기다린다. 가짜 서버의 기존 didOpen/didClose 텍스트 로그와 별개로, 시험용 선택 인자에만 임시 root의 JSON wire 기록을 추가하여 분할 전 기대값을 만든다. 실서버 ignored 시험은 유지한다.
5. store 기존 단위 시험, 가짜 서버 lifecycle/applyEdit/자손 종료를 실행한다. UTF-16/32·한글/이모지·incremental replay·generation/version 차단·completion 한번 소비를 확인한다.
6. 가짜 LSP의 요청/알림 JSON golden을 비교하고 editor compare **0 failures**, G. L4 병합 후 T1에서 tabs_lsp 재검증.

### L6 — diff 입력·git·patch 경계

- **Wave 1 · M · F0 · 5파일.** OWNED: `crates/pod-diff/src/model.rs`, `crates/pod-diff/src/model/{git,patch,load}.rs`, `crates/pod-diff/src/eol.rs`.

1. public SourceFile/ChangedFile 및 함수 경로는 model façade에 유지한다. patch 생성, 파일/git 입력을 분리한다.
2. git_command/cat_files/batch header를 git 하위 모듈로 옮기고 eol→git으로 연결해 model↔eol 의존을 끊는다.
3. 기존 `patch_*`, `cat_file_missing_path_with_spaces_and_numbers`, `load_changed_from_git`, `line_endings_match_git_diff`, `parse_rows_match_sources`를 실행한다. batch 한 프로세스·공백 경로·binary/rename·줄 끝·크기 제한 불변.
4. 새 GPUI feature 분리나 crate 분리 없이 review baseline compare **0 failures**, G. 화면 baseline의 빈 리뷰만으로 populated diff 동작을 증명했다고 하지 않는다.

### L7 — mods 관리 책임 분리

- **Wave 1 · L · F0 · 8파일.** OWNED: `crates/pod-mods/src/manager.rs`, `crates/pod-mods/src/manager/{state,handle,core,host_api,timers}.rs`, `crates/pod-mods/tests/{manager,wire_snapshots}.rs`.

1. handle/Input/Shared, Core, host API dispatch, Timer 처리를 private 모듈로 분리한다. queue·runtime thread 소유권은 그대로 유지한다.
2. state에 JSON adapter를 옮겨 storage를 사용한다. 부모 생성/오류 문구/권한 저장 순서는 불변.
3. `Host`·ModHandle·Limits·manifest/wire 타입의 기존 API를 유지한다. 권한/scope/quota 검사는 host_api의 단일 경로로 유지한다.
4. 양성 결과는 until, 멈춘 timer 부재는 stays로 검증한다. 550ms의 tick 개수 시험은 관찰 구간을 보존하고 단순 첫 tick 성공으로 약화하지 않는다.
5. `tests/manager`의 permission/scope, overflow, CPU/memory/errors, timer/reload와 runtime 없는 feature 구성을 검증한다. ModInfo/ModList/CommandInfo/StopReason의 JSON golden, G.

### L8 — discover 스캔과 OS 수집 분리

- **Wave 1 · M · F0 · 7파일.** OWNED: `crates/pod-discover/src/{lib,scan,procs}.rs`, `crates/pod-discover/src/procs/{classify,windows}.rs`, `crates/pod-discover/tests/{parsers,wire_snapshots}.rs`.

1. Source/Limits/Budget/Cache/Found의 기존 경로를 유지하고 scan orchestration만 scan.rs로 옮긴다.
2. procs의 순수 분류·연결은 classify, Windows snapshot 구현은 windows로 옮긴다. 새 Source trait나 source format은 도입하지 않는다.
3. 기존 parsers/procs/matching 시험으로 예산 배분, newest-first, 중복 제거, cache hit/retry, previews-off, 경로 경계를 검증한다. 실제 사용자 저장소·프로세스 스캔은 하지 않는다.
4. Cache/SessionMeta/RunningAgent의 직렬화 golden으로 누락/default를 고정한다. 운영체제 수집 순서와 실패 fallback도 기존 fixture 입력으로 비교, G.

### L9 — podd의 8 git helper 및 관련 대기 통합

- **Wave 1 · M · F0 · 9파일.** OWNED: `crates/podd/tests/{tasks,sessions,pr,orchestration,orch_runs,mobile,lifecycle,discovery}.rs`, `crates/podd/tests/common/mod.rs`.

1. 8개 helper를 반환 형태에 맞는 git_output/git_ok 호출로 바꾼다. 테스트 저장소의 사용자명·설정·fixture 내용은 그대로 둔다.
2. common의 daemon/socket/state/holder 정리 소유권을 유지하고 공통 준비 polling만 until을 사용한다.
3. 각 파일의 sleep을 준비/재시도/부재 관찰/시간 정책으로 분류한다. 준비만 조건 대기로, 부재는 stays로 바꾸고 grace·TTL·rate limit을 시험하는 시간은 유지한다.
4. 변경한 테스트 이름별로 격리 실행한다. helper stdout/오류 시 argv·stderr, deadline 실패 진단, PR 병합 후 무폴링, terminal/worker lifecycle의 기존 assertion이 모두 남아야 한다. G.

### L10 — tools 경로 의존과 workspace 정책 정리

- **Wave 1 · S · F0 · 8파일.** OWNED: `tools/{check-workspace.ps1,check-docs.ps1,check-boundaries.ps1}`, `tools/test-support/require_tools.rs`, `tools/visual/README.md`, `crates/pod-cli/tests/hooks.rs`, `crates/pod-remote/tests/system_ssh.rs`, `crates/podd/tests/gaps.rs`.

1. 세 require_tools 소비자를 dev crate로 바꾸고 옛 helper 파일은 `pub use pod_test_support::tools::*` 호환 shim으로 남긴다. editor 소비자는 L5 소유이므로 어느 병합 순서에서도 컴파일되어야 한다.
2. check-boundaries에서 test-support의 일반/build 의존 금지, process/storage의 도메인 역의존 금지, spike defaults 제외를 검사한다. check-workspace는 새 member도 lint/rust-version 규칙을 적용한다.
3. 시각 검증 안내의 불필요한 workspace build·4 jobs를 대상 crate·2 jobs·CPU/RAM gate로 정정한다. check-docs는 중첩 설계 문서의 상대 링크도 읽기 전용으로 검사하되 README 추가 편집을 요구하지 않는다.
4. 정책 스크립트·compare/uia selftest와 `require_tools_*` 시험만 실행한다. hooks의 실제 에이전트 시험·system ssh 네트워크 경로는 실행하지 않는다. spike 명시 빌드 방법과 기존 호출 경로 보존, G.

### T1 — 통합 경계·wire·시각 검증

- **Wave 2 · M · L1–L10 · 6파일.** OWNED: `crates/pod-editor/Cargo.toml`, `Cargo.lock`, `crates/podd/tests/libs_wire_snapshots.rs`, `docs/20-refactor/libs-validation.md`, `tools/check-boundaries.ps1`, `tools/test-support/require_tools.rs`.

1. editor→forge가 사라졌음을 소스 참조로 확인하고 manifest 의존성을 제거, lock만 정리한다. include 소비자가 0인 require_tools shim은 삭제한다. 제품 API 호환 재수출은 제거하지 않는다.
2. baseline master에서 얻은 고정 JSON과 통합 결과를 비교하는 wire 시험을 추가한다: orch RunInfo/WorkerInfo/mail, PR 결과·오류, discovery 결과, mod 결과 및 공통 schema. v19·enum/default/skip·token 제거·배열 순서 불변.
3. L2+L3의 journal 실패/복구, L4+L5의 tabs_lsp/save_ui, L1+podd worker_commit_filters를 다시 해당 필터만 실행한다. 개별 카드 결과와 실제 실행한 명령/환경/성공·실패를 validation에 남긴다.
4. 같은 통합 커밋의 UI fixture로 기존 baseline 전체 compare **0 failures**, 필수 UIA와 편집기 native QA를 확인한다. UI 병렬 변경이 baseline을 바꿨다면 그 소유자가 승인한 기준을 먼저 확정하고 이 리팩터의 비교를 다시 한다.
5. OWNED/공개 경로/의존 방향/프로토콜 버전 검사를 마친다. 미실행을 PASS로 쓰지 않는다. Cargo/lock과 시각 기준 충돌은 이 통합 창에서만 해결한다. G를 못 만족하면 완료 처리하지 않는다.

## 4. 위험과 하지 말 것

- **big-bang 금지:** 파일 이동과 새 동작·저장 형식·timeout 보강·보안 정책 변경을 섞지 않는다. 우선 façade+private impl 이동으로 경계를 만든다. 엔진을 한 번에 이벤트 소싱·DB·비동기 actor로 바꾸거나 편집기 상태를 여러 entity로 재작성하지 않는다.
- **교차 카드 hotspot:** root Cargo.toml/lock과 소비자 manifest는 F0가 선점, editor manifest/lock 정리는 T1만 수행한다. 각 `mod` 선언은 소유한 기존 façade 파일에서만 추가한다. L3는 orch/lib.rs를, L4는 editor/lib.rs·ui/mod.rs를, L5는 lsp/mod.rs를 건드릴 필요가 없도록 기존 경로를 유지한다. `pod-app/src/strings.rs`, `pod-editor/src/ui/strings.rs`, `pod-orch/src/text.rs`의 문구/키는 동결한다.
- **실패 순서가 API다:** mail 기록 전 메모리 변경, 저장 실패 임시 파일 삭제의 일괄 적용, read-only/외부 변경 배너 제거, subscription/task 조기 drop, generation/UTF 변환 위치 변경은 금지한다. JSON round-trip 통과만으로 wire 동일성을 주장하지 않는다.
- **테스트 지원의 역류 금지:** fake forge와 editor fixture는 제품 데모 소비자가 있으므로 dev crate로 이동하지 않는다. test-support에 데몬 프로토콜·GUI·네트워크·실제 에이전트 실행기를 넣지 않는다. 모든 sleep 제거를 목표로 삼지 않는다. 범위 밖 retention/stop_race 등의 시간 정책 시험은 이번 카드가 수정하지 않는다.
- **호환·검증의 한계:** 공개 선언 수는 정적 계수이고 macro/feature별 전체 API 검증을 대신하지 않는다. LSP 장수 프로세스와 bounded 단발 프로세스는 수명이 다르다. baseline의 review/gates는 빈/연결 끊김 장면이므로 diff 내용과 RPC 시험을 별도로 유지한다. 제3자 앱 화면·내부 자료를 문서나 golden에 넣지 않는다.

이번 평가의 실행 검증: `tools/check-workspace.ps1` **PASS (16 members, 14 default)**, `tools/check-docs.ps1` **PASS**. OWNED의 brace 목록을 펼쳐 자동 검사한 결과 **12카드, 같은 wave 중복 0, 카드당 최대 15파일/6단계**다. 크기·의존성·pub·helper/sleep 수는 로컬 정적 계수와 코드 대조로 측정했다. Cargo 빌드/시험·daemon·UI 캡처는 실행하지 않았으며 성능 향상 수치도 주장하지 않는다.

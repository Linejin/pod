# UI 동작 보존 리팩터링 설계

기준: 2026-11-10, master `8b835ff3930c5bf08ff1e70596ae02b82614246a`(분석 시작 시 HEAD와 동일). **평가·설계만 수행**했다. 아래 카드는 후속 구현용이며 제품 코드, 사용자 설정, 데몬을 변경하지 않았다. 병렬 UI 작업의 작업 파일 대신 이 커밋의 Git blob을 읽었다. Cargo·앱·시각 비교는 이번에 실행하지 않았으며 통과를 주장하지 않는다.

읽은 기준: [README 문서 목록](../../README.md#문서), [03 프로토콜·attach](../03-podd-p0.md), [08 작업](../08-tasks.md), [09 앱](../09-pod-app.md), [10 D4·D5](../10-ui-design.md), [19 §15–16](../19-worktree-sessions-orchestration.md), [시각 검증](../../tools/visual/README.md). 과거 단계 설명보다 현재 코드를 우선한다. 예를 들어 09의 초기 폴링 설명과 달리 현재 `source.rs:26`에는 lifecycle 활성 시 15/45초 안전망도 있다. 다른 앱의 내부 구현은 조사·인용하지 않았다.

## 1. 현재 구조 지도

### 실측 방법과 크기

이하 `A` = `crates/pod-app/src`, `U` = `crates/pod-ui/src`, `T` = `crates/pod-terminal-view/src`. `파일:줄`은 기준 커밋의 위치다. `git ls-tree -r --name-only <sha>`로 목록을 얻고 `git show <sha>:<path>`의 UTF-8 본문을 `splitlines()`로 셌다. 공백·주석·인라인 테스트 포함, `src/**/*.rs`만 포함한다.

| 범위 | 파일 수 / 줄 수 | bare `pub` 항목 / 필드 | 제한 공개 선언 | 테스트 attribute 수 |
|---|---:|---:|---:|---:|
| pod-app | 56 / 54,430 | 1,516 / 439 | 257 | 248 |
| pod-ui | 9 / 4,384 | 189 / 145 | 1 | 32 |
| pod-terminal-view | 13 / 5,022 | 116 / 90 | 16 | 44 |

공개 항목은 줄 시작의 `pub fn/struct/enum/trait/type/mod/use/static/const`(async/unsafe 허용), 필드는 `pub 이름:`, 제한 공개는 `pub(...)`를 별도 집계했다. 매크로 생성·trait의 암묵적 공개는 제외, 테스트·비공개 모듈 내부 선언은 포함한다. **외부에서 도달 가능한 API 개수는 아니다.** pod-app은 binary이고, 라이브러리 경계는 `U/lib.rs:2`의 모듈 및 glob 재수출, `T/lib.rs:17`의 모듈과 명시 재수출로 확인했다. `U/tests/select_scroll.rs`의 테스트 2개는 위 src 집계 밖이다.

| 주요 파일/묶음 | 줄 수 |
|---|---:|
| `A/workspace.rs`, `review.rs`, `native_qa.rs` | 4,327 / 2,688 / 2,677 |
| `A/layouts/blocks.rs`, `dense.rs`, `kanban.rs` | 2,604 / 2,515 / 2,268 |
| `A/layouts/review.rs`, `rail.rs`, `queue.rs` | 2,251 / 2,245 / 1,793 |
| `A/chrome.rs`, `groups.rs`, `source.rs` | 2,125 / 1,861 / 1,807 |
| `A/orch_ui` 전체 8파일 | 6,151 (model 1,822, pages 1,385, mod 1,010) |
| `A/mods_ui.rs`, `sessions_ui.rs`, `strings.rs`, `vm.rs` | 1,204 / 927 / 749 / 602 |
| `U/components.rs`, `themes.rs`, `controls.rs` | 1,286 / 1,051 / 666 |
| `T/element.rs`, `view.rs` | 1,010 / 922 |

7개 모드 중 별도 구현 파일은 6개(합계 13,676줄)다. Workbench는 `A/layout_mode.rs:166`의 어댑터와 `A/workspace.rs:3844`의 본문이다. 따라서 “7개 대형 레이아웃 파일”로 계산하지 않는다.

### 의존 방향과 결합도

```text
main → Workspace / chrome / LayoutRegistry
Workspace ↔ layouts, review, groups, orch_ui, mods_ui, sessions_ui
Workspace → source → podd client/protocol
             └→ vm → podd protocol + orch/strings
Workspace → panel → attach + pod-terminal-view
app / pod-editor → pod-ui → pod-status
pod-app / pod-spike → pod-terminal-view → GPUI + VT 엔진
```

`crates/*/Cargo.toml`의 일반 `[dependencies]` 중 path 의존을 셌다(dev/build/외부 패키지 제외). pod-app fan-out 7 = pod-diff/editor/forge/status/terminal-view/ui/podd, fan-in 0. pod-ui out 1(status), in 2(app/editor). terminal-view out 0, in 2(app/spike). **terminal-view는 이미 데몬·앱에서 독립적**이다.

앱 내부는 파일을 최상위 모듈로 합쳐, 주석 전용 줄을 제외한 명시적 `crate::<모듈>` 참조를 중복 제거했다. 자기 참조·`mod` 선언 제외, 테스트 포함; `super`, 별칭, 암묵적 호출을 해석하지 않은 정적 참조 지표다(컴파일러 call graph 아님).

| 모듈 | fan-in / fan-out | 해석 |
|---|---:|---|
| workspace | 21 / 23 | 기능들이 루트 상태와 메서드에 직접 접근 |
| layouts(하위 합산) | 2 / 22 | 레이아웃 교체가 데이터·패널·기능 전반에 접촉 |
| source | 18 / 7 | 실행용 fixture와 test backend 참조까지 포함 |
| chrome | 6 / 10 | 명령·설정·검사·기능 연결의 교차점 |
| review / groups | 각각 5 / 8 | Workspace·source·표현 유틸에 결합 |
| orch_ui | 5 / 12 | model 분리가 있으나 controller·전역 상태가 넓음 |
| mods_ui / sessions_ui | 4 / 4, 3 / 5 | feature 단위 분할 가능 |
| vm / strings | 15 / 2, 18 / 0 | vm은 orch·strings도 참조, strings는 변경 집중점 |
| native_qa | 7 / 14 | 공용 harness와 시나리오가 같은 모듈 |

### 확장을 막는 문제 10개

1. **Workspace에 수명·입력·화면이 집중**: `A/workspace.rs:302` 상태 필드, `:542` 이벤트, `:850` 복원, `:1504` 작업 생성 대화상자, `:2036` 편집기, `:2813` 상태바, `:3356` sidebar 렌더. 이 파일의 bare pub 항목 29개·제한 공개 선언 76개다. 패널 하나를 추가해도 복원·포커스·상태 표시 변경이 같은 파일에서 충돌한다.
2. **LayoutModule이 좁은 연결점이 아님**: `A/layout_mode.rs:69`는 `&mut Workspace`와 `Context<Workspace>`를 통째로 전달한다. `A/layouts/kanban.rs:475`는 렌더 중 키 등록·view 생성·전역 슬롯·배너·토스트까지 처리한다. 새 모드는 Workspace의 구현과 수명 규칙을 알아야 한다. 등록부의 교체/fallback 정책(`layout_mode.rs:113`, `:121`)은 보존할 자산이다.
3. **공통 표현의 중복과 서로 다른 정책이 섞임**: 동일한 `tc(hex) → pod_ui::tokens::color(hex)` 본문은 `A/orch.rs:46`, `layouts/queue.rs:708`, `dense.rs:1160` **3곳**. 동일 글꼴 계산은 `blocks.rs:1190`, `kanban.rs:1042`, `review.rs:897`, `dense.rs:1165` **4곳**. 연결 배너·hook·toast shell은 `kanban.rs:490`, `review.rs:342`에서 같은 구조다. 반면 행 높이(`blocks.rs:1185`, `review.rs:891`, `dense.rs:1173`)는 다르다. `orch.rs:532`와 `blocks.rs:391`의 epoch 시각 계산은 동등하지만 `orch_ui/fixture.rs:60`은 고정 시계까지 처리한다. 무차별 통합은 치수·시간 표시를 바꾼다.
4. **feature에서 정책·비동기 요청·render가 한 덩어리**: Review의 전송 선택 `A/review.rs:288`, diff 요청 `:585`, 전송 `:1038`, poll `:1172`, toolbar `:1215`; Group의 검증 `A/groups.rs:129`, `impl Workspace` 대화상자 `:162`, check/pick `:742`/`:872`, 카드 `:1196`. UI를 추가할 때 요청 중복·입력 보존 정책까지 건드리게 된다. `orch_ui/model.rs:191`의 순수 row 계산은 확장할 선례다.
5. **source 경계가 문서보다 넓고 변환 위치가 분산**: `A/source.rs:208` 실행용 fixture, `:344` scripted backend, `:609` 제어 루프, `:934` RPC, `:999` 입력, `:1085` 이벤트가 공존한다. `A/vm.rs:163`, `:316`도 protocol JSON을 파싱하고 `:118`은 orch 표시 로직에 의존한다. 새 wire 필드를 붙일 위치와 lenient fallback 책임이 불분명하다. control/input/attach의 별도 연결은 유지해야 한다.
6. **명령 경로가 여러 목록으로 분산**: `A/chrome.rs:24` 키, `:63` COMMANDS, `:377` 부근 실행 분기, `A/workspace.rs:1065` action 처리, `A/main.rs:191` 설치. 팔레트 ID·키 컨텍스트·실행 경로가 함께 검증되지 않으면 새 명령이 일부 진입점에서 빠진다. 동적 mod/orch 명령은 정적 목록과 별도 공급자로 남겨야 한다.
7. **문구 경계가 불규칙**: `A/strings.rs`에 bare pub 항목 372개와 `:370`의 `tr!`, `:726`의 toolkit locale 설치가 있다. 동시에 `mods_ui.rs:23`, `sessions_ui.rs:19`, `orch_ui/s.rs:1`, 각 레이아웃에 문구가 분산된다. `orch_ui/ko.rs:37`은 구조화 메시지 번역·unknown fallback이라는 별도 책임이다. 한 파일로 더 모으면 모든 기능 카드가 충돌한다.
8. **QA 공용 도구가 시나리오에 종속**: `A/native_qa.rs:42` 환경 격리, `:62` Harness, `:463` seed 뒤에 24개 테스트가 이어진다. 11개 `*_qa.rs`/`qa.rs`/`tests.rs` 파일만 7,834줄. `dense/tests.rs:10`, `queue/tests.rs:10`, `orch_ui/qa.rs:21`이 native_qa를 공용 라이브러리처럼 사용한다. `dense/tests.rs:14`/queue `:14`의 terminal builder, dense `:468`/queue `:351`의 dock 관측도 반복된다. 공용 harness 수정과 제품 시나리오 수정의 리뷰 범위가 섞인다.
9. **pod-ui의 내부 파일 분리와 공개 경계가 불균형**: `U/components.rs:8` 아이콘, `:400` 버튼, `:484` tree row, `:594` focus, `:620` frame, `:888` toast timeout이 공존하고 `U/lib.rs:11`은 glob 재수출한다. 컨트롤 확장이 도메인과 무관한 큰 파일을 건드린다. 기존 app/editor import 경로는 유지하며 내부만 나눌 수 있다.
10. **터미널의 좋은 crate 경계 안에서 책임이 집중**: `T/view.rs:436` 키, `:563` 마우스, `:814` IME handler와 `T/element.rs:117` prepaint, `:393` cursor, `:471` preedit, `:582` shaping이 밀집한다. 입력 방식·그리기 확장이 서로 간섭할 수 있다. `T/sink.rs:21` InputSink와 `T/lib.rs:3`의 PTY 비소유 원칙을 바꿀 이유는 없다.

## 2. 목표 구조와 배치 규칙

**새 crate 없이 세 crate를 유지한다.** file split → 기존 경로 facade 유지 → 사용처 이관 → 호환 shim 제거 순서다. 아래 경로의 소유권은 §3에서 확정한다.

```text
pod-app/src/
  workspace.rs                 Workspace 소유 상태·생성·얇은 조립부
  workspace/{dock,binding,layout_host,status_bar,commands,sidebar,
             dialogs,editor,notifications,shell,tests}.rs
  layout_mode.rs               LayoutMode / LayoutModule / Registry
  layouts/kit.rs               좁은 계약과 공통 shell
  layouts/kit/{model,widgets,metrics,clock,tests}.rs
  layouts/<mode>.rs            기존 facade·view 수명 연결
  layouts/<mode>/{model,view,copy,tests}.rs  필요한 책임만 생성
  source.rs + source/{control,input,events,rpc,mapping,fixture,
                      test_backend,tests,wire_tests}.rs
  vm.rs + vm/presentation.rs   데이터 형태 / 기존 표시 메서드 호환부
  review.rs, groups.rs, mods_ui.rs, sessions_ui.rs + 각각의 하위 모듈
  chrome.rs + chrome/{commands,palette,settings,inspector,notifications,copy,tests}.rs
  orch_ui/{mod,model,pages,inbox,...}.rs  기존 분리 위에 controller·page별 분리
  strings.rs + strings/{common,workspace,review,groups,templates,toolkit,tests}.rs
  native_qa.rs                  cfg(test) facade·시나리오 mod 목록
  native_qa/{harness,fixtures,keyboard,focus,ime,dpi,links,editor,extensions}.rs
pod-ui/src/components.rs        기존 공개 경로를 보존하는 facade
  components/{icons,buttons,rows,frames,text,focus,tests}.rs
  controls.rs + controls/{select,switch,tests}.rs
pod-terminal-view/src/
  view.rs + view/{input,ime,links,tests}.rs
  element.rs + element/{text,cursor,preedit,tests}.rs
```

### Layout kit와 상태 소유

- 최종 `LayoutModule`은 `mode()`, `render_body(&LayoutFrame, &mut dyn LayoutHost, &mut Window, &mut App) -> AnyElement`, `title_bar_extra(...)`, `status_counts(...)`를 갖는다. 기존 7개 ID, registry 교체·unknown fallback, Queue의 제목줄 집계 정책은 그대로다. 동적 플러그인 ABI를 새로 만들지 않는다.
- `LayoutFrame`은 tasks/terminals/연결 상태/게이트/선택/metrics의 읽기 전용 투영이다. 필요한 표시 데이터만 전달하고 매 프레임 전체 VM을 복제하지 않는다. `LayoutHost`는 기존 dock/sidebar/terminal/review view를 **빌려 표시하는 슬롯**, 기존 view 상태 접근, `UiIntent` 전달을 제공한다. 구현은 `workspace/layout_host.rs`만 Workspace를 안다. GPUI의 `AnyView`/Entity 수명·타입 소거는 이 어댑터 안에 둔다.
- 이벤트 closure에는 빌린 frame을 캡처하지 않고 안정 ID와 clone 가능한 `LayoutActions`를 전달한다. `LayoutActions`의 내부 weak Workspace는 host에 숨긴다. 즉시 포커스가 필요한 경로는 기존 동기 호출 순서를 유지하고, 기존 defer 경로만 defer한다. Workspace update 안에서 다시 Workspace를 update하는 재진입은 금지한다.
- `UiIntent`는 OpenTerminal/OpenReview/FocusRegion 등 UI 의도다. wire enum이 아니다. gate resolve·RPC는 기존 controller에 위임한다. 레이아웃별 `Nav`, `BoardModel`, `EventLog`, `TermBlocks`를 거대한 공용 enum으로 합치지 않는다. 특히 blocks의 추가 attach 소비자 수명은 별도로 보존한다.
- `kit/widgets`는 Header/RowFrame/CardFrame/ConnectionShell 같은 표현 조합만 소유한다. padding, row-height, hit-area, ID, 접근성 이름, focus handle을 명시 인자로 받아 **현재 값**을 재현한다. 이미 있는 `pod_ui::TreeRow/Frame/semantic_button`을 우선 조합한다. kit의 Pod 작업 의미를 pod-ui로 내려보내지 않는다.
- 색 변환은 기존 `pod_ui::tokens::color`를 재사용한다. 공통 font 계산만 `LayoutMetrics`로 추출하고 서로 다른 높이 공식은 각 mode에 남긴다. `Clock`은 epoch ms의 System/Frozen 공급원이다. 기존 caller별 시계 선택·갱신 시점을 보존하며 새 타이머나 전역 fixture freeze를 추가하지 않는다.

### 무엇을 어디에 둘 것인가

| 책임 | 위치 / 규칙 |
|---|---|
| 실제 작업·터미널 상태 | podd 유지. UI가 새 영속 권위나 백엔드 상태 머신을 만들지 않음 |
| transport·wire 변환 | source 및 feature controller의 명시 decoder. method/params/error/default/unknown-field 의미 보존. 일반 VM의 `from_json` 구현은 source/mapping으로 옮기되 기존 연관 함수 경로 유지 가능 |
| 순수 표시 정책 | feature `model.rs`: 입력→row/card/action availability. Window/Context·RPC·환경 변수·현재 시각 읽기 금지; now는 인자 |
| render | feature `view.rs`: 이미 결정한 모델을 그림. 이벤트는 controller 호출. 네트워크 요청을 렌더 반복 자체가 발생시키지 않음 |
| GPUI 수명 | 기존 entity·subscription·focus·dock 소유자를 먼저 유지. file split을 기회로 전역 상태 저장 방식을 한꺼번에 교체하지 않음 |
| workspace | dock/panels 복원과 detach는 dock, source pump/알림 순서는 binding, 명령 routing은 commands, 표시 계획은 status_bar/sidebar, 앱 창 조립은 shell |
| 명령 | `CommandSpec { id, label, shortcut, context }`와 실행 routing. 정적 명령은 chrome/commands, 동적 명령은 각 feature provider. 문자열 ID·키 컨텍스트를 유지 |
| 문구/i18n | 기존 공용 문구는 strings 하위 도메인, feature 문구는 local copy(orch의 s/ko 호환 경로 유지). strings.rs는 명시 재수출 facade. protocol ID·사용자 본문·명령줄은 번역하지 않음 |
| 포맷/번역 | tr!의 호출 문법·placeholder를 보존해 templates로 이동. toolkit hook는 toolkit, MsgText 번역·unknown English fallback은 orch_ui/ko 유지. 새 언어·런타임 언어 전환은 별도 기능 |
| 테스트 | 순수 테스트는 해당 model 근처; GPUI 공용 harness는 cfg(test) native_qa/harness, scripted transport는 cfg(test) source/test_backend. 기능 QA는 기능 소유 파일. 실제 `--ui-fixture`는 릴리스 캡처에도 필요하므로 cfg(test)로 숨기지 않음 |
| API | 새 내부 항목은 private 또는 pub(super), 형제 사용처가 있을 때만 pub(crate). 기존 라이브러리 재수출·타입·InputSink/VtEngine 시그니처 유지. 공개화로 테스트 접근 문제를 해결하지 않음 |

## 3. 병렬 worktree 작업 카드 — 19개

### 공통 완료 기준

- 아래 OWNED는 **수정/신규/삭제 가능한 파일의 전부**다. 중괄호·glob 대신 각 파일을 열거했다. A/U/T 접두어는 §1의 경로로 확장한다. 같은 wave 내 목록은 서로 겹치지 않는다. 파일 수는 테스트 포함이며 캡처 출력은 격리 temp artifact로 보관해 tracked baseline을 변경하지 않는다. 범위 초과 시 소유권을 재설계한 뒤 진행한다.
- **B(동작)**: 기존 테스트 이름·assertion을 유지해 관련 filter를 실행한다. 이동 전/후 테스트 목록을 대응시켜 테스트 누락을 막는다. entity ID, panel ID, focus 경로, detach≠kill, saved layout JSON, 알림/요청 횟수·순서까지 비교한다. 버그 수정·문구 변경을 섞지 않는다.
- **J(wire)**: source boundary와 feature controller에서 실제 송신 params/수신 fixture를 캡처한 고정 JSON을 비교한다. TerminalInfo/TaskInfo/StatusInfo, gate/group/review delivery, orch run/mail/settings, mod/layout preset, discovered session의 대표 성공·오류·누락·unknown 필드를 포함한다. 키 순서만 정규화하고 배열 순서·null/생략·enum tag·default·오류 코드는 유지한다. 기존 `podd/src/protocol.rs:1374,1434`, `orch_run_protocol.rs:767` wire 테스트는 선례이지 전체 golden coverage가 아니다. 새 UI 계약 fixture는 P2가 소유한다. 다른 카드의 추가 payload는 P2에 전달하고 그 파일을 직접 고치지 않는다. **PROTOCOL_VERSION = 19(`podd/src/protocol.rs:75`) 유지**, backend 타입 파일 변경 없음.
- **V(시각)**: F0에서 기준 커밋의 기존 18장면×dark/light **36장** 및 각 레이아웃의 해당 장면 전후 기준을 동결한다. 같은 binary provenance·monitor DPI(기존 기준 144)·1440×960 physical·폰트·데이터·시계를 사용한다. `capture.ps1 -Mode capture -Output <isolated-actual>` 후 `-Mode compare -Output <isolated-actual> -Baseline <frozen-baseline>`를 실행해 **0 failures**. compare는 재캡처하지 않고 기존 PNG를 비교한다(`capture.ps1:68`). 허용 오차 8을 높이거나 영역을 숨기거나 baseline을 다시 찍어 차이를 승인하지 않는다.
- V에는 7 layout×dark/light, 영향을 받는 style·HC, 작은 창·큰 글자·긴 한국어, selected/hover/focus 장면의 같은 조건 전후 비교를 더한다. baseline 디렉터리를 matrix별로 분리해야 compare가 다른 장면의 누락을 올바르게 검출한다. 기존 fixture Review/Gates는 비어 있어 populated review·gate·실제 terminal/IME를 증명하지 못한다. fake backend/격리 fake-agent 장면과 B의 IME·Tab·Esc 시험을 별도로 요구한다. 125/175% 실제 raster가 불가능하면 미검증으로 기록하고 해당 수락을 보류한다.
- **실행 제한**: 이번 문서 작업에서는 실행하지 않음. 후속 평가에서 Cargo 호출마다 아래 준비 후, 단일 crate·관련 filter만 실행한다. `tools/visual/README.md`의 전체 workspace 빌드·4 jobs 예시는 이 작업에 적용하지 않는다. capture 스크립트도 jobs=4를 설정하므로 이후 Cargo 직전에 준비를 다시 한다.

```powershell
Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
$env:RUSTC_WRAPPER = 'C:\Users\User\AppData\Local\Microsoft\WinGet\Packages\Mozilla.sccache_Microsoft.Winget.Source_8wekyb3d8bbwe\sccache-v0.19.0-x86_64-pc-windows-msvc\sccache.exe'
$env:CARGO_BUILD_JOBS = '2'
& C:\ai\ide\tools\cpu-gate.ps1 -MaxPct 70 -MaxCargo 1
while ((Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory * 1KB -lt 8GB) {
    Start-Sleep -Seconds 10
}
# 이후 예: cargo test -p pod-app layouts::rail::tests
```

실데몬이 필요한 검증만 temp root 아래 전용 POD_SOCKET/state/worktrees/config/themes 경로를 사용한다. `%APPDATA%\Pod`, 실제 podd, 에이전트 설정, 실제 Claude/Codex 프롬프트, 네트워크/GitHub 쓰기는 사용하지 않는다.

### Wave 0 — 기반, 전부 병합한 뒤 Wave 1 시작

**F0 · kit 계약과 전후 기준 · M · 의존 없음 · 9파일**

OWNED: `A/layout_mode.rs`, `A/layouts/mod.rs`, `A/layouts/kit.rs`, `A/layouts/kit/model.rs`, `A/layouts/kit/widgets.rs`, `A/layouts/kit/metrics.rs`, `A/layouts/kit/clock.rs`, `A/layouts/kit/tests.rs`, `crates/pod-app/qa/refactor/contract.json`.

1. B/J/V의 기준 revision·환경·장면 목록·artifact 경로를 contract에 기록하고 before를 동결한다.
2. LayoutFrame/Host/Actions/metrics/clock 및 표현 helper를 추가한다. 현재 renderer는 유지한다.
3. 새 좁은 renderer는 기본 `None`인 선택 hook로 추가한다. 아직 기존 dispatcher를 유지하며, F1이 host를 연결한 뒤 None이면 기존 render_body로 fallback한다. 모든 소비자는 독립적으로 이관 가능해야 한다.
4. registry fallback/re-register와 동일 font/color/clock 값 테스트를 보강한다. 증명: `layout_mode::tests` + kit tests, B/V; wire 변경 없음.

**F1 · Workspace 기계적 분할 · L · 의존 F0 · 12파일**

OWNED: `A/workspace.rs`, `A/workspace/dock.rs`, `A/workspace/binding.rs`, `A/workspace/status_bar.rs`, `A/workspace/commands.rs`, `A/workspace/sidebar.rs`, `A/workspace/dialogs.rs`, `A/workspace/editor.rs`, `A/workspace/notifications.rs`, `A/workspace/shell.rs`, `A/workspace/tests.rs`, `A/workspace/layout_host.rs`.

1. 기존 상태 소유와 메서드 시그니처를 고정하고 책임별 impl/자유 함수를 이동한다.
2. build_*·RestoreCtx는 dock, pump/on_source_event는 binding, 나머지는 목표 구조대로 배치한다.
3. 기존 workspace import·테스트 경로는 명시 재수출로 보존한다. layout_host에 F0 계약의 read/slot/action 어댑터를 구현하고 shell에서 새 hook→기존 renderer fallback을 연결한다. F0 병합 뒤 수행한다.
4. `workspace::tests`, `layout::tests`, `auto_open::tests`, native QA의 초기/닫기 focus·복원·저장 시나리오로 B/V를 확인한다. 상태 필드의 그룹화·수명 변경은 하지 않는다.

**F2 · 테스트 공용부와 native 시나리오 분리 · M · 의존 없음 · 10파일**

OWNED: `A/native_qa.rs`, `A/native_qa/harness.rs`, `A/native_qa/fixtures.rs`, `A/native_qa/keyboard.rs`, `A/native_qa/focus.rs`, `A/native_qa/ime.rs`, `A/native_qa/dpi.rs`, `A/native_qa/links.rs`, `A/native_qa/editor.rs`, `A/native_qa/extensions.rs`.

1. Harness·isolate_config·IME 입력 도구와 seed를 공용부로 옮긴다. 기존 `native_qa::{Harness,seed_two_tasks}`를 재수출한다.
2. 24개 native 테스트를 주제별 이동하고 기존 테스트와 1:1 목록을 남긴다. 반복 dock 관측·bounds 도구를 제공하되 기존 의미를 바꾸지 않는다.
3. native filter와 외부 소비자인 `layouts::dense::tests`, `orch_ui::qa`를 순차 검증한다. B: 환경 격리·요청 기록·focus·IME assertion 동일; 제품 렌더 변경 없음.

### Wave 1 — F0/F1/F2 병합 기준에서 병렬

모든 P 카드는 F0–F2에 의존하며 **서로 의존하지 않는다**. 기존 facade 덕분에 P2의 source 분할, P8의 pod-ui 분할을 기다릴 필요가 없다. 레이아웃 카드는 새 hook뿐 아니라 해당 view의 Workspace 읽기·동작 접근도 frame/host/actions로 이관한다. 기존 view 보유·갱신 순서는 host가 그대로 연결한다. 각 카드의 마지막 단계는 B/J/V 중 해당 검증과 변경 파일 목록 확인이다.

**P1 · Workspace 표현·명령과 chrome · L · 15파일**

OWNED: `A/chrome.rs`, `A/chrome/commands.rs`, `A/chrome/palette.rs`, `A/chrome/settings.rs`, `A/chrome/inspector.rs`, `A/chrome/notifications.rs`, `A/chrome/copy.rs`, `A/chrome/tests.rs`, `A/workspace/commands.rs`, `A/workspace/status_bar.rs`, `A/workspace/sidebar.rs`, `A/workspace/shell.rs`, `A/workspace/notifications.rs`, `A/orch.rs`, `A/ui_review_qa.rs`.

1. chrome를 나누고 정적 CommandSpec을 키·팔레트·실행의 연결표로 만든다. 동적 feature provider는 기존 API 유지.
2. StatusPlan/sidebar 표시 모델을 render에서 분리하고 Workbench shell에 kit를 적용한다. orch의 tc/now wrapper는 같은 동작의 공통 구현으로 위임한다.
3. `ui_review_qa`, workspace status/tree 테스트와 native shortcut/IME/notification/quit 시나리오로 B/V 확인. 좁은 창 숨김 순서, toast pause, 팔레트 ID를 고정한다.

**P2 · source/VM/wire 계약 · L · 13파일**

OWNED: `A/source.rs`, `A/vm.rs`, `A/vm/presentation.rs`, `A/source/control.rs`, `A/source/input.rs`, `A/source/events.rs`, `A/source/rpc.rs`, `A/source/mapping.rs`, `A/source/fixture.rs`, `A/source/test_backend.rs`, `A/source/tests.rs`, `A/source/wire_tests.rs`, `crates/pod-app/qa/refactor/wire.json`.

1. PoddSource/SourceEvent/Answer/TestBackend 경로를 유지하며 transport·입력·구독·fixture를 분리한다.
2. wire→VM 변환을 mapping, 표시 메서드를 presentation으로 이동한다. lenient fallback·future-field 수용·오류 코드를 유지한다.
3. 기준 blob의 decoder/요청을 사용해 J golden을 먼저 만들고 카드별 payload를 수집한다. expected를 새 구현으로 자동 갱신하지 않는다.
4. `source::tests`, `vm::tests`, wire_tests로 B/J, fixture 장면으로 V. 500ms reply filter·중복 제거·재연결·15/45초 fallback·event coalescing과 입력 순서를 그대로 둔다.

**P3 · Review 모델/controller/render · L · 7파일**

OWNED: `A/review.rs`, `A/review/model.rs`, `A/review/controller.rs`, `A/review/view.rs`, `A/review/copy.rs`, `A/review/tests.rs`, `A/review/qa.rs`.

1. selection/delivery/refusal/turn 정책과 표시 계획을 model로, 비동기·poll·draft 전이를 controller로 이동한다.
2. 기존 ReviewPanel·key binding·PR 연결 API를 유지하고 toolbar/list/card/drawer 렌더를 분리한다.
3. `review::tests`, `review::editor_tests`, native review keyboard/IME + populated comment/delivery fixture로 B/J/V. queued→delivered 1회, 실패 시 draft 유지, split 폭·focus·긴 문장 줄바꿈을 비교한다.

**P4 · Group 기능 분리 · M · 8파일**

OWNED: `A/groups.rs`, `A/groups/model.rs`, `A/groups/controller.rs`, `A/groups/dialog.rs`, `A/groups/view.rs`, `A/groups/copy.rs`, `A/groups/tests.rs`, `A/groups/qa.rs`.

1. AgentCounts/검증/check 요약과 MemberLive를 순수 모델로 이동한다.
2. Workspace 확장 impl은 facade로 남기고 dialog·check/pick 제어·카드를 분리한다.
3. `groups::tests` + fake backend의 check/cancel/pick refusal/force 경로로 B/J/V. argv round-trip·멤버 이름·선택/포커스·요청 횟수 동일.

**P5 · orch UI의 event controller와 페이지 분리 · L · 13파일**

OWNED: `A/orch_ui/mod.rs`, `A/orch_ui/model.rs`, `A/orch_ui/pages.rs`, `A/orch_ui/inbox.rs`, `A/orch_ui/qa.rs`, `A/orch_ui/controller.rs`, `A/orch_ui/board.rs`, `A/orch_ui/cleanup.rs`, `A/orch_ui/settings.rs`, `A/orch_ui/model/workers.rs`, `A/orch_ui/model/mail.rs`, `A/orch_ui/model/cleanup.rs`, `A/orch_ui/model/tests.rs`.

1. existing model을 worker/mail/cleanup 정책으로 나누고 pub 경로를 유지한다.
2. mod의 요청·응답·busy/coalescing을 controller, pages를 board/cleanup/settings 렌더로 나눈다. s/ko/fixture API는 읽기 전용으로 사용한다.
3. `orch_ui::model::tests`, `orch_ui::qa`, `orch_ui::ko` 테스트로 B/J/V. 이벤트 없으면 추가 RPC 0, run별 재조회 병합, mail 읽음 비변경, gate choice와 unknown 문구 fallback 동일.

**P6 · 확장 UI와 레이아웃 preset adapter · M · 7파일**

OWNED: `A/mods_ui.rs`, `A/mods_ui/model.rs`, `A/mods_ui/controller.rs`, `A/mods_ui/view.rs`, `A/mods_ui/copy.rs`, `A/mods_ui/layouts.rs`, `A/mods_ui/tests.rs`.

1. permissions/state/labels, RPC, view, preset→dock 계획을 각각 분리한다. `mods_ui::s` 등 기존 경로 유지.
2. 빈/누락/invalid preset 처리와 기존 dock·panel 재사용을 보존한다.
3. 기존 mods 단위 테스트, native `h_layout_preset...`·`h_missing_and_invalid...`로 B/J/V. 권한·mod command·설정 저장 형식을 바꾸지 않는다.

**P7 · 세션 발견 UI 분리 · M · 6파일**

OWNED: `A/sessions_ui.rs`, `A/sessions_ui/model.rs`, `A/sessions_ui/controller.rs`, `A/sessions_ui/view.rs`, `A/sessions_ui/copy.rs`, `A/sessions_ui/tests.rs`.

1. actions/heading/age를 model, scan/hide/register/resume를 controller, card/page를 view로 옮긴다. `s`/panel API 유지.
2. scan 자동 실행 여부·busy/error 전이·재개 대상 ID를 보존한다.
3. `sessions_ui::tests` + scripted request 기록으로 B/J/V. 실제 세션 폴더·에이전트 설정을 읽거나 바꾸는 테스트로 대체하지 않는다.

**P8 · pod-ui 내부 컴포넌트 정리 · M · 13파일**

OWNED: `U/lib.rs`, `U/components.rs`, `U/components/icons.rs`, `U/components/buttons.rs`, `U/components/rows.rs`, `U/components/frames.rs`, `U/components/text.rs`, `U/components/focus.rs`, `U/components/tests.rs`, `U/controls.rs`, `U/controls/select.rs`, `U/controls/switch.rs`, `U/controls/tests.rs`.

1. 기존 공개 타입·함수를 내부 모듈로 이동하고 components/controls/root의 기존 import 경로를 명시 재수출로 유지한다.
2. tokens/themes/status/agent 책임은 유지하고 새 app 의존을 넣지 않는다. 기존 수치·문구·SVG·색 계산은 이동만 한다.
3. pod-ui의 `tests`, `busy_tests`, `select_scroll` 관련 filter와 gallery/app V. focus-return, type-ahead scroll, IME, busy accessible name 및 app/editor 소비자 API 호환을 확인한다.

**P9 · terminal 입력·paint 내부 분리 · M · 11파일**

OWNED: `T/lib.rs`, `T/view.rs`, `T/view/input.rs`, `T/view/ime.rs`, `T/view/links.rs`, `T/view/tests.rs`, `T/element.rs`, `T/element/text.rs`, `T/element/cursor.rs`, `T/element/preedit.rs`, `T/element/tests.rs`.

1. 이벤트/IME 처리와 paint helper를 이동한다. TerminalView/TerminalElement/재수출 타입·InputSink/VtEngine 계약 유지.
2. keys/mouse/selection/VT 알고리즘과 feed/flush/resize 순서를 바꾸지 않는다.
3. `pod-terminal-view`의 keys/mouse/selection/links/element filter 및 app native IME/link QA로 B, terminal raster로 V. commit 전 바이트 0·commit 후 정확한 바이트, CJK/emoji/preedit 위치 동일. J는 해당 없음(daemon protocol 비의존).

**P10 · Rail + Queue 모델/kit 적용 · L · 10파일**

OWNED: `A/layouts/rail.rs`, `A/layouts/rail/model.rs`, `A/layouts/rail/view.rs`, `A/layouts/rail/copy.rs`, `A/layouts/rail/tests.rs`, `A/layouts/queue.rs`, `A/layouts/queue/model.rs`, `A/layouts/queue/view.rs`, `A/layouts/queue/copy.rs`, `A/layouts/queue/tests.rs`.

1. 두 모드의 순수 배열/selection 계획과 render를 각각 분리하고 좁은 layout hook를 구현한다. 필요한 legacy 어댑터는 유지.
2. shared shell·color·표현 frame을 적용하되 rail dock shape·최근 탭·queue dismissal/mission variant는 로컬 정책으로 둔다.
3. 두 기존 tests filter로 B/J/V. layout 전환 전후 panel/entity ID·dock 복원·queue 상태, inline gate keyboard, 긴 한국어 100–175% 동일.

**P11 · Dense 모델/kit 적용 · M · 6파일**

OWNED: `A/layouts/dense.rs`, `A/layouts/dense/model.rs`, `A/layouts/dense/log.rs`, `A/layouts/dense/view.rs`, `A/layouts/dense/copy.rs`, `A/layouts/dense/tests.rs`.

1. EventLog/Inputs/selection 모델과 렌더를 나누고 shared font/header/color 및 좁은 hook 적용.
2. 최초 관측 quiet seed·log cap·F6 컨텍스트·출력/세부 열 상태 수명 유지.
3. 기존 `layouts::dense::tests`로 B/J/V. layout 전환 시 dock/terminal entity 유지, inline gate·agent mark·글꼴 확장 치수 동일.

**P12 · Kanban 모델/kit 적용 · M · 6파일**

OWNED: `A/layouts/kanban.rs`, `A/layouts/kanban/model.rs`, `A/layouts/kanban/view.rs`, `A/layouts/kanban/copy.rs`, `A/layouts/kanban/tests.rs`, `A/layouts/kanban_qa.rs`.

1. Card/BoardModel/nav/필터를 model로 이동하고 좁은 hook·공통 shell/card 표현 적용.
2. drag/drop·pin/reorder·gate action 의미와 card 높이 공식·focus key 유지.
3. 기존 kanban 단위/QA filter로 B/J/V. option gate의 선택지 payload와 focus, 카드 순서·선택·빈 상태 동일.

**P13 · Blocks 모델/controller/kit 적용 · M · 7파일**

OWNED: `A/layouts/blocks.rs`, `A/layouts/blocks_model.rs`, `A/layouts/blocks/controller.rs`, `A/layouts/blocks/view.rs`, `A/layouts/blocks/copy.rs`, `A/layouts/blocks/tests.rs`, `A/layouts/blocks_qa.rs`.

1. 기존 blocks_model을 재사용하며 feed/attach 처리와 render를 분리한다. shell_integration은 변경하지 않는다.
2. 좁은 hook·공통 font/clock/shell 적용. attach reset/seq, 명령 블록 경계, 원시 터미널 전환·입력 대상·copy 의미 유지.
3. blocks_model/blocks/blocks_qa filter로 B/V, 입력 기록으로 B. 새 타이머·중복 attach·중복 입력이 없음을 확인한다.

**P14 · Review layout 모델/kit 적용 · M · 6파일**

OWNED: `A/layouts/review.rs`, `A/layouts/review/model.rs`, `A/layouts/review/view.rs`, `A/layouts/review/copy.rs`, `A/layouts/review/tests.rs`, `A/layouts/review_qa.rs`.

1. TaskRow/ReviewModel/nav를 model로 이동하고 공통 font/shell/row 표현·좁은 hook 적용.
2. P3의 ReviewPanel facade만 사용한다. review 패널·terminal tile 재사용과 F6/Ctrl+Shift+X·선택 task 규칙 유지.
3. 기존 layout review 단위/QA filter로 B/J/V. review-panel과 layout의 이름·소유권을 혼동하지 않고 drawer/terminal focus·tile 수·글꼴 치수를 비교한다.

### Wave 2 — 모든 P 병합 후, 공통 파일 마무리

**T1 · 공용 문구 facade 분할 · M · 의존 P1–P14 · 8파일**

OWNED: `A/strings.rs`, `A/strings/common.rs`, `A/strings/workspace.rs`, `A/strings/review.rs`, `A/strings/groups.rs`, `A/strings/templates.rs`, `A/strings/toolkit.rs`, `A/strings/tests.rs`.

1. 기존 상수 이름·문자열 bytes를 도메인별로 옮기고 root 명시 재수출을 유지한다. 기능 local copy/s/ko를 다시 중앙으로 합치지 않는다.
2. tr!와 toolkit hook를 분리한다. 문구 교정·키 이름 변경·새 locale을 넣지 않는다.
3. 상수/포맷의 대표 출력과 `host_catalog_reaches_dock_and_diff`, orch unknown fallback, 접근성 이름으로 B/V. protocol ID·CLI 문자열 무변경(J).

**T2 · 7개 모드 통합·호환 어댑터 제거 · L · 의존 P1–P14 · 14파일**

OWNED: `A/layout_mode.rs`, `A/layouts/kit.rs`, `A/layouts/rail.rs`, `A/layouts/queue.rs`, `A/layouts/dense.rs`, `A/layouts/kanban.rs`, `A/layouts/blocks.rs`, `A/layouts/review.rs`, `A/workspace.rs`, `A/workspace/layout_host.rs`, `A/workspace/shell.rs`, `A/main.rs`, `A/native_qa.rs`, `A/native_qa/layout_contract.rs`.

1. F1의 host adapter를 kit 계약에 연결하고 Workbench까지 좁은 renderer로 등록한다. 모든 모드 이관 후에만 legacy Workspace renderer hook를 제거한다.
2. startup 키 등록과 상태 슬롯 수명이 한 번씩 연결되는지 확인한다. 임의 pub 확대·unused compatibility만 남기는 종료를 피한다.
3. 7개 모드를 순환하며 terminal/review/editor entity·dock JSON·선택·focus·attach 수·요청 횟수·state 보존을 검사한다. unknown fallback/re-register도 유지.
4. B/J/V 통합 결과·테스트 목록 대응을 확인하고 fan-in/out·공개 선언 수를 같은 방식으로 재측정한다. runtime layout 경로에서 Workspace 직접 접근이 host 밖에 남으면 남은 위치를 기록하고 완료 처리하지 않는다. T1과 파일이 겹치지 않아 병렬 가능하나 **최종 검증은 둘 다 병합된 HEAD**에서 한다.

## 4. 위험과 하지 말 것

- **큰 파일을 이름만 바꾸는 분할로 끝내지 않는다.** 먼저 이동으로 동작을 고정하고 다음 카드에서 입력 모델·controller·host 경계를 좁힌다. 반대로 전 모드·Workspace 상태·transport를 동시에 다시 쓰는 big-bang도 금지한다. GPUI borrow/notify/defer/drop 순서는 기능이다.
- **공유 파일은 담당자만 수정한다.** `main.rs`는 T2, `layouts/mod.rs`는 F0, `layout_mode.rs`는 F0→T2, `workspace.rs`는 F1→T2, `strings.rs`는 T1. 동일 wave의 담당자 파일에 필요한 변경은 요청만 전달한다. `Cargo.toml`/`Cargo.lock`은 어느 카드도 소유하지 않는다. 새 crate·dependency가 필요해지면 별도 Wave 0 카드로 먼저 재계획한다.
- **기존 JSON·식별자를 UI 정리 명목으로 바꾸지 않는다.** protocol v19, panel_name/dump, layout ID, settings/layout JSON version, command ID, QA/UIA 이름, serde default/unknown fallback은 유지한다. wire snapshot은 보기 좋은 출력으로 새로 생성해 차이를 덮지 않는다.
- **시간·색·치수의 유사함을 동일함으로 취급하지 않는다.** now_ms의 fixture 동작, epoch와 monotonic 시간, 레이아웃별 높이·hit area, Mono의 border 생략은 분리해 보존한다. 1px 차이나 요청 1회 증가도 설명 없이 허용하지 않는다.
- **테스트 이동을 테스트 삭제로 숨기지 않는다.** native harness가 제품 API를 더 공개하게 만들지 않는다. fixture는 headless unit 지원과 release 캡처 지원을 구분한다. 프로세스 전역 env/clock 설정은 fixture 사이에서 누출되면 안 된다.
- **그림만으로 동작 보존을 선언하지 않는다.** 36 baseline은 전체 레이아웃·실제 diff·IME coverage가 아니다. B/J/V 중 미실행·환경 부족·기존 실패를 명시하고 0 failures를 얻기 위해 기준을 덮어쓰지 않는다. 공유 컴포넌트 변경은 pod-editor, terminal 공개 API 변경은 pod-spike 소비 경로도 검토한다.
- **병렬 UI 작업을 포함시키지 않는다.** 이 설계의 기준 SHA와 후속 구현 시 master 차이를 다시 확인하고 UI worker의 완성 변경만 별도 병합 이후 기준에 반영한다. 작업 파일 복구·일괄 stage·사용자 설정 접근·실데몬 실행으로 분석을 편하게 만들지 않는다.

## A-T2a 구현: kit·host 확장 API

A-T2a는 레이아웃을 옮기지 않고 kit와 host만 넓혔다. 레이아웃 카드 5장(A-P10~P14)의 legacy 어댑터가 Workspace에 직접 하던 일을 모두 `LayoutFrame`(읽기)과 `LayoutActions`(창 키·상태·관찰·dock·동작)로 할 수 있다. 제품 코드는 아직 쓰지 않는다(`layouts/mod.rs`의 A-F0 `dead_code` 허용은 A-T2z가 지운다). 구현: `layouts/kit/{model,host}.rs`, `workspace/layout_host.rs`. 테스트: kit 4개, `workspace::layout_host::tests` 11개(실제 Workspace).

**프레임 읽기** (`LayoutFrame`, 매 프레임 빌려줌)

| 새 항목 | 막는 빈 곳 |
|---|---|
| `labels` + `label_of(&TerminalVm)` | 터미널 라벨·저장한 이름(`Workspace::label_of`와 같은 결과): queue·dense·kanban·blocks·review |
| `all_gates` | 결정된 것까지 이번 세션의 모든 gate(`OrchState::gates`): dense 로그 |
| `toasts: FrameToasts` | toast를 데이터로(id·kind·title·body·at): dense 로그 |
| `tree_selection` | 트리에서 고른 행 키(`Sidebar::selected`): dense |
| `fixture` | `--ui-fixture` 소스 여부: blocks |
| `mono_family` | toolkit 테마 mono 글꼴(`Theme::mono_font_family`). 설정 적용 전에는 `metrics.mono_font`과 다름: kanban 카드·blocks |
| `panel_detail: PanelDetail` + `other_panels()` | 그룹·gates·agent tree 패널을 하나씩, mods·sessions·orch 페이지. `other_panels()`는 review의 dock signature 그대로, kanban은 여기에 `panels.reviews`를 더한다. `OpenPanels`는 바꾸지 않음 |

현재 작업은 패널 focus를 읽어 창이 필요하므로 프레임이 아니라 `LayoutActions::current_task(window, cx)`에 있다.

**창 키·상태 슬롯·관찰** (`kit/host.rs`)

| 새 항목 | 막는 빈 곳 |
|---|---|
| `WindowKey`, `LayoutActions::window_key()` | 창별 view 전역 맵(RailStates·QueueViews·DenseViews)의 키 |
| `LayoutActions::state::<T>()`, `state_or_insert_with(window, cx, make)` | 창마다 타입별 엔티티 하나. 처음 쓸 때 host 해제 hook을 한 번 걸고, host(Workspace)가 해제되면 그 창의 상태를 모두 버린다(명시 수명). blocks의 앱 전역 feed도 여기로 |
| `kit::state(key)`, `state_in_window(AnyWindowHandle)`, `states::<T>()`, `release_window(key)` | 창에서 출발하는 앱 단축키(Ctrl+Shift+U/D, FocusQueue)가 `AppGlobals.workspace` 없이 상태를 찾음. `release_window`는 명시 해제 |
| `HostChange::{Data, DockLayout, Tree}`, `LayoutActions::observe(changes, cx, f)`, `observe_in(changes, window, cx, f)` | `cx.observe(workspace)`·dock `LayoutChanged` 구독·sidebar 관찰 대신. 여러 변화를 Subscription 하나로 묶고, 버리면 모두 끝난다 |

`LayoutRenderer`가 view를 만들 때는 `host.actions().state_or_insert_with(...)` 안에서 `observe_in`을 걸면 된다. host의 shell/Workspace 호출 지점은 바꾸지 않았다(관찰은 GPUI 구독이 host 쪽에서 직접 연결).

**dock·패널 접근**

| 새 항목 | 막는 빈 곳 |
|---|---|
| `LayoutActions::dock(cx)` | 살아 있는 `DockArea`: layout 읽기·`select_panel`·`remove_panel`·구독(rail) |
| `LayoutActions::view(Slot::Dock / Slot::Sidebar, cx)` | host render 밖, 레이아웃 자신의 render에서 그릴 dock·사이드바 view(kanban 서랍, review 패널 view, rail 작업 공간 서랍). toast·hook banner는 host render에서만 빌려줌 |
| `terminal_panels`, `terminal_panel(id)`, `terminal_focus(id)`, `review_panels` | `live_panels`/`live_reviews` 직접 읽기와 터미널 view의 focus handle(dense F6, review·blocks focus 확인) |
| `new_terminal_panel(id, window, cx)`, `new_review_panel(task, window, cx)` | dock 밖 레이아웃 소유 패널 생성·등록(`WeakEntity<Workspace>` 필요 없음). 모르는 id, review capability 없음이면 `None`. review의 `set_split`·`agent_status`는 호출자가 계속 함 |
| `LayoutActions::source(cx)` | podd 요청(`terminal.read`·`terminal.run`·attach·자기 gate 경로) |

**동작**

| 새 항목 | 막는 빈 곳 |
|---|---|
| `UiIntent::RevealTerminal { id }` | 패널을 앞으로만, focus는 그대로(queue `select`, dense 세부 링크). 패널이 없으면 아무것도 안 함 |
| `UiIntent::OpenTaskTerminals { task }` + `LayoutActions::dispatch_then(intent, window, cx, then)` | 작업의 실행 중 터미널을 역순으로, 없으면 작업 셸 — 모두 Workspace update 한 번 안에서. `then`은 update가 끝난 뒤(rail의 재배치·focus). rail이 지금처럼 defer하려면 `window.defer` 안에서 부른다 |
| `UiIntent::OpenFocusedReview` | 06 활동 막대의 변경 검토: 키와 같은 `OpenReview` action을 focus 경로로 보냄 |
| `LayoutActions::resolve_gate(params, cx, done)` | 하나뿐인 gate 경로(`gates::resolve_gate`): 응답 병합+`touch`, 실패 toast(`COULD_NOT_RESOLVE_ID`), `done`으로 busy/error. queue·dense·kanban용. review·blocks의 자기 경로(고정 제목 toast, `notify`만)는 문구·갱신이 달라 합치지 않았다 — `source()`+`post([ApplyGate, Notify])`/`post([Toast, Notify])`로 그대로 옮긴다 |
| `HostUpdate::{Notify, Touch, Toast, ApplyGate, AddTerminal, TerminalInput}` + `LayoutActions::post(updates, cx)` | 창 없는 async 결과 전달: 순서대로 host update 한 번. rerun의 터미널 추가·오류 toast, 창 없는 toast, workspace notify, gate 응답 병합, 터미널 입력(`source.input`). 이미 있는 터미널 id는 다시 넣지 않음 |
| 06 제목줄 pill에서 열 만들기 | `title_bar_extra`에는 창이 없다: pill의 click handler(창 있음)에서 `state_or_insert_with`로 만든다 |

host가 사라진 뒤에는 읽기 `None`/빈 목록, `post`·`resolve_gate`는 `false`(아무것도 보내지 않고 `done`도 부르지 않음), 상태는 만들어 돌려주되 보관하지 않는다. `post`·`dispatch`는 host update 안에서 부르지 않는다(재진입 금지는 그대로).

**A-T2b-d에 남긴 것**: kit `Island`/`panel_header` 변형(review·kanban 값 차이), dense의 `ConnectionShell` 변형(가운데 섬 안 경고, 사이드바 아래 toast)은 표현 조합이라 각 레이아웃 카드에서 정한다. `layout_mode.rs` 테스트의 "queue still renders itself"는 queue가 renderer를 가지면 뒤집는다.

## T2 구현 기록

T2는 다섯 장으로 나눠 했다(README "A-T2 나누기"). A-T1(`27d1809`)이 먼저 병합되어, 아래 최종 검증은 T1·T2가 모두 들어간 HEAD 기준이다.

| 카드 | 병합 | 내용 |
|---|---|---|
| A-T2a | `ad53256` | kit·host API(위 절). host 수정 `d8a0463`: 렌더 중(작업 공간 lease 안)에도 dock·사이드바·source·관찰을 쓸 수 있게 함 |
| A-T2b | `9812a9e` | rail·queue를 `renderer()`로. 상태는 kit 상태 슬롯, legacy 어댑터 삭제. 레일 터미널 버튼의 이중 lease panic(4eed072부터)을 고치고 회귀 테스트를 더함 |
| A-T2d | `869974d` | blocks·review를 `renderer()`로. feed/view는 창마다 하나(창을 닫으면 해제) |
| A-T2c | `aff314e` | dense·kanban을 `renderer()`로. board·DenseView는 창 상태 슬롯 |
| A-T2z | 이 브랜치 | 작업대 renderer, legacy hook 제거, fixture 장면 이동, 7개 모드 순환 검사, 재측정 |

**A-T2z가 바꾼 것**

- `layouts/workbench.rs`(새 파일): 작업대도 kit renderer로 그린다. 사이드바·dock·toast·hook 배너는 host 슬롯, 열린 패널·연결은 frame, 빈 화면 버튼은 `UiIntent::{OpenRepository, NewTask}`. `EmptyState`도 이 파일로 옮겼다. frame에 `repos`를 더했다.
- `LayoutModule`은 `mode`·`status_counts`·`renderer`만 남았고 `renderer()`는 필수다. `render_body`·`title_bar_extra`(Workspace를 받던 legacy hook), shell의 본문 fallback, `Workspace::render_workbench_body`, 여섯 모드의 빈 stub을 지웠다. 모르는 id → 작업대, 다시 등록 → 교체는 그대로다.
- fixture 장면: `rail…`·`queue`·`dense`의 작업 공간 데이터(gate snapshot, 상태, toast, dock 배치)는 `fixture_scenes.rs`가 맡는다. 편집기 장면처럼 작업 공간 update 안에서 도는 장면의 정직한 주인이 이 파일이기 때문이다. 레이아웃 쪽 절반(`rail/fixture.rs`, `queue/fixture.rs`, 새 `dense/fixture.rs`)은 `LayoutActions`와 빌린 frame만 받는다. `main.rs`는 `fixture_scenes::{rail, queue, dense}`를 부른다.
- 시작 등록은 `main.rs`의 `install()` 한 곳이다. 레이아웃 등록부, 단축키, 패널 레지스트리를 등록하며 native QA도 같은 함수를 쓴다(등록부 global도 여기로 옮김). 화면에 나와야 의미가 있는 모드 키(kanban·blocks·review의 F6 등, queue의 Ctrl+Shift+Q)는 그대로 첫 렌더에서 앱당 한 번 묶는다. 시작 시점으로 옮기면 queue의 전역 Ctrl+Shift+Q가 queue를 보기 전에도 터미널 입력을 가로채서 동작이 바뀐다. 상태 슬롯의 해제 hook은 창마다 처음 쓸 때 한 번 건다.
- `layouts/mod.rs`의 A-F0 `dead_code`·`unused_imports` 허용을 지웠다. 그 허용이 가리던 것 가운데 테스트 밖 호출자가 없는 API를 지웠다: `UiIntent::{TaskTerminal, ArrangeGrid, FocusRegion}`, `HostUpdate::{Touch, TerminalInput}`, `LayoutActions::terminal_focus`, `LayoutFrame::{mono_family, counts}`. 이 API만 확인하던 테스트 줄도 함께 지웠고 테스트 이름은 그대로다. `kit::{state, release_window}`는 테스트용으로만 재수출한다.
- 새 검사 `native_qa/layout_contract.rs`: 실제 창 하나에서 7개 모드를 두 바퀴 돈다. 단계마다 그려진 본문, focus된 터미널·focus, 터미널 패널 수와 새로 만든 수(attach), Review·편집기 패널, dock JSON, 보낸 요청을 한 줄로 고정한다(18줄, `aff314e`에서 뽑았고 그 코드에서도 통과). 모드마다 같은 상태 엔티티를 쓰는지, 키를 한 번만 묶는지, 모르는 id fallback, 다시 등록을 확인한다. 두 번째 창은 자기 상태 6개를 갖고, 창을 닫으면 모두 해제되는지도 본다.

**레이아웃 런타임의 Workspace 접근: 0.** `layout_mode.rs`와 `layouts/**`(테스트 파일 제외)는 `Workspace` 타입을 부르지 않는다. `workspace` 모듈에서 가져오는 것은 데이터 타입 `ToastKind`·`Toast`(frame의 toast)와 action `ArrangeGrid`(레일의 Ctrl+Shift+G)뿐이다. 테스트(`*_qa.rs`, `tests.rs`)는 실제 창을 만들기 위해 Workspace를 쓴다.

**재측정** (§1과 같은 방법·스크립트. 테스트 포함, 파일을 최상위 모듈로 합침. 기준 `8b835ff`에서 §1의 수치를 그대로 재현함)

| 범위 | `8b835ff`(§1) | `aff314e`(T2z 전) | A-T2z 뒤 |
|---|---:|---:|---:|
| pod-app 파일 / 줄 | 56 / 54,430 | 163 / 69,812 | 166 / 70,025 |
| bare `pub` 항목 / 필드 / 제한 공개 | 1,516 / 439 / 257 | 1,602 / 457 / 901 | 1,604 / 457 / 901 |
| workspace fan-in / out | 21 / 23 | 21 / 25 | 20 / 25 |
| layouts fan-in / out | 2 / 22 | 5 / 22 | 7 / 23 |
| layout_mode fan-in / out | 4 / 3 | 4 / 3 | 5 / 2 |
| workspace 줄 / 제한 공개 | 4,327 / 76 | 6,349 / 122 | 6,124 / 117 |

`layout_mode → workspace`가 끊겼다(out 3→2, workspace fan-in 21→20). layouts의 fan-in·out이 는 것은 테스트 때문이다. native_qa·ui_review_qa가 `layouts::workbench`를 부르고, dense 테스트가 `fixture_scenes::dense`를 부른다. 런타임 경로는 늘지 않았다.

**검증**: B pod-app 391 통과(목록 = `aff314e` 390 + 순환 검사 1, 누락 0). J `wire_golden_matches_the_pre_split_source` 통과. V 릴리스 빌드에서 기본 `tools/visual/capture.ps1`로 8개 행렬을 `a-f0/before-12e3c76`과 비교: 56/56 통과, 바뀐 픽셀 0, PNG SHA-256 56/56이 contract.json과 같음. clippy `-p pod-app --all-targets -D warnings` 0, cargo deny `advisories ok, bans ok, licenses ok, sources ok`.

**후속으로 남김(이 카드에서 바꾸지 않음)**

- ~~블록 터미널에는 dock이 없어 그 모드에서 연 패널이 보이지 않는다. 칸반 서랍의 dock에서 F6을 눌러도 보드로 돌아가지 않는다.~~ fx-layouts에서 고침: 블록 터미널 오른쪽 패널 서랍(kit `Slot::Dock`, F6·Esc·포커스 복귀), 칸반 F6 보드↔dock, 칸반·리뷰에서 연 오케스트레이션 화면이 패널 보기로(kit `other_panels`에 orch 화면, `PanelWatch`). docs/10 §D5.5·D5.8·D5.9.
- ~~`pod_ui::apply_theme`이 설정을 적용할 때마다 PodModal 키 3개를 다시 묶는다. 그래서 레이아웃·테마를 바꿀 때마다 keymap이 커진다(pod-ui). 순환 검사는 이 키를 빼고 센다.~~ fx-app에서 고침(아래). 순환 검사의 제외 필터는 rf2-verify에서 없앴다(이제 keymap 증가도 순환 검사가 잡는다).
- 순환 중 focus가 블록 입력, 리뷰, 작업 공간 root 순으로 옮겨 간다. 작업대로 돌아와도 터미널 focus는 돌아오지 않는다. 기존 동작이라 고정만 해 두었다.

## fx-app 버그 수정 (2026-11-11)

리팩터링 중 찾은 버그 4개. 동작은 이 4개만 바뀌었다. 각 항목의 회귀 테스트는 수정 전 코드에서 실패한다(직접 되돌려 확인).

| 버그 | 수정 | 회귀 테스트 · 바뀐 고정값 |
|---|---|---|
| 리뷰: `review.comments.add`가 실패하면 줄 의견 초안이 닫혀 입력이 사라짐 | 초안은 저장이 끝날 때까지 열려 있다. 실패하면 글을 그대로 두고 초안 아래(`rv-draft-error`)와 토스트에 이유를 보인다. Enter로 다시 시도한다. 저장 중 두 번째 Enter는 보내지 않는다 | `review::qa::refused_send_keeps_the_instructions_and_checks` (A-P3의 "초안이 닫힘" 고정을 바꿈: 초안·글·이유 유지, 재시도 요청 1개 뒤 초안 닫힘) |
| 세션 페이지: `sessions.list`가 진행 중일 때 들어온 다시 읽기를 버려 목록이 낡음 | 진행 중 요청이 끝나면 한 번 더 읽는다(여러 번 요청해도 1번) | `sessions_ui::tests::a_reread_during_a_read_is_queued_once`; `opening_reads_the_list_once_and_does_not_scan`은 진행 중 열기 3번 → 읽기 2번으로 바꿈 |
| source: 제어·입력 연결에서 결과 디코드 실패를 전송 오류로 보고 연결을 버림 | `control::call`이 결과를 직접 디코드한다. 실패하면 그 요청만 `bad <method> result…`로 실패하고 stderr에 남기며 연결은 유지한다. 보내는 요청은 같다 | `source::wire_tests::a_bad_result_fails_the_request_and_keeps_the_connection`; J golden 5건을 손으로 고침(`wire.json`의 `changes`에 목록). 나머지 J는 84bdf9e 그대로 |
| `pod_ui::apply_theme`이 적용할 때마다 PodModal 키 3개를 다시 묶음 | `install_key_bindings`가 앱마다 한 번만 묶는다(전역 표시) | `crates/pod-ui/tests/theme_keys.rs` |

V: 바뀐 화면은 실패한 의견 저장(초안 아래 오류 줄) 하나뿐이고 캡처 행렬에 그 장면이 없다. 8개 행렬 56장은 바뀌지 않는다.

## A-T2a API 정정 (rf2-kit, 2026-11-12)

위 "A-T2a 구현" 표는 그 카드 당시의 API다. 지금 API와 다른 곳(rescore §3.2·§3.4·§3.7):

| A-T2a 표 | 지금 | 바뀐 카드 |
|---|---|---|
| `HostUpdate::{Touch, TerminalInput}` | 없음 | A-T2z(테스트 밖 호출자 없음) |
| `LayoutActions::terminal_focus(id)` | 없음 | A-T2z |
| `LayoutFrame::mono_family`(`counts`도) | 없음 | A-T2z |
| `UiIntent::{TaskTerminal, ArrangeGrid, FocusRegion}`(표 밖) | 없음 | A-T2z |
| `ActionSink` 한 trait, A-T2a 메서드는 `None`/`false` 기본값 | `HostHandles`(렌더 안전) + `HostCommands`(update 단계), 기본값 없음. `ActionSink`는 둘을 묶은 이름이고 kit 밖으로 내보내지 않음. 시험용 sink(`kit/tests.rs`)는 무동작을 직접 구현 | rf2-kit |
| `dispatch_then`: "`then`은 update가 끝난 뒤" | dispatch가 **돌아온 직후**이지 notify·dock 이벤트 뒤가 아니다(아래 순서) | rf2-kit |
| `focus_handle`은 토스트 슬롯에서 작업 공간을 읽음 | host가 만들 때 잡은 handle. 렌더 중에도 됨 | rf2-kit |

**단계.** renderer의 `render_body`·`title_bar_extra`는 host(Workspace)가 빌려진 동안 돈다.

- 렌더 안전(`HostHandles`, 어디서나): `window_key`, `focus_handle`, `state`·`state_or_insert_with`, `observe`·`observe_in`, `dock`, `view`, `focused_dock_panel`, `source`, `defer`(실행은 나중). 렌더 중 읽기는 인자로 받은 frame이다.
- update 단계(`HostCommands`, host render·update 밖: 이벤트 핸들러, 관찰 콜백, defer, async 완료, 레이아웃 자기 view의 render): `read`(새 frame), `dispatch`, `dispatch_then`, `current_task`, `terminal_panels`·`terminal_panel`·`review_panels`, `new_terminal_panel`·`new_review_panel`, `post`, `resolve_gate`.

**순서** (실제 host에서 고정, `workspace::layout_host::tests`):

- dock 변경 → notify → focus: `then`(새 패널 있음, 창 focus 이동함) → `DockLayout` 관찰 → `then` 안의 `window.defer` → `Data`(host가 dock 변화에 `touch`로 응답한 notify) → 그린 뒤 frame의 `focused_terminal`과 `Data` 한 번 더. 그래서 defer는 dock 이벤트 뒤, host notify 앞이다. `rail.rs`의 "Behind … the host's notify" 주석은 이 점에서 틀렸지만(rf2-vis에서 고침), 레일은 `DockLayout`에도 동기화하므로 동작은 맞다.
- gate: `resolve_gate`는 응답 병합(실패면 toast)과 `touch` → 같은 update에서 `done` → `Data` 관찰. `post`는 목록 전체를 update 한 번에 적용하고 관찰은 그 뒤 한 번이다(`Notify` 위치와 무관).
- 창이 닫힌 뒤: `post`·`resolve_gate`는 `false`, 아무것도 보내지 않는다. 닫히기 전에 보낸 요청의 늦은 응답은 아무 데도 닿지 않고 panic도 없다. 단 `resolve_gate`의 `done`은 그래도 불린다(`gates::resolve_gate` 그대로).

시험: `every_render_safe_call_works_while_the_workspace_is_leased`, `dock_change_then_notify_then_focus_after_dispatch_then`, `a_window_closed_before_a_late_answer_takes_nothing`, `gate_answers_land_before_done_and_the_observers`.

## rf2-vis (2026-11-12)

rescore §5의 2번 중 레이아웃 공개 범위와 좁은 패널 서랍 버그. 동작은 블록 터미널 패널 서랍(폭·가로 스크롤)과 블록 머리줄 말줄임만 바뀌었다.

**공개 범위**(rescore §3.1). 다른 모듈이 실제로 쓰는 것(grep·컴파일)만 넓게 두었다.

- rail·queue·dense: 상태 필드·메서드는 private(자식 view/model/fixture/tests는 조상의 private을 본다), model helper는 `pub(super)`. 남긴 `pub(crate)`: dense `fixture_view`·`fixture_select`(fixture_scenes)·`row_height`(kit 시험), workbench `EmptyState`(ui_review_qa, workspace 시험), `layouts::kit`.
- kanban·review·blocks: `Board`·`ReviewView`·`Feed`의 bare `pub` 멤버를 자기만 쓰면 private, 형제 QA 파일(`*_qa.rs`)·kit 시험이 쓰면 `pub(super)`, `Feed`에서 `blocks_qa`가 쓰는 14개는 `pub(in crate::layouts)`. `KEY_CONTEXT`는 private.
- 수치(`crates/pod-app/src`, 28520e2 → 이 브랜치): `pub(crate)` 출현 **468→354**(rescore의 461은 rf2-kit 전 값), bare `pub` 항목 1,613→1,564, 필드 459→447, 제한 공개 전체 918→921(`pub(crate)`이 대부분 `pub(super)`로 바뀌어 제한 공개 수 자체는 비슷하다). kit(119개)은 이 카드 밖이라 그대로.

**좁은 패널 서랍 잘림.** 원인: dock은 오른쪽에 붙은 패널(승인함·에이전트 트리 380 px, 받은 편지함 420 px, 상태 검사 304 px)을 자기 폭으로 그리고 중앙에는 나머지만 준다. 960 px 창의 블록 터미널 서랍은 351 px라 패널 오른쪽(메모 입력, 새로고침, 거절·답장 단추)이 잘렸다. 칸반 서랍은 보드 폭 전체라 잘리지 않았다(같은 시험을 더함).

- 서랍 폭 = 기존 폭(창의 42 %, 360–720, 블록 열 360 유지)과 "열린 왼쪽+오른쪽 dock + 테두리"(블록 열 280 px 남김) 중 큰 값(`blocks_model::drawer_width`). 1440 px 창에서는 원래 605 px라 그대로.
- 800 px 창처럼 그것도 안 되면 서랍은 280 px로 남고 dock은 자기 폭 그대로 서랍 안에서 가로로 스크롤된다(가로 휠만, `restrict_scroll_to_axis`).
- 블록 열이 280–330 px로 좁아지면 블록 머리줄의 에이전트 이름·상태가 `flex_shrink_0`이라 복사 단추가 블록 밖으로 밀렸다(첫 after 캡처에서 발견). 이제 명령 다음으로 이름·상태가 말줄임된다. 넓은 열에서는 그대로.
- 시험(앞의 둘은 고치기 전 실패): `blocks_qa::right_docked_panels_fit_in_the_drawer_at_960_and_1440`(전: 서랍 351 px, 승인함 380 px가 x 989까지; 머리줄 검사는 전 머리줄에서 복사 단추 x 639–655 > 블록 565), `blocks_qa::in_the_narrowest_window_the_dock_scrolls_sideways_in_the_drawer`, `kanban_qa::a_right_docked_panel_fits_in_the_drawer_at_960_and_1440`, `blocks::tests::the_drawer_holds_the_side_docks_and_keeps_280_px_of_feed`. `gates.rs`와 복사 단추에는 시험 빌드 전용 `qa_id`만 더했다.
- 캡처: `crates/pod-app/qa/rf2-vis/`(before/after, 960 px 창 = 1440×960 @150 %). 1440 px 창(2160 px)은 보조 모니터(1920 px)에 들어가지 않아 headless 시험으로만 본다.

**레일 주석**(rf2-kit 보고 1). `open_task_terminals`의 지연 rearrange는 dock `LayoutChanged` 뒤, host notify 앞에 돈다. 주석만 고쳤다.

**검증**: B 전체 `cargo test` 95 suites 1,245 통과·0 실패·4 무시, pod-app 410(기존 406 목록 그대로 + 새 4). J `wire_golden_matches_the_pre_split_source`, `native_qa/layout_contract` 7개 모드 순환, podd `protocol_compat` 11/11·`protocol_wire` 7(+1 무시) 통과. V: `dcc5aef` 릴리스 빌드, 기본 `tools/visual/capture.ps1`로 8개 행렬을 `a-f0/before-12e3c76`과 비교해 56/56 통과, 바뀐 픽셀 0, PNG SHA-256 56/56이 contract.json과 같음(서랍이 열린 장면은 행렬에 없어 baseline을 다시 찍지 않음). clippy `-p pod-app --all-targets -D warnings` 0, cargo deny `advisories ok, bans ok, licenses ok, sources ok`.

# Pod 확장 가이드

기준: 2026-11-12, master `ef9e785`(rf2-kit/bounds/verify/vis 반영). 기준: 프로토콜 v21(rm-o2 워커 끝점, [19 §20](19-worktree-sessions-orchestration.md)). 아래는 **앞으로 기능을 추가할 때의 체크리스트**다. 이 가이드 작성에서는 제품 코드·golden·baseline을 바꾸지 않았다.

설계와 이행 이력은 [리팩터 순서](20-refactor/README.md), [재평가 §5](20-refactor/rescore.md), [daemon](20-refactor/daemon.md), [libs](20-refactor/libs.md), [app](20-refactor/app.md)에 있다. 특히 app의 **A-T2a 구현 표는 당시 API**다. 뒤의 **T2 구현 기록 → A-T2a API 정정 → rf2-vis**와 현재 코드를 우선한다. 검증 근거는 [daemon-validation](20-refactor/daemon-validation.md), [libs-validation](20-refactor/libs-validation.md)을 참고한다.

## 1. 지금 구조 한 장

경로는 저장소 루트 기준. 아래 표의 `src/`는 해당 crate 안이다.

| 위치 | 넣을 것 / 경계 |
|---|---|
| `crates/pod-protocol` | `src/protocol.rs`의 envelope·`Method`·schema·버전 협상, `src/*_protocol.rs`의 params/result/event, `src/lib.rs`의 공용 `method!`. 파일·락·서버 실행을 넣지 않는다. orch/forge/mods/discover 타입 재수출이 남아 있어 serde-only SDK는 아니다. |
| `crates/pod-client` | `src/client.rs`의 blocking `Client`, attach/status stream, `transport.rs`의 framing, `endpoint.rs`, `detach.rs`, `env.rs`. app/cli/remote의 manifest에서 `podd = { package = "pod-client", ... }`이므로 소스의 `podd::` 철자는 서버 패키지 의존을 뜻하지 않는다. |
| `crates/podd` | `src/server/mod.rs`: `ServerConfig`·`Daemon`·모듈 조립. `server/runtime.rs`: 시작/종료, `connection.rs`: 연결·협상·stream, `rpc/{registry,domains}.rs`: 정적 등록, `server/<domain>.rs`: 기능. `holder/*`·`holder_proto.rs`는 별도 PTY 수명/바이너리 계약, `git/*`는 실행 정책. `src/lib.rs`는 protocol/client의 옛 경로를 동일 타입으로 재수출한다. |
| `crates/pod-app` | `src/source/*`: 전송/이벤트/변환, feature의 model/controller/view: 표시 정책·요청·그리기. `workspace/*`: dock·수명·조립. `layout_mode.rs` + `layouts/<mode>*`: 레이아웃. `layouts/kit/*`: frame·slot·action 계약, `workspace/layout_host.rs`: 실제 host 구현. 레이아웃 런타임에서 Workspace를 직접 빌리지 않는다. |
| UI 라이브러리 | `pod-ui`: 도메인 없는 컨트롤·토큰·테마. `pod-editor`: 파일/탭/LSP. `pod-diff`: diff 입력/patch. `pod-terminal-view`: `InputSink`·VT 표시/입력, PTY 비소유. |
| 기능 라이브러리·진입점 | `pod-orch`: DAG·journal, `pod-forge`: `Forge`/`Runner`, `pod-mods`: `Host`·권한/실행, `pod-discover`: `Source`·스캔, `pod-status`: 시간값을 받는 동기 상태 엔진. `pod-cli`: 명령·연결·출력, `pod-hook`: 별도 훅 relay, `pod-remote`: SSH 수명/설정. |
| leaf 3종 | `pod-process`: `bounded` 실행·`git::clear_overrides`; `pod-storage`: JSON 직렬화·write→rename 기계 동작; `pod-test-support`: 시험용 git·`wait::{until, stays}`·`tools::missing`. 도메인 정책은 호출자에게 둔다. |

`tools/check-boundaries.ps1`의 `$forbidden`, `$tokioFrom`/`$tokioServer`, `$leaves`, `$spikes`가 강제하는 규칙:

- `pod-app`/`pod-cli`/`pod-remote` → **패키지 `podd` 금지**, `pod-editor` → `pod-forge` 금지.
- `pod-protocol`/`pod-client` → `podd`, GPUI 계열, alacritty 계열 금지. tokio의 `full`, `rt`, `rt-multi-thread`, `net`, `process`, `signal`, `fs`, `io-std`, `test-util`도 금지한다(`sync`/`io-util` 같은 순수 helper까지 금지하는 것은 아님).
- leaf는 다른 workspace crate·GPUI를 의존하지 않는다. 예외는 process/storage의 **dev** `pod-test-support`뿐. `pod-test-support`의 일반/build 의존은 어디서든 금지한다.
- `pod-spike`·`diff-spike`는 member로 남지만 default-members 밖이고 다른 crate가 의존할 수 없다.
- 검사는 **normal/dev/build/target별 의존 표 전부**, package alias·workspace 상속·tokio feature 참조를 읽는다. manifest 검사이므로 함수 호출이나 런타임 수명까지 증명하지는 않는다.

## 2. 따라 하기 체크리스트

### A. 새 RPC 메서드 하나

- [ ] **타입 선언**: `crates/pod-protocol/src/<domain>_protocol.rs`(기본 terminal/repo/task는 `protocol.rs`)에 `Serialize`/`Deserialize`/`JsonSchema` params/result를 둔다. `lib.rs`에 정의된 `method!(ExampleGet, "example.get", ExampleParams, ExampleResult)`를 사용한다. `Method::{NAME, Params, Result}`를 연결하는 매크로이며 서버/schema를 자동 등록하지 않는다. serde default·생략·null·오류 코드와 구버전 의미를 먼저 정한다.
- [ ] **공개 목록/schema**: `crates/pod-protocol/src/protocol.rs::schema()`의 `entry::<ExampleGet>()`를 추가한다. 해당 도메인에 이름 목록이 있으면 함께 갱신한다: `METHODS`, `STATUS_METHODS`, `ORCH_METHODS`, `PR_METHODS`, `ORCH_RUN_METHODS`, `MOBILE_METHODS`, `SESSION_METHODS`, `SESSIONS_METHODS`. mods처럼 별도 목록이 없는 도메인도 schema/registry/golden에는 반드시 들어간다.
- [ ] **서버 등록/구현**: `crates/podd/src/server/rpc/domains.rs`의 해당 `const ...: &[MethodRegistration]`에 `unary::<ExampleGet>(...)`를 추가한다. 현재 표는 `SYSTEM`, `TERMINAL`, `STATUS`, `REPO_TASK`, `PR`, `REVIEW_GROUP_GATE`, `ORCH`, `DISCOVERY`, `MODS`, `MOBILE`이다. `params(p)?` → 도메인 처리 → `to_value(...)` adapter 또는 기존 `dispatch_<domain>`을 쓴다. 후자라면 도메인의 `handles`/`dispatch`도 갱신한다. `MOBILE_TEST_HOOKS`는 공개 API가 아니다.
- [ ] **등록 계약**: 같은 파일의 `REGISTERED`와 `duplicate(REGISTERED)`가 중복을 컴파일 때 잡고, `the_public_methods_are_the_schema_methods`가 공개 집합을 대조한다. `CLAIMS`는 *미등록 이름*의 기존 도메인 오류를 보존하는 fallback이다. 새 메서드를 여기만 넣지 않는다. 현재 stream은 `Mode::Attach`/`Mode::Subscribe` 두 종류뿐이며 새 stream은 `rpc/registry.rs`·`server/connection.rs`·client reader까지 별도 설계해야 한다.
- [ ] **호출자**: 보통 `crates/pod-client/src/client.rs`를 수정할 필요 없이 `Client::call::<ExampleGet>(&params)`를 쓴다. 앱에서는 feature controller → `PoddSource::request`(`crates/pod-app/src/source.rs`, `source/rpc.rs`)를 사용하고 GPUI 렌더에서 blocking call을 하지 않는다. 새 이벤트는 client의 `SubscribeEvent`/`next_any` 및 앱 `source/events.rs`, `source/mapping.rs`, `workspace/binding.rs`의 소비 경로를 확인한다.
- [ ] **CLI가 필요하면**: `crates/pod-cli/src/args.rs`의 clap enum, `src/main.rs::real_main`의 분기, `src/commands/<domain>.rs`와 `commands/mod.rs`(또는 기존 전문 명령 모듈)를 연결한다. `context.rs::CliContext`의 연결 정책, `output.rs`/명령별 text·JSON·exit code를 유지한다. help/guide/schema는 무연결, local server status/stop은 자동 시작 없음이다. `tests/command_contract.rs`·`tests/fixtures/commands.json`, `tests/refactor_acceptance.rs`의 도움말/schema 계약도 검토한다.
- [ ] **wire 시험**: `crates/pod-protocol/src/lib.rs::wire_golden`과 `crates/podd/tests/protocol_wire.rs` **양쪽**의 `method_types!`에 marker를 넣는다. 알림이면 `notification_types()`, 코드면 `error_codes()`, 목록이면 `constants()`도 확인한다. 현재 fixture는 `crates/podd/tests/fixtures/{schema-v21,wire-v21}.json`이다. `protocol_compat.rs`(동일 타입/재수출), `rpc_routes.rs::PROBE`, `refactor_acceptance.rs`의 집합·지문·고정 개수(현재 104 메서드/18 알림), 관련 `libs_wire_snapshots.rs`까지 변경 영향에 포함한다. 성공·잘못된 params·도메인 오류·모르는 이름을 격리 시험에 추가한다. 앱 호출은 `source/wire_tests.rs`와 `qa/refactor/wire.json`에 송수신 계약을 추가한다.

**새 기능과 리팩터를 구분한다.** 현재 golden은 v19 리팩터 전 동작의 증거다. 새 RPC는 wire 변경이므로 버전·호환성 정책과 기대 diff를 먼저 확정한다. `protocol_wire.rs`는 golden 작성기를 **새 프로토콜 버전에서만** 쓰도록 명시한다. v19 파일을 현재 결과로 덮어 통과시키지 말고 새 버전 fixture·참조·개수 단언을 명시적으로 검토한다(§3).

### B. 새 데몬 도메인

- [ ] `crates/podd/src/server/<domain>.rs`와 필요한 private 하위 모듈을 만들고 `server/mod.rs`에 선언한다. 공유 상태가 필요하면 `Daemon` 필드와 `server/runtime.rs::serve`의 load/init·종료를 연결한다. 이미 있는 `pod-orch` 엔진/저장소를 복제하지 않는다. wire·영속 레코드·런타임 상태는 분리한다.
- [ ] `crates/pod-protocol/src/lib.rs`의 모듈 선언, `protocol.rs`의 재수출/schema, 필요하면 `crates/podd/src/lib.rs`의 호환 재수출을 연결한다. 새 이름 목록을 만들면 `protocol.rs::tests::schema_lists_all_methods`의 순회와 양쪽 wire golden의 `constants()`에도 연결한다. A의 타입·wire 체크리스트를 따른다.
- [ ] `server/rpc/domains.rs`에 새 도메인 const 표와 adapter, `REGISTERED` 항목을 넣는다. prefix의 미등록 이름을 처리해야 할 때만 `CLAIMS`를 더한다. 같은 파일의 `claimed_by`, `each_method_keeps_its_former_dispatch`의 `tables`, `an_unregistered_name_reaches_at_most_one_claim` 사례도 늘려 prefix 중첩을 막는다.
- [ ] **상태/이벤트**: 새 lifecycle 알림은 protocol의 `NOTIFY_*`·event type·schema부터 정의하고, `server/status/hub.rs`의 `HubEvent`/publish 함수 → `server/status.rs` 재수출 → `status/stream.rs::stream_subscribe`까지 연결한다. 구독 필터·sequence·lagged·downgrade를 보존한다. 락 순서는 terminal stream state → `TermStatus` → hub publish lock이다. 감지 규칙은 `pod-status/src/engine/{input,evidence}.rs`, 훅 수신은 `server/status/ingest.rs`에 둔다.
- [ ] **저장**: `pod_storage::json::to_pretty_bytes` → `pod_storage::replace::write_then_rename(path, tmp, bytes)`를 쓴다(`crates/pod-storage/src/lib.rs`). 부모 생성·tmp 이름·실패 cleanup·오류 변환·손상 복구는 도메인 adapter 책임이다. 선례는 `podd/src/server/discovery.rs`, `pod-orch/src/store.rs`, `pod-mods/src/manager/state.rs`. JSONL journal을 이 primitive로 대체하지 않는다. orch completion의 mail append → 메모리 전이 → run 저장 순서를 유지한다.
- [ ] **단발 프로세스**: `pod_process::bounded::{run, Options, Keep, End, Outcome}`로 timeout·출력 상한·취소를 지정하고 종료 원인을 처리한다(`crates/pod-process/src/lib.rs`). `pod_forge::bounded` 재수출은 제거됐다. holder/장수 LSP의 소유권을 단발 실행기로 옮기지 않는다. git 환경 공유는 `pod-process/src/git.rs::clear_overrides`뿐이고 worker 정책은 podd에 둔다.
- [ ] **시험**: `crates/podd/tests/<domain>.rs`에서 `tests/common/mod.rs::Daemon`과 `common/fixtures.rs`를 재사용한다. 시작/재시작/저장 실패/종료 중 요청, 이벤트 순서, 격리 경로를 검증한다. daemon 안에서 fixture 빌드용 cargo를 실행하지 않는다. 필요 바이너리는 runner가 먼저 준비한다.

### C. 새 레이아웃 모드

- [ ] `crates/pod-app/src/layout_mode.rs`: `LayoutMode` variant, `ALL` 길이/항목, `id()`, `label()`을 추가한다. `from_id()`는 ALL을 읽는다. id는 settings.json/프리셋에 저장되므로 기존 id를 바꾸지 않는다. `LayoutRegistry::new`에 등록한다. unknown/unbuilt → Workbench, 같은 mode 재등록 → 교체 계약을 유지한다.
- [ ] `src/layouts/<mode>.rs`와 `layouts/mod.rs` 선언을 추가한다. `LayoutModule::{mode, renderer}`를 구현하고 필요하면 `status_counts`를 재정의한다. `renderer()`는 필수이며 반환하는 `LayoutRenderer`의 `render_body`/선택적 `title_bar_extra`가 frame/kit로 그린다. 옛 Workspace 인자 render hook이나 본문 fallback을 복구하지 않는다. 스타일 프리셋도 추가할 때만 `crates/pod-ui/src/themes.rs::PRESETS`를 바꾼다.
- [ ] **kit 사용 단계**: 아래 표를 지킨다. 정의는 `layouts/kit/model.rs`, facade 메서드는 `layouts/kit/host.rs`, 실제 구현은 `workspace/layout_host.rs`다. 부족한 기능은 이 계약·구현·시험을 함께 확장하며 Workspace 필드를 공개해 우회하지 않는다.

옛 표의 `HostUpdate::{Touch, TerminalInput}`, `LayoutActions::terminal_focus`, `LayoutFrame::{mono_family, counts}`, `UiIntent::{TaskTerminal, ArrangeGrid, FocusRegion}`은 제거됐다. 새 모드에 복사하지 않는다.

| 호출 시점 | 허용 API |
|---|---|
| host의 `render_body`/`title_bar_extra` 안(Workspace lease 중) | 인자로 받은 `LayoutFrame` 읽기, `LayoutHost::slot`/`actions`. `HostHandles`에 기반한 `LayoutActions::{window_key, focus_handle, state, state_or_insert_with, observe, observe_in, dock, view, focused_dock_panel, source}`. `defer` 예약은 가능하지만 실행은 나중이다. |
| host render/update 밖: 이벤트·관찰·defer·async 완료, 레이아웃 자체 Entity의 render | `HostCommands`에 기반한 `read`(새 frame), `dispatch`/`dispatch_then`, `current_task`, `terminal_panels`/`terminal_panel`/`review_panels`, `new_terminal_panel`/`new_review_panel`, `post`, `resolve_gate`. |

- [ ] **상태/수명**: `host.actions().state_or_insert_with::<ModeState>(...)`로 **창·타입마다 하나**를 둔다. 상태에 Subscription을 보관하고 `HostChange::{Data, DockLayout, Tree}`를 필요한 만큼 관찰한다. 창의 host가 해제되면 슬롯도 해제된다. 이벤트에는 stable id와 clone한 `LayoutActions`를 캡처하고 빌린 frame을 저장하지 않는다. 별도 panel은 `new_terminal_panel`/`new_review_panel`로 등록한다. `Slot::{Dock, Sidebar, Toasts, HookBanner}` 중 toast/hook은 host render에서 빌리고, 자체 view에서는 `actions.view`로 dock/sidebar만 얻는다.
- [ ] **키/시작 등록**: `src/main.rs::install`은 앱과 native QA가 함께 쓰는 등록 지점이다. 필요한 시작 키는 여기 연결한다. 기존 queue/kanban/blocks/review의 첫 render 시 앱당 한 번 묶는 키는 그 시점을 유지한다(미리 묶으면 터미널 키를 가로챌 수 있음). 매 render 또는 설정 적용마다 다시 bind하지 않는다.
- [ ] **fixture**: `src/fixture_scenes.rs`에 Workspace seed/조립, `layouts/<mode>/fixture.rs`에 frame/actions를 받는 레이아웃 seed를 둔다. `src/main.rs`의 `--fixture-scene` 분기에 연결한다. `src/source/fixture.rs`의 실행용 데이터와 `layouts/kit/clock.rs::Clock`의 고정 시각을 재사용한다. 실제 `--ui-fixture` 경로는 `cfg(test)`로 숨기지 않는다.
- [ ] **native QA/순환 계약**: `src/native_qa/{harness,fixtures}.rs`의 `Harness`/`seed_two_tasks`, `src/source/test_backend.rs`를 사용한다. `layouts/<mode>/tests.rs` 또는 `layouts/<mode>_qa.rs`와 모듈 선언을 추가한다. `native_qa/layout_contract.rs`의 `MARKERS`, `CYCLE`, `EXPECTED`, `state_of` 및 상태 개수 단언을 새 모드까지 확장한다. 현재 시험 `seven_modes_cycle_keeps_panels_dock_focus_requests_and_state`는 7모드 두 바퀴·두 번째 창·해제·재등록·fallback을 검사한다. 기존 7모드를 빼지 말고 attach/요청 수·dock JSON·focus·Entity 재사용을 보존한다. **PodModal을 포함한 모든 key binding**을 센다.
- [ ] **시각/UIA**: `tools/visual/capture.ps1 -Layout <id> -Scenes ...`로 별도 행렬에 캡처한다. `tools/visual/uia-required.ps1`의 `$script:PodUiaLayoutRequired`에 id와 필수 노드를 추가한다. 장면 공통 표는 `$script:PodUiaRequired`; 미등록 모드는 실패하며 `uia-selftest.ps1`이 LayoutMode와 대조한다. 새 장면의 baseline·PNG·tree·manifest를 함께 검토한다. 작은 창/큰 글자/긴 한국어·F6/Esc·서랍을 별도 확인한다. 기존 default baseline에는 열린 서랍 문제가 드러나지 않는다(`layouts/blocks_qa.rs`, `kanban_qa.rs` 참고).

### D. 새 설정 화면/패널

- [ ] **설정 항목/분류**: `crates/pod-app/src/chrome/settings.rs`의 `CATEGORIES`, `settings_view` 분기와 section을 추가한다. 분류를 늘리면 `chrome/inspector.rs::ChromePanel`의 `category_focus: [FocusHandle; 3]`와 생성부도 늘린다. 영속 값은 `src/theme.rs::Settings`/`Default`/load/save/apply 및 호환 시험에 연결하고 `commit_settings`의 저장 성공 후 적용·실패 toast 흐름을 재사용한다.
- [ ] **별도 dock 패널**: feature에 `Panel`·`Focusable`·`EventEmitter<PanelEvent>` 구현과 안정적인 panel_name/dump·복원 builder를 둔다(`chrome/inspector.rs` 선례). `src/main.rs`의 모듈 선언과 `install`의 `register_panel`로 등록한다. Workspace 소유·복원이 필요하면 `src/workspace.rs`의 슬롯, `workspace/dock.rs`의 `RestoreCtx`/`RestoredPanel`/`adopt_restored`를 연결한다. builder는 dock load 중 Workspace를 다시 빌리지 않는다. 레이아웃 서랍에도 보여야 하면 kit의 `OpenPanels`/`PanelDetail`, `LayoutFrame::other_panels`와 `workspace/layout_host.rs::{open_panels, panel_detail}` 집계에 포함한다.
- [ ] **진입 명령**: 정적 팔레트 항목은 `src/chrome/commands.rs::COMMANDS`의 `CommandSpec`(id/label/detail/shortcut/run)에 둔다. shortcut의 context·action/listener와 도움말(`chrome/copy.rs`)도 확인한다. 레이아웃에서 열어야 할 때만 kit의 `UiIntent`와 host dispatch를 확장한다.
- [ ] **표현/문구/UIA**: `pod_ui::{IconButton, semantic_button, TreeRow, Frame}` 및 category/select/switch 컨트롤을 조합한다. 공용 컴포넌트 자체를 추가할 때는 `crates/pod-ui/src/components/{buttons,rows,frames,text,focus,icons}.rs` 또는 `controls/{select,switch}.rs`와 facade 재수출을 고친다. 공용 문구는 `pod-app/src/strings/<domain>.rs` + `strings.rs`의 **명시 재수출**, 형식 문구는 `strings/templates.rs`의 `tr!`, 기능 전용 문구는 local `copy.rs`에 둔다. wire id·사용자 입력·명령줄은 번역하지 않는다. role·`aria_label`·키보드 focus를 주고, 테스트 전용 `qa_id`를 UIA 이름으로 착각하지 않는다.
- [ ] **시험**: `chrome/tests.rs`의 category/설정 기본값/정적 명령·키·도움말 계약, `theme.rs`의 이전 설정/default/손상 복구 시험을 갱신한다. `native_qa/{focus,keyboard,ime,dpi}.rs`, `ui_review_qa.rs::command_keys_and_palette_ids_open_the_same_thing`에 해당 동작을 추가한다. 컨트롤 변경은 pod-ui의 단위 시험과 `tests/{select_scroll,theme_keys}.rs`도 확인한다. fixture scene과 UIA 필수 표를 갱신하고 저장→재시작 복원·Tab/Shift+Tab·Esc·IME 조합 중 Enter를 검증한다.

## 3. 자주 빠지는 함정

| 함정 | 지킬 계약 / 증거 |
|---|---|
| 렌더 중 다시 host 읽기 | `actions.read()`도 HostCommands다. renderer 안에서는 받은 frame을 읽는다. `HostHandles`/`HostCommands`에 기본 무동작 구현은 없고 `ActionSink`는 kit 내부 결합 이름이다. `workspace/layout_host.rs::tests::every_render_safe_call_works_while_the_workspace_is_leased`로 안전 범위를 고정한다. |
| `dispatch_then`을 이벤트 완료로 해석 | dispatch가 반환하면 바로 `then`이다. 새 패널과 창 focus는 보이지만 observer는 아직 안 돌았다. dock 변경 경로의 순서는 **then → DockLayout → then에서 예약한 window.defer → host Data → 다음 draw 후 focused_terminal/Data**. host 반응 이후 데이터가 필요하면 `Data`를 관찰한다. `dock_change_then_notify_then_focus_after_dispatch_then`을 약화하지 않는다. |
| notify/defer·늦은 응답 | `post`는 목록을 한 update에 적용한 뒤 관찰 한 번. gate는 응답 병합/실패 toast·touch → 같은 update의 done → Data. 닫힌 host에 새 `post`/`resolve_gate`는 false이며 요청하지 않는다. 닫기 전에 보낸 gate의 늦은 응답은 host를 건드리지 않지만 done은 호출된다. `a_window_closed_before_a_late_answer_takes_nothing`, `gate_answers_land_before_done_and_the_observers`가 증거다. |
| Windows 8.3 경로 | `RUNNER~1`/긴 이름/대소문자/구분자를 문자열 비교하지 않는다. `crates/podd/src/git.rs::{normalize, same_path, is_inside}`를 사용한다(`path_key`만으로 8.3 alias가 해결되지는 않음). 없는 자식 경로도 부모 정규화가 필요하다. `tests/short_paths.rs`와 shortpath runner로 확인한다. |
| CRLF checkout | raw-string golden도 CRLF가 될 수 있다. acceptance의 `check`는 `lines()`로 비교하고 CLI 출력 정규화는 범위를 명시한다. wire 값·PTY bytes·diff 줄 끝까지 일괄 trim/replace하지 않는다. `.gitattributes`는 editor demo fixture만 LF 고정이다. `git diff --check`와 `pod-diff`의 EOL 시험을 활용하고 파일 전체 개행 변경을 피한다. |
| worker sandbox·git guard | 새 기능을 위해 전역 agent 설정·sandbox 권한을 넓히지 않는다. Pod의 `server/codex_allowance.rs` 제한 목록, `orch_runs/launch.rs`의 guard 주입, `pod-client/src/env.rs::worker_git_guard`, `podd/src/git/{command,worker_commit}.rs`의 bounded·filter/hook/fsmonitor/signing 차단과 경로 검사를 보존한다. `orch.commit`을 일반 git 실행으로 우회하지 않는다. `tests/worker_commit_filters.rs`를 유지한다. 워커가 새 메서드를 불러야 하면 `server/rpc/worker.rs`의 `allowed`에 대상 검사와 함께 넣는다(기본은 거부, full pipe는 OS 사용자, [19 §20](19-worktree-sessions-orchestration.md)). |
| golden/baseline으로 실패 덮기 | 리팩터 golden은 기준 코드에서 얻은 증거다. 배열 순서·null/누락·enum·오류 문구를 정규화로 숨기지 않는다. wire 새 버전은 임시 출력의 기대 diff를 검토한 뒤 별도 계약으로 반영한다. UI의 의도된 변경은 **변경 이미지 전부 확인 후** baseline·tree·manifest를 함께 갱신한다. 재녹화·허용 오차 확대·영역 마스킹으로 실패를 없애지 않는다. |

`refactor_acceptance.rs` 두 파일(podd/pod-cli)의 자체 `until`과 podd의 날짜 고정 `git` helper는 의도된 예외다. 같은 시험이 공용 helper가 없는 baseline `64e9530`에서도 컴파일되어야 한다. 일반 신규 시험은 `pod-test-support`/`tests/common`을 쓰고 이 예외를 복제하지 않는다.

## 4. 검증 순서

**1 → 2 → 해당 변경 행 → 4 순서로 실행한다.** 이 문서의 명령은 후속 구현용이며 이번 문서 작업의 실행 결과가 아니다. 로컬 평가에서는 대상 crate/filter만, Cargo 한 프로세스만 실행한다. 실제 사용자 Pod/agent 설정·실제 podd·실제 모델 프롬프트·네트워크/GitHub 쓰기는 사용하지 않는다.

1. **정적 확인**: `git diff --check`, `tools/check-docs.ps1`, `tools/check-workspace.ps1`, `tools/check-boundaries.ps1`. manifest 규칙을 바꾸면 boundaries의 `-SelfTest`도 실행한다. 새 최상위 번호 문서는 README 문서 표에 링크한다. vendor 변경은 `tools/check-vendor-notices.ps1`도 필요하다.
2. **매 Cargo 직전 자원 준비**(runner 호출 전도 적용):

   ```powershell
   Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
   $env:RUSTC_WRAPPER = 'C:\Users\User\AppData\Local\Microsoft\WinGet\Packages\Mozilla.sccache_Microsoft.Winget.Source_8wekyb3d8bbwe\sccache-v0.19.0-x86_64-pc-windows-msvc\sccache.exe'
   $env:CARGO_BUILD_JOBS = '2'
   & C:\ai\ide\tools\cpu-gate.ps1 -MaxPct 70 -MaxCargo 1
   while (((Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory / 1MB) -lt 8) {
       Start-Sleep -Seconds 15
   }
   ```

3. **변경 종류별 대상**(각 Cargo 명령은 따로 준비·실행):

| 변경 | 먼저 돌릴 시험/도구 |
|---|---|
| RPC/wire/등록 | `cargo test -p pod-protocol wire_golden`; `cargo test -p podd --lib server::rpc`; `tools/test-daemon-refactor.ps1 -Test protocol_wire,protocol_compat,rpc_routes,refactor_acceptance` + 해당 도메인. client 경계 변경은 `cargo test -p pod-client`와 runner의 `client_compat`. |
| 도메인 상태/이벤트/저장/worker | runner의 `-Test <domain> -Filter <test>`부터 시작. 영향에 따라 `status`, `lifecycle`, `stream_order`, `libs_wire_snapshots`, `worker_commit_filters`, `refactor_acceptance`를 선택한다. primitive는 `cargo test -p pod-storage`, 프로세스는 `cargo test -p pod-process bounded`; journal은 pod-orch의 저장 실패/replay filter. |
| CLI | 격리 환경에서 `cargo test -p pod-cli --test command_contract`, `--test refactor_acceptance`를 각각 실행한다. fixture 바이너리를 먼저 준비하고 real-agent hooks/실제 SSH 시험은 켜지 않는다. daemon runner는 **podd 시험만** 실행하므로 CLI 실행을 대신하지 않는다. |
| 경로/Windows | `tools/test-shortpath.ps1 .\tools\test-daemon-refactor.ps1 -Test short_paths,stream_order`처럼 **명령/대상을 명시**한다. 인자 없는 shortpath는 전체 workspace 시험이다. 8.3 이름 없는 볼륨은 exit 2이며 PASS가 아니다. |
| 레이아웃/kit | `cargo test -p pod-app layout_mode::tests`, `layouts::kit::tests`, `workspace::layout_host::tests`, `native_qa::layout_contract`를 각각 filter로 실행하고 해당 mode QA를 더한다. `source::wire_tests`로 요청 변화를 확인한다. |
| 설정/패널/공용 UI | pod-app의 `chrome::tests`, `theme::tests`, 관련 `native_qa::focus`/`keyboard`/`ime`/`dpi` 및 feature QA filter. 공용 컨트롤은 `cargo test -p pod-ui --test select_scroll`, `--test theme_keys`와 해당 단위 시험. |
| 화면 변경 | 아래 capture/compare + `tools/visual/uia-selftest.ps1`, `tools/visual/compare-selftest.ps1`. native QA는 Win32 입력 변환·실제 IME 후보창·Narrator를 증명하지 않으므로 필요한 실제 창 QA도 별도로 남긴다. |

daemon runner는 fixture를 선빌드한 뒤 시험 exe를 직접 순차 실행한다. `-SkipBuild`는 해당 worktree/profile의 `target/<profile>/podd-test-exes.json` 색인과 실제 바이너리가 있을 때만 쓴다(최신 파일명 추측 금지, 변경 후에는 재빌드). TEMP/TMP·home·APPDATA·LOCALAPPDATA·worktrees·mods·discovery를 임시 root로 분리하고 `POD_SOCKET`을 제거한 뒤 고유 USERNAME/시험 pipe를 쓴다. 직접 CLI/daemon을 실행한다면 고유 `POD_SOCKET`과 state 디렉터리를 따로 지정한다. **터미널 자식 환경은 사용자 registry 환경에서 다시 만들 수 있으므로**, fixture가 쓰는 home/config 경로는 명시적으로 전달한다.

시각 검증은 같은 커밋의 release 앱을 준비한 뒤 실행한다([세부 절차](../tools/visual/README.md)). 빌드·캡처 전 CPU/RAM gate를 적용한다.

```powershell
# 위 자원 준비 후 필요한 앱만 빌드
cargo build -p pod-app --release --locked
# 새/변경 모드는 -Layout <id> -Scenes ...와 별도 Output/Baseline 행렬 사용
powershell -NoProfile -File tools/visual/capture.ps1 -Mode capture -Output target/visual-extension
powershell -NoProfile -File tools/visual/capture.ps1 -Mode compare -Output target/visual-extension -Baseline tools/visual/baseline
```

compare는 재캡처 없이 baseline 폴더의 PNG 전부를 비교하므로 subset 캡처를 전체 baseline과 비교하지 않는다. DPI(기존 기준 144)·1440×960 **physical**·폰트·테마·데이터·시계를 맞추고 누락/UIA 실패도 실패로 기록한다. 기존 리팩터 행렬/지문은 `crates/pod-app/qa/refactor/contract.json`에 있다. `capture.ps1`은 현재 `CARGO_BUILD_JOBS=4`를 설정하므로 별도 PowerShell 프로세스로 호출하고 다음 Cargo 전에 반드시 2로 재설정한다. 기본 review/gates fixture는 비어 있으므로 populated 동작을 별도 scripted backend/native QA로 검증한다.

4. **마지막**: 변경 crate의 `cargo fmt -p <crate> -- --check`, `cargo clippy -p <crate> --all-targets --locked -- -D warnings`, 정책 검사를 마치고 실행한 filter·환경·실패/미실행·artifact를 기록한다. 전체 검사는 CI/통합 담당 범위다. `.github/workflows/ci.yml`은 정책 검사 5종(`check-*` 4종 + `test-publish-public`), fmt, ConPTY 준비, clippy, `cargo doc -p podd`(warnings 오류), build/test(두 spike 제외), cargo-deny를 정의한다. 시험은 `POD_REQUIRE_TOOLS=1`, 실 Codex hooks 시험 한 개는 이름으로 제외한다. workflow의 push 대상은 `main`이며 파일에는 아직 활성화되지 않았다는 주석도 있으므로 **로컬 master 검사 통과를 CI 성공으로 적지 않는다**. 시각 비교는 이 CI 목록에 없어 별도로 수행해야 한다.

작성 검증: 코드·시험·위 검증 기록 대조, `check-docs`·`check-workspace`·`check-boundaries`·문서 경로/링크 검사·`git diff --check` 통과. Cargo/daemon/UI 실행은 하지 않았다.

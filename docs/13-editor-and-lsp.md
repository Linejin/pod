# P3 편집: 파일 트리 · 퀵오픈 · 코드 에디터 · LSP (`crates/pod-editor`)

작성 2026-11-09. 브랜치 `SJin-Choi/p3-editor`. **pod-app 통합 완료(2026-11-09, 브랜치 `SJin-Choi/p3-integrate`, §8)**. 이 문서는 구조, pod-app용 API, 서버 설정, 통합, 남은 일을 정리한다.

## 1. 구성

```
pod-editor (lib)
├─ GUI 없는 핵심 (단위 테스트 대상)
│  ├─ tree.rs     .gitignore 인식 지연 로딩 트리 모델 + 키보드 이동
│  ├─ git.rs      `git status --porcelain=v1 -z` → 파일/폴더 표시(M/A/U/R/D/!)
│  ├─ watch.rs    notify 감시 + 250ms 디바운스, 중첩 .gitignore·info/exclude·전역 제외, git index·HEAD·refs 변경 보고(연결 워크트리는 본 저장소 쪽 gitdir 감시)
│  ├─ fuzzy.rs    자체 퍼지 매처(파일 이름·단어 시작·연속 문자 가중치, 일치 위치 반환)
│  ├─ index.rs    백그라운드 파일 색인(ignore 워커), 최근 파일 가중치, 50만 개 상한
│  ├─ text.rs     손실 없는 읽기·쓰기(인코딩·BOM·줄 끝, 섞인 줄 끝 보존), 디스크 스탬프·원자적 쓰기, 2MB/16MB 크기 제한, 바이너리 감지, UTF-8/16/32 열 변환
│  ├─ uri.rs      Windows 드라이브 문자·UNC·한글/공백 경로 ↔ file URI
│  ├─ fixture.rs  데모·스모크 테스트용 작은 git 저장소 생성(파일은 체크아웃 줄 끝과 무관하게 항상 LF로 씀 + `.gitattributes`)
│  └─ lsp/
│     ├─ rpc.rs        Content-Length 프레이밍, 메시지 분류
│     ├─ client.rs     요청/응답 짝 맞추기(oneshot), 서버→클라이언트 요청 기본 응답
│     ├─ job.rs        Windows Job Object(KILL_ON_JOB_CLOSE)로 서버 프로세스 트리 소유
│     ├─ registry.rs   언어 판별, 서버 설정(기본값 + JSON 덮어쓰기), 루트 찾기, PATH/PATHEXT 검색, 설치 확인
│     ├─ server.rs     프로세스 실행, initialize/shutdown, 재시작 백오프(1·2·4·8·16초, 5회)
│     ├─ store.rs      작업 트리당 GPUI 엔티티 LspStore: 서버 상태, 문서 동기화, 진단 저장, 요청
│     │  └─ store/    lifecycle·documents·diagnostics·requests·edits (같은 LspStore의 private impl)
│     └─ providers.rs  gpui-component 에디터 Completion/Hover/Definition 공급자 어댑터
└─ ui/ (GPUI)
   ├─ workspace.rs  EditorWorkspace: 트리 | 탭·배너·에디터·문제/참조 패널 | 상태 표시줄 + 오버레이
   │  └─ workspace/  session·buffers·files·tabs·tree·lsp_actions·render_* (같은 상태의 private impl)
   └─ quick_open.rs Ctrl+P 퀵오픈
bin/pod-editor-demo   창 하나(보조 모니터)로 실행하는 데모, --scenario로 스크린샷용 상태 재현
bin/pod-editor-fake-lsp  테스트용 가짜 언어 서버(제품 아님)
```

에디터 자체는 gpui-component 0.9의 `EditorState`(코드 에디터 모드)를 그대로 쓴다: Tree-sitter 강조(Rust, Python, TypeScript/TSX, JavaScript, JSON, TOML, Markdown — `gpui-kit` 기능 플래그로 켬), 줄 번호, 찾기/바꾸기(Ctrl+F/Ctrl+H, `searchable(true)`), 진단 밑줄, 호버/완성 팝업, Ctrl+클릭 정의 이동.

### 데이터 흐름

```
EditorState ──InputEvent::Change──▶ EditorWorkspace ──120ms 디바운스──▶ LspStore.change_document (전체 텍스트 didChange)
     ▲   공급자 호출(완성/호버/정의) 시 먼저 현재 rope를 flush ──▶ LspStore.request ──▶ Client(stdin)
     │                                                                         │
     └── set_editor_diagnostics ◀── LspStoreEvent::DiagnosticsChanged ◀── publishDiagnostics(reader 스레드 → mpsc → GPUI 태스크)
```

- 위치 단위: gpui-component는 `Position.character`를 **유니코드 문자 수**로 해석한다. 클라이언트는 `positionEncodings: ["utf-32","utf-16"]`을 제시하고(rust-analyzer는 utf-32 선택, 변환 없음), utf-16 서버는 `text.rs`가 경계에서 변환한다. 열려 있지 않은 파일의 범위(정의 대상, 진단)는 디스크 내용으로 변환한다. 진단의 `version`이 열린 문서의 현재 version과 다르면 인코딩과 관계없이(utf-32 포함) 버린다 — 줄이 이미 옮겨졌을 수 있다. version 없는 진단은 그대로 쓴다.
- 줄 끝: 버퍼는 항상 LF. CRLF 파일은 저장할 때 CRLF로 되돌린다(BOM도 유지). CRLF와 LF가 섞인 파일은 편집하지 않은 줄의 줄 끝을 그대로 두고 새 줄만 다수 쪽을 쓴다(줄 대응은 `similar`). 상태 표시줄 `혼합 (CRLF n · LF m)`. 줄 번호가 같으므로 LSP 위치도 그대로 유효하다.
- 인코딩: UTF-8이 아닌 파일은 **읽기 전용** + 배너(“UTF-8이 아님”). 그대로 저장하면 글자가 깨지므로 저장·LSP 편집을 막는다. 배너에서 EUC-KR(CP949)·Shift_JIS·Windows-1252로 다시 열 수 있고, 다시 인코딩해 같은 바이트가 될 때만 편집할 수 있다. 저장은 그 인코딩으로 하고, 표현할 수 없는 글자가 있으면 저장을 거절한다.
- 저장: 열거나 저장할 때 파일 바이트의 스탬프(길이·해시)를 기억한다. 저장 직전에 디스크가 다르면 쓰지 않고 배너 **디스크 내용 불러오기 / 차이 보기 / 덮어써서 저장**을 띄운다(감시기 250ms 전에 에이전트가 쓴 경우). 쓰기는 같은 폴더의 임시 파일(고유 이름, `create_new`) + 이름 바꾸기(원자적). 이름 바꾸기가 실패하면(다른 프로그램이 삭제 공유 없이 파일을 연 경우 등) 몰래 덮어쓰지 않는다: 원본은 그대로, 임시 파일은 지우고, 배너 **다시 저장 / 그대로 덮어쓰기(비원자적) / 닫기**를 띄운다. 다른 쓰기 오류도 같은 배너(다시 저장)를 띄운다. 읽기 전용 속성 파일은 쓰지 않는다.
- 동기화: 전체 텍스트(TextDocumentSyncKind::FULL 방식). 2MB 넘는 파일은 읽기 전용·강조 없음·LSP 없음이라 비용 상한이 있다. 증분 동기화는 남은 일.

### 외부 변경(에이전트가 파일을 바꾼 경우)

감시기 배치에 열린 파일이 있으면 디스크 내용을 읽어 비교한다.
- 우리 저장과 같으면 무시.
- 버퍼가 깨끗하면 자동으로 다시 불러오고 커서 위치 유지, 상태 표시줄에 알림(`auto_reload_clean`, 기본 켬).
- 저장하지 않은 변경이 있으면 경고 배너: **디스크 내용 불러오기 / 차이 보기 / 내 변경 유지**. 삭제된 경우 **탭 닫기 / 내 변경 유지**.
- 생성·삭제가 있으면 퀵오픈 색인을 다시 만들고(판단은 백그라운드, 색인 HashSet 조회), 모든 배치 뒤 git 상태를 다시 읽는다. 에이전트가 파일을 바꾸지 않고 `git commit`/`checkout`만 해도 index·HEAD·refs 변경으로 배치가 온다.

### 단축키 (키 문맥 `PodEditor`)

| 키 | 동작 |
|---|---|
| Ctrl+P | 퀵오픈 (↑↓ PgUp/PgDn, Enter, Esc) |
| Ctrl+S / Ctrl+K S | 저장 / 모두 저장 |
| Ctrl+W, 가운데 클릭, × | 탭 닫기(저장 안 된 탭은 저장/버리기/취소 확인) |
| Ctrl+Tab / Ctrl+Shift+Tab | 최근 사용 순서로 이동(Ctrl을 누른 채 계속 누르면 더 이전으로) |
| Ctrl+PageDown / Ctrl+PageUp | 탭 위치 순서로 다음/이전 |
| Ctrl+Shift+T | 닫은 탭 다시 열기(편집기 안에서. 밖에서는 pod-app의 새 터미널) |
| Ctrl+K Enter, 탭 더블클릭 | 미리 보기 탭 고정 |
| Ctrl+G | 줄로 이동 (`42`, `42:5`) |
| Ctrl+F / Ctrl+H | 파일 안 찾기 / 바꾸기 (gpui-component 기본) |
| F12, Ctrl+클릭 | 정의로 이동(다른 파일이면 탭 열기) |
| Shift+F12 | 참조 찾기 → 아래 패널 |
| F2 | 이름 바꾸기 → 미리 보기(파일별 줄 전/후) → Enter 적용 |
| F8 | 다음 문제 |
| Ctrl+Space | 완성 목록 |
| Ctrl+J | 문제/참조 패널 열고 닫기 |
| Alt+1 / Alt+2 | 트리 / 에디터로 초점 이동 |
| 트리(문맥 `PodEditorTree`) | ↑↓ 이동, → 펼치기/자식, ← 접기/부모, Enter 열기(고정 탭), Space 미리 보기, Home/End |

pod-app의 전역 단축키(Ctrl+Shift+…)와 겹치지 않게 골랐다. 겹치더라도 문맥이 더 깊은 쪽이 이긴다.

## 2. pod-app용 API

```rust
// 앱 시작 시 한 번 (gpui_kit::init, pod_ui::apply_theme 다음)
pod_editor::init(cx);

// 작업(워크트리)마다 하나
let editor = cx.new(|cx| pod_editor::EditorWorkspace::new(task.worktree_path.clone(), mode, window, cx));
cx.subscribe(&editor, |this, _, ev: &pod_editor::EditorEvent, cx| match ev {
    EditorEvent::FileOpened(p) | EditorEvent::FileClosed(p) => {}
    EditorEvent::ActiveFileChanged(p) => {}          // 탭 제목/인스펙터 갱신
    EditorEvent::DirtyChanged { path, dirty } => {}   // 작업 목록에 "저장 안 됨" 표시
    EditorEvent::FileSaved(p) => {}                   // 리뷰 diff 새로고침
    EditorEvent::ExternalChange { path, reloaded } => {} // 에이전트가 파일을 바꿈
}).detach();
```

| 메서드 | 용도 |
|---|---|
| `open_file(path)` / `goto(path, line, col)` | 파일 열기(상대 경로는 루트 기준) / 1부터 세는 줄·열로 이동. 리뷰 화면 "에디터에서 열기", 터미널 경로 클릭에 사용 |
| `open_quick_open()`, `quick_open_query(q)` | 퀵오픈 |
| `save_active()`, `save_all()`, `dirty_paths()` | 작업 삭제·전환 전 저장 확인 |
| `active_path()`, `open_paths()`, `active_editor()` | 상태 조회 |
| `set_mode(Mode)` | 다크/라이트(전역 `apply_theme`는 호스트가 호출) |
| `show_panel(references)` | 문제/참조 패널 |
| `shutdown(cx) -> Task<()>` | 언어 서버 정중히 종료(2초 뒤 프로세스 트리 강제 종료). 작업 닫을 때, 앱 종료(`on_app_quit`) 때 호출 |
| `lsp() -> &Entity<LspStore>` | `statuses()`, `diagnostic_counts()`, `set_enabled(false)` 등 |
| `session(cx) -> EditorSession`, `restore_session(&s)` | (통합 때 추가) 열린 탭·커서·활성 탭 저장/복원. 복원한 탭은 뷰가 처음 포커스를 받을 때까지 LSP를 시작하지 않는다 |
| `close_path(path, force)`, `has_overlay()` | (추가) 탭 닫기(저장 안 됨이면 확인), 오버레이 여부 |
| `start_new_entry(dir, folder)`, `start_rename_entry(path)` | (추가) 트리 파일 조작 프롬프트 |
| `LspStore::running_servers()`, `suspend(key)`, `touch(path)` | (추가) 호스트의 동시 서버 상한·LRU: 실행 중 서버와 마지막 사용 시각, 정중히 멈춤(`ServerState::Suspended`), 다시 쓰면 재시작 |
| `ServerStatus::short_label()` | (추가) `rust-analyzer 실행 중 / 색인 중 / 설치 안 됨 / 일시 중지됨` |
| `preview_file(path)`, `tab_states()`, `closed_paths()`, `reopen_closed()`, `switch_mru(fwd)`/`end_switch()`, `step_tab(fwd)`, `close_active()`, `pin_active()` | (탭) 미리 보기로 열기, 탭 상태(활성·미리 보기·저장 안 됨·디스크 변경), 닫은 탭, Ctrl+Tab, Ctrl+PageUp/Down, Ctrl+W, 고정. 이벤트 `TabsChanged`(순서·고정·최근 순서 → 레이아웃 저장) |
| 액션 `QuickOpenFile, Save, SaveAll, CloseTab, NextTab, PrevTab, NextTabPosition, PrevTabPosition, ReopenClosedTab, PinTab, GotoLine, FindReferences, RenameSymbol, NextProblem, TogglePanel, FocusTree, FocusEditor` | 명령 팔레트(Ctrl+K)에 등록 |

### 통합 계획 (→ §8에서 구현, 차이는 §8에 적음)

1. **Dock 패널로 넣기**: `EditorWorkspace`를 감싸는 얇은 `Panel` 구현(`pod-app/src/editor_panel.rs`)을 만들어 작업마다 하나씩 Dock 탭으로 연다(제목 "편집 · <작업 이름>"). 터미널 패널과 같은 Dock에 두면 나란히 볼 수 있다. 뷰 하나 = 워크트리 하나 = LspStore 하나라서 작업 간 서버가 섞이지 않는다.
2. **수명**: 작업 삭제/닫기 → `dirty_paths()`가 비어 있지 않으면 확인 대화상자 → `shutdown().await` 후 drop. 앱 종료 시 모든 에디터 `shutdown` (Job Object 덕분에 비정상 종료여도 서버는 같이 죽음).
3. **연결 지점**: 리뷰 화면 파일 목록·라인 코멘트 → `goto`; 사이드바 작업 컨텍스트 메뉴 "파일 열기" → `open_quick_open`; 팔레트에 위 액션 등록; 상태 배지 옆에 `DirtyChanged` 표시.
4. **설정**: 설정 화면에 "언어 서버" 섹션 — `LspStore::statuses()`로 상태 표시, `%APPDATA%\Pod\lsp.json` 편집 안내, `set_enabled`.
5. 테마: pod-app이 테마를 바꿀 때 `apply_theme` 다음 각 에디터 `set_mode`.
6. podd 경유는 필요 없다: 편집은 로컬 파일 작업이고 언어 서버는 UI 프로세스의 자식이다(UI를 끄면 서버도 끈다 — 에이전트 실행과 무관하므로 "UI를 꺼도 에이전트는 계속 돈다" 원칙과 충돌하지 않음).

## 3. 언어 서버 설정

기본 등록(`ServerRegistry::default`):

| id | 언어 | 명령 | 루트 표식 | 루트 규칙 | 설치 확인 |
|---|---|---|---|---|---|
| rust-analyzer | rust | `rust-analyzer` | Cargo.toml | 가장 바깥(워크스페이스) | `--version` 실행 (rustup 프록시는 구성요소가 없어도 PATH에 있으므로) |
| pyright | python | `pyright-langserver --stdio` | pyproject.toml, setup.py, setup.cfg, pyrightconfig.json, requirements.txt | 가장 가까운 | PATH |
| typescript-language-server | typescript, typescriptreact, javascript, javascriptreact | `typescript-language-server --stdio` | tsconfig.json, jsconfig.json, package.json | 가장 가까운 | `--version` |

루트는 워크트리 밖으로 나가지 않으며, 표식이 없으면 워크트리 루트. (서버 id, 루트)마다 프로세스 하나.

덮어쓰기/추가: `%APPDATA%\Pod\lsp.json`(사용자, `POD_EDITOR_CONFIG_DIR`로 바꿀 수 있음) → `<워크트리>\.pod\lsp.json`(저장소). 같은 `id`는 통째로 바뀐다.

저장소 파일은 남이 쓴 코드다(에이전트나 클론한 저장소가 쓸 수 있고, `command`·`args`·`probe_args`와 `initialization_options`(예: rust-analyzer `overrideCommand`)가 프로그램을 실행한다). 그래서 `.pod/config.toml`(문서 19 §3.2)과 같은 방식으로 **내용 해시 승인**을 거친다: 승인 전에는 `"enabled": false`만 적용하고 나머지는 무시한다. 편집기 배너가 실행될 명령 전체를 보여 주고 **이 내용 신뢰하고 사용 / 무시**를 묻는다. 승인은 `(저장소 = git common dir, 파일 SHA-256)`으로 `<사용자 설정>\lsp-trust.json`에 남고(같은 저장소의 모든 워크트리에 적용), 파일이 한 바이트라도 바뀌면 다시 묻는다. 감시기가 파일 변경을 보면 레지스트리를 다시 읽는다.

```json
{ "servers": [
  { "id": "rust-analyzer", "languages": ["rust"], "command": "C:/tools/rust-analyzer.exe",
    "root_markers": ["Cargo.toml"], "root_strategy": "outermost",
    "initialization_options": { "checkOnSave": false }, "probe_args": ["--version"] },
  { "id": "taplo", "languages": ["toml"], "command": "taplo", "args": ["lsp", "stdio"], "root_strategy": "nearest" },
  { "id": "pyright", "languages": ["python"], "command": "pyright-langserver", "enabled": false }
] }
```

상태 표시(상태 표시줄 오른쪽 점 + 문구, 필요하면 에디터 위 배너): 확인 중 → 시작 중 → 실행 중(진행 중이면 `rust-analyzer · Indexing 45%`) / **설치 안 됨**(배너에 이유와 설치 명령) / 재시작 중 (n/5) / 중지됨(오류, 배너에 "다시 시작" 버튼) / 종료됨.

Windows 처리: 서버마다 Job Object(KILL_ON_JOB_CLOSE)라서 rustup 프록시 → 실제 rust-analyzer → cargo check/proc-macro 서버, node 하위 프로세스까지 한 번에 죽는다(테스트로 확인). 설치 확인 프로브는 `pod_process::bounded`로 돌린다: 실행과 출력 읽기를 합쳐 10초, stdout/stderr는 실행 중에 크기 제한 버퍼로 읽고, 실패한 프로브의 자손이 stderr를 잡고 있어도 0.5초 뒤 트리를 죽이고 첫 stderr 줄을 이유로 보여 준다. `CREATE_NO_WINDOW`로 콘솔 창이 뜨지 않는다. `.cmd` 셈(npm 전역 설치)은 PATHEXT로 찾는다. URI는 `file:///C:/...`로 보내고 서버가 `c%3A`, 소문자 드라이브로 돌려줘도 같은 파일로 본다.

디버깅: 환경 변수 `POD_EDITOR_LSP_LOG=<파일>`이면 주고받은 알림·서버 요청을 한 줄씩 남긴다.

설치 현황(이 PC, 2026-11-09): rust-analyzer는 `rustup component add rust-analyzer`로 설치(1.99.1). Node는 있으나 pyright/typescript-language-server는 설치하지 않음(지시대로) → 해당 파일을 열면 "설치 안 됨" 상태가 표시된다.

## 4. 라이선스와 의존성

새 의존성: `lsp-types 0.97`(MIT, gpui-component와 같은 버전), `ignore`(Unlicense/MIT), `notify 8`(CC0-1.0, 이미 Cargo.lock에 있었고 deny 허용 목록에 있음), `futures`, `serde`, `serde_json`, `anyhow`(MIT/Apache), `windows-sys`(MIT/Apache), 테스트용 `tempfile`. Tree-sitter 문법은 gpui-component 기능 플래그(MIT).

**nucleo는 쓰지 않았다**: nucleo/nucleo-matcher는 MPL-2.0이라 "Apache/MIT/ISC만" 규칙에 맞지 않는다. 대신 퍼지 매처를 직접 작성했다(`fuzzy.rs`, fzf v1식 좌측 탐욕 + 오른쪽 조이기, 파일 이름/단어 시작/연속 가중치). 5만 개 경로 질의 테스트 포함.

클린룸: Zed 등 다른 제품의 코드를 보지도 복사하지도 않았다. gpui-component(Apache-2.0)는 공개 API만 사용.

## 5. 테스트

`cargo test -p pod-editor`: 트리/.gitignore(중첩 포함)/키보드, git porcelain 파싱·폴더 집계·실제 저장소, 감시기 필터·실제 배치, 퍼지 순위·일치 위치, 색인·최근 가중치·5만 개 질의, CRLF/BOM/바이너리/크기 제한, UTF-16/8 열 변환(이모지·한글), URI(드라이브 표기 4종, UNC, `\\?\`), JSON-RPC 프레이밍(유니코드 길이, 대소문자 헤더, 잘린 프레임), 클라이언트(서버 요청 응답, 서버 종료 시 대기 요청 실패), Job Object로 손자 프로세스 종료, 레지스트리 병합·루트 규칙·PATHEXT, 재시작 백오프, 이름 바꾸기 편집 적용, 줄:열 파싱.

`tests/lsp_lifecycle.rs`(가짜 서버 `pod-editor-fake-lsp`): initialize(utf-16 협상) → didOpen → 진단(이모지 열 변환 확인) → hover/definition/references/rename/completion → 알 수 없는 메서드 오류 → shutdown/exit; 크래시 시 Exited(세대 번호 포함)와 대기 요청 실패; 서버 drop 시 손자 프로세스 종료; 미설치 서버 보고.

`rust_analyzer_smoke`(`#[ignore]`, rust-analyzer 없으면 건너뜀): 작은 고정 저장소(공백 있는 경로)에서 실제 rust-analyzer로 initialize(utf-32) → main.rs 오류 진단 → 다른 파일(geometry.rs)로 정의 이동 → hover → 종료. `cargo test -p pod-editor --test lsp_lifecycle -- --ignored` (약 70초, CPU 사용 큼 — Pod 저장소가 아니라 임시 고정 저장소에서만).

## 6. 데모와 화면 확인

```
cargo build -p pod-editor --release
target\release\pod-editor-demo.exe <경로>                         # 보조 모니터에 창
target\release\pod-editor-demo.exe --fixture <빈 폴더> --scenario hover [--light] [--duration 45]
```

`--fixture`는 고정 저장소를 만들어 연다(자체 `[workspace]`를 가진 Cargo 패키지 — 다른 워크스페이스 안에 만들어도 `cargo metadata`가 실패하지 않게. 처음에는 이게 없어서 rust-analyzer가 크레이트 그래프를 못 만들고 진단이 0개였다). 시나리오: `tree, quickopen, editor, notinstalled, diagnostics, hover, completion, definition`.

스크린샷(150% 보조 모니터, DPI 인식 캡처, 다크/라이트) — `crates/pod-editor/qa/`:

| 파일 | 내용 |
|---|---|
| `tree-*.png` | 트리: 폴더 우선, git 표시(M 주황, U/추가 초록, 폴더 점), .gitignore 숨김(target.log 없음), 탭 2개 |
| `quick-open-*.png` | Ctrl+P `geo` → 일치 글자 굵게/강조, 경로 흐리게, 결과/파일 수 |
| `editor-highlighting-*.png` | Rust Tree-sitter 강조, 한글 문자열, 들여쓰기 안내선, 상태 표시줄(줄/열·LF·UTF-8·언어·서버 진행률) |
| `diagnostics-*.png` | rust-analyzer 오류 밑줄 + 문제 패널(오류 1, 관련 힌트) |
| `hover-*.png` | `greet` 호버(시그니처 + 문서) |
| `completion-*.png` | `task.fi` 완성 → 접두어로 거른 `finish` |
| `go-to-definition-*.png` | `perimeter`에서 F12 → `geometry.rs` 탭이 열리고 정의 줄로 이동 |
| `server-not-installed-*.png` | Python 파일: "pyright 설치 안 됨" 배너 + 상태 표시줄 |

참고: 이 PC에 rust-analyzer를 설치한 뒤, 다른 Claude Code 세션의 LSP 기능이 자기 저장소에서 rust-analyzer를 띄워 3GB 넘게 쓰는 것이 관찰됐다(우리 데모와 무관, 조정자에게 보고함).

## 7. 남은 일

- ~~증분 동기화~~ → fu-app: 편집기의 `InputEditsEvent`(바이트 범위 + 새 텍스트)를 `LspStore::edit_document`가 서버 단위(UTF-16/32) 범위로 바꿔 `didChange {range, text}`로 보낸다(서버가 `textDocumentSync` Incremental일 때, 아니면 전체 텍스트). 저장본이 어긋나면(문자 경계가 아님) 범위를 버리고 기존 120 ms 전체 텍스트 동기화가 복구한다. 테스트 `incremental_ranges_replay_to_the_same_text`(이모지·한글, UTF-16/32).
- ~~완성 항목 `resolve`, 스니펫, 코드 액션~~ → fu-app:
  - **resolve**: 서버가 `resolveProvider`면 거른 목록의 앞 5개(문서 없는 것)를 `completionItem/resolve`로 동시에 채운다. 메뉴에 지연 해석 고리가 없어서 최대 250 ms만 기다리고, 늦으면 원래 항목을 쓴다. (gpui-component의 `resolve_completions`는 메뉴가 호출하지 않아 쓸 수 없다.)
  - **스니펫**: `snippetSupport: true`로 알리고, 메뉴에는 펼친 일반 텍스트를 넘긴다(`${1:x}`→`x`, 선택지는 첫 값, 변수는 기본값, `\$` 이스케이프). → small-fixes에서 탭 정지 세션 추가(아래 §7.1).
  - **코드 액션**: `CodeActionProvider`를 연결(Ctrl+.). 범위와 겹치는 진단을 같이 보내고, 고르면 `codeAction/resolve`(필요할 때) → 편집을 모든 파일에 적용(열려 있지 않으면 탭으로 열고 저장은 사용자) → `command`가 있으면 `workspace/executeCommand`. 서버가 그 명령 안에서 보내는 `workspace/applyEdit`도 적용한다(§7.1).
- 시그니처 도움말, 시맨틱 토큰, 인레이 힌트.
- ~~실제 rust-analyzer로 코드 액션·resolve 확인~~ → §7.1 (프로토콜 수준. 화면 캡처는 아님).
- ~~`workspace/applyEdit`~~ → §7.1. `didChangeWatchedFiles`는 아직 미지원 → 에이전트가 만든 새 파일을 서버가 늦게 알 수 있음(rust-analyzer는 자체 감시로 대부분 해결).
- 이름 바꾸기 결과의 파일 생성/이름 변경 작업(resource operations)은 무시하고 텍스트 편집만 적용. 열려 있지 않은 파일은 탭으로 열어 적용하고 저장은 사용자가 한다.
- ~~저장 안 된 탭 닫기 확인 대화상자~~ → 통합 때 구현(저장 후 닫기 / 저장하지 않고 닫기 / 취소).
- ~~트리 파일 조작(새 파일/이름 바꾸기/삭제)~~ → 통합 때 구현(확인 대화, 워크트리 밖·`..`·`.git` 금지). 드래그, 파일 아이콘은 남음.
- 에디터 배경·구문 색은 gpui-component 기본 테마 그대로(pod-ui 결정과 같음). Pod 토큰에 맞춘 구문 색 세트는 D3에서.
- 여러 루트(멀티 루트 워크스페이스)와 원격(SSH/WSL) 경로.
- pyright/typescript-language-server 실제 연동 확인(이 PC에 미설치라 "설치 안 됨" 표시만 확인).
- 파일마다 별도 Dock 탭: §7.2 설계 메모(구현하지 않음). 대신 파일 패널 안 탭 줄을 완성했다(§9).

### 7.1 small-fixes: 스니펫 세션 · 자동 import · applyEdit (2026-11-09)

- **스니펫 탭 정지**(`lsp/snippet.rs`): `Snippet::parse`가 텍스트와 탭 정지 범위(`$1`, `$2`, … 다음 `$0`, `$0`이 없으면 끝; 같은 번호는 첫 위치만 — 미러 연동 없음)를 돌려준다. `completion()`이 응답마다 항목별 `CompletionExtra`(펼친 텍스트, 스니펫, 자동 import 편집, 지연 해석용 원본 항목)를 `LspStore`에 기억하고(`note_completion`), 편집기의 `InputEditsEvent`에서 **같은 줄에 같은 텍스트(2글자 이상)가 삽입되면** 그 항목으로 보고 꺼낸다(`take_completion`, 한 번만, 120초 뒤 만료). 메뉴의 삽입 함수(gpui-base `insert_completion`)는 vendor가 아니라 손대지 않고, 이벤트로 알아낸다.
  - 세션(`SnippetSession`): 삽입 직후 첫 자리 표시자를 선택. **Tab** 다음, **Shift+Tab** 이전, 마지막(`$0`)에 오면 캐럿만 두고 끝. **Esc** 세션 끝(완성 메뉴가 열려 있으면 메뉴가 먼저 닫힘). 자리 표시자 안에서 입력하면 그 범위가 늘고 줄며, 앞쪽 편집(자동 import)은 모든 정지를 민다. 정지 경계를 가로지르는 편집이나 캐럿이 현재 정지를 벗어난 뒤의 Tab은 세션을 끝내고 원래 들여쓰기가 된다. 키는 `EditorWorkspace` 루트의 **capture 단계** 액션(`IndentInline`/`OutdentInline`/`Escape`)으로 편집기보다 먼저 받는다.
- **`additionalTextEdits`**: 항목에 있으면 삽입 직후 적용. 없고 서버가 `resolveProvider` + 항목에 `data`가 있으면(rust-analyzer 자동 import가 이 방식) 삽입 뒤 `completionItem/resolve`로 받아 적용(`resolve_additional_edits`).
- **열린 버퍼 편집**(코드 액션·이름 바꾸기·자동 import·applyEdit 공통 `edit_buffer`): 전체 `set_value` 대신 뒤에서부터 범위별 `replace` — 실행 취소 기록이 남고, 증분 `didChange`로 가며, 캐럿은 앞쪽 편집만큼 밀린다(이전에는 줄·열을 그대로 두어 import 뒤 캐럿이 한 줄 위로 갔다).
- **`workspace/applyEdit`**: 클라이언트 능력 `applyEdit: true`. 읽기 스레드가 요청을 `ServerEvent::ApplyEdit {params, reply}`로 넘기고 최대 5초 답을 기다린다 → `LspStoreEvent::ApplyEdit` → 편집기가 `apply_file_edits` 후 `reply.send(모든 파일 적용 여부)`, 상태 표시줄 알림 `<label>: 파일 n개를 고쳤습니다 (저장 전)`. 답이 없거나 편집기가 없으면 `applied: false`와 이유.
- **테스트**: `snippet.rs` 3(정지 순서·중첩·선택지·한글, 자명한 스니펫, 편집 추적·이동·인접 정지), `store.rs` `completion_extras_match_the_inserted_text_once`, `server.rs` `apply_edit_waits_for_the_editor_answer`, `lsp_lifecycle.rs` `server_apply_edit_is_forwarded_and_answered`(가짜 서버 `fake.insertHeader` 명령 → applyEdit → 적용/거절이 서버에 그대로 전달), pod-app 네이티브 QA `g_snippet_completion_tab_stops_and_auto_import`(메뉴와 같은 `insert_completion` → import 위에 추가, 첫 자리 표시자 선택, Tab/Shift+Tab, 입력으로 바꾸기, Esc 뒤 Tab은 들여쓰기).
- **실제 rust-analyzer**(`rust_analyzer_code_actions`, `#[ignore]`, 고정 저장소에서만, 끝나면 서버 종료 확인): `HashMap` 위 코드 액션 `Import std::collections::HashMap`(편집 없이 옴 → `codeAction/resolve`) → `use std::collections::HashMap;` 추가; `BTreeS` 완성 `BTreeSet`(추가 편집 없이 옴 → `completionItem/resolve`로 `use` 받음); `word_c` 완성 `word_count(${1:text})$0` → 정지 `text`, 끝. 출력: `crates/pod-app/qa/small-fixes/rust-analyzer-code-actions.txt`. 이 확인 중 rust-analyzer가 서버 URI를 `file:///c%3A/…`처럼 쓰는 것을 봤다 — 제품 경로(`workspace_edit`)는 이미 경로로 비교한다.

### 7.2 설계 메모: 파일마다 별도 Dock 탭 (보류)

지금은 파일 패널 하나 = 워크트리 하나(트리 + 내부 탭). 파일마다 Dock 탭을 두려면 (1) `EditorWorkspace`에서 버퍼(EditorState, 저장 상태, 디스크 감시, LSP 문서)를 소유하는 작업별 **버퍼 저장소**와 (2) 파일 하나를 보여 주는 얇은 **뷰 패널**로 나눠야 한다. 같은 파일을 두 탭에 열면 같은 `EditorState`를 두 요소가 그리게 되는데 gpui-component 에디터는 뷰 하나를 전제로 스크롤·선택을 상태에 둔다 → 분할 보기는 "같은 파일은 한 탭" 규칙이 필요하다. 저장 안 됨 표시·닫기 확인·레이아웃 저장(`PodEditor` 세션 → 탭마다 `{task, path}`)·LSP 수명(마지막 탭이 닫혀야 문서 닫기)도 패널 단위에서 저장소 단위로 옮겨야 한다. pod-app `workspace.rs`/`layout.rs`와 pod-editor UI를 넓게 바꾸는 일이라 "작게"에 맞지 않아 이번 묶음에서는 빼고 남겨 둔다.

## 8. pod-app 통합 (2026-11-09)

브랜치 `SJin-Choi/p3-integrate`. 사용자 관점의 화면·조작은 [docs/09 §10](09-pod-app.md#10-p3-편집-통합-파일--퀵오픈--에디터--lsp-2026-11-09).

**구조**: `crates/pod-app/src/editor_panel.rs`의 `EditorPanel`(Dock `Panel`, 이름 `PodEditor`)이 작업마다 `EditorWorkspace` 하나를 가진다(§2 계획 1 그대로 — 파일 트리 + 내부 탭). 계획과 다른 점: 파일마다 별도 Dock 탭으로 나누지는 않았다(버퍼 소유와 뷰를 나눠야 함, 남은 일). `Workspace`(pod-app)가 `editors`를 들고 `open_editor(task, Some((rel, line, col)) | None, quick_open)`로 연다. 연결 지점: 전역 Ctrl+P, 팔레트 명령 3개, 사이드바 작업 메뉴, 리뷰 파일 헤더 `↗ 편집`·파일 목록·Ctrl+Shift+O, 저장 → 리뷰 diff 새로 고침, 외부 변경(저장 안 됨) → 경고 토스트.

**LSP 수명**:

| 사건 | 동작 |
|---|---|
| 파일 패널 열기, 트리 보기 | 서버 없음 |
| 지원 파일을 처음 활성화(트리·퀵오픈·리뷰·탭) | `open_document` → (서버 id, 루트) 서버 시작. pod-editor가 활성화 때로 미뤘다(이전엔 탭 생성 때) |
| 레이아웃 복원 | 탭만 되살림, 뷰가 처음 포커스를 받을 때 활성 탭의 서버 시작 |
| 서버 상태 변화 | 앱이 모든 파일 패널의 `running_servers()`를 모아 상한(기본 2) 초과분을 LRU로 `suspend` + 정보 토스트 |
| 일시 중지된 서버의 파일로 돌아감 | `touch`/`open_document`가 다시 시작 |
| 파일 패널 닫기 / 작업 삭제 / 작업이 목록에서 사라짐 | `shutdown()` (2초 정중 종료 후 Job Object 트리 종료) |
| 앱 종료 | `on_app_quit`: 레이아웃 저장, 모든 편집기 `shutdown()` 대기. 강제 종료여도 Job Object가 정리 |

**저장 안 함 보호**: 탭 닫기 확인(pod-editor 오버레이), 저장 안 한 파일이 있는 파일 패널은 Dock에서 닫을 수 없음(`closable`), 창 닫기 확인(`on_window_should_close` → 대화, 선택 후 `cx.quit()`), 작업 삭제 전 확인.

**접근성·테마**: 트리(`Tree`/`TreeItem`, 선택 상태, git 표시 포함 이름), 탭 목록(`TabList`/`Tab`, 저장 안 됨 표시), 탭 닫기 단추 이름, 에디터 이름 `편집기: <경로>`, 오버레이·배너 단추 이름과 포커스 링(`pod_ui::focus_style`), 확인 대화는 `AlertDialog`. gpui-component 에디터의 접기 단추에 이름이 없어서 vendor 패치에 `Input.Fold/Unfold` 키를 추가했다(POD-PATCHES.md, 한국어는 pod-app 카탈로그). 새 문구는 pod-app `strings.rs`, pod-editor는 `ui/strings.rs`(기존 문구는 코드 옆에 그대로).

**QA 장면**(`--ui-fixture --fixture-scene …`, `fixture_scenes.rs`): `editor`(시각 회귀 기준, LSP 끔), `editor-tree`, `editor-quickopen`, `editor-new-file`, `editor-close`, `editor-dirty`, `editor-agent-edit`, `review-editor`, `review-editor-open`, `editor-lsp`/`editor-hover`/`editor-completion`(작은 고정 저장소에서 실제 rust-analyzer), `editor-lsp-cap`(`.pod/lsp.json`으로 rust-analyzer/pyright id를 가짜 서버로 바꾸고 상한 1). 픽스처 소스는 이 장면들에서만 `task-000`에 워크트리를 주고 `task.diff`를 그 저장소의 git으로 답한다. 실행: `crates/pod-app/qa/editor/run-editor-qa.ps1`.

**스크린샷**(`crates/pod-app/qa/editor/`, 150% 보조 모니터, DPI 인식 캡처, 모두 직접 확인):

| 파일 | 내용 |
|---|---|
| `dark/light-editor-tree.png` | 작업 공간 안 파일 패널: 트리(폴더 우선, M/U 표시, 숨김 규칙), 탭 2개, Rust 강조, 상태 표시줄 |
| `dark/light-editor-quickopen.png` | Ctrl+P `geo` → `geometry.rs`, `settings.json`, 일치 글자 강조, 결과/파일 수 |
| `dark-editor-lsp.png` | rust-analyzer 진단: 문제 패널(오류 1 + 관련 힌트), 에디터·앱 상태 표시줄 `rust-analyzer 실행 중` |
| `dark-editor-hover.png` | `greet` 호버(시그니처 + 문서) |
| `dark-editor-completion.png` | `task.fi` → `finish` 완성 |
| `dark/light-review-editor.png` | 리뷰 diff 파일 헤더마다 `↗ 편집` |
| `dark/light-review-editor-open.png` | 리뷰에서 연 `src/main.rs`가 리뷰 옆 탭으로 열리고 첫 변경 줄(16)에 커서 |
| `dark/light-editor-agent-edit.png` | 에이전트가 저장 안 한 `lib.rs`를 바꿈: 배너(불러오기/유지) + 경고 토스트, 깨끗한 README는 자동 다시 불러옴(상태 표시줄 알림) |
| `dark/light-editor-close.png` | 저장 안 한 탭 닫기 확인 |
| `dark-quit-unsaved.png` | 창 닫기 확인(저장 안 한 파일 목록) |
| `dark/light-editor-new-file.png` | 트리 `새 파일 · src/` 프롬프트 |
| `dark-editor-lsp-cap.png` | 상한 1: LRU 일시 중지 토스트 2개, 돌아간 서버가 `실행 중` |
| `dark-restore-before/after.png` | 종료 전 탭 상태 / 다시 시작 후 복원된 탭·커서 |

결과 요약과 명령 로그: `qa-result.txt`, `clippy.txt`, `tests.txt`, `deny.txt`, `release.txt`, 시각 회귀 `visual/compare.json`.

**참고(작업 환경)**: 이 작업을 한 Claude Code 세션 자체가 파일을 고칠 때마다 Pod 저장소 전체에 rust-analyzer(약 3.5GB)를 자동으로 띄웠다. 지시대로 그때마다 종료했다(세션의 LSP 기능을 쓰지 않았음에도 발생). 워커 환경에서 이 자동 시작을 끄는 설정이 필요하다.

## 9. 파일별 편집기 탭 (2026-11-09, 브랜치 `SJin-Choi/editor-tabs`)

파일 패널(작업 하나) 안의 탭 줄을 완성했다. §7.2의 "파일마다 Dock 탭"은 버퍼 소유자/뷰 분리가 필요해 그대로 보류하고, 탭 규칙은 GUI 없는 모델 `tabs.rs`(`TabSet`)에 두어 단위 테스트한다. `EditorWorkspace`는 버퍼 `Vec`을 같은 순서로 맞춘다.

| 동작 | 규칙 |
|---|---|
| 미리 보기 탭 | 트리 한 번 클릭·Space, 퀵오픈으로 연 파일. 제목이 기울임꼴, 이름에 `(미리 보기)`. 하나만 있고 다음 미리 보기가 **같은 자리**를 바꾼다(바뀐 탭은 닫은 탭 목록에 넣지 않음) |
| 고정 | 편집(저장 안 됨이 되는 순간), 탭 더블클릭, 트리 더블클릭·Enter, Ctrl+K Enter, 호스트의 `open_file`/`goto`(리뷰, 터미널 링크, 정의 이동) |
| 새 탭 위치 | 활성 탭 바로 오른쪽 |
| 닫기 | Ctrl+W·가운데 클릭·×. 저장 안 된 탭은 `저장 후 닫기 / 저장하지 않고 닫기 / 취소`(Enter 저장, Esc 취소 → 편집기로 초점 복귀). 닫으면 **최근 사용 순서**의 다음 탭이 활성, 초점은 그 편집기(탭이 없으면 트리) |
| Ctrl+Tab | Ctrl을 누르고 있는 동안 최근 사용 목록을 따라 이동, Ctrl을 떼면 순서 확정 → Ctrl+Tab 한 번은 두 파일 사이 왕복 |
| Ctrl+Shift+T | 마지막에 닫은 탭을 원래 자리에 고정 탭으로(최대 20개, 이미 다시 연 파일·더 이상 읽을 수 없는 파일은 건너뜀). 삭제된 파일, 호스트가 닫은 탭은 넣지 않음 |
| 저장 안 됨 / 디스크 변경 | 닫기 단추 자리에 `●`. 저장 안 한 내용이 있는데 디스크가 바뀌면 탭 앞에 경고 아이콘(삭제는 빨강), 이름에 `(디스크에서 바뀜)` / `(디스크에서 삭제됨)`. 배너는 활성 탭에서(§1 외부 변경 그대로) |
| 접근성 | 탭 줄 `TabList`(이름 `열린 파일`), 탭 `Tab` + 선택 상태, 이름 `main.rs — src/main.rs (미리 보기, 저장 안 됨)`, 닫기 단추 `탭 닫기 <이름>`. 키보드만으로 모든 동작(위 단축키) |
| 저장(레이아웃) | `PodEditor` 세션에 탭별 `preview`(참일 때만)와 `mru`(탭 번호, 최근 순) 추가. 이전 레이아웃도 그대로 읽힌다. 복원한 미리 보기 탭은 미리 보기로 남는다 |
| 작업 삭제 | 기존 흐름 그대로(저장 확인 → 편집기 닫기 → 모든 탭의 문서 닫기와 서버 종료) |

**LSP**: 탭이 서버에 문서를 연 경우에만(활성화 때 `didOpen`) 닫을 때 `didClose`를 보낸다. 미리 보기 교체도 닫기다. 복원한 뒤 보이지 않은 탭은 서버에 열리지 않는다.

**테스트**: `tabs.rs` 단위 9(새 탭 위치, 미리 보기 교체·고정, 닫을 때 최근 탭, 다시 열기 위치·건너뛰기, 20개 상한, Ctrl+Tab 누름/뗌, Ctrl+PageUp/Down, 이름 바꾸기, 최근 순서 저장), `strings.rs` 탭 이름 1, 세션 JSON 1. `tests/tabs_lsp.rs`(GPUI 테스트 플랫폼 + 가짜 서버 `--log`): 미리 보기 → 교체 → 고정 → 닫기 → 다시 열기 → 호스트 닫기 동안 파일마다 `didOpen`/`didClose` 수가 정확하고 중복이 없으며 열린 탭 수와 서버에 열린 문서 수가 같음; 복원한 탭은 보일 때만 열림. pod-app 네이티브 QA `h_editor_tabs_keyboard_preview_mru_close_reopen`: 키보드만으로 퀵오픈 미리 보기 교체, 입력으로 고정, Ctrl+Tab 왕복·누른 채 이동, Ctrl+PageUp/Down, Ctrl+W 확인 → Esc(초점 복귀) → Enter(저장 후 닫기), Ctrl+Shift+T, 패널 상태로 다시 만든 편집기에서 순서·미리 보기·활성·최근 순서 복원, 마지막 탭을 닫으면 초점이 트리로.

**고친 점**: 저장 안 된 탭 닫기 확인과 트리 삭제 확인이 포커스를 받지 않아 키보드 Enter/Esc가 먹지 않던 문제 — 확인 창이 자기 포커스를 갖고(버튼 사이 Tab 이동 포함) Esc는 편집기로 초점을 돌려준다(네이티브 QA `h_tree_delete_prompt_esc_cancels_enter_deletes`). GPUI는 문맥 없는 단축키를 가장 깊게 취급해 pod-app의 Ctrl+Shift+T(새 터미널)가 편집기 단축키를 이긴다 → pod-app이 초점이 파일 패널 안이면 `reopen_closed`로 넘긴다.

**화면**(`crates/pod-app/qa/editor-tabs/`, 장면 `--fixture-scene editor-tabs`, 보조 모니터 150 %, DPI 인식 캡처, 직접 확인): `dark/light-editor-tabs.png`, `hc/dark-editor-tabs.png`(고대비) — 고정 `README.md`, 저장 안 됨 `●` + 디스크 변경 경고 아이콘의 `lib.rs`, 기울임꼴 미리 보기 `main.rs`(활성). 같은 폴더 `*.tree.txt`의 UIA 덤프에 탭 이름 `lib.rs — src/lib.rs (저장 안 됨, 디스크에서 바뀜)`, `main.rs — src/main.rs (미리 보기)`, 닫기 단추 `탭 닫기 …`. 전체 테스트 로그 `tests.txt`.

**남은 일**: 파일마다 Dock 탭(§7.2). 끌어서 순서 바꾸기·탭 메뉴·같은 이름 구분·넘침은 §9.1.

### 9.1 탭 순서·메뉴·같은 이름·넘침 (2026-11-09, 브랜치 `SJin-Choi/editor-tabs2`)

| 동작 | 규칙 |
|---|---|
| 순서 바꾸기 | 탭을 끌어 놓는다. 놓을 자리(탭의 왼쪽 절반 = 앞, 오른쪽 절반 = 뒤, 마지막 탭 뒤 빈칸 = 끝)에 강조색 세로 막대. 키보드: Ctrl+Shift+PageUp/PageDown이 활성 탭을 한 칸씩(끝에서 멈춤, 돌지 않음). 옮긴 미리 보기 탭은 고정된다. 순서는 레이아웃 세션의 탭 순서 그대로 저장 |
| 탭 메뉴 | 탭 우클릭, 또는 Shift+F10·메뉴 키(활성 탭, 탭 아래에 열림). `닫기 / 다른 탭 닫기 / 오른쪽 탭 닫기 / 저장된 탭 닫기 / 모두 닫기 │ 고정·고정 해제 │ 경로 복사 / 상대 경로 복사 │ 파일 트리에서 보기`. 할 수 없는 항목(오른쪽 탭 없음 등)은 흐리게, 이름에 `(사용할 수 없음)`, 화살표가 건너뜀. ↑↓ Home End(순환), Enter·Space 실행, Esc·Tab 닫기 → 초점은 메뉴를 열기 전 자리로 |
| 여러 탭 닫기 | 다른 탭/오른쪽 탭 닫기는 메뉴를 연 탭이 활성으로 남는다. 저장된 탭 닫기는 묻지 않는다. 저장 안 된 탭이 섞이면 **확인 창 하나**에 그 파일 목록과 `취소 / 저장하지 않고 닫기 / 모두 저장 후 닫기`(Enter 저장, Esc 취소 = 아무것도 닫지 않음). 저장에 실패한 파일은 열린 채 남는다. 오른쪽부터 닫아 Ctrl+Shift+T가 왼쪽부터 원래 자리로 되돌린다 |
| 고정 해제 | "고정"은 §9의 미리 보기 고정과 같은 뜻이다. 고정 해제하면 그 탭이 미리 보기 탭이 되고(다음 미리 보기가 그 자리를 바꿈), 원래 미리 보기 탭은 닫지 않고 고정한다. 저장 안 된 탭은 해제할 수 없다 |
| 경로 복사 | 절대 경로 / 워크트리 기준 상대 경로(OS 구분자)를 클립보드로, 상태 줄에 `복사했습니다: …` |
| 파일 트리에서 보기 | 상위 폴더를 펼치고 그 파일을 선택, 트리로 초점 |
| 같은 이름 | 이름이 같은 탭은 `mod.rs · podd/src`처럼 **서로 다른 가장 짧은 상위 경로 끝부분**을 흐린 글씨로 붙인다(대소문자 무시, 루트 파일은 `./`, 구분할 수 없으면 상위 경로 전체). 도움말과 접근성 설명은 절대 경로, 접근성 이름은 기존대로 상대 경로 포함 |
| 넘침 | 탭 줄은 가로로 스크롤(휠도 가로로). 열기·전환·Ctrl+Tab·다시 열기·옮기기·닫기 뒤 활성 탭이 항상 보이도록 스크롤한다. 넘치면 오른쪽 끝에 `열린 파일 모두 보기` 단추(Tab으로 갈 수 있음) → 모든 탭 목록 메뉴(현재 탭 선택, Enter로 이동) |

스타일은 `pod-ui` 토큰만 쓴다(막대 `accent_fg`, 메뉴 `bg_raised`/`border_subtle`/`bg_selected`/`bg_hover`, 흐린 글씨 `text_muted`). 새 색은 없다(D4 재디자인 전 임시 모양).

**구현**: 규칙은 `tabs.rs`(`move_tab`, `drop_index`, `move_active`, `unpin`, `BulkClose` + `bulk_targets`, `close_many`, `disambiguate`). 메뉴는 툴킷 컨텍스트 메뉴가 키보드로 열리지 않아 편집기 안의 자체 메뉴(`Role::Menu`/`MenuItem`, 초점은 메뉴, 선택 항목은 active descendant)로 만들었다. 닫기 확인 창은 탭 하나와 여럿을 같은 코드로 처리한다.

**테스트**: `tabs.rs` 단위 +7(옮기기·활성 유지·고정, 놓을 자리 → 위치, 활성 탭 옮기기 끝에서 멈춤, 고정 해제, 일괄 닫기 대상, 일괄 닫기 순서·활성·다시 열기, 같은 이름 구분: 깊은 경로·세 갈래·한쪽이 다른 쪽 끝부분·루트·대소문자·같은 경로·한글), `strings.rs` +3. `tests/tabs_ui.rs`(GPUI 테스트 플랫폼 + 실제 DirectWrite 글꼴): 좁은 창에서 12개 탭 — 열 때마다·Ctrl+PageDown 순환·Ctrl+Tab·Ctrl+PageUp·Ctrl+Shift+T·Ctrl+Shift+PageUp/Down 뒤 활성 탭이 전부 보이고 넘침 단추가 생기며, 넓히면 사라짐; 같은 이름 제목 `mod.rs · podd/src` / `mod.rs · pod-app/src`, 하나를 닫으면 접미사가 빠짐. pod-app 네이티브 QA `h_editor_tab_menu_and_move_keyboard`: 키보드만으로 옮기기(미리 보기 고정, 끝에서 멈춤, 레이아웃 순서), Shift+F10·메뉴 키, 항목 목록, ↑ 순환·Home, Esc 초점 복귀, 상대/절대 경로 복사, 트리에서 보기(트리 초점), 고정 해제/고정, 다른 탭 닫기 → 저장 안 된 두 파일을 나열한 확인 창 → Esc는 아무것도 닫지 않음, 저장된 탭 닫기는 묻지 않음, 모두 닫기 + Enter는 저장 후 닫음, Ctrl+Shift+T 순서, 사용할 수 없는 항목 건너뛰기.

**화면**(`crates/pod-app/qa/editor-tabs/menu-overflow/`, 장면 `--fixture-scene editor-tabs-overflow|editor-tabs-menu|editor-tabs-close`, 보조 모니터 150 %, DPI 인식 캡처, 직접 확인, 어둡게·밝게): `*-editor-tabs-overflow.png` — 탭 9개가 넘쳐 활성 탭 `mod.rs · podd/src`까지 스크롤, 오른쪽 끝 `⌄` 단추; `*-editor-tabs-menu.png` — Shift+F10으로 연 탭 메뉴(구분선 3개, `Ctrl+W` 표시); `*-editor-tabs-close.png` — 모두 닫기에서 저장 안 된 `src/lib.rs`, `crates/podd/src/mod.rs`를 나열한 확인 창. `*.tree.txt` UIA 덤프에 `메뉴 탭 메뉴: mod.rs`, `메뉴 항목 …`, `탭 항목 mod.rs — crates/podd/src/mod.rs`, `단추 열린 파일 모두 보기`. 로그 `clippy.txt`, `tests.txt`, `deny.txt`.

**남은 일**: 끌어 놓기는 마우스 실제 입력으로만 확인(GPUI 테스트에서 끌기 이벤트를 흉내 내지 않음; 규칙은 `drop_index` 단위 테스트), 다른 파일 패널(작업)로 탭 옮기기, 파일마다 Dock 탭(§7.2), D4에서 탭 줄 재디자인.

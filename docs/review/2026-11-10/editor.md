# 평가: 편집기 · LSP · diff/리뷰 · PR · 터미널 뷰 (2026-11-10)

대상: master 47dff8a. 범위: `crates/pod-editor`(트리·퀵오픈·에디터·탭(tabs2 포함)·자체 LSP 클라이언트·스니펫·applyEdit), `crates/pod-diff`(eol/autocrlf 포함), `pod-app` 리뷰 패널·전달(`review.rs`), `crates/pod-forge` + `task.pr.*` + PR 탭(`pr_panel.rs`), `crates/pod-terminal-view`(링크·넓은 글자·이모지 맞춤). 문서 05·06·13·15.

방법: 코드·문서·테스트를 읽고, 의심 가는 곳은 직접 확인했다. 실행한 것: `cargo test -p pod-forge`(격리 target, jobs 2) → **26개 통과**. 실제 `git cat-file --batch`로 공백 경로 응답 확인(editor-09). pod-editor·pod-diff 테스트는 gpui 전체 빌드가 필요해 **돌리지 않았다**(자원 규칙). 제품 코드는 바꾸지 않았다.

## 1. 점수표

| 기준 | 점수 | 근거 |
|---|---|---|
| 정확성(버그 없음) | 6 | 핵심 경로(전체 문맥 diff, CRLF 정리, UTF-16 변환, 탭 규칙)는 정확하다. 다만 비 UTF-8 파일 손상, WorkspaceEdit 중복 적용, 같은 위치 삽입 순서 뒤집힘이 확인됐다 |
| 견고성(오류·경합·재시작) | 6 | Job Object, 세대 번호, 백오프, 2초 정중 종료는 잘 돼 있다. 하지만 재시작 횟수가 성공 뒤에도 0으로 돌아가지 않고, 디스크를 확인하지 않고 저장하며, cat-file 응답을 잘못 파싱할 수 있다 |
| 보안·개인정보 | 4 | 저장소 안의 `.pod/lsp.json`이 임의 명령을 실행한다(에이전트가 쓸 수 있는 파일). URL 열기는 http/https만 허용해 안전하다 |
| 성능·자원 | 7 | 2MB/16MB 상한, 증분 didChange, cat-file 한 번, LRU 서버 상한은 좋다. 대신 감시기 배치가 UI 스레드에서 (경로 수 × 색인 크기)만큼 돈다 |
| 테스트 품질 | 7 | eol 시나리오를 `git diff --numstat`과 대조하고, 가짜 LSP로 수명 주기와 탭↔didOpen/didClose 개수를 확인하는 등 결함을 잡는 테스트다. 하지만 아래 결함 대부분(중복 편집, 같은 위치 삽입, 비 UTF-8, 워크트리 감시)은 테스트가 없다 |
| 코드 구조·유지보수성 | 6 | GUI 없는 핵심(tabs, text, eol, fuzzy, forge)은 잘 분리돼 있다. 반면 `ui/workspace.rs` 3169줄과 `lsp/store.rs` 1369줄이 너무 커서, 정렬·편집 적용 로직이 두 곳(`apply_edits`/`edit_buffer`)에 중복된다 |
| 문서 정확성 | 7 | 대체로 코드와 맞고 남은 일도 솔직하게 적었다. 다만 watch.rs의 ".git/index·HEAD 변경 보고"와 text.rs의 "never silently rewritten"은 실제 동작과 다르다(editor-07, 08) |
| 사용성·접근성 | 8 | 키보드만으로 모든 탭 조작이 되고, UIA 이름·포커스 복귀·확인 창·PR 상태 모양+색을 갖췄다. 리뷰 "보내기"의 대체 동작은 예상 밖이다(editor-11) |

**종합: 6.2 / 10** — 가중치: 정확성 20, 견고성 15, 보안 15, 테스트 15, 성능 10, 구조 10, 사용성 10, 문서 5 (합 100).
계산: (6·20 + 6·15 + 4·15 + 7·15 + 7·10 + 6·10 + 8·10 + 7·5) / 100 = 6.2.

## 2. 발견 사항 (확인됨)

개수: 치명 0 · 높음 2 · 중간 5 · 낮음 7.

| id | 심각도 | 제목 | 근거 | 영향 | 수정 제안 | 크기 |
|---|---|---|---|---|---|---|
| editor-01 | 높음 | 저장소 `.pod/lsp.json`이 임의 명령 실행 | `lsp/registry.rs:172` 워크트리 `.pod/lsp.json`을 사용자 파일 뒤에 병합한다(같은 id는 통째로 교체, `command`·`args` 포함). `check_available`(`registry.rs:247`)이 파일을 활성화할 때 `probe_args`로 바로 실행한다. 신뢰 확인은 없다 | 에이전트(샌드박스 안 Codex 포함)나 클론한 저장소가 `{"id":"rust-analyzer","command":"cmd.exe","args":["/c",…]}`를 쓰면, 사용자가 .rs 파일을 여는 순간 **샌드박스 밖 UI 프로세스 권한으로** 실행된다. orch-dogfood-fixes의 "Codex 좁은 샌드박스" 작업을 우회하는 경로다(겹침 주의) | 저장소 파일에서는 `command`/`args`/`probe_args`를 무시하고 `initialization_options`·`enabled`·루트 규칙만 받는다. 명령을 바꾸려면 사용자 `%APPDATA%\Pod\lsp.json`만 쓰거나, 내용 해시 기반 "이 저장소 설정 신뢰" 확인을 거친다 | M |
| editor-02 | 높음 | 비 UTF-8 파일을 깨진 채 편집·저장 | `text.rs:119` 잘못된 UTF-8을 `from_utf8_lossy`로 바꾼다. `Loaded`에는 손실 표시가 없어 편집 가능한 상태로 열린다. `save_buffer`(`ui/workspace.rs:1208`)는 U+FFFD를 그대로 쓴다 | CP949/EUC-KR·Latin-1 파일에서 한 글자만 고쳐 저장해도 파일의 비 ASCII 글자가 모두 영구히 손상된다. 경고는 없다. 이름 바꾸기·코드 액션이 다른 파일을 탭으로 열어 고칠 때도 같다 | `decode`가 `lossy: bool`을 돌려주고, 손실이 있으면 읽기 전용 + 배너("UTF-8이 아님")를 띄운다. 나중에 인코딩 감지/선택을 둔다. 테스트: 잘못된 바이트 → 읽기 전용 | S |
| editor-03 | 중간 | WorkspaceEdit의 `changes`와 `documentChanges`를 둘 다 적용 | `lsp/store.rs:1120`, `:1125` 두 필드의 편집을 같은 파일 목록에 모두 넣는다. LSP 규격은 `documentChanges`가 있으면 그것만 쓰라고 한다. 클라이언트는 `documentChanges: true`를 알린다(`server.rs` capabilities) | 두 필드를 함께 보내는 서버에서 이름 바꾸기·코드 액션·applyEdit가 같은 편집을 두 번 적용해 텍스트가 깨진다 | `if let Some(dc) = document_changes { … } else if let Some(changes) = … { … }`. 두 필드를 다 넣은 단위 테스트를 추가한다 | S |
| editor-04 | 중간 | 같은 위치 삽입 여러 개의 순서가 뒤집힘 | `lsp/store.rs:1255`(`apply_edits`)와 `ui/workspace.rs:415`(`edit_buffer`) 모두 `sort_by_key(Reverse(start))`인데, 안정 정렬이라 같은 시작점의 원래 순서 A,B가 유지된 채 A, B 순으로 같은 offset에 넣는다. 결과는 "BA" | LSP 규격("같은 위치 삽입은 배열 순서대로")을 어긴다. 자동 import 여러 줄이나 코드 액션이 같은 위치에 넣는 편집의 순서가 바뀐다(예: `use` 줄 순서, 괄호 쌍) | 정렬 키에 원래 인덱스를 넣어 `(start desc, index desc)`로 적용하고, 두 구현을 하나로 합친다. 테스트: 같은 위치 삽입 2개 | S |
| editor-05 | 중간 | 재시작 횟수가 정상 실행 뒤에도 초기화되지 않음 | `restarts`는 `restart()`(:650)와 `resume_slot`(:718)에서만 0이 된다. `on_initialized`가 성공하는 곳(`store.rs:508`)에서는 그대로다 | 몇 시간 간격의 크래시 5번(긴 세션의 rust-analyzer라면 흔하다)으로 서버가 영구히 `Failed`가 되고, 백오프는 이미 16초 단계에 있다 | 초기화가 성공하고 일정 시간(예: 60초) 살아 있으면 `restarts = 0`. 테스트: 크래시 → 성공 → 크래시가 1번째 백오프인지 | S |
| editor-06 | 중간 | 저장 시 디스크 변경을 확인하지 않음(유실 경합) | `save_buffer`(`ui/workspace.rs:1203-1208`)는 `disk` 상태나 수정 시각을 보지 않고 `fs::write`한다. 외부 변경은 감시기 250ms 디바운스 뒤에야 `check_disk`가 안다. 쓰기는 원자적이지 않다(잘라낸 뒤 씀) | 에이전트가 파일을 쓴 직후(≤250ms + 배치 처리) 사용자가 Ctrl+S를 누르면 에이전트 변경이 경고 없이 덮인다. 쓰는 중에 에이전트나 빌드가 반쯤 쓴 파일을 읽을 수도 있다 | 열거나 저장할 때 (mtime, 길이, 해시)를 기억해 두고, 저장 직전 디스크와 다르면 기존 배너("디스크에서 바뀜")로 묻는다. 임시 파일 + rename으로 원자적으로 쓴다 | S–M |
| editor-10 | 중간 | 감시기 배치 처리가 UI 스레드에서 O(경로×색인) | `ui/workspace.rs:1446` 바뀐 경로마다 `files.iter().any(...)`(색인 상한 50만)과 `starts_with` 전체 스캔, `p.is_file()`/`exists()` 시스템 호출을 한다. 감시기는 루트 `.gitignore`만 본다(`watch.rs` 59-61: 하위 `.gitignore`, `info/exclude`, 전역 제외 무시) | 하위 `.gitignore`로만 제외되는 빌드 출력(모노레포의 `frontend/dist` 등)이 수천 개 이벤트를 내면 UI가 멈출 수 있다. **멈추는 시간은 추정**(코드 경로만 확인) | 색인을 `HashSet`(+ 폴더 접두사 집합)으로 두고, 배치 분류는 백그라운드에서 한다. 감시기는 `ignore`의 중첩 규칙(`WalkBuilder`와 같은 매처)을 쓴다 | M |
| editor-07 | 낮음 | 워크트리에서는 git 메타데이터 변경을 감시하지 못함(문서 불일치) | `watch.rs:34`는 루트 아래 `.git/` **디렉터리**만 본다. Pod 작업은 `git worktree add`(podd `git.rs:263`)라서 `.git`이 파일이고, index/HEAD는 본 저장소 `.git/worktrees/<이름>/`에 있다. `git_changed`는 workspace.rs에서 쓰이지도 않는다. 문서 13 §1의 "`.git/index·HEAD 변경 보고`"와 다르다 | 에이전트가 파일을 바꾸지 않고 `git commit`/`checkout`만 하면 트리의 M/U 표시와 상태 표시줄 변경 수가 다음 파일 변경 때까지 틀린 채 남는다 | `git rev-parse --git-dir`로 실제 gitdir(그리고 common dir의 refs)을 따로 감시하거나, 포커스·작업 상태 이벤트 때 `refresh_git`을 부른다. 문서를 고친다 | S |
| editor-08 | 낮음 | 줄 끝이 섞인 파일은 저장할 때 한쪽으로 통일됨 | `text.rs` `detect_line_ending`은 다수결, `to_lf`는 전부 LF, `from_lf`는 전부 하나로 쓴다. 모듈 주석(`text.rs:6` "never silently rewritten")·문서 13 §1과 다르다 | 줄 끝이 섞인 파일에서 한 줄만 고쳐도 소수 쪽 줄 끝이 모두 바뀐다. 리뷰 diff와 에이전트 커밋에 잡음이 생기고, eol만 바뀐 파일로 표시된다 | 섞인 파일은 읽기 전에 경고하고 상태 표시줄에 `Mixed`를 띄운다. 또는 줄별 원래 끝을 기억했다가(편집되지 않은 줄) 되살린다. 최소한 주석·문서를 고친다 | S–M |
| editor-09 | 낮음 | `cat-file --batch` 응답을 공백 기준으로 파싱 | `pod-diff/src/model.rs:189` 세 번째 공백 토큰을 크기로 읽는다. 실제 git 확인: `printf 'HEAD:a b 12\n' \| git cat-file --batch` → `HEAD:a b 12 missing` → 크기 12로 잘못 읽고 다음 응답까지 먹어 버린다. 인덱스 조회(`:path`, 추적 안 된 CRLF 파일)는 "missing"이 흔하다 | 세 번째 단어가 숫자인 공백 경로(예: `My Song 2 3.txt`)가 있으면 리뷰 diff를 불러오지 못하거나, 뒤 파일 내용이 어긋날 수 있다 | 마지막 토큰이 `missing`/`ambiguous`인지 먼저 보거나, `--batch='%(objecttype) %(objectsize)'` 형식을 쓴다. 테스트: 공백·숫자 경로 | S |
| editor-11 | 낮음 | 리뷰 "보내기"가 선택하지 않은 코멘트를 보낼 수 있음 | `pod-app/src/review.rs:236` 체크한 코멘트가 모두 해결됨이면 `None` → `all_open: true` | 해결된 코멘트 하나만 체크하고 보내면 체크하지 않은 열린 코멘트가 **전부** 에이전트로 간다 | 체크된 것이 있으면 `Some(빈 목록)`으로 두고 "보낼 수 있는 코멘트 없음"이라고 거절한다. 아무것도 체크하지 않았을 때만 전부 보낸다 | S |
| editor-12 | 낮음 | 터미널이 폭 0 문자(ZWJ, VS16, 결합 문자)를 버림 | `pod-terminal-view/src/vt/alacritty.rs:196` `CellSnap{c: cell.c}`만 복사하고 alacritty의 `zerowidth()`는 쓰지 않는다(`grep zerowidth` 결과 없음) | `⚠️`·`✔️`(VS16)가 텍스트 모양으로, 가족·피부색 이모지는 첫 글자만, NFD 한글(macOS 파일 이름)은 자모로 풀려 보인다. 에이전트 CLI 출력에서 자주 나온다 | `CellSnap`에 `extra: SmallVec<char>`를 두고 세그먼트 텍스트에 이어 붙여 함께 모양을 잡는다 | M |
| editor-13 | 낮음 | 원격 URL의 자격 증명이 상태 JSON·오류 문구로 나감 | `pod-forge/src/lib.rs:70` `ForgeRemote.url`은 "설정 그대로", `TaskPrInfo.remote`로 직렬화된다. `remote.rs:82` 오류 메시지에 URL 원문이 들어간다. 테스트 `https://user:tok@github.com/o/r.git/`처럼 토큰이 든 URL이 그대로 남는다 | `pod pr status --json`·오류 토스트·에이전트에게 붙여 넣은 로그에 토큰이 노출된다 | 직렬화·메시지 전에 `userinfo`를 `***`로 가린다(`redact_url`) | S |
| editor-14 | 낮음 | 트리 파일 조작의 `.git` 차단이 대소문자를 구분 | `ui/workspace.rs:2977` `c.as_os_str() == ".git"`. NTFS는 대소문자를 구분하지 않는다 | 트리 "새 파일"에 `.GIT/hooks/pre-commit`처럼 쓰면 `.git` 안에 파일을 만들 수 있다(사용자 조작 한정, 문서 13 §7 "`.git` 금지"와 다르다) | `eq_ignore_ascii_case(".git")`. 테스트 목록(`workspace.rs:3148`)에 `.GIT/x`를 추가한다 | S |

### 추정(확인 안 됨, 따로 둠)

- **LSP 요청/종료 경합**: `client.rs:81` `is_alive()` 확인과 `pending.insert` 사이에 읽기 스레드가 `alive=false` → `drain`을 하면, 늦게 넣은 요청이 영원히 대기할 수 있다. 쓰기가 먼저 실패하면 빠지므로 드물다. 해결: `drain` 뒤에도 `alive`를 다시 확인하거나, `pending`을 잠근 상태에서 `alive`를 읽는다.
- **Job Object 배정 전 손자 프로세스**: `job.rs:46` spawn 뒤에 배정하므로, 그 사이에 rustup 프록시가 자식을 띄우면 자식이 작업 밖에 남는다. `CREATE_SUSPENDED` + 배정 + 재개, 또는 `PROC_THREAD_ATTRIBUTE_JOB_LIST`를 쓴다.
- **forge 러너 파이프 대기**: `runner.rs:168` 프로세스가 끝난 뒤의 `join()`에는 시간 제한이 없다. 손자(자격 증명 도우미 등)가 stdout을 쥐고 있으면 podd의 `pr_ops` 뮤텍스가 무기한 묶일 수 있다. 문서 15 §8-9의 "자식 프로세스가 남는다"와 같은 원인이다.
- **진단 버전 무시**: `publishDiagnostics.version`을 보지 않고 현재 텍스트로 UTF-16 → 문자 변환을 하므로, 빠르게 입력하는 중에 utf-16 서버(pyright/tsserver)의 밑줄이 어긋날 수 있다.
- `check_available` 시간 초과 때 프로브 프로세스의 직접 자식만 죽인다.

## 3. 잘 된 점

- **eol/autocrlf**: git 규칙(text/eol/crlf, safer autocrlf, 이진 판정)을 옮겨 오고, 11개 시나리오를 실제 `git diff --numstat`과 대조하는 테스트가 있다. 줄 번호가 보존되는 것까지 확인한다.
- **LSP 수명 관리**: 서버마다 Job Object를 두고, 세대 번호로 이전 프로세스의 이벤트를 무시한다. 백오프, 2초 정중 종료 후 강제 종료, LRU 상한·일시 중지, 활성화할 때 didOpen을 보낸다.
- 위치 인코딩 협상(utf-32 우선)과 경계 변환, 증분 sync의 저장본이 어긋나면 전체 텍스트로 복구한다. 가짜 서버로 didOpen/didClose 개수 불변식을 테스트한다.
- **탭 모델**(`tabs.rs`)이 GUI 없이 단위 테스트되고, 키보드·UIA 접근성이 꼼꼼하다.
- **pod-forge**: gh CLI에 인증을 맡기고, 오류를 종류별로 나누고, 녹화 픽스처와 가짜 forge를 둔다. https만 브라우저로 열고, 원격이 없으면 아무것도 만들지 않는다. `cargo test -p pod-forge` 26/26 통과.

## 4. 우선 수정 제안

1. **editor-01**: 저장소 `.pod/lsp.json`에서 실행 관련 필드(`command`/`args`/`probe_args`)를 막는다. 보안 경계라 가장 먼저 고친다. orch-dogfood-fixes의 샌드박스 작업과 함께 검토한다.
2. **editor-02 + editor-08**: 텍스트 불러오기를 "손실 없는 왕복"으로 만든다. 비 UTF-8 → 읽기 전용, 섞인 줄 끝 → 경고/보존. 왕복 테스트(바이트 그대로)를 추가한다.
3. **editor-03 + editor-04**: WorkspaceEdit 적용을 한 함수로 합치고, `documentChanges` 우선 + 같은 위치 삽입 순서를 지킨다. 단위 테스트 2개를 추가한다.
4. **editor-06**: 저장 전 디스크 확인(mtime/해시)과 원자적 쓰기. 에이전트가 같은 파일을 만지는 Pod의 핵심 상황이다.
5. **editor-05 + editor-10**: 재시작 횟수를 초기화하고, 감시기 배치를 HashSet으로 처리하면서 백그라운드로 옮긴다. 작고 효과가 크다.

## 5. 수정 결과 (fix-editor, 2026-11-10)

브랜치 `SJin-Choi/fix-editor`. 모든 항목에 회귀 테스트를 붙였다.

| id | 상태 | 무엇을 바꿨나 |
|---|---|---|
| editor-01 | 해결 (3779aef) | 저장소 `.pod/lsp.json`은 `(저장소 = git common dir, 파일 SHA-256)` 승인 전에는 `"enabled": false`만 적용한다. 편집기 배너가 실행될 명령 전체를 보여 주고 승인을 받는다(`<사용자 설정>\lsp-trust.json`, `POD_EDITOR_CONFIG_DIR`). 파일이 바뀌면 다시 묻는다. 테스트 `repo_lsp_json_needs_content_approval` |
| editor-02 | 해결 (589e6c0) | UTF-8이 아니면 읽기 전용 + 배너, 저장·LSP 편집 거절. EUC-KR(CP949)·Shift_JIS·Windows-1252로 다시 열기(왕복해 같은 바이트일 때만 편집 가능, 표현 못 하는 글자는 저장 거절). 테스트 `non_utf8_is_flagged_and_reopens_in_an_encoding`, `save_ui::non_utf8_file_is_read_only_until_reopened_in_its_encoding` |
| editor-03 | 해결 (4757450) | `documentChanges`가 있으면 `changes`는 무시. 테스트 `document_changes_win_over_changes` |
| editor-04 | 해결 (4757450) | `back_to_front`(시작 위치 내림차순, 같은 위치는 뒤 항목 먼저) 하나로 `apply_edits`·`edit_buffer`가 같은 순서를 쓴다. 테스트 `inserts_at_the_same_spot_keep_their_order` |
| editor-05 | 해결 (4757450) | initialize 뒤 60초 이상 살아 있던 서버가 죽으면 재시작 횟수를 0부터 센다. 테스트 `crash_count_resets_after_a_stable_run` |
| editor-06 | 해결 (589e6c0) | 읽거나 쓴 바이트의 스탬프(길이·해시)를 저장 직전 디스크와 비교, 다르면 쓰지 않고 배너(불러오기 / 차이 보기 / 덮어써서 저장). 쓰기는 임시 파일 + 이름 바꾸기. 테스트 `save_ui::save_does_not_overwrite_an_unseen_disk_change`, `atomic_write_and_stamps` |
| editor-07 | 해결 (224ff5a) | 연결 워크트리는 본 저장소 쪽 gitdir(index·HEAD)과 common dir의 refs·packed-refs를 따로 감시한다. 문서 13 수정. 테스트 `linked_worktree_git_meta`, `linked_worktree_commit_is_reported`(실제 `git worktree add`) |
| editor-08 | 해결 (589e6c0) | 줄 끝이 섞인 파일은 편집하지 않은 줄의 줄 끝을 그대로 둔다(`similar`로 줄 대응). 상태 표시줄 `혼합 (CRLF n · LF m)`. 모듈 주석·문서 수정. 테스트 `mixed_line_endings_survive_an_edit` |
| editor-09 | 해결 (67eea84) | `<sha> <type> <size>` 모양일 때만 blob 헤더로 읽는다. 테스트 `cat_file_missing_path_with_spaces_and_numbers`(실제 git) |
| editor-10 | 해결 (224ff5a) | 색인에 `HashSet`(파일·폴더) 조회표, 구조 변경 판단은 백그라운드. 감시기는 중첩 `.gitignore`·`info/exclude`·전역 제외를 색인 walk와 같이 적용. 테스트 `structural_batches`, `nested_gitignore_and_exclude`. 멈춤 시간은 재지 않았다(성능 측정 금지 규칙) |
| editor-11 | 해결 (14acdb5) | 체크한 코멘트가 모두 해결됨이면 `Some(빈 목록)` → 보내지 않고 경고. 아무것도 체크하지 않았을 때만 전부. 테스트 `selection_or_all_open` |
| editor-12 | 해결 (487c899) | `Snapshot.zerowidth`·`RowText.zerowidth`로 폭 0 문자를 옮기고, 그런 셀은 한 덩어리로 모양 잡아 1/2칸에 맞춘다. 복사에도 포함. 테스트 `zero_width_chars_are_kept`, `segments_keep_zero_width_marks_with_their_cell`. 남은 점: ZWJ 뒤 이모지는 alacritty가 따로 칸을 잡아 가족 이모지는 여전히 여러 칸 |
| editor-13 | 해결 (de86c77) | `redact_url`/`redact_urls_in`: `ForgeRemote.url`, 지원 안 되는 원격 문구, gh/git stderr 분류 문구에서 userinfo를 `***`로. 테스트 `credentials_are_redacted` |
| editor-14 | 해결 (fbda241) | `.git` 비교를 대소문자 무시 + 끝 점/공백 제거 + 8.3 이름(`GIT~1`)까지. 테스트 `tree_names_stay_inside_the_root` |

### 추정 항목 확인 결과

| 항목 | 결과 | 커밋 |
|---|---|---|
| LSP 요청/종료 경합 | 실제(코드상 가능). `alive`를 `pending` 잠금 안에서 읽고, 읽기 스레드도 같은 잠금 안에서 `alive=false` 후 비운다. 테스트 `requests_racing_the_exit_all_finish` | 해결 (c60798b) |
| Job Object 배정 전 손자 프로세스 | 실제. 서버와 프로브를 `CREATE_SUSPENDED`로 띄워 작업에 넣은 뒤 재개(`job::spawn_in_tree`). 테스트 `spawn_in_tree_runs_inside_the_job` | 해결 (c60798b) |
| forge 러너 파이프 대기 | 실제(`start /b`로 재현). 프로세스가 끝난 뒤 파이프는 2초까지만 기다리고 그때까지 읽은 출력을 쓴다. 테스트 `inherited_pipes_do_not_block_after_exit`. 프로세스 트리·출력 크기 제한은 공용 "제한된 자식 프로세스 실행" 도우미(fix-orch, security-02·03)에서 | 해결 (963237d) |
| 진단 버전 무시 | 실제(UTF-8/16 서버만). 열린 문서와 버전이 다른 진단은 변환하지 않고 버린다(서버가 새 버전으로 다시 보냄). 테스트 `stale_diagnostics_are_skipped_for_utf16` | 해결 (09c68f0) |
| `check_available` 시간 초과 때 직접 자식만 종료 | 실제. 프로브도 자기 Job Object에서 돌고 시간 초과 때 트리 전체를 끝낸다. stdout은 읽지 않으므로 null로 | 해결 (c60798b) |

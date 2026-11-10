# Pod UI·에디터 재평가 — 2026-11-10

대상: master와 작업 브랜치의 공통 HEAD **`813589e`**. 최초 평가 `47dff8a`의 [ui.md](ui.md)·[editor.md]와 이후 수정·보류 기록을 코드, 테스트의 assertion, 저장된 PNG/UIA 증거에 대조했다. 제품 코드는 변경하지 않았다.

**범위 종합 6.40 → 7.35 / 10**. UI 6.60 → 7.45, 에디터 6.20 → 7.25. 기존 번호 33건은 **해결 확인 30 · 부분 3 · 미해결 0 · 회귀 0**이다. 이는 기존 결함의 판정이며, 후속 수정에서 발견한 별개 회귀는 아래 표에 포함한다. 현재 확인된 문제는 **치명 0 · 높음 1 · 중간 6 · 낮음 2**. 전체 6개 영역의 최초 평균 **6.71을 새 전체 점수로 대체하는 평가는 아니다**.

## 1. 점수표

기존 두 보고서의 기준과 가중치를 각각 그대로 적용한 뒤 영역별 종합을 1:1 평균했다. 가중치 순서나 영역 비중을 바꾸지 않았다. 각 점수는 0–10.

| 기준 | UI 가중치 | UI 이전 → 현재 | 에디터 가중치 | 에디터 이전 → 현재 | 현재 판단 근거 |
|---|---:|---:|---:|---:|---|
| 정확성(버그 없음) | 20% | 7 → 8 | 20% | 6 → 8 | 잘못된 에이전트 이름·집계, 손실 인코딩 저장, 중복 LSP 편집은 수정됐다. 긴 선택 목록·본문 생략·구버전 kill 응답은 남는다. |
| 견고성(오류·경합·재시작) | 10% | 7 → 7 | 15% | 6 → 7 | 설정 백업·서버 안정 실행 후 백오프 초기화·저장 직전 충돌 검사가 추가됐다. 비원자적 저장 fallback과 프로브 파이프 대기가 감점이다. |
| 보안·개인정보 | 5% | 8 → 8 | 15% | 4 → 6 | LSP 내용 승인, 원격 URL userinfo 마스킹, `.git` 이름 우회 차단은 유효하다. 새 `orch commit`의 필터 실행 경계가 불완전하다. |
| 성능·자원 | 10% | 8 → 8 | 10% | 7 → 7 | 가상화 유지, 색인 HashSet·백그라운드 분류와 bounded runner는 개선. 프로브는 모든 경로에 실행·출력 상한이 있지 않다. 성능 수치는 측정하지 않았다. |
| 테스트 품질(결함 검출·불안정성) | 15% | 6 → 7 | 15% | 7 → 8 | 선택 상태·최소 폭·저장 충돌·인코딩·LSP 순서·워크트리 감시의 회귀 assertion이 생겼다. 긴 select, rename 실패, v18 kill 및 캡처 환경 차이는 놓친다. |
| 코드 구조·유지보수성 | 10% | 6 → 6.5 | 10% | 6 → 6.5 | 공통 컨트롤·정렬·러너 분리는 좋다. 큰 workspace/store와 서로 다른 자식 프로세스 수명 관리가 여전히 수정 누락을 만든다. |
| 문서 정확성(문서↔코드) | 10% | 6 → 7.5 | 5% | 7 → 7 | D4/D5·P2 설명과 인코딩/감시 문서는 개선됐다. 원자성·실행 제한·진단 버전 및 시각 검증의 완결성을 과장하는 부분은 남는다. |
| 사용성·접근성 | 20% | 6 → 7.5 | 10% | 8 → 8 | OS 제목·선택/펼침·상태 접근 이름, 알림·설정·의견 서랍이 개선됐다. 긴 목록의 보이지 않는 키보드 선택과 긴 의견 토큰 생략은 새 장애다. |
| **영역 종합** | **100%** | **6.60 → 7.45** | **100%** | **6.20 → 7.25** | **통합 = (7.45 + 7.25) / 2 = 7.35** |

### 검증 범위와 실행 결과

- 앱·실제 데몬·실제 Claude/Codex를 실행하지 않았다. 사용자 Pod/에이전트 설정, GitHub, 네트워크 쓰기를 사용하지 않았다. 아래에서 “테스트 확인”은 따로 실행 결과를 적지 않은 경우 **테스트 소스와 assertion 검토**다. GUI/GPUI 의존 크레이트의 대규모 빌드는 하지 않았다.
- `cargo test -p pod-forge bounded::tests -- --test-threads=1`: **7 passed, 0 failed, 30 filtered out**, 테스트 7.61초. 호출 전 `CARGO_TARGET_DIR` 제거, 지정 sccache, jobs=2, `cpu-gate.ps1 -MaxPct 70 -MaxCargo 1`, 여유 RAM ≥8GB를 확인했다(사전 확인 32.05GB).
- `python -I -B tools/visual/contrast.py`: **12648/12648 AA pairs pass across 16 styles**.
- 기준 manifest·PNG·tree는 각각 **36개**, 가장 짧은 tree도 **75행**이다. `dark-settings.png`, `dark-editor.png`, `qa/review/after-800-drawer-open.png`를 직접 보았다. **현재 앱을 다시 캡처하거나 36장 compare를 실행한 결과는 아니다.** 과거 보고서의 `0/36 실패`를 이번 실행 결과로 인용하지 않는다.
- 임시 루트 `%TEMP%\pod-rescore-probes-dcz_ll70`에서 원본 `write_atomic` 함수만 추출한 Rust 하네스: 삭제 공유 없이 읽기 핸들을 연 파일에 저장 → **`write_atomic=Ok(())`, 내용=new, `temporary_file_left=true`**.
- 같은 임시 루트의 독립 Git 저장소에서 사용자/system Git 설정을 배제하고, `filter.repo.clean = sh ./clean.sh`, `payload.txt filter=repo`를 설정했다. 제품과 같은 `-c core.hooksPath=<빈 폴더> -c core.fsmonitor=false add -- payload.txt` 및 `commit --no-verify`로 **저장소 밖·임시 루트 안의 표식 `filter-ran=executed`** 생성 확인. RPC 전체 실행이나 실제 샌드박스 탈출 실험은 아니다.
- 구버전 `TerminalState { Running, Exited }`의 serde 하네스에 kill 응답 상태를 넣으면 **`unknown variant 'unreachable', expected 'running' or 'exited'`**. 실제 데몬 접속 없이 응답 생산·변환 코드와 역직렬화를 대조했다.

## 2. 발견 사항 — 확인됨, 심각도순

파일 경로는 저장소 루트 기준이다. “신규”는 이번에 새로 확인한 문제이며, 모두 이번 수정에서 처음 생겼다는 뜻은 아니다. 발생 구분을 제목에 붙였다.

| id | 심각도 | 제목 | 증거 | 영향 | 구체적 수정 제안 | 크기 |
|---|---|---|---|---|---|---|
| ui-editor-01 | 높음 | **해결** (`a5c7da3`, docs/19 §18.5: plumbing 커밋·filter 드라이버 비움·필터 경로 거부·bounded, facts/diff도 차단) — **신규 기능: `pod orch commit`이 저장소의 clean 필터를 샌드박스 밖에서 실행** | `crates/podd/src/server/codex_allowance.rs:61`의 commit 허용 규칙 → `server/orch_runs.rs:1856`의 기록된 worktree → `crates/podd/src/git.rs:126`의 hooks/fsmonitor만 끈 `add`. 위 격리 재현에서 필터 스크립트가 실행됐다. | **필터가 구성된 저장소**에서 워커가 수정 가능한 `clean.sh` 등을 기존 필터 명령이 호출하면 승인 없는 daemon 권한 실행이 된다. top-level 검사와 토큰 검증은 이 실행을 막지 않는다. 필터가 없는 모든 저장소에서 자동 재현된다는 주장은 아니다. | 기본 allowance의 commit은 필터 등 실행 가능한 Git 확장에 대해 fail-closed로 처리하거나 sandbox 안에서 stage하게 한다. 필터를 지원하려면 실행 신뢰/승인을 별도로 정의한다. 저장소 스크립트를 호출하는 clean/process 필터 회귀 테스트 추가. | M |
| ui-editor-02 | 중간 | **해결 (5679c0c)**: 고유 create_new 임시 파일·실패 시 항상 삭제, rename 실패는 ReplaceFailed 오류로 원본 보존, 배너 다시 저장/그대로 덮어쓰기/닫기. 단위·UI 테스트(삭제 공유 없는 핸들). — **부분 수정 editor-06: 원자적 저장 실패를 일반 덮어쓰기로 숨김** | `crates/pod-editor/src/text.rs:385`의 PID 고정 임시 파일, `:393`의 `rename(...).or_else(fs::write)`, `:395`의 실패 시에만 정리. 원본 함수 하네스에서 fallback 성공과 임시 파일 잔존 재현. | 파일을 다른 프로세스가 삭제 공유 없이 읽는 정상 상황에서도 기존 파일을 truncate/write한다. 독자는 부분 내용을 볼 수 있고 실패 중 파일 손상 가능성이 돌아온다. 작업 폴더에 저장 내용 복제본도 남는다. | rename 실패를 저장 실패로 보고하고 원본을 보존한다. 임시 파일은 고유 이름/create_new로 만들고 모든 종료 경로에서 정리. 공유 핸들·쓰기 실패 테스트 추가. | S |
| ui-editor-03 | 중간 | **해결 (d582494)**: 모든 응답·알림을 `write_json`에서 `downgrade_json`으로 변환. v18 타입으로 list·kill 디코드 시험. — **v19 회귀: unreachable `terminal.kill` 응답은 v18로 변환하지 않음** | `crates/podd/src/server/mod.rs:547`은 **TerminalList만** 변환. `:1370` kill → `server/lifecycle.rs:546`의 `u.info()` 반환은 state=unreachable. `crates/podd/tests/unreachable.rs:198`은 옛 클라이언트의 list만 검사하고 kill은 새 클라이언트로 실행한다. 구버전 serde 거절 확인. | 구버전 앱/CLI는 제거가 실제 완료됐는데 응답 파싱 실패를 보게 된다. 재시도는 not found가 되어 재시작·업데이트 흐름을 혼란스럽게 한다. | 협상 버전에 따라 단일 `TerminalInfo` 응답도 변환. v18 hello → list → unreachable kill → 구버전 타입 디코드까지 검사한다. | S |
| ui-editor-04 | 중간 | **설정 회귀: 긴 select의 키보드 강조 위치가 화면 밖으로 사라짐** | `crates/pod-ui/src/controls.rs:280`은 8행 높이 상한, `:298`은 전 항목 생성, `:301`은 배경/aria만 변경. `:380` 및 `crates/pod-app/src/chrome.rs:1386`은 index만 바꾸며 scroll handle/scroll-to-item이 없다. 스타일 목록은 `chrome.rs:1849`에서 전체 테마를 사용한다. | 8개 초과 목록에서 End/반복 ↓ 또는 뒤쪽 현행값으로 열면 선택 후보를 볼 수 없다. Enter는 보이지 않는 테마를 적용한다. 기존 QA는 3개 터미널 글꼴만 검사한다. | 지속되는 ScrollHandle로 Open/Move 때 해당 행을 Nearest로 노출. 16개 이상 옵션에서 첫/끝 항목의 실제 bounds와 Enter 결과를 검사. Shift+Tab/blur 닫힘도 함께 검증한다. | S · **해결** (`b3d4fcf`) |
| ui-editor-05 | 중간 | **keep-all 회귀: 리뷰 본문의 긴 경로·코드 토큰을 펼쳐 볼 수 없이 생략** | `crates/pod-app/src/review.rs:2200`이 의견 본문을 `words`에 전달. `:2229`는 모든 단어에 max-width + **truncate**, 원문 tooltip/확장 동작 없음. `crates/pod-app/src/pr_panel.rs:762`의 메시지 본문도 같은 함수다. | 긴 공백 없는 경로/식별자/URL의 뒤쪽을 UI에서 읽을 수 없다. 줄바꿈 미관 개선이 실제 리뷰 정보 누락을 만든다(저장된 원문 자체는 보존). | 보통 어절만 keep-all, 컨테이너보다 긴 토큰은 줄 안에서 나누거나 선택·복사 가능한 원문 보기 제공. 좁은 의견 열에서 긴 토큰 전체를 읽는 테스트 추가. | S · **해결** (`2ab831d`) |
| ui-editor-06 | 중간 | **해결 (2aa8cc8)**: 프로브를 `pod_forge::bounded`로 실행 — 실행+drain 10초, 실행 중 크기 제한 읽기, 종료 후 0.5초 뒤 트리 kill. stderr를 잡은 손자 테스트. — **잔존: LSP 프로브 실패 뒤 stderr 읽기에 10초 제한이 없음** | `crates/pod-editor/src/lsp/registry.rs:463`은 실패 종료 뒤 `:466`에서 EOF까지 동기 `read_to_string`. 제한 검사는 `:475`의 아직 실행 중인 분기에만 있다. Job Object도 읽기가 돌아와야 drop된다. | 실패한 프로브의 자손이 stderr를 열고 있으면 검사 작업이 계속 묶인다. 실행 중 stderr가 파이프 용량을 넘으면 배출하지 않아 정상 프로그램도 timeout될 수 있다. 코드 경로 확인이며 실제 10초 초과 실행 측정은 하지 않았다. | stderr를 실행 중부터 크기 제한하며 읽고, **종료 후 drain까지 같은 deadline**을 적용. 조기 실패+stderr 상속 자손 및 stderr 폭주 테스트 추가. | M |
| ui-editor-07 | 중간 | **해결 (b16df6c)**: fixture가 쓸 때 CRLF→LF, `.gitattributes`로 fixture 소스 eol=lf, LF/CRLF 동일 바이트 테스트. 보류: `dark-editor.png` 재캡처, UIA 필수 노드 검사(tools/visual, 이 작업 범위 밖). — **부분 수정 ui-01: 시각 기준은 갱신됐지만 체크아웃에 따라 달라짐** | `crates/pod-editor/src/fixture.rs:18`의 include_str 원문 → `:64` 저장 → `:83`의 LF 문자열 삽입. 같은 HEAD로 계산한 결과 **LF 체크아웃=LF 22, CRLF 체크아웃=CRLF 19+LF 3**. `tools/visual/baseline/dark-editor.png`는 후자 표시. `tools/visual/capture.ps1:114`는 UIA tree 1회 취득, `:65` compare는 PNG만 검사. | UI 변경 없이 editor 비교가 실패한다. 빈 UIA 캡처를 성공 자료로 남기는 것도 막지 못해 접근성 검증이 약하다. 현재 저장된 36개 tree가 비었다는 뜻은 아니다. | fixture EOL을 쓰기 전에 고정하고 manifest에 입력/빌드 식별자를 기록. UIA 필수 노드 검사·제한 재시도·누락 실패 처리 추가. LF/CRLF 체크아웃에서 동일 fixture 바이트 검증. | S |
| ui-editor-08 | 낮음 | **해결 (201aa0e)**: ZWJ·피부색·국기 시퀀스를 셀을 넘는 하나의 cluster로 shaping, 전체 칸에 원래 크기로 가운데(확대 없음, 넘치면 축소). segment·배치 테스트; 화면 캡처는 미실행. — **부분 수정 editor-12: ZWJ 결합 이모지가 여러 shaping 단위로 남음** | `crates/pod-terminal-view/src/vt/alacritty.rs:243`은 zero-width를 보존하지만 `src/element.rs:699`는 wide/marks 셀마다 segment를 끝낸다(`:713`). 테스트 `vt/alacritty.rs:475`는 ZWJ 존재만 검사한다. | 결합 문자·VS16의 유실은 해결됐지만 가족 이모지 전체를 하나의 glyph로 shaping하지 못한다. “문자 보존”과 “올바른 결합 표시”는 다른 보장이다. | 셀 경계를 넘는 grapheme shaping과 칸 폭 정책을 정하고 ZWJ 가족/피부색 시퀀스의 segment·복사·화면 결과를 검사. | M |
| ui-editor-09 | 낮음 | **해결 (d94a7d3)**: version이 다른 진단은 모든 인코딩에서 버림, 테스트 기대값 수정. — **부분 수정 진단 버전: UTF-32 서버의 오래된 진단은 계속 수락** | `crates/pod-editor/src/lsp/store.rs:1719`는 `enc != Utf32`일 때만 버전 차이를 버린다. `:2117` 테스트는 UTF-32의 오래된 버전을 **버리지 않는 것**을 기대한다. | 문서가 바뀌면 줄 자체가 이동/삭제되므로 UTF-32도 과거 밑줄·문구가 현재 코드에 붙을 수 있다. 인코딩 변환 유무와 진단 신선도는 별개다. | 명시된 진단 version과 현재 version 비교는 모든 인코딩에서 적용하고, version 없는 서버 정책은 별도로 유지한다. UTF-32 테스트 기대값 수정. | S |

### 추정·미확인 — 위 개수에 포함하지 않음

- **bounded runner의 프로세스 트리 탈출**: `crates/pod-forge/src/bounded.rs:212`는 suspended 없이 spawn 후 assign, `:213`은 생성/배정 실패를 None으로 흡수하고 `:261`은 직접 자식만 kill한다. 이 오류 처리/경합 창은 코드로 확인했지만 실제 빠른 자손 탈출이나 배정 실패는 이번에 재현하지 않았다. 정상 Job 경로 7개 테스트 통과가 이 경로의 보장은 아니다. LSP의 `job.rs:19`처럼 배정 성공 전 실행을 막고 실패를 반환하는 방식으로 통일하는 것이 좋다.
- **창 활성화 시 dock UIA 누락·상태 패널 전환**: `crates/pod-app/qa/settings/README.md:10`의 기존 재현 기록은 남아 있다. 앱을 실행하지 않았으므로 현재 재현 여부/원인은 판정하지 않는다. 새 선택 상태 구현이 모든 전경 전환에서 동작한다고 보증하지 않는다.
- Narrator 실제 낭독, IME 후보 창, 125/175% 실제 래스터와 OS 고대비 전환은 이번에도 미실행. headless 상태/배치 테스트와 구분한다.

## 3. 이전 발견 전수 대조

아래 **해결 확인**은 원래 지적한 경로를 코드·assertion·저장 증거로 확인했다는 뜻이다. 미실행 테스트의 통과를 뜻하지 않는다. 경로 약어: **A**=`crates/pod-app/src`, **U**=`crates/pod-ui/src`, **E**=`crates/pod-editor/src`, **D**=`crates/pod-diff/src`, **T**=`crates/pod-terminal-view/src`, **F**=`crates/pod-forge/src`.

### UI 19건

| 이전 id | 이전 심각도 | 현재 | 한 줄 증거 |
|---|---|---|---|
| ui-01 | 높음 | **부분** | 기준 36 PNG/36 tree/manifest가 갱신됐고 mark가 보이나 EOL 의존성이 남음(ui-editor-07); 앱 재캡처는 미실행. |
| ui-02 | 높음 | 해결 확인 | `U/components.rs:1158` RadioButton+toggled, `A/chrome.rs:1795` 프리셋 동일; `qa/settings/after/dark-behavior.txt`에 TabItem/ComboBox/Toggle 실제 상태 기록. |
| ui-03 | 중간 | 해결 확인 | `A/workspace.rs:3752`가 `set_window_title`, `A/ui_review_qa.rs:161`에서 작업 제목 변화 검사; 현재 기준 tree의 Window=Pod. |
| ui-04 | 중간 | 해결 확인 | `A/layouts/review.rs:1298` identify_in, `:1221/:1610/:1698` mark_el; 저장된 drawer 캡처도 Claude/Codex로 표시. |
| ui-05 | 중간 | 해결 확인 | `A/layouts/review.rs:1821`이 작업 목록 폭을 빼고 drawer 전환; `A/review.rs:1666` Esc 포커스 복귀, `layouts/review_qa.rs:490`이 800/960/1440 bounds·키 입력 검사. |
| ui-06 | 중간 | 해결 확인 | `A/workspace.rs:4127` 폭별 계획과 오른쪽 고정, `A/ui_review_qa.rs:21`이 800×600·100/150%에서 알림/응답 bounds 검사. |
| ui-07 | 중간 | 해결 확인 | `A/workspace.rs:2854` 작업 중/작업 수를 구분, `:2841` layout status_counts 반영; 현재 기준에서 `작업 중 49`, tree에서 `작업 100개`. |
| ui-08 | 중간 | 해결 확인 | `A/workspace.rs:3508` summary.label, `:3588` aria_expanded, `:2889` Status; `U/components.rs:766` Heading, 기준 tree에도 응답 수·상태 이름 존재. |
| ui-09 | 중간 | 해결 확인 | `A/theme.rs:188` 손상 필드 복구+`:218` 원본 백업+경고; `:440` 테스트가 원본 바이트와 정상 필드 보존을 검사. |
| ui-10 | 중간 | 해결 확인 | `A/chrome.rs:731` 평평한 알림 행, `:1280` category/select/switch, `workspace.rs:4026` 상황별 단일 CTA; `orch_ui/pages.rs:429/:1250` radio 선택. 후속 select의 별개 회귀는 ui-editor-04. |
| ui-11 | 낮음 | 해결 확인 | `U/components.rs:964` BUSY_LABEL=`처리 중…`, `:1278` loading 문자열 검사. |
| ui-12 | 낮음 | 해결 확인 | `U/themes.rs:180` alpha 허용 토큰 구분, `:454` 사용자 대비 검사; `:1004` 8자리 거절/경고 테스트와 `A/chrome.rs:1609` 경고 표시. |
| ui-13 | 낮음 | 해결 확인 | `A/workspace.rs:3665` tree_key와 Nearest; `A/ui_review_qa.rs:132` 첫 ↓·End·부모 ←·Home 실제 키 경로 검사. |
| ui-14 | 낮음 | 해결 확인 | 원래 빈 화면/queue 한글 어절 문제는 `U/components.rs:781` keep_all로 해결. 후속 Review 적용의 내용 생략 회귀는 ui-editor-05. 원 심각도는 초기 보고서 커밋 `84b0af0`에서 복원. |
| ui-15 | 낮음 | 해결 확인 | `A/pr_panel.rs:35` TAB_COMMENTS=`의견`. |
| ui-16 | 낮음 | 해결 확인 | `A/chrome.rs:1602`가 agent::override_errors를 모양 notices에 포함; `A/theme.rs:426` reload 오류 검증. |
| ui-17 | 낮음 | 해결 확인 | `docs/09-pod-app.md:11/:126` P2 대체 표기, `docs/10-ui-design.md:863/:942` 현재 차이/7개 모드 명시; 밀도도 모양에 있음. |
| ui-18 | 낮음 | 해결 확인 | `A/workspace.rs:428` focus-in/out 정지, `A/ui_review_qa.rs:52` 닫기 버튼 포커스 중 시간 경과와 이탈 후 종료 검사. 원 보고서의 “hover만” 주장은 현재 코드에 해당하지 않음. |
| ui-19 | 낮음 | 해결 확인 | `git ls-files '*__pycache__*'` 결과 없음; `.gitignore:25`에서 제외. |

### 에디터 14건

| 이전 id | 이전 심각도 | 현재 | 한 줄 증거 |
|---|---|---|---|
| editor-01 | 높음 | 해결 확인 | `E/lsp/registry.rs:231` 미승인 파일은 disable만, repo+SHA-256 승인; `E/lsp/store.rs:338` 승인 직전 hash 재검사, `repo_lsp_json_needs_content_approval`이 변조 뒤 거절 검사. |
| editor-02 | 높음 | 해결 확인 | `E/text.rs:252` lossless 판정, `E/ui/workspace.rs:1623/:2415` 저장/LSP 편집 거절; `crates/pod-editor/tests/save_ui.rs:155` CP949 원본 보존과 명시 인코딩 재열기 검사. |
| editor-03 | 중간 | 해결 확인 | `E/lsp/store.rs:1680/:1702` documentChanges 우선 else-if; `:2137` 두 필드 동시 입력 테스트. |
| editor-04 | 중간 | 해결 확인 | `E/lsp/store.rs:1873` start/index 역순 공통 함수, `E/ui/workspace.rs:499` 같은 함수 사용; `store.rs:2122` 동위치 삽입 순서 검사. |
| editor-05 | 중간 | 해결 확인 | `E/lsp/store.rs:742/:875` 성공 초기화 시간과 60초 안정 실행 반영; `:2092` 안정 실행 뒤 첫 백오프 회복 검사. |
| editor-06 | 중간 | **부분** | `E/ui/workspace.rs:1641` 저장 직전 바이트 stamp 비교와 `tests/save_ui.rs:108`은 유효. `E/text.rs:393`의 비원자적 fallback은 ui-editor-02. |
| editor-07 | 낮음 | 해결 확인 | `E/watch.rs:183` gitdir/common refs 별도 watch, `:337` 실제 linked-worktree commit 이벤트 검사; `E/ui/workspace.rs:2104` git 갱신 연결. |
| editor-08 | 낮음 | 해결 확인 | `E/text.rs:330` unchanged line의 혼합 EOL 복원, `:596` 삽입/수정 후 원래 LF 보존 검사; 저장 뒤 format도 재설정(`E/ui/workspace.rs:1658`). |
| editor-09 | 낮음 | 해결 확인 | `D/model.rs:250` SHA/type/size 3필드 검증; `:535` 공백+숫자 missing 경로와 다음 blob을 실제 Git으로 검사. |
| editor-10 | 중간 | 해결 확인 | `E/index.rs:28/:55` 파일/폴더 HashSet, `E/ui/workspace.rs:2081` 백그라운드 분류; `E/watch.rs:286` 중첩 ignore·exclude/재포함 검사. |
| editor-11 | 낮음 | 해결 확인 | `A/review.rs:299` 체크했으나 모두 resolved면 Some(empty); `:1043` 송신 전 거절, `:2585` selection_or_all_open 검사. |
| editor-12 | 낮음 | **부분** | `T/vt/alacritty.rs:243`, `T/selection.rs:150`에서 zero-width 렌더/복사 보존. 셀을 넘는 ZWJ shaping은 ui-editor-08. |
| editor-13 | 낮음 | 해결 확인 | `F/remote.rs:95/:137/:151` userinfo 마스킹, `F/error.rs:80` stderr도 적용; `remote.rs:217` 이하 토큰·SSH·오류 문구 검사. |
| editor-14 | 낮음 | 해결 확인 | `E/ui/workspace.rs:4584` 대소문자·끝 점/공백·GIT~숫자 거절; `:4785` 경로 테스트에 우회 이름 포함. |

### 이전 추정·후속 관찰까지 대조

원 보고서에서 심각도가 지정되지 않은 항목은 `—`로 유지한다. 미실행/미확인 행은 확인된 제품 결함 개수에 더하지 않는다.

| 이전 항목 | 이전 심각도 | 현재 | 근거·한계 |
|---|---|---|---|
| UI 추정: 열린 알림 목록이 갱신되지 않음 | — | 해결 확인 | `A/ui_review_qa.rs:104`가 열린 동안 blocked 해제/새 요청 반영을 검사하고 현재 행 렌더가 live 목록을 사용. |
| UI 추정: sidebar ScrollStrategy::Top 점프 | — | 해결 확인 | `A/workspace.rs:3674` Nearest로 교체. |
| UI 추정: Narrator/IME 후보/실제 DPI/OS 고대비 검증 | — | 미해결 | 검증 공백. 이번에도 실행하지 않음; 제품 결함 확정 아님. |
| next-settings: category·select·switch 및 banner keep-all | — | **부분** | category·상태·단일 키 활성화는 구현/QA 기록과 일치; 긴 목록 키보드 스크롤은 ui-editor-04. banner는 `U/components.rs:797` 긴 조각 fallback이 있어 Review의 무조건 truncate와 다름. |
| next-settings: 창 활성화 뒤 dock UIA 누락 | — | 미해결 | 기존 QA 기록은 있으나 현재 재현 미확인; 위 추정 절 참조. |
| tail: keep-all 단어 사이 간격 | — | 미해결 | `A/review.rs:2222` gap_x(4) 그대로. 정보 누락과 관계없는 간격 자체는 스타일 관찰이므로 결함으로 계수하지 않음. |
| tail: editor fixture EOL 의존 | — | 미해결 | LF/CRLF 입력별 0+22 대 19+3을 독립 계산 확인(ui-editor-07). |
| tail: UIA 캡처 빈 tree 불안정 | — | **부분** | 저장된 36개 tree는 충실하지만 `capture.ps1:114` 재시도/필수 노드 검증 없음. 현재 실행의 불안정 빈도는 미측정. |
| tail: unreachable 사유 tooltip | — | 해결 확인 | `A/vm.rs:212` 사유 문구, `A/workspace.rs:4150` tooltip/접근 이름 테스트. 구버전 kill 응답 문제는 별개(ui-editor-03). |
| Editor 추정: LSP request/exit 경합 | — | 해결 확인 | `E/lsp/client.rs:63/:106` pending 잠금 아래 alive 판정/해제; `:349` 동시 요청 종료 테스트. |
| Editor 추정: LSP Job 배정 전 자손 | — | 해결 확인 | `E/lsp/job.rs:19` suspended → adopt → resume, 실패 시 child 종료; `:181` Job 소속 검사. 공용 bounded runner는 별개 추정 절 참조. |
| Editor 추정: forge 종료 뒤 파이프 join 무한 대기 | — | 해결 확인 | 현재 `F/bounded.rs:300` 이후 종료 후 drain/kill grace 적용; 이번 상속 파이프·자손 정리 테스트 통과. |
| Editor 추정: 진단 version 무시 | — | **부분** | UTF-8/16 거절은 적용됐지만 UTF-32 제외(ui-editor-09). |
| Editor 추정: check_available timeout에서 직접 자식만 kill | — | 해결 확인 | `E/lsp/registry.rs:456/:479` 프로브도 자체 Job/트리 종료. 실패 종료 후 stderr 분기는 별도 잔존 문제(ui-editor-06). |

### 요청된 신규 연결점 검토

- **holder protocol 3**: `crates/podd/src/server/terminal.rs:346`은 holder≥3에만 ACK 예산을 켜고 ≥2에만 DaemonHello를 보낸다. `crates/podd/src/holder.rs:594`도 daemon≥3 뒤에만 ACK를 켠다. 1/2 holder에 새 프레임을 무조건 보내는 회귀는 코드에서 찾지 못했다. `tests/input_budget.rs`는 큰 입력/kill을 다루지만 이번 실행 대상은 아니다.
- **unreachable/v19**: Lost로 표시해 attach를 막고 사유를 노출하는 VM 경로, 재시도/포기/ID 보존 테스트는 존재한다. 다만 구버전 응답 호환은 list만 검증되어 kill 누락을 놓쳤다(ui-editor-03).
- **orch_ui·Codex allowance**: unconfirmed wake 표시/재시도와 radio 선택은 `A/orch_ui/pages.rs:529/:587/:1250`, `qa/orch/wake/uia-selection.txt`에 연결된다. allowance는 저장소 설정을 읽어 표시하는 것으로 실제 각 워커의 설치 성공을 증명하지는 않는다. commit 토큰·열린 attempt·자기 task 경로 검사는 있으나 필터는 막지 못한다(ui-editor-01).

## 4. 잘 된 점

- 기존 고위험 두 편집기 결함인 **미승인 LSP 실행과 비 UTF-8 저장 손상**을 실제 사용 경로에서 막았다. 단순 helper 테스트에 그치지 않고 저장/편집 거절까지 검사한다.
- UI 제목·상태·라디오/탭/토글 의미가 명시되고, 최소 창·의견 서랍·알림 갱신·토스트 포커스에 구체적인 회귀 assertion이 생겼다.
- 공용 러너의 정상 트리 종료·출력 제한, HashSet 색인 조회, 원본 설정 백업, Git 원격 토큰 마스킹은 실질적인 개선이다. 시각 증거도 과거 전체 불일치 상태에서 갱신됐다.

## 5. 우선 수정 제안

1. **ui-editor-01**: allowance로 실행되는 commit의 Git 필터 경계를 먼저 닫는다. hooks/fsmonitor 비활성화 테스트에 clean/process 필터와 수정된 저장소 스크립트를 추가한다.
2. **ui-editor-02·06**: 실패를 성공으로 바꾸는 저장 fallback을 없애고, 프로브의 종료 후 출력 읽기까지 deadline/크기 제한을 적용한다.
3. **ui-editor-03**: v18의 list뿐 아니라 kill 응답을 실제 구버전 타입으로 검증한다. 프로토콜 버전별 응답 변환을 한곳으로 모은다.
4. **ui-editor-04·05**: 긴 설정 목록의 포커스 가시성과 좁은 의견 열의 원문 가독성을 복구한다. 짧은 fixture만으로 통과하는 UI 테스트를 보강한다.
5. **ui-editor-07**: 시각 fixture의 바이트를 EOL과 무관하게 고정하고, PNG 비교와 UIA 필수 노드 검증을 함께 실패 조건으로 만든다. 실제 Narrator/전경 전환 검증은 별도 미완료 항목으로 유지한다.

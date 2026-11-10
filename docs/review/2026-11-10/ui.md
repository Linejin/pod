# Pod 리뷰 — 앱 UI/UX·디자인 (2026-11-10)

대상: master `47dff8a` · 범위: `crates/pod-ui`, `crates/pod-app`(chrome·sidebar·레이아웃 7종·orch_ui·설정 › 모양·알림·Review/PR·편집기 탭), 한국어 문자열, 접근성, 시각 증거. 문서: 09, 10, 13.

**직접 확인한 것**
- `cargo build --release -p pod-app`: 통과(5분 9초, jobs 2, sccache).
- `cargo test --release -p pod-ui`: 20개 통과, 0개 실패.
- `python -I tools/visual/contrast.py`: 16개 스타일에서 AA 대비 쌍 12648/12648 통과.
- `tools/visual/capture.ps1`: 보조 모니터 150%에서 fixture 장면 36장 + 레이아웃 7종 + 라이트 + 800×600 logical 최소 창을 캡처하고, 모두 직접 보고 갤러리 목업과 비교했다(캡처는 커밋하지 않음).
- `capture.ps1 -Mode compare`를 저장된 기준 이미지에 대해 실행했다.
- 제품 코드는 바꾸지 않았다. 병렬 작업 중인 podd/pod-cli 쪽 변경과 겹치는 발견은 없다.

## 1. 점수표

| 기준 | 점수 | 근거 |
|---|---|---|
| 정확성(버그 없음) | 7 | 핵심 흐름은 안정적이다. 다만 리뷰 레이아웃이 에이전트 종류를 잘못 표시하고, 최소 창 폭에서 상태 표시줄이 잘린다. 사용자 테마의 8자리 hex 오해석, 영어 "Loading..." 같은 작은 결함도 있다. |
| 견고성(오류·경합·재시작) | 7 | 패널을 공유하고 폴링하지 않는 설계는 좋다. 하지만 `settings.json`이 손상되면 조용히 기본값으로 덮어쓴다. |
| 보안·개인정보 | 8 | SVG 덮어쓰기 검사(script·href·image 금지, 64KB), CDN·웹폰트 없음, fixture는 데몬 없이 동작한다. 사용자 테마 대비를 검사하지 않는 점은 사용성 문제다. |
| 성능·자원 | 8 | uniform_list 가상화, `cached` sidebar, 타이머 없는 갱신을 쓴다. 이번 리뷰에서 성능을 따로 측정하지는 않았다(규칙상). |
| 테스트 품질 | 6 | headless 키보드·IME·DPI 테스트가 풍부하다. 그러나 **시각 기준 이미지 36/36이 master에서 실패한다**. 접근성 상태(선택·펼침)를 검사하는 테스트도 없다. |
| 코드 구조·유지보수성 | 6 | 레이아웃 등록부와 테마 JSON 구조는 깔끔하다. 그러나 `workspace.rs` 2599줄·`blocks.rs` 1700줄 같은 거대 파일, 문자열 match로 짠 설정 행, 레이아웃마다 다른 에이전트 이름 규칙(중복 로직)이 있다. |
| 문서 정확성 | 6 | 레이아웃 QA README는 목업과의 차이를 정직하게 적었다. 반면 D4.6 사양과 구현의 차이, docs/09 §1·§5의 낡은 설명, D5.4·D5.11 문장이 코드와 맞지 않는다. |
| 사용성·접근성 | 6 | 키보드·포커스·한국어 이름은 잘 되어 있다. 하지만 선택 상태·펼침 상태·창 제목·정적 텍스트가 UIA에 없다. 알림·설정·빈 화면은 여전히 투박하다. |

**종합 6.6 / 10.** 가중치: 정확성 20, 사용성·접근성 20, 테스트 15, 견고성 10, 성능 10, 구조 10, 문서 10, 보안 5. UI 영역이라 사용성·접근성의 비중을 높였다.

**세련됨 평가**: 갤러리와 나란히 보면 칸반·블록·고밀도·대기열(06)은 목업에 가깝다. 작업대(04 quiet) sidebar도 정돈되어 있다. 투박해 보이는 곳은 다음이다.
- 알림 대화 상자: 외곽선 버튼이 반복된다.
- 설정과 새 작업 대화 상자: 선택지마다 파란 채움 버튼이라, 주요 CTA와 경쟁한다.
- 오케스트레이션 보드 머리: 파랑 채움 버튼 + 외곽선 버튼 3개.
- 빈 화면: 외곽선 카드에 버튼 두 개.
- 상태 표시줄: 항목이 너무 조밀하다.
- 한글이 단어 중간에서 줄바꿈된다.

## 2. 발견 사항 (검증됨)

| id | 심각도 | 제목 | 증거 | 영향 | 수정 제안 | 크기 |
|---|---|---|---|---|---|---|
| ui-01 | 높음 | 시각 기준 이미지가 master와 맞지 않아 회귀 검사가 무력화됨 | `capture.ps1` → `-Mode compare`: **36/36 실패**(장당 3.7k–19k px). diff 이미지는 sidebar·에이전트 트리의 에이전트 표시(mark) 자리다. 기준 이미지 갱신 `9e1443b`(07:13)가 mark 병합 `47aec88`(08:15)보다 앞선다. CI 강제도 없다. | 이후의 진짜 시각 회귀를 잡지 못하고, README의 "28/28 통과" 기록은 더 이상 사실이 아니다. | 이미지를 검토한 뒤 기준 36장과 UIA 트리를 갱신한다. 병합 체크리스트(또는 상태 커밋)에 compare를 넣고, mark처럼 화면 전체에 영향을 주는 변경은 기준 이미지 갱신을 같은 PR에서 요구한다. | S |
| ui-02 | 높음 | 설정·선택형 버튼의 "현재 선택"이 접근성 트리에 없음 | `chrome.rs:739`, `chrome.rs:882`: 선택 여부를 `.primary()` 색으로만 표시한다. UIA 덤프 `dark-settings.tree.txt`의 `단추 테마: 다크`, `단추 밀도: 테마 기본`, 그리고 `qa/native/uia/settings.uia.txt:42`의 `{focusable}`에는 선택·눌림 상태가 없다. 프리셋 카드의 `aria_selected`(Role::Button)도 UIA에 나오지 않는다. | 화면 낭독기 사용자는 현재 테마·밀도·글꼴·레이아웃을 알 수 없다. 색만으로 상태를 전달하므로 docs/10 §4.1 원칙에도 어긋난다. | 단일 선택은 RadioGroup/RadioButton(checked)이나 toggle(pressed) 역할로 바꾼다. 최소한 접근 이름에 ", 선택됨"을 넣는다. UIA 덤프에서 상태를 검사하는 테스트를 추가한다. | M |
| ui-03 | 중간 | 메인 창의 OS 제목이 비어 있음 | 모든 UIA 덤프 2행이 `Window: ""`다. `main.rs:232`가 `TitleBar::title_bar_options()`(`title: None`, `vendor/gpui-component/src/title_bar.rs:62`)를 쓰고, 어디에서도 `set_window_title`을 호출하지 않는다. | 작업 표시줄 미리보기, Alt+Tab, 낭독기의 창 이름에 이름이 없다. | 시작할 때 `"Pod"`, 작업이 바뀔 때 `"<작업> — Pod"`로 `window.set_window_title`을 호출한다. | S |
| ui-04 | 중간 | 리뷰 중심 레이아웃이 에이전트 종류를 잘못 표시함 | `layouts/review.rs:870`, `:1147`, `:1206`이 `capitalized(&t.program)`을 쓴다. 캡처 `review-editor-layout`에서 sidebar의 Claude mark가 붙은 Worker 0이 머리와 타일에 "Powershell · Worker 0"으로 나오고 mark도 없다. 칸반(`kanban.rs:185`)과 고밀도는 `agent_kind::identify`를 쓴다. | 레이아웃마다 같은 터미널의 정체가 다르게 보여 §D5.11의 목적("어떤 AI인지 한눈에")이 깨진다. | `agent_kind::identify_in` + `mark_el`로 바꾸고 이름 규칙을 한 함수로 모은다. | S |
| ui-05 | 중간 | 리뷰 레이아웃(960 logical)에서 의견 열이 잘림 | 캡처 `dark-review-editor-layout-review-review.png`: "아직 의견이 없습니다. 줄을…", 입력 자리표시자, 보내기 단추 글자가 오른쪽에서 잘린다. D4.6은 "설명·한국어·전송 버튼은 잘리지 않아야", "가용 폭 <560이면 comments drawer"라고 정한다. | 기본 150% 모니터에서 의견 쓰기와 보내기가 불완전하게 보인다. | 리뷰 레이아웃 안에서도 Review 패널의 폭 임계값(drawer 전환)을 적용한다. 레이아웃의 작업 목록 폭을 빼고 계산한다. | M |
| ui-06 | 중간 | 최소 창(800×600 logical)에서 상태 표시줄의 알림 단추가 잘림 | `-Width 1200 -Height 900` 캡처(150%)에서 오른쪽 "알림" 단추가 반쯤 잘린다. `workspace.rs:1948`이 `overflow_hidden`인데 항목을 접는 규칙이 없다. D4.6의 "좁으면 branch→완료→작업 overflow, 연결단절/응답/알림 유지"가 구현되지 않았다. | 지원하는 최소 크기에서 알림에 접근하기 어렵다. | 폭에 따라 숨길 항목의 우선순위를 정한다. 오른쪽 상태·알림은 `flex_shrink_0`으로 고정하고 앞쪽 통계부터 숨긴다. | S |
| ui-07 | 중간 | "작업 N"이 두 가지 뜻(작업 중 / 작업 수)으로 동시에 표시됨 | `strings.rs:384`의 `WORKING`과 `:387`의 `TASKS`가 둘 다 `"작업 {}"`이다. 상태 표시줄에 `작업 49 … 작업 100`(`workspace.rs:1958`, `:1962`)이 함께 보인다. 06 관제형은 제목줄 알약과 상태 표시줄에 같은 집계가 중복된다. | 핵심 집계를 오독할 수 있다. 한국어 문구 품질 문제다. | 작업 중은 `작업 중 49`, 작업 수는 `작업 100개`처럼 구분한다. queue 레이아웃에서는 상태 표시줄 집계를 생략한다. | S |
| ui-08 | 중간 | 트리의 응답·실패 수와 펼침 상태, 정적 텍스트가 접근성 트리에 없음 | `workspace.rs:2313`: 작업 항목 이름이 제목 + 에이전트뿐이고 `응답 1`·실패 수가 없다(§4.4는 "실제 수는 accessible name에서 제공"이라고 함). TreeItem에 expanded/collapsed가 없다(UIA: `Secondary Actions: Select`만 있음). 상태 표시줄의 연결·집계와 빈 화면 제목은 UIA 덤프에 아예 없다. | 낭독기로는 응답이 필요한 작업과 연결 상태를 찾을 수 없다. | 작업 aria_label에 `summary.label()`을 넣는다. expanded 속성을 내보낸다. 상태 표시줄 그룹에 Role::Status와 이름을 붙이고, 빈 화면 Frame에 heading 역할을 준다. | M |
| ui-09 | 중간 | 손상된 `settings.json`을 조용히 기본값으로 바꾸고, 다음 저장 때 덮어씀 | `theme.rs`의 `Settings::load()`가 `...ok().unwrap_or_default()`로 오류를 버린다. 이후 설정 단추 하나, sidebar 접기(←/→, `workspace.rs` keydown → `settings.save()`)만 해도 전체 파일을 기본값으로 덮어쓴다. | 사용자의 테마·글꼴·최근 항목·접힘 상태가 경고 없이 사라진다. | 파싱이 실패하면 `settings.json.bad-<시각>`으로 백업하고 토스트로 알린다. 필드 단위 `#[serde(default)]`가 있는지 점검한다. | S |
| ui-10 | 중간 | D4 "세련됨" 사양 중 투박함을 만드는 항목이 미구현(문서는 기준으로 제시) | (a) 알림: `chrome.rs:367`이 요청마다 full-width 외곽선 Button이다. 아이콘·작업·시간이 없다(캡처 `dark-notifications`). D4.1이 정확히 이 문제를 지적했다. (b) 설정과 새 작업의 선택지가 파란 채움 버튼이다(D4.6: segment/select). (c) 빈 화면이 외곽선 카드 + CTA 두 개이고, 저장소가 이미 있어도 주요 CTA가 "저장소 열기"다(`workspace.rs:2545`–`2568`, D4.6: "등록됨→새 task" 하나). | 사용자가 요청한 "투박하지 않은" 인상을 가장 많이 해치는 화면들이다. | 32px 평평한 알림 목록(아이콘 + 제목 + 시간)으로 바꾼다. 설정 행은 `pod_ui::segment`나 select로 바꾼다. 빈 화면은 상황별 CTA 하나와 테두리 없는 max360으로 만든다. | M |
| ui-11 | 낮음 | 승인 진행 중 단추에 영어 "Loading..."이 표시됨 | `pod-ui/src/components.rs:815`. 사용처: `gates.rs:216`, `layouts/queue.rs:829`, `layouts/dense.rs:1241`. | 한국어 UI에 영어가 섞인다. | `"처리 중…"` 같은 문자열을 strings에 두고 호출하는 쪽에서 주입한다. | S |
| ui-12 | 낮음 | 사용자 테마에서 8자리 `#RRGGBBAA`(scrim 외) 색을 잘못 해석함 | `themes.rs:164`의 `parse_hex`가 8자리를 받는다. 하지만 `rgb()`(gpui `color.rs:15`)는 최상위 바이트를 버려 `GGBBAA`가 RGB로 그려진다. 사용자 테마는 대비 검사도 하지 않는다(`parse_user` → `validate`만). | 사용자 테마 색이 엉뚱하게 나오고, 대비가 부족해도 경고하지 않는다. | scrim이 아닌 토큰은 6자리만 허용하거나 alpha를 거부한다. `contrast_failures`로 경고를 설정 화면에 보인다. | S |
| ui-13 | 낮음 | sidebar 키보드: 첫 ↓가 첫 행을 건너뜀, Home/End와 부모로 이동 없음 | `workspace.rs:2363`: 선택이 없으면 `unwrap_or(0)` 다음 `+1` → 두 번째 행이 선택된다. 자식에서 ←를 눌러도 부모로 가지 않는다. | 키보드 트리 탐색이 WAI-ARIA tree 관례와 어긋난다. | 선택이 없을 때 ↓는 0번 행을 선택한다. Home/End를 추가하고, 자식 행에서 ←는 부모로 이동한다. | S |
| ui-14 | 해결 | `54eccb9`, `332ba38`, `bec7490` | 어절 단위 줄바꿈(`keep_all`), 제목 18/26. Review 패널·PR 탭 문구도 어절 사이에서만 줄바꿈(`review::words`), 말줄임 경로·의견 위치는 tooltip. Review 오류 배너(`pod_ui::banner`)는 pod-ui 담당(next-settings)이 keep-all로 고친다. |
| ui-15 | 낮음 | 용어 불일치: "코멘트" 탭 vs "의견" | `pr_panel.rs:32`의 `TAB_COMMENTS = "코멘트"`와 같은 화면의 "의견 (미해결 0 · 전체 0)", 단축키 표 "의견 추가". | 같은 개념에 두 이름이 쓰인다. | "의견"으로 통일한다. | S |
| ui-16 | 낮음 | 에이전트 mark 덮어쓰기 오류가 설정에 표시되지 않음(문서 불일치) | `chrome.rs:972`는 테마 레지스트리 오류만 보여 준다. mark 오류는 `reload_themes()` 반환값에만 있어 시작 시 `eprintln`(GUI 서브시스템이라 보이지 않음)이나 "테마 다시 읽기" 토스트로만 나온다. §D5.11은 "잘못된 파일은 테마 오류와 함께 설정에 보인다"라고 한다. | 사용자가 그림 파일이 왜 무시됐는지 모른다. | mark 오류를 전역에 보관하고 설정 › 모양에 함께 표시한다. | S |
| ui-17 | 낮음 | 문서가 코드와 맞지 않음 | docs/09 §1은 sidebar 250px·"Other terminals"·어두운 테마만이라고 하고, §5-8은 "설정 화면·명령 팔레트 없음", §5-9는 "250px 고정"이라고 한다. 대체됐다는 표시가 없다. docs/10 D5.4는 "이번 단계는 작업대만 구현"이라고 한다(7개 모두 등록됨). D4.6은 "PID/version은 연결 popover"라고 하지만 `workspace.rs:1931`은 상태 표시줄에 `podd … · pid …`를 직접 표시한다. D5.3은 밀도를 모양 섹션에 둔다고 하지만 실제로는 일반 섹션에 있다(`chrome.rs:678`). | 새 기여자나 리뷰어를 오도한다. | 09 §1·§5에 "P2 당시, D2–D5로 대체됨" 머리말을 붙이고 D5.4 문장을 갱신한다. D4.6과 실제의 차이를 표로 남긴다. | S |
| ui-18 | 낮음 | 토스트가 hover에서만 멈추고 keyboard focus에서는 멈추지 않음 | `workspace.rs` `render_toasts`: `on_hover`만 `paused`를 설정한다. D4.6은 "hover·focus 중 정지"라고 한다. 닫기는 28px 아이콘이 아니라 텍스트 Button이다. | 키보드로 토스트 목록에 들어가도 시간이 지나면 토스트가 사라진다. | 토스트 목록이 focus-within일 때도 paused로 둔다. | S |
| ui-19 | 낮음 | 저장소에 `__pycache__/*.pyc`가 추적됨 | `git ls-files`에서 `tools/themes/__pycache__/gen.cpython-313.pyc`가 나온다. `contrast.py`를 한 번 실행하면 작업 트리가 dirty해진다(리뷰 중 되돌림). | 리뷰나 캡처를 실행하는 사람마다 잡음 변경이 생긴다. | 파일을 삭제하고 `.gitignore`에 `__pycache__/`를 추가한다. | S |

**심각도별 수**: 치명 0 · 높음 2 · 중간 8 · 낮음 9.

### 추정(미검증, 별도)
- 알림 대화 상자는 열 때의 `blocked` 목록을 그대로 그리므로(`chrome.rs:359`), 열려 있는 동안 상태가 바뀌어도 갱신되지 않을 것이다. 실행으로 확인하지는 않았다.
- sidebar ↑/↓의 `ScrollStrategy::Top`은 이동할 때마다 행을 맨 위로 끌어올려 목록이 튈 가능성이 있다. gpui 동작에 따라 다르며 확인하지 않았다.
- 실제 Narrator 낭독, Microsoft IME 후보 창, 실제 125/175% 래스터, OS 고대비 전환은 이번에도 실행하지 않았다(docs/10 §12의 NOT RUN 그대로).

## 3. 잘 된 점
- **테마 구조**: 스타일 × 레이아웃 두 축이다. 테마 JSON은 생성기에서 만든다. `contrast.py`가 16개 스타일 × 4개 팔레트 × 강조색까지 12648쌍 AA를 통과한다. 에이전트 mark의 3.2:1 보장은 테스트가 있다.
- **레이아웃 연결점**: `LayoutRegistry`로 모드를 한 줄로 등록한다. 패널은 dock이 계속 소유해 레이아웃을 바꿔도 재attach가 없다. 폴링하지 않는 갱신 원칙을 일관되게 지킨다.
- **접근성 기반**: 한국어 접근 이름을 일관되게 쓴다. 포커스 링이 보이고 Esc를 누르면 포커스가 원래 자리로 돌아간다. headless 네이티브 QA(Tab 순환, IME preedit, 100–175% 배치)가 있다.
- **QA 문서의 정직함**: 레이아웃별 README가 "목업과 같은 점 / 의도한 차이"를 구분해 적는다. fixture가 덮지 못하는 부분은 NOT RUN으로 남긴다.
- **목업 재현도**: 칸반·블록·고밀도·06 관제 화면은 갤러리 목업과 구조·색·밀도가 잘 맞는다.

## 4. 우선 수정 제안
1. **ui-01**: 기준 이미지 36장을 검토·갱신하고 병합 전 compare를 의무화한다. 다른 모든 시각 수정의 안전망이다.
2. **ui-02 + ui-08 + ui-03**: 선택·펼침 상태, 작업의 응답 수, 창 제목, 상태 표시줄을 접근성 트리에 노출한다. 그리고 이를 검사하는 UIA 테스트를 하나 추가한다.
3. **ui-04 + ui-05**: 리뷰 레이아웃의 에이전트 표시를 `agent_kind`로 통일하고 의견 열 drawer 임계값을 적용한다.
4. **ui-10 + ui-07 + ui-14**: 알림 목록, 설정 segment, 빈 화면 CTA, "작업" 용어, 한글 줄바꿈을 손본다. 체감 세련도를 가장 크게 올리는 묶음이다.
5. **ui-09 + ui-06**: 설정 파일 손상 시 백업과 알림을 넣고, 상태 표시줄 접기 우선순위를 구현한다.

## 5. 수정 결과 (fix-ui, 2026-11-10)

브랜치 `SJin-Choi/fix-ui`. 행은 지우지 않고 상태만 적는다.

| id | 상태 | 커밋 | 메모 |
|---|---|---|---|
| ui-01 | 해결 | `044e26a` | 36장 모두 직접 보고 다시 찍음(144 DPI). UIA 트리·manifest 함께. README에 병합 전 compare 규칙. |
| ui-02 | 해결 | `54eccb9`, `0846f9b` | 설정 행·프리셋 카드가 RadioGroup/RadioButton(checked). 실제 앱 UIA에서 그룹마다 `IsSelected` 하나 확인. |
| ui-03 | 해결 | `a738363` | 창 제목 `Pod` / `<작업> — Pod`. |
| ui-04 | 해결 | `991ade5` | `agent_kind::identify_in` + `mark_el`(타일·에이전트 줄·응답 줄). |
| ui-05 | 해결 | `991ade5`, `332ba38` | 캔버스 폭 < 910이면 파일 목록을 숨긴다. < 560(800 logical 최소 창)이면 의견 열이 Review 패널의 의견 서랍이 된다: 툴바 `의견`/Alt+C로 열고 Esc로 닫음, 포커스 이동·복귀, UIA `대화 상자 의견 서랍`. 560~660에서는 의견 열이 224까지 줄어든다. 테스트 `review_qa::comments_column_fits_or_is_a_drawer_at_800_960_1440`, 캡처 `crates/pod-app/qa/review/`. |
| ui-06 | 해결 | `a738363` | 오른쪽 그룹 고정, 폭에 따라 터미널 수→작업 수→완료→작업 중 순서로 숨김. 800×600(100/150%) 테스트. |
| ui-07 | 해결 | `a738363` | `작업 중 N` / `작업 N개`, 요약 문구도 `작업 중`. 06 관제형은 상태 표시줄 집계 생략. |
| ui-08 | 해결 | `a738363`, `54eccb9` | 작업 이름에 응답·실패 수, expanded 노출(UIA `Expanded` 확인), 상태 그룹 Role::Status, 빈 화면 제목 heading. |
| ui-09 | 해결 | `f2d9434` | `settings.json.bad-<초>` 백업, 읽히는 항목은 살림, 경고 알림. |
| ui-10 | 해결(a·b·c) | `0846f9b`, `a738363`, `0ed8d5a` | 32px 평평한 알림 목록, 설정·새 작업·새 터미널·새 그룹 선택지 segment, 빈 화면 CTA 하나. 남은 것: 설정 category 열·select·switch, 오케스트레이션 화면의 채움 버튼(orch_ui) → orch_ui 부분 **해결** `8167717`, `9e2a313`(브랜치 `SJin-Choi/next-orch-ui`): 보드 Run 선택·오케스트레이션 설정·자원 한도가 `radio_group`+`choice`(UIA `IsSelected` 그룹마다 하나, `qa/orch/wake/uia-selection.txt`), 정리 화면 선택은 체크박스. 받은 편지함 버튼·답장 등 동작 단추의 강조는 선택지가 아니라 그대로. 설정 category 열·select·switch는 next-settings 몫. |
| ui-11 | 해결 | `54eccb9` | `처리 중…`. |
| ui-12 | 해결 | `5fa9761`, `0846f9b` | 8자리는 scrim만, 사용자 테마 AA 경고를 설정에 표시. |
| ui-13 | 해결 | `a738363` | 첫 ↓는 0번 행, Home/End, ←부모, →자식. |
| ui-14 | 해결(빈 화면·대기열) | `54eccb9` | 어절 단위 줄바꿈(`keep_all`), 제목 18/26. Review 패널 자체 문구(review.rs)는 편집기 담당 범위라 그대로. |
| ui-15 | 해결 | `133b180` | `의견`. |
| ui-16 | 해결 | `5fa9761`, `0846f9b` | mark 오류를 설정 › 모양에 표시. |
| ui-17 | 해결 | `9569b6a` | 09 §1·§5 머리말, D5.4 문장, D4.6과 구현 차이 기록. 밀도는 모양 섹션으로 옮겨 D5.3과 맞춤. |
| ui-18 | 해결 | `a738363` | focus 정지는 이미 있었다(`on_focus_in`이 목록 안의 닫기 단추까지 포함) — 테스트로 확인. 닫기를 28px 아이콘 단추로. |
| ui-19 | 해결 | `0e8ce2f` | `.pyc` 삭제, `__pycache__/` 무시. |
| 추정: 알림 목록 갱신 | 사실, 해결 | `0846f9b` | 열린 동안 응답된 요청이 빠지고 새 요청이 들어오는 것을 테스트로 확인. |
| 추정: ScrollStrategy::Top | 사실, 해결 | `a738363` | gpui는 아래로 벗어난 행을 맨 위로 올린다(uniform_list 소스 확인). `Nearest`로 바꿈. |
| 추정: Narrator·IME 후보·125/175% 래스터·OS 고대비 | 보류 | — | 이번에도 실행하지 않음(대화형 데스크톱 필요). |

## 6. 후속 수정 (next-settings, 2026-11-10)

브랜치 `SJin-Choi/next-settings`. 5절 ui-10의 "남은 것" 중 설정 부분. 증거: `crates/pod-app/qa/settings/`.

| id | 상태 | 커밋 | 메모 |
|---|---|---|---|
| ui-10 설정 category 열 | 해결 | `d15ce1f`, `3e0c101` | 왼쪽 160 세로 탭 목록(`모양`·`글꼴`·`일반`), 선택은 bg.selected + 왼쪽 2px accent(채움 버튼 아님), UIA TabItem `IsSelected`, ↑↓·Home/End, 선택 탭만 Tab 정지. 내용은 오른쪽 max640·좌우24, 좁으면 아래로. |
| ui-10 select | 해결 | `d15ce1f`, `3e0c101`, `0dbb3f5` | `pod_ui::select`: 32px, border.control, chevron, 폭200, 열린 목록에 체크 표시. UIA ComboBox `ExpandCollapse`·값, 항목 ListItem `IsSelected`. 스타일·레이아웃·글꼴·글자 크기에 사용. Enter 뒤 다시 열리던 문제(GPUI keyup click)를 고침. |
| ui-10 switch | 해결 | `d15ce1f`, `3e0c101`, `0dbb3f5` | `pod_ui::switch`: 트랙 + `켬`/`끔` 글자, UIA Toggle. 고대비 미리보기·움직임 줄이기·Windows 알림에 사용. 클릭·Space에 한 번씩만 바뀜. |
| banner keep-all | 해결 | `348e70d` | `banner` 제목·설명이 어절 단위로 줄바꿈(왼쪽 정렬). 24자 넘는 조각(경로)만 안에서 끊김. |
| 추정: 창 활성화 시 dock 패널 UIA 누락 | 사실, 보류 | — | 창이 전경으로 활성화되면 dock 패널 내용이 UIA 트리에서 빠지고 `상태` 패널이 열린다. master `88cf7f4`에서도 같음. 원인은 dock/gpui 쪽으로 보이며 이 작업 범위 밖. |

## 7. 기준 이미지 재캡처 (tail, 2026-11-10)

브랜치 `SJin-Choi/tail-baselines`, master `d84efd3`(fix-ui·next-* 병합, 전체 fmt 뒤). 보조 모니터 144 DPI, 1440×960 physical. 기존 기준과 `-Mode compare`: **18/36 실패**. 실패한 장마다 기준·실제·diff를 직접 봤다. 다크·라이트가 같은 원인이다.

| 장면(다크·라이트) | 변경 px | 분류 | 원인 |
|---|---|---|---|
| settings | 252k·281k | 의도 | 설정 category 열(`모양`·`글꼴`·`일반`), 프리셋 격자가 오른쪽으로(`3e0c101`). |
| orch-settings | 77k·118k | 의도 | 오케스트레이션 설정·자원 한도의 채움 버튼 → `radio_group` segment(`8167717`). 비활성 `중첩 깊이 한도`도 선택 3이 보임. |
| orch-board | 67k·62k | 의도 | Run 선택이 segment(`8167717`). |
| orch-cleanup | 13k·8k | 의도 | 정리 화면 `선택됨/선택 안 됨` 버튼 → `보관 대상` 체크박스(`8167717`). |
| review | 10k·10k | 의도 | 오류 배너·의견 열 문구가 어절 단위 줄바꿈(`348e70d`, `332ba38`, `bec7490`). |
| inspect | 10k·10k | 의도(부수) | 인스펙터 좌우 여백이 `p_4`(1rem=12) → `px(16)`(`3e0c101`, 설정 좌우24와 함께 명시). docs/10의 좌우16 관례와 맞아 그대로 둔다. |
| gates | 849·849 | 의도 | `podd client stopped` 배너가 keep-all(`348e70d`). |
| editor | 1606·1606 | 의도 | 상태 표시줄 줄 끝 `CRLF` → `혼합 (CRLF 19 · LF 3)`(editor-02 무손실 읽기, `589e6c0`). 픽스처 `main.rs`가 실제로 혼합이다. |

**회귀: 없음.** 관찰(고치지 않음, 범위 밖):
- keep-all 조각 사이 간격이 `gap_x(4)`라 한 칸 띄어쓰기보다 조금 넓다(`podd  client  stopped`, `0 · 전체`). 의도한 구현의 부수 효과.
- editor 장면은 체크아웃의 줄 끝에 의존한다. 픽스처가 `include_str!`로 작업 트리(autocrlf=true → CRLF)를 넣고, 변경분은 LF로 붙인다. LF 체크아웃에서는 `혼합` 숫자가 달라져 compare가 실패한다. 고치려면 pod-editor 픽스처에서 줄 끝을 고정해야 한다.
- UIA 트리 덤프가 불안정하다: 한 번 돌리면 36장 중 10~16장이 제목줄만 있는 10줄 트리다(이전 기준 10장, 이번 첫 실행 16장). 장면마다 무작위라 앱이 아니라 `orca computer get-app-state` 시점 문제로 보인다. 이번 기준은 빈 트리가 나온 장면만 다시 띄워 전체 트리를 받았다(PNG는 같음을 compare로 확인). `capture.ps1`에 재시도를 넣는 것은 남은 일.

재캡처(`b6cac17`) 뒤 compare **0/36 실패**. sidebar의 unreachable 터미널(v19)은 여전히 `Lost`처럼 흐리게 보이고, tooltip·접근 이름 앞에 `podd가 이 터미널에 연결할 수 없음: <이유>`를 붙인다(`vm.rs` `lost_reason`, 테스트 `unreachable_terminal_row_tooltip_gives_the_reason`, `6651bf7`).

### 7.1 라운드 2 tail (r2-tail, 2026-11-10)

master `abdbc4c`(라운드 2 병합 뒤), 같은 환경(보조 모니터 144 DPI, 1440×960 physical). 기존 기준과 compare: **2/36 실패**, 둘 다 editor. 기준·실제·diff를 직접 봤다.

| 장면 | 변경 px | 분류 | 원인 |
|---|---|---|---|
| dark-editor | 1550 | 의도 | 상태 표시줄 줄 끝 `혼합 (CRLF 19 · LF 3)` → `LF`. fixture가 체크아웃과 무관하게 LF로 쓴다(`b16df6c`, ui-editor-07). |
| light-editor | 188k → 1550 | 의도 + 캡처 흔들림 | 첫 실행은 창이 최대화 상태처럼 그려져(제목줄에 복원 아이콘, 내용 11px 아래로) 전체가 어긋났다. 같은 빌드로 두 번 더 찍으면 두 번 다 1550px, 다크와 같은 `LF` 차이만 남는다. 제품 회귀 아님. |

예상했던 settings select 스크롤, review 긴 토큰 변경은 픽셀 차이가 없었다(fixture 장면에 드러나지 않음). **회귀: 없음.** editor 두 장만 다시 찍어(PNG·tree, manifest 값은 같음) 새로 전체 캡처한 뒤 compare **0/36 실패**.

- **LF/CRLF 체크아웃**: `core.autocrlf=false`/`true`로 각각 `crates/pod-editor/fixtures`를 체크아웃해 바이트가 같음을 확인(`.gitattributes` eol=lf). editor 장면의 입력이 같으므로 결과도 같다. 두 번째 빌드·캡처는 하지 않았다.
- **UIA 재시도**: `capture.ps1`이 장면별 필수 노드(`uia-required.ps1`)가 나올 때까지 1초 간격 최대 6번 다시 덤프하고, 끝내 없으면 빠진 노드 이름과 함께 실패한다. 실제 실행에서 36장 중 11장·14장이 두 번째 덤프에서 완전해졌고 3번 이상 걸린 장면은 없었다. 자체 시험: `tools/visual/uia-selftest.ps1`.

### 7.2 UI wave 0 (Linejin/astra-ui)

기준 `8b835ff`. [QA 기록](../../../crates/pod-app/qa/ui-wave0/README.md). 기준 이미지는 변경하지 않음. 시각 변경 예상: 1400 logical px 이상에서 현재 작업 branch가 상태 표시줄에 추가됨(기존 1440×960 physical·150% 장면에서는 숨김). 실기·재캡처는 coordinator에게 넘긴다.

| id | 상태 | 커밋 | 메모 |
|---|---|---|---|
| dock-uia 코드 | 해결 | `76a6b39` | GPUI 캐시가 접근성 노드를 재사용하지 않아 dock 내용이 빠지는 경로를 확인. UIA 활성 시 패널 캐시를 우회하고 배치는 유지. Apache-2.0 고지·UIA 회귀 판정 추가, headless 활성화에서 인스펙터 자동 생성 없음. |
| dock-uia 실제 전경 활성화 | 보류 | — | **NOT RUN: 잠금 해제된 Windows 세션 필요.** `OpenInputDesktop` error 5, `LogonUI=True`. Coordinator 지시로 대기하지 않음. 실제 UIA 재현·수정 확인과 스크린샷은 미검증. |
| status-branch | 해결 | `00c0a70` | 기존 task.branch로 git-branch 아이콘·전체 이름 tooltip/UIA 표시. 1400px 미만에서 먼저 숨김. 작업 전환·편집기 우선·branch 없음·좁은 창 headless 검사. |
| select-blur | 해결 | `9cde4d9` | focus-out·창 비활성화에서 선택 없이 닫음. 프로그램 포커스·blur·부모 Escape·Alt+Tab·mouse-down-out headless 검사. |

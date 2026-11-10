# P4 리스크 스파이크: 네이티브 GPUI 코드 리뷰 Diff 뷰 (diff-spike)

작성일: 2026-11-08 · 코드: `crates/diff-spike` · 스크린샷: `crates/diff-spike/screenshots/`
목적: 로드맵 P4(리뷰)에서 난이도 **상**으로 평가한 "Combined diff 뷰"([01 §5.2](01-plan.md))를 Windows에서 GPUI로 쓸 만하게 만들 수 있는지 확인.

---

## 0. 결론

**가능하다. 리스크는 "상"에서 "하~중"으로 낮춘다.**

- gpui-component 0.9에 **읽기 전용 리뷰용 Diff 컴포넌트**(`gpui_component::diff`, Apache-2.0, 약 9천 줄)가 이미 들어 있다. 좌/우·인라인 모드, 변경 없는 구간 접기와 펼치기(위/아래/전체), 단어/글자 단위 강조, tree-sitter 구문 강조(백그라운드), 가상화 목록, 라인 앵커 주석(annotation), 줄 클릭·범위 선택 콜백까지 제공한다.
- 01 문서가 가정한 "에디터 컴포넌트 재사용 + diff 뷰 직접 구현"은 필요 없다. 스파이크에서 직접 만든 건 **입력 계층(두 파일 / git → 패치), 파일 목록, 코멘트 모델·입력창·목록·JSON 내보내기, 측정 도구**뿐이다(약 1,100줄).
- 1만 줄 / 약 950곳 변경 diff: **첫 프레임 0.35초**, 스크롤 **75fps(모니터 상한)에서 프레임 CPU 평균 2.5–4.5ms, 16.6ms 넘는 프레임 0개**. 5만 줄에서도 같다.
- 남은 위험: (1) 사전 1.0 라이브러리(gpui-pre 0.3 / gpui-kit 0.9) API 변경. 필요하면 Apache-2.0이라 포크할 수 있다. (2) 아래 §6의 기능 공백. 모두 "만들면 되는" 종류이고 구조적 막힘은 없다.

---

## 1. 빌드·실행

```bash
cargo build --release -p diff-spike
```

| 사용 예 | 의미 |
|---|---|
| `diff-spike --old A --new B` | 두 파일 비교 |
| `diff-spike --git <repo> [--rev <ref>]` | 작업 트리(추적 파일 변경 + untracked) vs `HEAD`/ref |
| `diff-spike --gen-bench <dir> [--lines 10000] [--changes 1000]` | 성능 측정용 합성 파일 쌍을 만들고 연다 |

| 옵션 | 의미 |
|---|---|
| `--mode split\|unified` | 초기 보기 (기본 split) |
| `--expand-all` | 접지 않고 전체 줄 표시 |
| `--wrap` / `--no-wrap` | 줄 바꿈 (기본: split이면 켬, unified면 끔. §5-1 참고) |
| `--export <file.json>` | 코멘트 내보내기 경로 (기본 `./diff-comments.json`, 클립보드에도 복사) |
| `--demo-comments` | 스크린샷용: 첫 변경 줄에 저장된 코멘트 1개 + 열린 입력창 1개 |
| `--bench-scroll <px/프레임> [--bench-delay S]` | 매 프레임 합성 휠 이벤트로 스크롤 (600프레임마다 방향 전환) |
| `--duration S` · `--no-throttle` · `--display primary\|secondary\|N` | pod-spike와 같음 (기본 서브 모니터) |

stdout: `[open]` 단계별 시간, 매초 `[stats]`, 끝날 때 `[summary]`, `[export]`. 릴리스 exe는 GUI 서브시스템(PE subsystem = 2 확인)이라 콘솔 창이 뜨지 않는다.

UI: 위 툴바(Side-by-side / Inline / Show all lines·Collapse unchanged / Wrap / Prev·Next change / Export), 왼쪽 파일 목록(A/M/D, +/− 수, 클릭하면 해당 파일로 스크롤), 가운데 diff, 오른쪽 코멘트 목록(클릭하면 해당 줄로, ✕로 삭제). 줄을 클릭하거나 hover 시 나오는 `+`(드래그 선택 시 범위)를 누르면 그 줄 아래에 입력창이 열리고, Enter나 "Comment"로 저장한다.

---

## 2. 구조

```
main.rs    인자, 모니터 선택(pod-spike 방식), 창 열기, [open] 로그
model.rs   입력 계층: 두 파일 / git → SourceFile{old,new} → 전체 문맥 패치, 벤치 파일 생성
           (P4에서 `crates/pod-diff/src/model.rs`로 옮겨 pod-app과 같이 씀, docs/09 §7.2)
review.rs  루트 뷰: 툴바 · 파일 목록(uniform_list) · Diff · 코멘트 목록, 코멘트 모델과 JSON 내보내기, 스크롤 벤치 드라이버
stats.rs   fps / 프레임 CPU 카운터 (pod-spike와 같은 정의) + 프레임 끝 마커 Element
```

데이터 흐름:
1. `model`이 각 파일의 두 버전 전체 텍스트를 읽는다. git 모드는 `git diff --name-status --no-renames -z <rev>` + `git ls-files --others --exclude-standard`로 목록을 얻고, 이전 버전은 `git show <rev>:<path>`, 새 버전은 작업 트리 파일이다.
   - **줄바꿈(crlf-diff)**: 이전 버전은 git이 저장한 형태(blob), 새 버전은 체크아웃된 형태라 `core.autocrlf=true`면 CRLF/LF 차이만으로 모든 줄이 바뀐 것으로 보였다. 이제 `git diff`처럼 새 버전에 git의 "clean" 줄바꿈 변환을 먼저 적용한다(`pod-diff/src/eol.rs`): `-text`는 그대로, `text`·`eol=`·`crlf`는 CRLF→LF, `text=auto`나 속성 없음+`core.autocrlf` true/input은 바이너리처럼 보이거나 index 버전에 이미 CRLF가 있으면(저장소에 CRLF로 들어 있는 파일) 그대로, 아니면 CRLF→LF. 속성 없음+`autocrlf=false`는 변환 없음. CRLF가 든 파일이 있을 때만 `git config`·`git check-attr --stdin`·`cat-file --batch`(index)를 한 번씩 더 부른다. 줄 수는 바뀌지 않으므로 코멘트 줄 번호와 전달 텍스트(podd가 작업 파일을 줄 번호로 읽고 `\r`을 뗌)는 그대로 맞는다. filter·`ident`·`working-tree-encoding`은 적용하지 않는다. 회귀 테스트 `model::tests::line_endings_match_git_diff`(11가지 경우를 `git diff --numstat`과 비교).
2. `similar`(Patience 알고리즘)로 줄 diff를 계산해 **파일 전체를 담은 hunk 하나짜리 git 패치**를 만든다. CRLF는 그대로 보존하고 `\ No newline at end of file`도 처리한다. 진짜 줄바꿈 변환(예: `autocrlf=false`에서 LF→CRLF)은 `git diff`처럼 모든 줄이 바뀐 것으로 보이고, 파일 목록에 **"줄바꿈만 바뀜"**이 붙는다(`FileRow::eol_only`).
   - 이유: 컴포넌트는 패치에 있는 줄만 접고 펼칠 수 있다. `git diff`의 3줄 문맥 패치를 넣으면 "펼치기"가 파일 나머지를 보여줄 수 없다. 전체 문맥 패치를 넣고 `DiffState::with_context_lines(Some(3))`로 접게 했다.
   - `DiffFile`의 공개 생성자는 `DiffFile::parse(patch)`뿐이다(두 텍스트에서 만드는 빌더는 `#[cfg(test)]`). 그래서 패치 문자열을 만들어 다시 파싱한다. 5만 줄에서 2.7MB, 파싱 19ms라 문제없다.
3. 단어/글자 단위 강조와 구문 강조는 `DiffState`가 백그라운드 executor에서 준비하고, 끝나면 화면에 반영한다.
4. 코멘트는 `Diff::annotations([DiffAnnotation::line(id, DiffLinePosition{path, side, line})])` + `render_annotation`으로 해당 줄 바로 아래에 렌더링한다(가상 목록 행 높이를 컴포넌트가 다시 잰다). 입력창은 gpui-component `Input`이다.

내보내기 JSON (에이전트에 보낼 형식의 초안, `screenshots/comments-export-example.json`):
```json
{ "version": 1, "source": "git . (working tree vs HEAD)",
  "comments": [ { "id": 1, "path": "crates/diff-spike/src/review.rs", "side": "new",
                  "line": 11, "end_line": 11, "body": "Please split this function",
                  "code": "    Diff, DiffAnnotation, ...\r\n" } ] }
```
`code`에 해당 줄 원문을 넣어, 받는 쪽이 체크아웃 없이도 맥락을 알 수 있게 했다.

---

## 3. gpui-component에서 재사용한 것 / 직접 만든 것

| 기능 | 출처 | 비고 |
|---|---|---|
| diff 렌더링, 가상화 (`Diff` + `DiffState`, 내부는 gpui `list`) | **재사용** | 모든 파일이 하나의 가상 목록을 공유하고 파일마다 헤더(접기 버튼, +/−)가 붙는다 |
| 좌/우(split) · 인라인(unified) 전환 | **재사용** | `DiffState::set_mode` |
| 변경 없는 구간 접기 + 펼치기(↓ 20줄 / ↑ 20줄 / ⇕ 전체) | **재사용** | `with_context_lines`, `expand_unchanged`/`collapse_unchanged`. 펼침 줄 수는 `with_expansion_lines` |
| 줄 안 강조 (단어/글자) | **재사용** | 내부적으로 `similar` 사용, 유사도 25% 미만이면 줄 전체만 강조 |
| 구문 강조 | **재사용** | gpui-kit feature `tree-sitter-rust/javascript/typescript/python/toml/markdown`. 커스텀 하이라이터 훅(`with_syntax_highlighter`)도 있다 |
| 줄 클릭, hover `+`, 드래그 범위 선택, 복사, 키보드 이동, 다음/이전 변경, 파일·줄로 스크롤 | **재사용** | `on_line_click`, `on_add_annotation`, `scroll_to_file`, `scroll_to_line`, `next_change` |
| 줄 앵커 주석 슬롯 | **재사용** | 코멘트 카드와 입력창을 여기 넣었다. 데코레이션·거터 마커 API도 있지만 쓰지 않았다 |
| 입력창, 버튼, 테마, Lucide 아이콘 | **재사용** | 아이콘은 `application().with_assets(gpui_kit::assets::Assets)`가 있어야 보인다 (§5-3) |
| 두 파일 / git 입력 → 전체 문맥 패치 | 직접 | `model.rs`, `similar` Patience |
| 파일 목록 (+/− 수, 클릭 시 스크롤) | 직접 | `uniform_list`로 가상화 |
| 코멘트 모델, 입력 흐름, 목록 패널, JSON 내보내기 | 직접 | `review.rs` |
| 측정 (`[open]`, fps/프레임 CPU, 스크롤 벤치) | 직접 | `stats.rs`, `bench_tick` |

---

## 4. 측정 결과

환경: pod-spike와 같음 (Ryzen 7 7800X3D, RTX 4060 Ti, Windows 11). 창은 서브 모니터(150% 스케일)에 최대화. 측정 정의는 [02 §6](02-ui-spike-notes.md)과 같다(fps = 루트 `render` 호출 수/초, 프레임 CPU = 루트 render 시작 → 그 프레임 마지막 paint 끝, GPU 시간 제외). 합성 파일: `--gen-bench`로 만든 Rust 형태 코드, 변경 대상 1,000곳(중복 제거 후 약 950곳: 수정 3 : 삽입 1 : 삭제 1) → **+761 / −761줄**.

### 4.1 여는 시간

| 입력 | 파일 읽기 | 줄 diff + 패치 | 패치 파싱 | DiffState | **첫 프레임** (프로세스 시작부터) | 구문/줄 안 강조 표시 |
|---|---|---|---|---|---|---|
| 1만 줄, 약 950곳 변경 | <1ms | 10–12ms | 4ms | 1ms | **0.34–0.37s** | ≤1s |
| 5만 줄, 약 4,700곳 변경 (+3,799/−3,799) | 135ms | 177–183ms | 19ms | 3.5ms | **0.70–0.76s** | 1–2s |
| git 모드 (이 저장소, 7개 파일 9.9k줄) | 285ms (git 프로세스 약 10번) | 2ms | 3ms | 0.4ms | **0.63s** | — |

- 첫 프레임 시간의 대부분(~0.3s)은 GPUI 창·GPU 초기화다. diff 자체는 1만 줄에서 20ms 안팎이다.
- 강조 표시 시점은 공개 API로 알 수 없어서, 1초 간격으로 캡처해 강조색 픽셀 수가 바뀌는 시점으로 측정했다. 그 전에도 코드는 강조 없이 바로 보이고 스크롤된다.

### 4.2 스크롤 (`--no-throttle`, 17초 × 1초 샘플, 1만 줄 기준)

| 시나리오 | fps 평균 / 최저 | 프레임 CPU 평균 / 최대 | 16.6ms 초과 프레임 |
|---|---|---|---|
| split + wrap, 접힘, 40px/프레임 (3,000px/s) | 75.0 / 74.3 | 2.53 / 4.24ms | 0 |
| unified, 접힘, 40px/프레임 | 75.1 / 74.2 | 2.88 / 5.43ms | 0 |
| split + wrap, **전체 1만 줄 펼침**, 40px/프레임 | 75.0 / 74.3 | 2.57 / 3.83ms | 0 |
| split, 줄 바꿈 없음(가로 스크롤), 전체 펼침, 40px/프레임 | 75.0 / 74.3 | 4.41 / 6.96ms | 0 |
| unified, 전체 펼침, **400px/프레임 (30,000px/s)** | 69.8 / 30.7* | 4.45 / 6.96ms | 0 |
| split + wrap, 전체 펼침, 400px/프레임 | 75.0 / 74.4 | 3.51 / 5.85ms | 0 |
| **5만 줄** split + wrap, 접힘, 40px/프레임 | 75.1 / 74.3 | 2.55 / 4.02ms | 0 |

- 75fps가 상한이다. 서브 모니터 주사율로 추정한다(확인하지 않음). 모든 시나리오에서 프레임 CPU는 7ms 이하였다.
- \* 1초 구간 두 번이 30.7fps였다. 그 구간의 프레임 CPU 최대가 6ms라 렌더 비용 문제는 아니다. 이 속도면 약 7초 만에 목록 끝에 닿는데(이후 스크롤이 무효), 그 시점·캡처 시점과 겹친다. 원인은 확정하지 않았다.
- 스크롤 벤치는 `window.dispatch_event(PlatformInput::Scroll(..))`로 매 프레임 휠 이벤트를 넣는다. 실제 휠과 같은 hit-test·목록 스크롤 경로를 지나지만 OS 입력 큐는 거치지 않는다.

---

## 5. 발견과 함정

| # | 증상 | 원인 | 해결/대응 |
|---|---|---|---|
| 1 | split 모드에서 오른쪽(새 버전) 칼럼이 안 보이고 코멘트 자리가 빈칸 | 컴포넌트가 두 칼럼을 **하나의 가로 스크롤 캔버스**에 놓는다(폭 = (가장 긴 줄 + 거터) × 2). 좁은 창(서브 모니터 논리 폭 1280 − 사이드 패널)에서는 오른쪽 칼럼이 화면 밖으로 나간다 | split에서는 `soft_wrap` 기본 켬, 툴바 토글. 칼럼별 독립 가로 스크롤은 지원하지 않는다 |
| 2 | git 모드 여는 데 2.9초, 창 깜빡임 | GUI 서브시스템 exe가 콘솔 프로그램(git)을 띄우면 **호출마다 새 콘솔**이 만들어진다(호출당 약 250ms) | `creation_flags(CREATE_NO_WINDOW)` → 285ms. 제품의 모든 자식 프로세스 생성에 적용해야 한다 |
| 3 | 접기 행·파일 헤더 아이콘이 비어 보임 | gpui-kit 아이콘(SVG)은 asset source를 등록해야 로드된다 | `gpui_kit::application().with_assets(gpui_kit::assets::Assets)`. pod-spike에는 이 줄이 없다 |
| 4 | `git diff` 패치로는 펼치기가 hunk 밖으로 못 나감 | 컴포넌트는 패치에 있는 줄만 안다 | 전체 문맥 패치 생성 (§2) |
| 5 | 캡처 스크립트에 배열 인자를 `powershell -File`로 넘기면 한 문자열로 합쳐짐, 상대 경로 저장 시 GDI+ 오류 | PowerShell 인자 전달 방식, .NET 현재 디렉터리 ≠ PS 위치 | 같은 세션에서 `& tools/run-and-capture.ps1 -AppArgs @(..)`로 호출하고 `-OutDir`는 절대 경로 |

상호작용 검증: 실제 창에 `PostMessage`로 마우스·키보드 메시지(WM_LBUTTONDOWN/UP, WM_CHAR, Enter)를 보내(실제 커서는 움직이지 않음) 다음을 확인했다. 파일 목록 클릭 → 해당 파일로 스크롤, 새 버전 줄 클릭 → 입력창, 입력 + Enter → 저장, 이전 버전 줄 코멘트(`side: "old"`), "1223 unchanged lines" 클릭 → 펼침, Inline 전환, Export → JSON 파일 생성.

---

## 6. 공백 (스파이크에서 하지 않은 것 / 알려진 한계)

1. **추가·삭제 파일도 split에서 반쪽이 빈 칼럼** — 모드가 diff 전체에 하나뿐이다. 파일별 모드는 컴포넌트를 바꾸거나 포크해야 한다.
2. 코멘트: 한 줄 입력(`InputState`. 여러 줄은 `TextareaState`로 바꾸면 됨), 편집·해결(resolve)·스레드 없음, 디스크에 저장하지 않음, 코드가 바뀔 때 앵커를 다시 맞추지 않음. 범위 코멘트는 `on_add_annotation`(드래그 선택 후 `+`) 경로로 연결했지만 자동 테스트는 하지 않았다.
3. git: staged/unstaged 구분, 커밋 범위(`A..B`), rename 감지(`--no-renames`로 끔) 없음. 바이너리는 건너뛰고 submodule은 무시한다. 파일마다 `git show`를 띄우므로(약 50ms/회) 파일이 많으면 `git cat-file --batch`나 gix로 바꿔야 한다.
4. 파일 목록: 트리 구조 없음, 스크롤 위치와 현재 파일 동기화 없음, "viewed" 체크 없음.
5. 구문 강조 문법은 6개만 켰다(빌드 시간·크기 때문). `Cargo.lock`처럼 확장자로 판별되지 않는 파일은 강조되지 않는다.
6. 측정 한계: 합성 휠 이벤트, GPU 시간 제외, 75Hz 상한, 메모리는 측정하지 않았다. 강조 준비 시간은 픽셀 비교로 1초 단위만 측정했다.
7. 테마: 다크 테마 하나만 확인했다. 툴바·패널 색은 하드코딩했다(diff 본문은 테마를 따른다).

---

## 7. P4 판정과 공수 추정

**판정: P4 진행 가능. "Combined diff 뷰" 난이도를 상 → 하~중으로 조정한다.** 렌더링·가상화·접기·강조·라인 앵커가 컴포넌트에 있으므로, P4의 남은 일은 대부분 데이터 계층과 워크플로다.

| 작업 | 추정 |
|---|---|
| 리뷰 뷰 제품화: 코멘트 영속(podd SQLite), 여러 줄 입력·편집·해결, 범위 코멘트 UX, staged/unstaged/커밋 범위 선택, rename, gix 또는 `cat-file --batch`, 파일 트리 + viewed + 스크롤 동기화, 단축키, 추가/삭제 파일 표시 개선(§6-1), 테마 연동 | 1.5–2주 |
| 코멘트 → 에이전트 전송: JSON → 프롬프트 포맷, 대상 에이전트 터미널로 `pod send` (P0 API 의존) | 0.5–1주 |
| Checks 패널, PR 생성 (gh CLI / GitHub API) | 1.5–2주 |
| **합계** | **3.5–5주** — 로드맵의 P4 5주 안에 들어간다 |

권고:
- gpui-kit/gpui-component 버전을 고정하고, `diff` 모듈 API(`DiffFile::parse`, `DiffState`, `Diff` 슬롯)를 얇은 어댑터 뒤에 둔다. 업스트림이 깨지면 이 모듈만 포크한다(Apache-2.0).
- 자식 프로세스는 전부 `CREATE_NO_WINDOW` (§5-2).
- 앱 시작 시 `with_assets` 등록 (§5-3).

라이선스: 새로 추가된 의존성은 `similar`(Apache-2.0), `serde`/`serde_json`(MIT/Apache), tree-sitter 0.26과 문법 크레이트(MIT), `streaming-iterator`(MIT/Apache)다. Zed 앱 크레이트는 쓰지 않았다.

---

## 8. 스크린샷 (`crates/diff-spike/screenshots/`)

| 파일 | 내용 |
|---|---|
| `side-by-side.png` | split + wrap, 1만 줄 벤치 스크롤 중(t=10s). 단어 단위 강조(`saturating_add` → `wrapping_add`), 접기 행, hover `+` |
| `inline-with-comments.png` | unified, 저장된 코멘트 카드 + 열린 입력창, 접기 행(⌄ N unchanged lines) |
| `collapsed-multifile.png` | git 모드: 파일 목록(A/M, +/−), 파일 헤더, `↓ ↑ ⇕ 1223 unchanged lines` 펼치기 컨트롤 |
| `line-comment-saved.png` | 실제 클릭·입력·Enter로 저장한 코멘트(new:11), 오른쪽 목록, 내보내기 완료 상태줄 |
| `comment-draft-split.png` | split에서 코멘트 카드와 입력창이 새 버전 칼럼 아래 앵커됨 |
| `expanded-inline.png` | 접힌 구간 클릭으로 펼친 뒤 Inline 전환 |
| `comments-export-example.json` | 위 상호작용으로 내보낸 JSON |

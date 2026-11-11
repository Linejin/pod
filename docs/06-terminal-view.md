# pod-terminal-view — 재사용 GPUI 터미널 뷰

작성일: 2026-11-08 · 코드: `crates/pod-terminal-view` (라이브러리), 사용처 `crates/pod-spike`
관련: [01 기획 §5·§8](01-plan.md) · [02 스파이크 노트](02-ui-spike-notes.md)

스파이크의 터미널 위젯을 pod-app이 그대로 쓸 수 있는 크레이트로 분리하고, 입력·IME·선택·스크롤백·바뀐 패널만 다시 그리기를 완성했다.

---

## 1. 구조 — 뷰는 PTY를 갖지 않는다

```
            raw PTY bytes                         keys / IME / paste / mouse / query replies / resize
podd ──attach──▶ TerminalView::feed(&[u8]) ──▶ VtEngine(alacritty) ──snapshot──▶ TerminalElement (GPUI paint)
                        │                                                         │
                        └──────────────── InputSink::write / resize ◀─────────────┘
```

- 결정 사항(§5.3): 데몬 podd가 **원시 PTY 바이트**를 스트리밍하고 UI가 **자체 VT**를 돌린다. 그래서 `TerminalView`는 바이트를 먹는 VT 모델만 소유하고, 사용자가 만든 바이트는 추상 `InputSink`로 내보낸다.
- 오늘의 sink: pod-spike의 로컬 ConPTY (`crates/pod-spike/src/pty.rs`, portable-pty). 내일의 sink: podd attach 연결.
- VT 엔진은 `VtEngine` 트레잇 뒤에 있다. alacritty 타입은 `vt/alacritty.rs` 밖으로 나오지 않음 → libghostty-vt 전환 시 `VtEngine` 구현 하나만 추가.

| 파일 | 역할 |
|---|---|
| `lib.rs` | 재수출, `DEFAULT_SCROLLBACK`(5000), `POD_TERM_DEBUG` 트레이스 매크로 |
| `sink.rs` | `InputSink` 트레잇, `TermSize`, `NullSink`, `FnSink` |
| `vt/mod.rs` | `VtEngine` 트레잇 + 엔진 중립 데이터(`Snapshot`, `CellSnap`, `Modes`, `RowText`, `VtEvent`) |
| `vt/alacritty.rs` | `AlacrittyVt`: alacritty_terminal 0.26의 `Term` + `vte::ansi::Processor` (PTY 없음) |
| `view.rs` + `view/{input,ime,links}.rs` | `TerminalView` 엔티티: 상태·입출력·선택·클립보드·스크롤·Render / 키·마우스 / IME(`EntityInputHandler`) / 화면 링크 |
| `element.rs` + `element/{text,cursor,preedit}.rs` | `TerminalElement`: 그리드 그리기·선택 강조·스크롤바·마우스 리스너 / 글자 분할·모양·넓은 글자 맞춤 / 커서 / IME 프리에딧 |
| `boxdraw.rs` | U+2500–259F 직접 그리기 + **대각선 ╱╲╳ (U+2571–2573) 추가** |
| `keys.rs` / `mouse.rs` / `selection.rs` | 순수 함수 인코더·선택 모델 (GPUI 무관, 단위 테스트) |
| `palette.rs`, `perf.rs` | 기본 색, 페인트 시간 훅(pod-spike 통계용) |

라이선스: Apache-2.0. 새 의존성은 `unicode-width`(MIT/Apache), pod-spike의 `portable-pty`(MIT; 하위 nix/serial2/shared_library/shell-words/winreg 모두 MIT·Apache·BSD). Zed `terminal_view` 코드는 보지도 복사하지도 않음 — GPUI 공개 API(`EntityInputHandler`, `ElementInputHandler`, `Entity::cached`)만 사용.

---

## 2. 크레이트 API

```rust
use pod_terminal_view::{InputSink, TermSize, TerminalConfig, TerminalEvent, TerminalView};

// 1) sink 구현 (아래 §3의 podd 예시 참고)
struct MySink { /* … */ }
impl InputSink for MySink {
    fn write(&self, bytes: &[u8]) { /* 큐에 넣고 바로 반환 (UI 스레드를 막지 말 것) */ }
    fn resize(&self, size: TermSize) { /* cols, rows, cell_width, cell_height */ }
}

// 2) 뷰 생성 (window가 필요: 포커스 in/out 구독)
let view = cx.new(|cx| TerminalView::new(Box::new(MySink { .. }), TerminalConfig::default(), window, cx));

// 3) 프로그램 출력 공급 — 받은 바이트를 그대로
view.update(cx, |v, cx| v.feed(&bytes, cx));

// 4) 이벤트 (탭 제목, 벨)
cx.subscribe(&view, |this, _, ev: &TerminalEvent, cx| match ev {
    TerminalEvent::TitleChanged(t) => { /* None = 기본 제목으로 */ }
    TerminalEvent::Bell => {}
});

// 5) 배치: cached로 감싸면 이 터미널이 바뀔 때만 다시 그림
let mut style = StyleRefinement::default();
style.size.width = Some(relative(1.).into());
style.size.height = Some(relative(1.).into());
div().child(view.clone().cached(style))
```

| 메서드 | 설명 |
|---|---|
| `new(sink, config, window, cx)` / `with_engine(vt, sink, config, window, cx)` | 기본 alacritty VT / 임의 `Box<dyn VtEngine>` |
| `feed(&[u8], cx)` | 출력 파싱 → 쿼리 응답(DA/DSR/OSC 10·11·4, CSI 14t)은 sink로, 제목/벨/OSC 52 저장은 이벤트·클립보드로. DEC 2026 동기 업데이트 타임아웃도 처리 |
| `write_input(&[u8], cx)` | 사용자 입력 전송 (스크롤백 보는 중이면 맨 아래로 이동) |
| `paste(&str, cx)` / `paste_from_clipboard(cx)` | 줄바꿈 CR 정규화 + 브래킷 붙여넣기(모드 2004) |
| `copy_selection(cx) -> bool`, `selection_text()`, `selection()`, `set_selection()` | 선택/복사 |
| `scroll_lines(i32, cx)` | 스크롤백 이동 (+ = 과거) |
| `title()`, `modes()`, `grid_size()`, `marked_text()`, `screen_text()`, `vt()` | 조회 (`screen_text`는 테스트·디버그용) |
| `config()` | `TerminalConfig { font_family, font_size, line_height, scrollback }` |

- 크기 결정은 뷰가 한다: 요소 prepaint에서 셀 크기로 cols/rows 계산 → VT resize → `sink.resize(TermSize)` (값이 바뀔 때만).
- `Focusable` 구현 — 호스트는 `view.read(cx).focus_handle(cx)`로 포커스 이동.
- 디버그: 환경변수 `POD_TERM_DEBUG=1`이면 키·마우스·휠 처리 경로를 stdout에 출력.

---

## 3. pod-app에 podd attach sink 연결하기

podd attach 프로토콜은 `SJin-Choi/podd-core`에서 진행 중이므로 이 크레이트는 의존하지 않는다. 필요한 것은 두 방향뿐:

```rust
/// pod-app 쪽 (예시). attach 연결의 쓰기 작업(task)에 프레임을 넘긴다.
struct PoddSink { tx: futures::channel::mpsc::UnboundedSender<ClientFrame> }

impl InputSink for PoddSink {
    fn write(&self, bytes: &[u8]) { let _ = self.tx.unbounded_send(ClientFrame::Input(bytes.to_vec())); }
    fn resize(&self, s: TermSize) { let _ = self.tx.unbounded_send(ClientFrame::Resize { cols: s.cols, rows: s.rows }); }
}

// 읽기: attach 스트림 → feed. 프레임을 모아 한 번에 넣으면 파싱/알림이 줄어든다.
cx.spawn(async move |_, cx| {
    while let Some(ServerFrame::Output(bytes)) = stream.next().await {
        let _ = view.update(cx, |v, cx| v.feed(&bytes, cx));
    }
});
```

설계 시 맞춰야 할 점:
1. **attach 직후 상태 복원**: UI VT는 빈 상태로 시작한다. podd는 attach 시 (a) 원시 바이트 링버퍼 재생 또는 (b) 현재 화면을 VT 시퀀스로 직렬화해 보내야 한다. (a)가 단순하지만 버퍼 앞부분이 모드 설정(대체 화면, 마우스 모드)을 놓칠 수 있으므로 (b) 또는 "직렬화된 화면 + 이후 바이트"를 권장.
2. **쿼리 응답 중복**: DA/DSR 등 응답은 뷰가 sink로 보낸다. 데몬이 자체 VT로도 응답하면 프로그램이 응답을 두 번 받는다 → 데몬 VT는 응답하지 않거나, 붙어 있는 클라이언트가 없을 때만 응답하도록 한다. (여러 클라이언트가 붙으면 한 클라이언트만 "응답 담당")
3. **크기 소유권**: 여러 클라이언트가 같은 터미널을 보면 resize가 충돌한다. 정책(포커스된 클라이언트 우선 / 최소 크기) 결정 필요.
4. `write`는 UI 스레드에서 호출되므로 반드시 비차단(채널).
5. 백프레셔: 스파이크 sink는 UI가 소비하지 않은 바이트가 8MB를 넘으면 읽기를 멈춘다. podd 경로도 소켓 버퍼 + 상한을 둘 것.

pod-spike의 `pty.rs`가 이 구조의 실제 예: 읽기 스레드가 바이트를 모아 두고 깨우기를 한 번만 보내며(coalesce), UI 펌프가 `take_output()`으로 몽땅 가져가 `feed` 한 번 호출.

---

## 4. 입력 인코딩 표

원칙: **글자(printable)는 키 이벤트로 인코딩하지 않는다.** Ctrl/Alt 없는 글자는 플랫폼 텍스트 입력 경로(WM_CHAR, IME 확정)로 `replace_text_in_range`에 들어오고 UTF-8 그대로 전송된다. 그래서 키보드 배열·AltGr·IME가 모두 동작한다. 키 핸들러는 텍스트 경로가 표현 못 하는 키만 처리한다.
수정자 값 `m = 1 + Shift(1) + Alt(2) + Ctrl(4)` (xterm `modifyCursorKeys=2`).

| 키 | 수정자 없음 | 수정자 있음 | 비고 |
|---|---|---|---|
| ↑ ↓ → ← | `CSI A/B/C/D`, 응용 커서 모드(DECCKM)면 `SS3 A..D` | `CSI 1;m A..D` | |
| Home / End | `CSI H` / `CSI F` (응용 커서: `SS3 H/F`) | `CSI 1;m H/F` | Shift+Home/End는 스크롤백 중일 때만 맨 위/맨 아래 |
| Insert / Delete | `CSI 2~` / `CSI 3~` | `CSI 2;m~` / `CSI 3;m~` | |
| PgUp / PgDn | `CSI 5~` / `CSI 6~` | `CSI 5;m~` / `CSI 6;m~` | **Shift+PgUp/PgDn = 스크롤백** (대체 화면이 아닐 때) |
| F1–F4 | `SS3 P/Q/R/S` | `CSI 1;m P..S` | |
| F5–F12 | `CSI 15~ 17~ 18~ 19~ 20~ 21~ 23~ 24~` | `CSI n;m~` | |
| Enter | `CR` | Alt: `ESC CR` | |
| Tab | `HT` | Shift: `CSI Z`, Alt: `ESC HT` | |
| Backspace | `DEL(0x7f)` | Ctrl: `BS(0x08)`, Alt: `ESC DEL` | |
| Esc | `ESC` | Alt: `ESC ESC` | |
| Space | 텍스트 경로 | Ctrl: `NUL`, Alt: `ESC SP` | |
| Ctrl+문자 | — | `a–z → 0x01–0x1a`, `@ 2 → 0x00`, `[ 3 → ESC`, `\ 4 → 0x1c`, `] 5 → 0x1d`, `^ 6 ~ → 0x1e`, `_ 7 - / → 0x1f`, `8 ? → DEL` | Ctrl+Alt+x = `ESC` + 제어 바이트 |
| Alt+문자 | — | `ESC` + 문자(배열 문자 우선) | meta = ESC 접두 |
| AltGr 문자 | 텍스트 경로 (`prefer_character_input`) | | |
| Win 키 조합 | 보내지 않음 | | |
| **Ctrl+Shift+C / V** | 복사 / 붙여넣기 (프로그램에 안 감) | | |

붙여넣기: `\r\n`·`\n` → `\r`. 모드 2004면 `ESC[200~ … ESC[201~`로 감싸고, 본문에 들어 있는 `ESC[200~`/`ESC[201~`는 제거(괄호 탈출 방지).
포커스 보고(모드 1004): 포커스 받으면 `ESC[I`, 잃으면 `ESC[O`.

### 마우스
| 상황 | 동작 |
|---|---|
| 프로그램이 마우스 모드 켬(1000 클릭 / 1002 드래그 / 1003 모든 움직임) | 누름·뗌·(모드에 따라) 움직임·휠(버튼 64/65) 보고. Shift 누르면 보고 대신 로컬 선택 (xterm 관례) |
| 인코딩 | SGR 1006 `CSI < b;x;y M/m` 우선, UTF-8 1005, 기본(레거시) `CSI M` (좌표 223 초과는 보고 안 함) |
| 버튼 코드 | 왼 0 / 가운데 1 / 오른 2 / 없음 3, 움직임 +32, Shift +4, Alt +8, Ctrl +16. 레거시 뗌은 3 |
| 마우스 모드 아님 + 대체 화면 + 1007(기본 켜짐) | 휠 → 위/아래 화살표 (pager·편집기) |
| 마우스 모드 아님 + 일반 화면 | 휠 → 스크롤백 (시스템 "한 번에 스크롤할 줄 수" 적용) |
| 왼쪽 드래그 / 더블 / 트리플 클릭 | 문자 / 단어 / (소프트랩 포함) 줄 선택. Shift+클릭 = 선택 확장. 위·아래 가장자리 밖으로 드래그하면 스크롤 |
| 오른쪽 클릭 | 선택이 있으면 복사 후 해제, 없으면 붙여넣기 (Windows Terminal 방식) |

단어 경계 문자: `` ,│`|:"' ()[]{}<>\t `` (alacritty 기본값). 복사 시 줄 끝 공백 제거, 소프트랩 줄은 개행 없이 이어 붙임, 전각 문자 보존.

---

## 5. 한글 IME

- `TerminalView`가 GPUI `EntityInputHandler`를 구현하고, 요소 paint에서 `window.handle_input(focus, ElementInputHandler::new(bounds, view))` 등록 (포커스 있을 때만 GPUI가 활성화).
- 조합 중 문자열(`replace_and_mark_text_in_range`)은 **커서 위치에 밑줄 + 캐럿과 함께 그린다** (전각 폭 계산은 `unicode-width`). PTY에는 아직 안 보낸다.
- 확정(`replace_text_in_range`, IME GCS_RESULTSTR 또는 일반 WM_CHAR) 시 UTF-8로 sink에 전송.
- 후보 창 위치: `bounds_for_range`가 커서 셀(숨겨진 커서라도 VT가 아는 위치)을 반환 → Windows가 그 아래에 후보 창 표시.
- 조합 중에는 키 핸들러가 키를 가로채지 않는다(조합 키는 Windows에서 VK_PROCESSKEY로 오고 GPUI가 키 이벤트로 만들지 않음).

**자동 검증 (2026-11-08)**: `drive-and-capture.ps1 -Scenario ime` — 창의 기본 IME 창에 `WM_IME_CONTROL`로 한글 모드를 켜고 실제 키 `g k s r m f` 입력 → `echo 한` 확정 + `글` 조합 중 화면 캡처, Space로 확정 → PowerShell이 `한글` 표시. (`target/captures/ime-composing.png`, `ime-committed.png`)

**수동 테스트 절차** (자동화가 다루지 못한 부분 포함):
1. `cargo build --release` → `target\release\pod-spike.exe --terminals 2 --cmd "powershell.exe -NoLogo"` (서브 모니터에 뜸).
2. 첫 터미널 클릭, 한/영 키로 한글 모드.
3. `ㅎㅏㄴ` 입력 중: 커서 자리에 `한`이 밑줄과 함께 보이고 후보/조합 창이 커서 근처에 있어야 함. 셸에는 아직 안 보임.
4. `ㄱ` 입력 → `한` 확정되어 셸에 표시, `ㄱ` 조합 시작.
5. 조합 중 **Enter**: 조합 글자가 확정된 뒤 명령이 실행되어야 함 (글자 유실/Enter 유실 없음).
6. 조합 중 **Backspace**: 조합 글자의 자모가 지워져야 하고 셸의 앞 글자가 지워지면 안 됨.
7. 조합 중 **←/→ 또는 마우스 클릭**: 조합이 확정되고 커서 이동.
8. 한자 키(조합 중 `한` → 한자 변환 후보) 선택 → 선택한 한자가 확정되어야 함.
9. 다른 터미널 클릭(포커스 이동) 시 조합 중이던 글자가 원래 터미널에 확정되거나 버려지고, 새 터미널로 새지 않아야 함.
10. Claude Code(`--cmd "cmd.exe /c claude"`)에서 3–5 반복 — 시작 화면까지만 띄우고 프롬프트 입력은 사람이 확인.

---

## 6. 렌더링 효율 — 바뀐 터미널만 다시 그리기

- `TerminalView`는 엔티티이고, 출력이 들어오면 **자기만** `cx.notify()`. 호스트는 `view.cached(style)`로 배치 → 알림이 없던 뷰는 GPUI가 지난 프레임의 prepaint/paint 결과(히트박스·마우스 리스너·입력 핸들러 포함)를 재사용.
- 참고: gpui-component Dock의 `TabPanel`도 패널을 `cached`로 감싼다 (`tab_panel.rs:805`). 그래서 스파이크에서 "모두 다시 그리기" 기준선을 만들려면 `--no-view-cache`가 매 출력마다 `cx.refresh()`(모든 뷰 캐시 무시)를 호출하게 했다 — 뷰 캐시가 없는 앱의 동작과 같다.
- 측정 지표(새로 추가): `paints/frame` = 프레임당 실제로 paint된 터미널 수, `paint ms/frame` = 프레임당 터미널 paint CPU 합.

---

## 7. 측정 결과 (Ryzen 7 7800X3D, RTX 4060 Ti, 서브 모니터 150%, `--no-throttle`, 30초)

| 시나리오 | fps 평균 / 최저 | 터미널 paints/frame | paint ms/frame | 프레임 CPU 평균 / 최대 | 느린 프레임 |
|---|---|---|---|---|---|
| 16개 모두 최대 출력 `--terminals 16 --stress` (최종 바이너리, 조용한 상태) | **69.5 / 64.2** | 14.65 | 2.62 | 4.60 / 13.19 ms | 0 |
| 같은 조건, 개발 중간 바이너리 | 69.3 / 65.0 | 14.92 | 2.56 | 4.44 / 9.93 ms | 0 |
| 같은 조건 재측정 (다른 워크트리가 rustc 16개로 빌드 중) | 70.2 / 23.1\* | 10.35 | 4.00 | 6.80 / 102.9\* ms | 1\* |
| 1개 출력 + 15개 유휴(프롬프트만) · **전부 다시 그리기 기준선** (`--no-view-cache`) | 65.8 / 63.3 | **16.00** | 0.459 | 2.29 / 4.08 ms | 0 |
| 같은 조건 · **캐시 (기본)** | 65.4 / 61.5 | **1.00** | 0.235 | 1.53 / 2.75 ms | 0 |
| 1개 출력 + 15개 유휴(화면 가득 텍스트, `--idle-cmd "cmd.exe /c dir /s …"`) · 기준선 | 65.7 / 61.7 | **15.99** | 0.927 | 2.95 / 4.95 ms | 0 |
| 같은 조건 · **캐시 (기본)** | 64.4 / 53.4† | **1.00** | 0.262 | 1.67 / 3.01 ms | 0 |

\* t=2s 시작 샘플 1개(첫 프레임 폰트 로딩 100ms). 이후 매초 63–75fps.
† 30초 중 1초만 53.4 (느린 프레임 0, 프레임 CPU 최대 3ms → CPU가 아니라 이 시각 다른 부하로 추정).

- **판정**: 16개 최대 출력 60fps 이상 유지 ✅. 1개만 바쁠 때 프레임당 paint 수 16 → 1, 터미널 paint CPU 0.93 → 0.26ms(−72%), 프레임 CPU 2.95 → 1.67ms(−43%).
- 스파이크(이전) 대비 변화: VT 파싱이 alacritty EventLoop의 PTY 스레드 → UI 스레드(`feed`)로 이동. 16×~3,000줄/s에서도 프레임 CPU 4–7ms로 여유. 바이트는 읽기 스레드에서 모아 UI 깨우기 1회당 `feed` 1회.
- 재현:
  ```powershell
  & tools/run-and-capture.ps1 -AppArgs "--terminals","16","--stress","--no-throttle","--duration","30" -ShotsAt 10
  & tools/run-and-capture.ps1 -AppArgs "--terminals","16","--stress","--stress-count","1","--idle-cmd",'"cmd.exe /c dir /s C:\Windows\System32\drivers"',"--no-throttle","--duration","30" -ShotsAt 12
  #   … 위에 "--no-view-cache" 추가 = 기준선
  ```

---

## 8. 검증 스크린샷 (`target/captures/`, `crates/pod-spike/tools/drive-and-capture.ps1`로 실제 마우스·키·IME 입력 후 캡처)

| 파일 | 내용 | 확인 |
|---|---|---|
| `select-drag.png` | 드래그 선택 강조 | Ctrl+Shift+C 후 클립보드 = 선택한 표 6줄 그대로 |
| `select-word.png` / `select-line.png` | 더블 클릭 단어 / 트리플 클릭 줄 | 클립보드 `pod_terminal_view` / 줄 전체 |
| `scroll-wheel-up.png`, `scroll-shift-pgup.png` | 휠·Shift+PgUp 스크롤백 + 오른쪽 스크롤바 썸 | 400줄 출력 후 229–270행 표시 |
| `scroll-after-typing.png` | 입력하면 맨 아래로 | |
| `vim-click.png`, `vim-drag-visual.png`, `vim-wheel.png` | `vim -S`(set mouse=a number) | 드래그 → vim VISUAL 16–23행, 휠 → vim 스크롤 (로컬 선택 안 생김) |
| `ime-composing.png`, `ime-committed.png` (+`-zoom`) | 한글 조합 중 `글` 밑줄 프리에딧 / 확정 후 `한글` | |
| `diagonals.png` (+`-zoom`) | ╱╲╳ 직접 그리기, 행 사이 이음새 없음 | |
| `final-16-stress/shot-10.png`, `one-flood-full-*/shot-12.png` | 성능 측정 화면 | |

---

## 9. pod-spike 변경 사항

- `TerminalPanel` = `TerminalView` + 로컬 ConPTY sink(`pty.rs`, portable-pty 0.9). VT·렌더러·boxdraw 코드는 크레이트로 이동(스파이크에서 삭제).
- 새 옵션: `--stress-count K`(앞 K개만 출력 폭주), `--idle-cmd "<cmd>"`(나머지 터미널 프로그램), `--no-view-cache`(전부 다시 그리기 기준선).
- `--cmd`가 큰따옴표 인자를 지원 (`vim -c "set mouse=a"`). 프로그램 이름은 PATH+PATHEXT로 찾음(portable-pty는 `.exe`를 안 붙여 `vim`을 못 찾았음).
- 탭 제목이 "Unnamed"로 나오던 문제 수정 (gpui-component `Panel::title` 미구현이 원인 — `tab_name`만으로는 부족).
- 통계 줄에 `paints/frame`, `paint ms/frame` 추가.

---

## 10. 알려진 한계 / 다음 할 일

| 항목 | 상태 |
|---|---|
| 응용 키패드 모드(DECKPAM) | 모드는 추적하지만 GPUI Windows가 숫자 키패드를 일반 숫자키와 구분해 주지 않아(`VK_NUMPAD*` → `"0"`…) 키패드 전용 시퀀스(`SS3 p`…)를 보낼 수 없음. 숫자 키패드는 숫자로 입력됨 |
| X10 마우스 모드(9) | alacritty가 추적하지 않음 → 1000/1002/1003만. (X10 모드를 쓰는 앱은 거의 없음) |
| kitty 키보드 프로토콜, modifyOtherKeys | 미지원 (`Config.kitty_keyboard=false`, 질의에 응답 안 함 → 앱이 레거시 모드로 동작). Shift+Enter는 `CR`과 구분 안 됨 |
| 선택 vs 가득 찬 스크롤백 | 스크롤백이 상한(5000줄)에 도달한 상태에서 출력이 오면 줄 번호가 밀리므로 선택을 해제함 → 계속 폭주하는 터미널에서는 선택 유지 불가. 해결: 엔진이 "누적 스크롤 줄 수"를 제공하도록 (ghostty 전환 때) |
| 스크롤바 | 표시만 (드래그·클릭 이동 없음) |
| 드래그 자동 스크롤 | 마우스가 움직일 때만 한 줄씩 (타이머 없음) |
| 가운데 클릭 붙여넣기, 우클릭 메뉴 | 없음 (우클릭 = 복사/붙여넣기) |
| OSC 52 읽기 | 보안상 거부 (쓰기만 허용) |
| OSC 8 하이퍼링크, 검색, 커서 깜빡임, 이모지 폰트 대체 확인 | 미구현 / 미검증 |
| 폭 0 문자(결합 문자, ZWJ, VS16) | 셀의 `zerowidth()`를 `Snapshot.zerowidth`(셀 번호 → 문자)로 옮기고, 그런 셀은 글자 + 폭 0 문자를 한 덩어리로 모양 잡아 1칸(넓은 글자는 2칸)에 맞춘다. 복사(`RowText.zerowidth`)에도 들어간다. ZWJ로 이은 다음 이모지는 alacritty가 따로 칸을 잡는다(ZWJ는 앞 칸에 붙음). `segments()`는 ZWJ 뒤 이모지·피부색 수식자·국기의 두 번째 지역 표시자를 앞 덩어리에 이어 여러 칸에 걸친 **한 shaping 덩어리**로 만든다(`continues_cluster`, 201aa0e, ui-editor-08). 배치는 `cluster_fit`: 글꼴이 하나의 글리프로 그리면 원래 크기로 전체 칸 가운데, 조각으로 그려 넘치면 칸에 맞게 축소, 확대는 하지 않는다. 칸 수는 alacritty 그대로(가족 👨‍👩‍👧 6칸, 피부색·국기 4칸; 짝 없는 지역 표시자는 따로 2칸)라 커서·선택·복사 위치는 바뀌지 않는다. 테스트: `emoji_sequences_across_cells_are_one_cluster`, `joined_cluster_is_centered_never_enlarged`. 실제 화면 캡처는 미확인 |
| 넓은 글자 그리기 | small-fixes(docs/09 §13.1): 2칸 글자는 자연 폭으로 모양을 잡아 2칸에 맞게 키우거나(최대 1.35배) 줄이고 가운데 정렬(`element::wide_fit`) — 대체 글꼴(맑은 고딕) 한글이 0.92 em이라 생기던 음절 사이 빈칸 해결, Windows Terminal과 비슷한 밀도. 이모지는 Segoe UI Emoji로 그려짐(칸 안에서 작게 보임) |
| IME | Space 확정까지 자동 검증. Enter·Backspace·방향키·한자 변환·포커스 이동 중 조합은 §5 수동 절차로 확인 필요. 커서를 숨기고 자체 커서를 그리는 TUI는 실제 커서 위치에 프리에딧이 그려짐 (Claude Code는 입력 위치에 실제 커서를 둠 — 확인 필요) |
| 마우스 hover 판정 | GPUI `is_hovered`는 키 입력 직후(마우스가 움직이기 전) false → 휠은 `should_handle_scroll`로 판정해 해결. 클릭은 GPUI가 마우스 입력 시 모달리티를 바꾸므로 문제 없음 |
| podd 연동 | 미연결 (§3 설계만). 쿼리 응답 중복·크기 소유권·attach 복원 정책은 podd 쪽 결정 필요 |
| 스크린샷 스크립트 | 실제 마우스 포인터를 움직임. 내비게이션 키는 `KEYEVENTF_EXTENDEDKEY` 필요(없으면 숫자패드로 해석되어 Shift가 빠짐) |


## D2 host palette boundary

`TerminalPalette` supplies host defaults as 0xRRGGBB values (foreground, background,
cursor, selection_bg, ANSI 16). `Default` exactly preserves the original colors.
`TerminalView::set_palette(palette, cx)` repaints immediately and forwards to
`VtEngine::set_palette`; custom engine adapters must forward this method.
The alacritty adapter resolves OSC 4/10/11/12 overrides before host defaults,
preserves truecolor SGR, and keeps xterm 16-255 independent of the host theme.
Pod app's AttachVt retains the palette when replacing its engine on reconnect.

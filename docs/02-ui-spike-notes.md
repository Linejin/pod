# P0' UI 스파이크 기술 노트 (pod-spike)

작성일: 2026-11-08 · 코드: `crates/pod-spike` · 결과 요약은 [01 기획 문서 §6.1](01-plan.md) 참고.
이 문서는 **다음 작업자가 다시 조사하지 않아도 되도록** 환경·API·함정·측정 방법을 기록한다.

---

## 1. 환경 구성

| 항목 | 값 |
|---|---|
| Rust | rustup(winget `Rustlang.Rustup`) → stable 1.99.1 msvc, `%USERPROFILE%\.cargo\bin` |
| 빌드 도구 | VS 2022 Community MSVC 14.40, Windows SDK 10.0.26100 (기존 설치) |
| Zig | 미설치 (ghostty-vt 전환 시 Zig 0.17 필요) |
| 하드웨어 | Ryzen 7 7800X3D, RTX 4060 Ti + AMD iGPU, Citrix 가상 디스플레이 어댑터 |
| 모니터 | DISPLAY1 2560×1440 100% (메인) / DISPLAY2 서브 150% 스케일 |

> PowerShell 새 세션에서 `cargo`가 PATH에 없을 수 있음 → `$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"`

## 2. 빌드·실행

```bash
cargo build --release -p pod-spike
```
```bash
target\release\pod-spike.exe --terminals 16 --stress
```

| 옵션 | 의미 |
|---|---|
| `--terminals N` / `-n N` | 터미널 개수 (1–64, 기본 4), 정사각에 가까운 그리드로 배치 |
| `--stress` | 각 터미널에서 PowerShell 컬러 출력 폭주 실행 |
| `--batch N` `--sleep-ms M` | stress 출력량: N줄을 M ms마다 (기본 50줄/16ms ≈ 3,000줄/s) |
| `--cmd "<prog> <args>"` | 각 터미널에서 실행할 프로그램 (공백으로만 분리 — 따옴표 인자 불가) |
| `--display primary\|secondary\|N` | 띄울 모니터 (기본 secondary, 없으면 primary) |
| `--no-throttle` | 비활성 창 30fps 제한 끄기 (성능 측정용) |
| `--duration S` | S초 후 종료하며 `[summary]` 출력 (자동 측정용) |

- 매초 `[stats]` 줄을 stdout에 출력, 상태바에도 표시.
- 릴리스 빌드는 GUI 서브시스템 → 콘솔 창 안 뜸. stdout 리다이렉트 시 파일로 출력되고, 터미널에서 직접 실행하면 부모 콘솔에 붙어 출력.

## 3. 구조

```
main.rs     Workspace(루트 뷰): DockArea 그리드 + 상태바, 통계 타이머, 모니터 선택
panel.rs    TerminalPanel: 세션 소유, PTY 이벤트 펌프(cx.spawn), 키 → xterm 바이트
element.rs  TerminalElement(커스텀 GPUI Element): 그리드 렌더러
terminal.rs TerminalSession: alacritty_terminal Term + EventLoop(ConPTY) 래퍼, snapshot()
stats.rs    프레임/페인트 카운터
```

렌더링 방식 (`element.rs`):
1. `prepaint`: 셀 크기 계산 → 그리드 크기 변경 시 Term·PTY resize → 락 안에서 `snapshot()` (0xRRGGBB로 해석된 셀 배열 복사)
2. `paint`: 같은 배경색이 이어지는 칸은 quad 하나로 합쳐 그림 → 행별 텍스트를 세그먼트로 나눠 `shape_line(.., force_width = cell_w)` → 커서
3. 세그먼트는 **전각 문자에서만 끊음** → 고정 글자 폭으로 모노스페이스 정렬 유지
4. PTY 스레드의 Wakeup은 `AtomicBool`로 합쳐서(coalesce) UI에 한 번만 전달

VT 엔진은 `TerminalSession`/`Snapshot` 경계 뒤에 있음 → libghostty-vt로 교체 시 `terminal.rs`만 바꾸면 됨.

## 4. 사용한 API (gpui-kit 0.9 / gpui-pre 0.3.11 / alacritty_terminal 0.26)

- 앱: `gpui_kit::application().run(|cx| { gpui_kit::init(cx); gpui_kit::open_window(opts, cx, |window, cx| cx.new(..)) })` — `open_window`가 Root를 자동으로 감쌈
- `use gpui_kit::*;` = gpui 전체. `.when()` 은 `gpui_kit::prelude::FluentBuilder` 필요
- Element trait: `request_layout / prepaint / paint` + `id`, `source_location` (a11y 메서드는 기본 구현 있음)
- 텍스트: `window.text_system().shape_line(text, font_size, &[TextRun], Some(force_width))` → `ShapedLine::paint(origin, line_height, TextAlign::Left, None, window, cx)`
- 폰트 측정: `resolve_font(&font)` → `advance(font_id, size, 'm')`
- 사각형: `window.paint_quad(fill(bounds, color))`, `outline(bounds, color, BorderStyle::Solid)`
- `Pixels`의 내부 값은 private → `.as_f32()`
- Dock: `DockSkin::dock_area(id, None, window, cx)` → `area.set_center(DockLayout::v_split().child(DockLayout::h_split().child(DockLayout::tabs().panel_view(..), None), None), window, cx)`
  - **탭 제목을 쓰려면** `PanelHandle::new(entity)`로 감싸서 `panel_view`에 넘겨야 함 (`.panel(entity)`로 넘기면 `panel_name` 문자열이 표시됨)
  - 패널은 `gpui_base::dock::Panel` (+ `EventEmitter<PanelEvent> + Render + Focusable`) 과 `gpui_component::dock::Panel`(tab_name) 둘 다 구현
- 테마: `gpui_kit::component::theme::Theme::change(ThemeMode::Dark, None, cx)`
- 모니터: `cx.displays()`, `cx.primary_display()`, `WindowOptions.display_id`, `WindowBounds::Maximized(Bounds::centered(Some(id), size, cx))`
- 프레임 제한: `WindowOptions.inactive_frame_interval` (기본 `Some(33.333ms)`), `low_power_frame_interval`, `thermal_frame_interval`
- alacritty: `tty::new(&Options{shell, env, ..}, WindowSize, 0)` → `EventLoop::new(term, listener, pty, false, false)` → `.channel()` / `.spawn()`; 입력은 `Msg::Input`, 크기는 `Msg::Resize`
  - **`Event::PtyWrite` 응답(DA/DSR)을 PTY로 되돌려 보내야** vim 등이 멈추지 않음 → listener에 `OnceLock<EventLoopSender>`
  - 색: `vte::ansi::Color { Named, Indexed, Spec }`, `Colors[..]`가 `None`이면 기본 팔레트 사용

## 5. 함정과 해결

| 증상 | 원인 | 해결 |
|---|---|---|
| fps가 정확히 30으로 떨어지고 그대로 유지 | GPUI가 **포커스 없는 창을 30fps로 제한** (기본값). 백그라운드 셸에서 실행하면 Windows의 포그라운드 잠금 때문에 창이 활성화되지 않음 | 제품: 그대로 둠(절전). 측정: `--no-throttle` |
| 서브 모니터에서 화면이 1.5배로 잘려 보임 | **캡처 스크립트가 DPI 비인식** (150% 모니터에서 논리 좌표로 창 크기를 읽음). 앱은 정상 | 캡처 프로세스에서 `SetProcessDpiAwarenessContext(-4)` 후 `PrintWindow(hwnd, hdc, 2)` |
| GPUI가 서브 모니터를 (1707,157)로 보고 | 서브 모니터 실제 위치(2560,235)를 1.5로 나눈 값 — 배율 섞인 좌표 | 크기를 직접 지정하지 않고 `Maximized`로 띄움 |
| 앱 실행 시 메인 모니터에 cmd 창이 뜸 | 콘솔 서브시스템 exe | `#![windows_subsystem = "windows"]` (release) + `AttachConsole(ATTACH_PARENT_PROCESS)` |
| `CopyFromScreen` 캡처에 다른 앱이 찍힘 | 창이 다른 창 뒤에 있음 | `PrintWindow`로 해당 창만 캡처 |
| 탭 제목이 "Unnamed" | 셸이 빈 제목을 보냄 | 빈 제목은 무시 |
| `--cmd`의 따옴표 인자가 깨짐 (vim E10) | `split_whitespace`로 분리 | 스파이크 한계. 실제 제품은 argv 배열로 받을 것 |
| cargo: `gpui-kit` feature `gpui-component` 없음 | feature 이름은 `component`(기본 켜짐) | 기본 feature 사용 |

## 6. 측정 방법

- **fps** = 루트 뷰 `render` 호출 수/초. GPUI는 매 프레임 창 전체를 다시 그리므로(뷰 캐시 미사용) 프레임 수와 같음. 출력이 없으면 다시 그리지 않으므로 출력이 적을 때 fps가 낮게 나오는 건 정상.
- **프레임 CPU** = 루트 render 시작 → 마지막 터미널 paint 끝. GPU 시간은 포함 안 됨.
- **slow** = 프레임 CPU > 16.6ms인 프레임 수.
- 자동 측정 + 캡처 스크립트: [tools/run-and-capture.ps1](../tools/run-and-capture.ps1) — 앱을 실행하고 지정 시각에 창만 캡처(DPI 인식, PrintWindow), 끝나면 `[stats]`/`[summary]` 출력. 결과는 `target/captures/`.
  ```bash
  powershell -File tools/run-and-capture.ps1 -AppArgs "--terminals","16","--stress","--duration","30","--no-throttle" -ShotsAt 10
  ```

## 7. 알려진 결함 / 다음 할 일

1. ~~블록·박스 그리기 문자(U+2500–259F) 줄 사이 틈~~ **수정됨** — `boxdraw.rs`가 셀 크기 quad로 직접 그림(디바이스 픽셀 스냅, 텍스트에서는 공백으로 대체해 정렬 유지). 배경 run quad도 같은 스냅 사용. 대각선 ╱╲╳(U+2571–2573)만 아직 폰트 글리프
2. ~~Claude Code 로고 색 미표시~~ **수정됨** — 원인: pod-spike를 띄운 셸(에이전트 도구 셸)의 `NO_COLOR=1`이 ConPTY 자식에 상속 → Claude Code가 SGR 0개 출력. alacritty `Options.env`는 추가만 가능하므로 `main` 시작 시 `NO_COLOR` 제거(`POD_SPIKE_KEEP_NO_COLOR=1`이면 유지). 디버그: `POD_SPIKE_RECORD=1`이면 PTY 원본 출력을 `./alacritty.recording`에 기록. 참고: `CLAUDECODE`, `CLAUDE_CODE_CHILD_SESSION` 등 다른 에이전트 변수도 그대로 상속됨(제품에서는 자식 환경을 명시적으로 구성할 것)
3. 뷰 캐시 미사용 → 한 터미널만 바뀌어도 전체 재렌더. 터미널 수가 많아지면 변경된 패널만 다시 그리도록 최적화 여지
4. 키 매핑 최소 구현 (F5+, Alt 조합 일부, 마우스 리포팅, 붙여넣기, 선택/복사 없음)
5. 스크롤백 UI 없음 (Term은 5,000줄 보관)
6. VT 엔진 ghostty-vt 전환 (Zig 0.17 설치 필요)

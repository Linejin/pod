# Pod 기획 (v0.1)

작성일: 2026-11-08 · 이 문서는 Pod의 제품 목표, 구조, 기술 스택, 로드맵, 결정 사항, 백로그를 담는다.

---

## 1. 제품 목표

- 여러 코딩 에이전트(Claude Code, Codex 등)를 **동시에 띄우고 → 지켜보고 → 검토해서 → 머지하는** 과정을 한 앱에서 끝낸다.
- 에이전트 터미널은 **가볍고 죽지 않아야** 한다: UI를 꺼도, 앱을 업그레이드해도 에이전트는 계속 돈다.
- 에이전트 상태(작업 중·막힘·완료·오류)를 **정확하게, 출처와 함께** 보여 준다.
- 작업 단위는 **git worktree + 터미널 트리**다. 에이전트가 하위 에이전트를 띄우고 기다리는 구조를 터미널 수준에서 지원한다.
- 리뷰(diff·라인 코멘트)와 사람 승인(Gate)을 에이전트 흐름 안에 넣는다.
- 네이티브(GPU) UI, Windows 먼저, 오픈소스(Apache-2.0).

## 2. 동기

Pod는 에이전트 오케스트레이션 IDE와 터미널 멀티플렉서에서 영감을 받았다. 앞의 것은 워크트리·리뷰·통합 워크플로가 강하지만 웹 기술 기반이라 무겁고, 뒤의 것은 가볍고 세션이 죽지 않지만 편집·리뷰 같은 IDE 기능이 거의 없다. Pod는 둘의 장점을 하나로 묶되, 다른 제품의 코드·에셋은 쓰지 않고 공개된 프로토콜과 Pod 자체 설계로 처음부터 만든다(클린룸).

## 3. 설계 방향 요약

| 축 | Pod의 선택 |
|---|---|
| 런타임 | **Rust 코어 + GPUI 네이티브 UI** (§5) |
| 세션 영속 | **독립 코어 데몬** `podd` + holder 프로세스 |
| 터미널 엔진 | ConPTY(번들) + VT 엔진(alacritty_terminal, ghostty-vt 검토) |
| 상태 감지 | **3계층 결합**(§4.2): 훅 > OSC 7501 > 휴리스틱 |
| 작업 단위 | **Task = worktree + 터미널 트리** |
| 오케스트레이션 | 터미널 부모-자식 트리 위에 Gate/리뷰/그룹 비교 |
| 리뷰 | Combined diff, 라인 코멘트 → 에이전트 전송, Checks, PR |
| 확장 | **단일 확장 모델**(QuickJS 샌드박스) |
| 원격 | 코어 간 연결 + 모바일 승인 뷰어 |

---

## 4. Pod 기획

### 4.1 한 줄 컨셉
> **가볍고 죽지 않는 에이전트 터미널 코어** 위에 **워크트리·리뷰·통합 워크플로**를 얹은, "여러 에이전트를 띄우고 → 지켜보고 → 검토해서 → 머지하는" IDE.

### 4.2 핵심 설계
**(1) 코어 데몬 `podd` (Rust)**
- tokio + ConPTY/forkpty + VT 엔진, holder 분리로 UI 재시작에도 세션 유지
- 로컬 소켓 JSON-RPC, schemars로 스키마 공개 → UI·CLI·플러그인·원격이 같은 API 사용
- 저장소: SQLite (작업/세션/이벤트 로그)

**(2) 에이전트 상태 3계층 엔진**
1. *네이티브 훅* (Claude Code·Codex처럼 훅을 지원하는 에이전트 → 가장 정확)
2. *OSC 상태 프로토콜* (OSC 7501: Claude Code가 직접 내보내는 상태 시퀀스. 규격을 공개해 어떤 도구든 자가보고)
3. *휴리스틱* (포그라운드 프로세스 + 출력 패턴 + 무출력 시간)
→ 단일 상태 머신 `idle / working / blocked / done / error / unknown`, 출처·신뢰도를 함께 표시

**(3) Task = Worktree + Terminal Tree**
- 태스크 생성 시 git worktree 자동 생성(로컬/SSH/WSL), 그 안에서 루트 에이전트 실행
- 각 터미널에 `$POD_TERMINAL_ID` 주입, 에이전트는 `pod run/send/wait`으로 하위 에이전트 생성
- 같은 프롬프트를 N개 에이전트에 fan-out → 결과 비교 → 베스트 머지

**(4) 리뷰 & 머지 파이프라인**
- Combined diff + 라인 코멘트 → 코멘트를 해당 에이전트 터미널로 바로 전송
- 테스트/CI Checks 패널, PR 생성, GitHub/Linear/Jira 이슈 ↔ 태스크 링크
- "Gate": 사람 승인 지점(blocked 상태와 연동, 알림·사운드)

**(5) 화면 구성**
```
┌────────────┬──────────────────────────────────┬──────────────┐
│ 태스크 보드  │  메인: 에이전트 터미널 그리드      │ 우측 패널     │
│ (worktree별) │  / 에디터 / Diff 리뷰 탭           │ - 상태 트리    │
│ ● working   │                                  │ - Checks/PR   │
│ ▲ blocked   │  플로팅 패널                      │ - Notes       │
│ ✓ done      │                                  │              │
├────────────┴──────────────────────────────────┴──────────────┤
│ 상태바: 실행중 에이전트 수 · blocked 알림 · 포트 · 머신 · 리소스    │
└───────────────────────────────────────────────────────────────┘
```

**(6) 확장**: JS 샌드박스(QuickJS) 하나로 사용자 모드·레이아웃·플러그인(패널·상태바·명령)을 통합. 에이전트용 Skills는 CLI 가이드로 배포.

**(7) 원격/모바일**: 코어끼리 `machine/terminal` 주소로 연결, 모바일은 읽기+승인(blocked 응답) 중심의 웹 뷰어 — E2EE 페어링.

### 4.3 범위 밖 (범위 통제)
- MVP에서 제외: computer-use, 에뮬레이터, 음성 STT, 펫/온보딩 투어, 다국어 i18n, native chat UI
- 다른 제품의 코드·에셋은 복사하지 않는다 — **클린룸 구현**

---

## 5. 기술 스택 — **결정: GPUI 네이티브** (2026-11-08)

검토했던 옵션: A. Tauri+웹 UI / **B. GPUI 네이티브 (채택)** / C. Electron.
채택 이유: 핵심 가치가 "풀 IDE 워크플로를 네이티브 터미널 앱의 가벼움·속도로"이므로 UI도 네이티브로 간다.

### 5.1 스택
| 레이어 | 선택 | 비고 |
|---|---|---|
| 언어/런타임 | Rust (workspace), tokio | 코어·앱 단일 언어 |
| UI 프레임워크 | **gpui** (Zed, Apache-2.0) | |
| 컴포넌트 | **gpui-component** (Longbridge, Apache-2.0) | Dock/탭/리사이즈/가상 리스트/테이블 + **코드 에디터(Tree-sitter, LSP 진단·완성·호버, 20만 줄)** (crates.io 패키지명 `gpui-kit`) |
| 레이아웃 | taffy (gpui 내장) | |
| 터미널 VT | libghostty-vt (Rust 바인딩) | 대안: alacritty_terminal (Zed 방식, 현재 사용) |
| PTY | ConPTY(Windows) / portable-pty | |
| Git | gix(gitoxide) + 필요한 경우 `git` CLI | worktree 조작은 CLI가 안정적 |
| Diff | `imara-diff` / `similar` + 자체 GPUI 뷰 | |
| 마크다운 | gpui-component markdown/`pulldown-cmark` | |
| 확장 | QuickJS (rquickjs) 샌드박스 | |
| 저장소 | SQLite (rusqlite) | |
| 원격 | russh (SSH), 코어 간 연결은 자체 프로토콜 + TLS/Noise | |

> ⚠️ **라이선스 경계**: gpui·gpui-component는 Apache-2.0이지만 **Zed의 `editor`/`terminal_view`/`git_ui` 등 앱 크레이트는 GPL/AGPL** → 직접 가져오지 말고 참고만. 의존성 추가 시 `cargo-deny`로 라이선스 정책 강제.

### 5.2 GPUI 선택으로 직접 만들어야 하는 것 (리스크)
| 컴포넌트 | 난이도 | 전략 |
|---|---|---|
| 터미널 그리드 렌더러 | 중 | ghostty-vt 셀 그리드 → GPUI 텍스트 런 배치 렌더, 글리프 아틀라스 캐시. 패널 수십 개 가정해 **보이는 터미널만 렌더** |
| Combined diff 뷰 | 상 | 에디터 컴포넌트 재사용 + 좌/우·인라인 모드, 접힘, 라인 코멘트 앵커 — 가장 큰 작업 |
| 파일 트리/퀵오픈 | 하 | 가상 리스트 + 퍼지 매칭(nucleo) |
| 이미지/PDF/노트북 뷰어 | — | MVP 제외, 외부 앱으로 열기 |
| 내장 브라우저 | — | 제외 (GPUI에 웹뷰 없음). 필요 시 별도 창으로 시스템 브라우저/CDP |

### 5.3 프로세스 구조
```
pod-app (GPUI)  ──JSON-RPC/소켓──  podd (코어 데몬)  ── holder ── PTY들
   ▲                                   ▲
   └── 모든 UI 동작은 코어 API 호출      ├── pod CLI (에이전트가 사용)
                                       └── 원격 podd (machine/terminal)
```
UI는 상태를 소유하지 않는다 — 꺼도 에이전트는 계속 돈다.

---

## 6. 로드맵

GPUI 채택으로 UI 단계가 길어짐. 대신 **P0와 UI 스파이크를 병행**해 렌더 리스크를 초기에 검증한다.

| 단계 | 기간(가정) | 산출물 | 완료 기준 |
|---|---|---|---|
| P0 코어 | 3–4주 | `podd`: PTY + ghostty-vt + holder 영속, JSON-RPC, `pod` CLI(run/send/read/wait/attach) | 앱 없이 CLI만으로 Claude 실행·재attach |
| P0' UI 스파이크 (병행) | 2주 | GPUI 창 + gpui-component Dock + 터미널 그리드 렌더러 1개 | 터미널 16개 동시 출력 60fps, `vim`/Claude TUI 정상 표시 |
| P1 상태 | 2주 | 3계층 상태 엔진, Claude/Codex 훅 어댑터, OSC 프로토콜 스펙 v1 | 상태 오판율 측정 가능 |
| P2 UI MVP | 5–6주 | 태스크 보드, 터미널 그리드/플로팅, 상태 트리, 알림·사운드, 설정 | 일상 사용 가능 (dogfooding 시작) |
| P3 워크트리·편집 | 3주 | worktree 생성/정리, 파일 트리, 퀵오픈, gpui-component 에디터 + LSP | 간단한 수정은 앱 안에서 |
| P4 리뷰 | 5주 | Combined diff 뷰, 라인 코멘트→에이전트 전송, Checks, PR 생성 | PR까지 앱 밖으로 안 나감 |
| P5 오케스트레이션 | 3주 | fan-out/compare, Gate, 부모-자식 트리 시각화 | |
| P6 확장·원격 | 이후 | QuickJS mods/레이아웃, SSH/WSL, 멀티머신, 모바일 승인 뷰어(웹) | |

---

## 6.1 P0' UI 스파이크 결과 (2026-11-08)

코드: `crates/pod-spike` — gpui-kit 0.9(gpui-pre 0.3.11) + gpui-component Dock + 자체 터미널 그리드 Element + alacritty_terminal 0.26(ConPTY).
환경: Ryzen 7 7800X3D, RTX 4060 Ti, 모니터 2대(메인 100%, 서브 150% 스케일), Windows 11.

| 시나리오 (`--no-throttle`) | 평균 fps | 최저 fps | 프레임 CPU 평균/최대 | 판정 |
|---|---|---|---|---|
| 16개 · 에이전트 수준 출력 (터미널당 ~100줄/s) | 60.5 | 48.6* | 4.6 / 11.3ms | ✅ |
| 16개 · 많은 출력 (~600줄/s) | 59.8 | 41.6* | 6.0 / 17.0ms | ✅ |
| 16개 · 최대 출력 (~3,000줄/s) | 66.5 | 60.1 | 6.9 / 19.0ms | ✅ |
| 32개 · 최대 출력 | 68.1 | 63.7 | 8.6 / 21.4ms | ✅ |

\* fps는 다시 그린 프레임 수. 출력이 적으면 새로 그릴 내용이 없는 프레임이 생겨 낮게 나옴(그 구간에서도 느린 프레임 0개).

- **기준 1 (16개 동시 출력 60fps): 통과.** 32개도 60fps 이상.
- **기준 2 (TUI 표시)**: vim(구문 강조/truecolor), Windows `edit.exe`(한글 메뉴·전각 문자) 정상. Claude Code 시작 화면 표시됨. 단 아래 결함 있음.
- **발견 1 — GPUI는 비활성 창을 30fps로 제한**(`WindowOptions.inactive_frame_interval` 기본 33.3ms). 제품에서는 유지(백그라운드 절전), 측정 시 `--no-throttle`.
- **발견 2 — 블록/박스 문자 줄 간격 틈 → 수정됨**: 줄 높이 1.3배라 폰트 글리프가 셀을 다 못 채움. U+2500–259F(직선·모서리·굵은선·이중선·둥근 모서리·점선·블록·4분할·음영)를 셀 크기 사각형으로 직접 그림(`boxdraw.rs`, 디바이스 픽셀 스냅). 대각선 ╱╲╳만 폰트 사용. 성능 영향 없음(16개 최대 출력 67fps, 프레임 CPU 4.9ms).
- **발견 3 — Claude 로고 색 미표시 → 수정됨**: 원인은 터미널이 아니라 **실행한 셸에서 상속된 `NO_COLOR=1`**(에이전트 도구 셸이 설정). ConPTY 자식은 부모 환경을 전부 물려받으므로 Claude Code가 SGR을 하나도 출력하지 않았음(PTY 원본 바이트로 확인). 시작 시 `NO_COLOR` 제거(`POD_SPIKE_KEEP_NO_COLOR=1`로 유지 가능). ConPTY는 24비트 SGR을 그대로 전달함.
- 서브 모니터 실행: `--display secondary`(기본) — 150% 스케일에서 정상.


## 7. 결정 사항
- [결정 완료] UI 스택 → GPUI 네이티브 (§5)
- [결정 완료, 2026-11-08] 대상 OS → **Windows 먼저** (ConPTY, named pipe). macOS/Linux는 이후
- [결정 완료] 지원 에이전트 → **Claude Code와 Codex 둘 다** 1순위
- [결정 완료] **오픈소스** (워크스페이스 라이선스 Apache-2.0) → 의존성은 Apache/MIT 호환만, GPL/AGPL 금지
- [결정 완료] 외부 도구(Zig 등)는 필요할 때 설치
- [결정 완료] 원격 저장소는 나중에 연결 (지금은 로컬 git만)
- [결정 완료] 에이전트 훅은 **전역 설정에 설치**(`~/.claude`, `~/.codex`). Pod 밖에서는 아무 일도 하지 않음. `pod-hook.exe`는 고정 위치(`%LOCALAPPDATA%\Pod\bin`)로 복사해 연결
- [결정 완료] **Codex 폴더 신뢰는 자동**: Pod가 만든 워크트리 경로만, 작업 삭제 시 함께 제거

## 9. UI 다듬기 계획 (2026-11-08 추가)

목표: 기능을 다 갖춘 뒤가 아니라 **P2와 나란히** 디자인을 정해, 화면을 만들 때부터 같은 규칙으로 만든다. 사이드바 중심 작업 목록, 차분한 색, 상태 배지, 명령 팔레트를 기본으로 하고 네이티브 앱의 가벼운 느낌을 유지한다.

원칙
- **독자 디자인**: 다른 제품의 아이콘·이미지·CSS를 가져오지 않는다. 아이콘은 라이선스가 맞는 공개 세트(Lucide, ISC)를 쓴다.
- **토큰으로 관리**: 색, 간격, 모서리, 글꼴 크기, 그림자, 애니메이션 시간을 한 곳(`pod-ui`)에 정의하고 화면 코드는 토큰만 쓴다. 다크·라이트 두 테마.
- **상태가 한눈에**: 에이전트 상태(작업 중·막힘·완료·오류)는 색과 모양 둘 다로 구분한다(색맹 대응).
- **한글 우선 확인**: 한글 글꼴 대체, 150% 배율 모니터에서 흐림·잘림이 없는지 매 단계 스크린샷으로 확인한다.
- **담당 에이전트**: D0–D3 등 디자인 작업은 Codex의 **Astra 모델**(`gpt-6-astra`)에 맡긴다. 그 밖의 엔지니어링 작업은 Claude가 기본이고, Codex도 함께 쓸 수 있다.

| 단계 | 내용 | 결과물 | 시점 |
|---|---|---|---|
| D0 디자인 언어 | Pod 디자인 원칙·색 토큰·상태 표현·화면별 와이어프레임 초안 | `docs/10-ui-design.md` | 지금 (코드와 무관) |
| D1 디자인 시스템 | `pod-ui` 크레이트: 토큰, 아이콘, gpui-component 테마 연결, 공용 컴포넌트(배지, 목록 행, 빈 화면, 토스트, 대화상자 틀) | 크레이트 + 견본 화면 스크린샷 | P2 골격 병합 후 |
| D2 앱 적용 | pod-app에 전면 적용: 사용자 정의 타이틀바, 사이드바(작업·터미널 묶음, 상태 배지), 탭 막대, 명령 팔레트(Ctrl+K), 알림, 빈 화면, 설정 화면 | 화면별 전후 스크린샷 | D1 후, P2 후속과 함께 |
| D3 마감 | 전환 애니메이션, 단축키 일관성, 키보드만으로 조작, 접근성(AccessKit), 배율·한글 글꼴, 시각 회귀 스크린샷 세트 | 체크리스트 통과 | 직접 써 보기(dogfooding) 시작 전 |


## 8. 백로그 (2026-11-08, 로드맵 순서 + 병렬 가능성 기준)

| 상태 | 작업 | 로드맵 | 먼저 필요한 것 |
|---|---|---|---|
| 완료 | D0 UI 디자인 언어 (Codex Astra): Pod 디자인 언어 v1(독자 색 토큰, 상태별 모양+한국어 문구), 화면별 와이어프레임, Lucide 아이콘 → `docs/10` | UI 다듬기 | — |
| 완료 | D1 디자인 시스템 `pod-ui` (Codex Astra): 토큰, gpui-component 연결, 상태 배지(모양+한국어 문구, 흑백 구분 확인), 공용 틀, Lucide, 견본 화면 → master `7059fce` | UI 다듬기 | — |
| 완료 | D2 1부 (Codex Astra): 다크·라이트 테마, 타이틀바, Ctrl+K 팔레트, 설정, 상태 배지 사이드바, 터미널 색상 API → master `18b4129` | UI 다듬기 | — |
| 완료 | D2 2부 (Codex Astra): 화면 문구 한국어 통일, 사이드바 검색·전체 이름, 설정 탭, 오른쪽 상태 인스펙터, 퍼지 검색·최근 항목, 알림 위치, 빈 리뷰 화면 수정 → master `6c97801` | UI 다듬기 | — |
| 완료 | D3 마감 (Codex Astra): 접근성 이름·포커스 테두리, 촘촘한 사이드바, 단축키 도움말, 움직임 줄이기, 고대비 미리보기, E2E UX 5건, 시각 회귀 기준 26장, gpui-component 접근성 패치(vendor, Apache) → master `0d9e36c` (테스트 321 통과). 사용자 수동 확인: 키보드만 조작·한글 IME·내레이터·125/175%·고대비 | UI 다듬기 | — |
| 완료 | 스파이크 결함 수정: 블록 문자 틈(박스·블록 문자 직접 그리기), Claude 로고 색(부모의 NO_COLOR 유출) | P0' | — |
| 완료 | podd 첫 단계: 데몬 + `pod` CLI + CLI 종료 후 터미널 유지 (브랜치 `SJin-Choi/podd-core`) | P0 | — |
| 완료 | R1 상태 감지 조사: Claude Code가 OSC 7501을 직접 보냄, Codex는 훅 필요, 기본 ConPTY에서는 7501 미동작(OpenConsole 번들 필요 추정) → `docs/04` | P1 사전 | — |
| 완료 | S2 diff 화면 스파이크: 실현 가능(위험 높음 → 중하), gpui-component 0.9의 Diff 컴포넌트 재사용, 1만 줄 diff 0.35초·75fps, P4 예상 3.5–5주 (브랜치 `SJin-Choi/diff-spike`, `docs/05`) | P4 위험 검증 | — |
| 완료 | podd P0 마무리: holder 프로세스(재시작에도 유지), attach(ANSI 스냅샷 + 순번 원시 바이트), 트리 종료, 환경변수 정리, OSC 7501 질의 응답, 번들 ConPTY(기본 ConPTY는 OSC 응답을 버림), cargo-deny → master 병합 `91b8733` | P0 | — |
| 보류 | VT 엔진 ghostty-vt 전환: 번들 ConPTY로 7501 문제가 해결돼 급하지 않음. alacritty_terminal 유지, 필요해지면 진행 | P0 | — |
| 완료 | 터미널 위젯 `pod-terminal-view`: 바이트 입력 VT, 키·마우스·붙여넣기·한글 IME·선택/복사·스크롤백, 바뀐 패널만 다시 그리기(1개만 바쁠 때 프레임 CPU 2.95→1.67ms) → master `dfd229b` | P2 | — |
| 완료 | 상태 엔진 라이브러리 `pod-status` + `pod-hook` (테스트 67개) → master `07c272d` | P1 | — |
| 완료 | P1b 상태 엔진 podd 연결: ls/show 상태, status.subscribe·wait, `pod wait --status`, 토큰 인증 훅 수신, 재시작 후 상태 복원(OSC 재생). **훅 전역 설치 완료**(2026-11-08, 백업 `*.pod-backup-20261108-130611`), Codex 자동 신뢰(`-c projects` 인라인 테이블, 설정 파일 쓰기 없음) → master `d887075` | P1 | — |
| 완료 | UI MVP `pod-app`: podd 자동 시작·재접속, 작업별 사이드바·상태 배지, dock attach, 새 터미널·작업 대화상자, 알림, 레이아웃 저장 → master. 16개 성능은 부하 상태에서 61.4/53.6fps로 재측정 중 | P2 | — |
| 완료 | 워크트리 API/CLI: 저장소 레지스트리, Task = worktree(`pod/<name>`) + 에이전트 터미널, ahead/behind·dirty·diff(numstat), 안전한 삭제, 재시작 생존 → master 병합, `docs/08` | P3 | podd |
| 완료 | 워크트리 화면: P2에서 작업별 사이드바·새 작업 대화상자·작업 정보·안전 삭제로 구현 | P3 | — |
| 완료 | P4/P5 백엔드: 리뷰 코멘트 → 에이전트 전달(작업 중이면 대기 후 전달), 여러 에이전트에 나눠 맡기고 비교·선택, Gate(사람 승인) → master `05856ad` | P4·P5 | — |
| 완료 | P4 리뷰 화면: 작업 diff, 라인 코멘트(podd 저장), 에이전트에 보내기(작업 중이면 대기) → master `405fc4c` (`pod-diff` 크레이트, 1만 줄 diff 0.4초) | P4 | — |
| 완료 | podd 보강: 자동 시작 시 핸들 상속 버그, 비동기 check, Gate 만료, 테스트 불안정, 종료된 터미널 정리 → master, 프로토콜 v6 | 공통 | — |
| 완료 | 오케스트레이션 화면 (그룹 비교, Gate 승인, 부모-자식 트리) → master `c845d81` (그룹 대화상자·멤버 카드·check·우승 선택·Gate 승인함·에이전트 트리, review.delivery 이벤트) | P5 | — |
| 완료 | podd 남은 결함 2: terminal.run 부모 기록, attach 중복 응답·큐 제한, 스냅샷 충실도, holder 버전 확인, 리뷰 대기 만료, 막힘 상태 빨리 해제 → master `1e193a6`, 프로토콜 v7 | 공통 | — |
| 완료 | P3 편집 크레이트 `pod-editor` (LSP 포함): 파일 트리(.gitignore·git 표시·자동 갱신), 빠른 열기(Ctrl+P, 자체 퍼지 검색: nucleo는 MPL이라 제외), 편집기(탭·저장 시 CRLF/BOM 유지·외부 변경 감지·찾기), LSP(오류·설명·정의 이동·자동 완성·참조·이름 바꾸기) → master `bc73f50`, `docs/13` | P3 | — |
| 완료 | P3 앱 연결: 작업별 파일 패널(트리·편집기 탭·문제 목록), Ctrl+P·팔레트·리뷰에서 열기(Ctrl+Shift+O), LSP 지연 시작·최대 2개·작업 삭제 시 종료, 에이전트 수정 시 다시 불러오기/확인, 저장 안 한 변경 확인, 탭·커서 복원, 트리 새 파일·이름 바꾸기·삭제 → master `111b3aa` (테스트 326, 시각 회귀 28/28). 남은 것: 터미널 경로 Ctrl+클릭, 파일당 독립 탭 | P3 | — |
| 완료 | 성능 재측정: pod-app 16개 폭주 55.2fps(같은 조건 pod-spike 48.7), 프레임 CPU 5ms·느린 프레임 0 → 렌더 비용 기준 통과. 리뷰 1만 줄 스크롤 74.9fps(모니터 상한) → `docs/09` §6.2.1 | 검증 | — |
| 완료 | 실제 Codex로 처음부터 끝까지 시험 (Codex Astra, Codex 턴 12회): 4 통과 / 4 실패, 버그 7건 → `docs/12`, master `a768119` | 검증 | — |
| 완료 | E2E 버그 수정 1: B1 리뷰 전달 후 실제 제출 확인(Enter 타이밍, 재시도), B2 승인하면 바로 작업 중, B6 데몬 재시작에도 대기 유지, 느린 테스트 28초→2초, 실제 Codex 3턴 확인 → master `752e59f`, 프로토콜 v8 | 검증 | — |
| 완료 | E2E 버그 수정 2 (앱): B3 하위 터미널 트리, B4 OSC 응답 섞임(전용 입력 연결 + 늦은 응답 버림), B5 알림 누락, B7 패널 복원, 제출 확인 표시와 Enter 누르기 버튼 → master `60dd481` | 검증 | — |
| 완료 | 제출 확인을 부하에 강하게: 출력 멈춤에 맞춰 Enter 대기 시간 조정(최대 5초), 테스트 경쟁 상태 수정 → master `b9cc0ee` (전체 테스트 271 통과) | 검증 | — |
| 완료 | E2E 재검증 (Codex Astra, Codex 7턴): B1~B7 모두 통과 — 승인 후 50ms에 작업 중, Esc 0.7초, 리뷰·긴 입력 Enter 1번에 제출, 데몬 재시작에도 Gate 대기 유지, 패널 복원, 하위 터미널 트리 → master `149c103`. 남은 UX 5건은 D3로 | 검증 | — |
| 완료 | 수동 확인 자동화: 앱 내부 테스트로 키보드만 조작·Esc 포커스 복귀·Tab 순서·한글 IME 5곳·배율 1.0~1.75 확인, UI Automation 13화면 이름 누락 0, OS 고대비 감지 추가, 접근성 버그 9건 수정(시작 시 포커스 없음, Enter 제출 안 됨 등) → master `a5614d6`, Shift+Tab 수정 `97c0909` (테스트 339). 잠금 때문에 미실행: 실제 키 입력·내레이터 음성·IME 후보 창·실제 125/175% | 검증 | — |
| 완료 | 설치 패키지: 사용자 단위 Inno Setup 설치 파일(12.2MB, 한국어 화면)과 휴대용 zip, 업그레이드해도 실행 중 터미널 유지, 터미널 열려 있으면 제거 거부, 훅은 제안만, 469개 crate 고지(GPL 없음), 버전 0.1.0 통일 → master `40f1b10`, `docs/14`. 남은 것: 코드 서명, 앱 아이콘, exe 버전 정보, 앱 안 훅 설치 안내 | 배포 | — |
| 완료 | 후속 1 앱: 터미널 경로·URL Ctrl+클릭(팔레트로도), 훅 설치 안내(미리보기·확인 후에만 설치), 터미널에 Ctrl+P/K 보내기, LSP 상태 메뉴, 편집기 자동 완성 상세·스니펫·코드 액션·증분 동기화, 변경 이벤트 구독(폴링은 느린 예비만), 저장 시 task.touch, 작업 삭제 전 편집기 저장/닫기 → master `9667298` (테스트 374) | 후속 | — |
| 완료 | 작은 후속 (Claude, small-fixes): 터미널 넓은 글자를 2칸에 맞춰 그림(Windows Terminal과 비교), 넓은 글자 선택·커서; release에서 native QA(테스트 전용 경계 기록 `qa_id`); doctest 255는 재현 안 됨(바깥 강제 종료로 판단, 기록); 스니펫 탭 이동·자동 import·`workspace/applyEdit`, 실제 rust-analyzer로 코드 액션 확인; 파일별 독립 탭은 설계 메모만 → 브랜치 `SJin-Choi/small-fixes` (docs/09 §13) | 후속 | — |
| 완료 | 후속 2 아이콘: 독자 아이콘 "터미널 칸"(시안 3개 중 선택, 16~256px 9개 크기 손질), 실행 파일 5개 버전 정보·아이콘, 창·작업 표시줄 아이콘, 설치 파일 아이콘(설치 시험 63/63) → master `c249ca2` | 후속 | — |
| 완료 | 후속 3 데몬: 프로토콜 v9 — 여러 앱의 색 응답 중복 제거와 0.5초 넘은 늦은 응답 차단, 변경 이벤트(terminal/task.changed, task.removing, task.touch), `pod hooks uninstall`(백업, 다른 도구 훅 보존), `pod-hook --version` → master `1f1a9cb` (테스트 347). 앱이 이벤트로 바꾸는 것은 후속 1에 추가 | 후속 | — |
| 완료 | P6-1 GitHub 연동: `pod-forge`(gh CLI, 가짜 forge로 시험), 프로토콜 v10 task.pr.status/create/merge, `pod pr create/status/merge/open`, 리뷰의 PR 탭(원격 없음 안내, 제목·본문 미리 채움, 상태·리뷰·체크, 병합 방식 선택·확인, 그룹 우승자 PR) → master `4f4e6aa` (테스트 414). 실제 GitHub 쓰기 검증은 남음(`docs/15` §7) | P6 | — |
| 완료 | P6-1 실제 저장소 검증: 시험용 비공개 저장소 SJin-Choi/pod-pr-e2e-20261109-1754에서 PR 만들기(초안·일반)·체크 대기→통과/실패·스쿼시 병합·충돌·오류 경로(푸시 거절·기준 브랜치 없음·시간 초과) 통과, 버그 6건 수정 → master `ee98e8e` (테스트 449). 저장소는 보관됨(삭제 권한 없음). 남은 것: workflow 권한이 없어 Actions 대신 커밋 상태로 체크 시험, 리뷰 diff가 autocrlf에서 파일 전체 변경으로 보임, `pod server stop` 직후 경합 | P6 | — |
| 완료 | P6-2 확장: `pod-mods`(rquickjs 샌드박스, 메모리 16MiB·CPU 시간 제한, 권한·작업 범위 검사), podd 안에서 실행(프로토콜 v11 mod.*), 상태 출처 mod(hook > osc > mod > heuristic), 설정 "확장" 화면, 예제(Aider 상태 감지·그룹 완료 알림·레이아웃 프리셋) → master `0685f35` (테스트 447). 남은 것: setTimeout, 실제 클릭으로 레이아웃 적용 확인, OS 알림 | P6 | — |
| 완료 | P6-3 공개: 역분석 내용을 private/(git 제외)로 옮기고 README·CONTRIBUTING·SECURITY 추가 → 공개 저장소 https://github.com/SJin-Choi/pod `main`(새 이력 1커밋, GitHub noreply 작성자). 이후 공개는 `tools/publish-public.ps1`(검사 후 master 트리를 main에 한 커밋으로) — master 이력은 비공개 유지 | P6 | — |
| 완료 | 문제 수정: 리뷰 diff가 autocrlf/eol 속성에서 `git diff`와 같은 줄 수(줄바꿈만 바뀐 파일 표시), `pod server stop` 경합(종료까지 대기, SERVER_STOPPING, 다음 데몬에 재전송), 넓은 글자 실측(가나는 정상, 이모지 맞춤 수정) → master `6365f51`·`dd3ae9f`·`dac4121` | 후속 | — |
| 완료 | 편집기 파일별 탭(미리보기·고정·MRU·다시 열기·복원·LSP 열기/닫기) → master `8802806`. 확장 후속: setTimeout/setInterval, Windows 알림(창 비활성 시), 레이아웃 프리셋 실제 클릭 검증·버그 5건 → master `ab1b5fe` (master 검증 484 통과, clippy 0, deny ok) | 후속 | — |
| 완료 | 테마: 갤러리 18종을 고를 수 있는 테마로(스타일 16종 + 프리셋 18, 대비 12648 통과), Lucide 아이콘, D4 화면 틀, 설정 › 모양 → `259d86c`. 레이아웃 7종(작업대·집중 레일·응답 대기열/관제·칸반·블록 터미널·리뷰 중심·고밀도) → `8a3d9f9`. 에이전트 표시(Claude·Codex 독자 아이콘) → `a15663c`. master 검증 791 통과 | 디자인 | — |
| 진행 | P6-4 원격 `docs/17`: R1·R2(v12)·R3(SSH, 실제 서버 미검증) 완료, 모바일 M1(v13, 짧은 메시지 포함) 완료. 남은 것 R4~R6(다른 기기 podd 실제 검증 포함), WSL(설치는 사용자 몫), M2~M6(모바일 승인 웹 화면) | P6 | — |
| 진행 | 워크트리 세션·오케스트레이션 `docs/19`: S1~S8 완료(v14~v18: 세션 만들기·설정·보관/재개, 코어·깨우기, 자원 한도·선택지 승인, 앱 실행 보드·받은 편지함, 안내·연동·한국어 문구), 세션 찾기 `docs/18` 완료, 실제 Claude·Codex 시험 1회 PASS(`docs/19` §17). 진행: 2026-11-10 리뷰 수정(`docs/review/2026-11-10`) | 오케스트레이션 | — |
| 완료 | 전체 평가와 수정 2회(`docs/review/2026-11-10`): 6개 영역 평균 6.70 → 7.25, 1차 63건(해결 확인 52·부분 8) + 재평가 높음 4·중간 다수 수정(워커 커밋이 git filter/hook를 실행하던 탈출 차단, 외부 프로세스 트리·출력 제한, v19 unreachable holder), 전체 fmt·CI 파일(실행은 workflow 권한 필요), 기준 이미지 36장 0 실패 → master `643e6e6` (테스트 943) | 검증 | — |
| 완료 | 공개 저장소 이전(2026-11-10): https://github.com/Linejin/pod `main`(새 이력, Linejin noreply 작성자, `publish-public.ps1 -Root`). 이전 SJin-Choi/pod는 그대로 둠 | P6 | — |
| 완료 | 확장성 리팩터링(`docs/20-refactor`, 2026-11-10~11): 설계 3건(Codex Astra) → 카드 51장(워크트리 워커, 동시 빌드 3). 새 crate pod-protocol·pod-client·pod-process·pod-storage·pod-test-support, podd RPC 등록 표·server/git/status/holder/orch_runs 분할, pod-cli 명령 모듈, 편집기·LSP·orch·mods·discover·diff 분할, pod-app workspace/chrome/source/review/groups/orch_ui/mods/sessions 분할, 7개 레이아웃 전부 레이아웃 kit renderer(작업대 직접 접근 0). 동작 보존: 리팩터링 전 64e9530 기준 golden·계약 시험, 기준 화면 56장 0px. 덤으로 8.3 짧은 경로 버그·레일 터미널 버튼 패닉·holder 불안정 시험 수정 → master `73e698b` (테스트 1221) | P7 | — |
| 완료 | 리팩터링 재평가(Codex Astra) 5.4→7.7 뒤 정리: 리팩터링 중 찾은 기존 버그 11건 수정(블록 터미널 패널 서랍·칸반 F6·리뷰 초안 보존·세션 재읽기·응답 해석 실패 시 연결 유지·단축키 누적·LSP 종료 대기·diff 빈 파일·podd 문서 경고+CI 문서 검사·서랍 폭), 재평가 권고 5가지(host 계약을 render/update 단계로 분리, 금지 의존 간선 검사, 실시간 알림 순서 golden, pod-app pub(crate) 468→354, 확장 안내서 `docs/21-extending.md`), CI 불안정 시험 3건 수정 → master `176b3a0` (테스트 1246) | P7 | — |
| 완료 | 원격 R4 전송(v20: 32 KiB 조각·청크 스냅샷·input_once·재접속 세션·장애 주입 프록시) + 위협 모델 `docs/22`(22건) + 오케스트레이션 보안 O1~O5: 에이전트 글은 출처 붙은 데이터·승인 중개기와 policy.json(O1), 워커 전용 제한 통로 v21(O2), 워커 허용·커밋 경계(O3), 훅 신뢰(O4), 깨우기 규칙·no-hooks 폴더·저장소 .codex 신뢰 거부(O5). 실제로 재현된 샌드박스 탈출 4건(가짜 pod.exe, GIT_CONFIG_PARAMETERS, 저장소 .codex 신뢰, 임시 파일 이름 위장) 차단 → master `672c493` (테스트 1346) | P7 | — |
| 대기 | P6-5 코드 서명: SignPath Foundation 오픈소스 신청(사용자 본인 신청 필요), 승인 후 GitHub Actions에서 설치 파일 서명 | P6 | 사용자 |

### 참고 출처
- gpui-component: https://github.com/longbridge/gpui-component · https://docs.rs/crate/gpui-component/0.6.0

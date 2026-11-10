# D2b 화면 비교

브랜치: `SJin-Choi/d2b-polish`. master에 병합하거나 원격으로 푸시하지 않았습니다.

주요 커밋: `bc29d22` 한국어 UI·탐색, `b39d6db` 툴킷 메뉴·접근성 한국어,
`9f755b9` 비교 증거와 번역 테스트 기대값, `7a63fdb` 최신 master(a35648d) 통합,
`e4476b3` 제출 확인 문구 중앙화·Review flex, `8d0f633` 빈 파일 목록 숨김.
최종 문서/증거 커밋은 이 보고서를 포함한 커밋입니다.

기준 앱은 실제 `18b4129`를 별도 바이너리로 빌드했습니다. 아래 13개 화면의
after는 `e4476b3`이며, 추가 빈 Review 회귀 캡처는 최종 `8d0f633`입니다.
두 버전 차이는 변경 파일이 없을 때 파일 목록을 숨기는 한 조건뿐입니다.
검색·최근 항목·글꼴 보조 캡처는 `b39d6db`이며 이후 해당 동작은 바뀌지 않았습니다.
바이너리 SHA256은 [기록](d2b-binaries.txt)에 있습니다.

모두 보조 모니터, 144 DPI, 대상 HWND PrintWindow로 캡처했고 직접 열어 확인했습니다.
동일한 fixture 레시피를 썼으며 v8 통합 때 재생성해 PID/경로/시간/기준 해시는
달라질 수 있습니다. 실제 Claude/Codex 요청, 승인 또는 에이전트 전송은 하지 않았습니다.

| 화면 | 변경 | 이전 다크 | 이후 다크 | 이전 라이트 | 이후 라이트 |
|---|---|---|---|---|---|
| review | 한국어 검토/의견/전송 문구, 주요 작업을 가리지 않는 알림 | [before/dark](d2b-before-dark-review.png) | [after/dark](d2b-after-dark-review.png) | [before/light](d2b-before-light-review.png) | [after/light](d2b-after-light-review.png) |
| settings | 설정 대화 상자를 중앙 탭으로 변경, UI/터미널 글꼴·크기와 즉시 단축키 적용 | [before/dark](d2b-before-dark-settings.png) | [after/dark](d2b-after-dark-settings.png) | [before/light](d2b-before-light-settings.png) | [after/light](d2b-after-light-settings.png) |
| palette | 한국어 명령/힌트, 최근 항목, 유니코드 퍼지 검색 | [before/dark](d2b-before-dark-palette.png) | [after/dark](d2b-after-dark-palette.png) | [before/light](d2b-before-light-palette.png) | [after/light](d2b-after-light-palette.png) |
| repository | 한국어 경로 안내와 열기 버튼 | [before/dark](d2b-before-dark-repository.png) | [after/dark](d2b-after-dark-repository.png) | [before/light](d2b-before-light-repository.png) | [after/light](d2b-after-light-repository.png) |
| new-task | 한국어 작업 입력과 도움말, 식별자 보존 | [before/dark](d2b-before-dark-new-task.png) | [after/dark](d2b-after-dark-new-task.png) | [before/light](d2b-before-light-new-task.png) | [after/light](d2b-after-light-new-task.png) |
| new-terminal | 한국어 프로그램/이름/경로 입력 | [before/dark](d2b-before-dark-new-terminal.png) | [after/dark](d2b-after-dark-new-terminal.png) | [before/light](d2b-before-light-new-terminal.png) | [after/light](d2b-after-light-new-terminal.png) |
| new-group | 한국어 비교 요청과 멤버 입력 | [before/dark](d2b-before-dark-new-group.png) | [after/dark](d2b-after-dark-new-group.png) | [before/light](d2b-before-light-new-group.png) | [after/light](d2b-after-light-new-group.png) |
| agents | 한국어 계층 안내, master의 부모 관계 수정 통합 | [before/dark](d2b-before-dark-agents.png) | [after/dark](d2b-after-dark-agents.png) | [before/light](d2b-before-light-agents.png) | [after/light](d2b-after-light-agents.png) |
| gates | 한국어 승인/만료/결정 문구; 실제 승인 실행 없음 | [before/dark](d2b-before-dark-gates.png) | [after/dark](d2b-after-dark-gates.png) | [before/light](d2b-before-light-gates.png) | [after/light](d2b-after-light-gates.png) |
| notifications | 한국어 알림 내역과 왼쪽 하단 알림 스택 | [before/dark](d2b-before-dark-notifications.png) | [after/dark](d2b-after-dark-notifications.png) | [before/light](d2b-before-light-notifications.png) | [after/light](d2b-after-light-notifications.png) |
| group | 한국어 비교/검사/멤버 작업과 결과 요약 | [before/dark](d2b-before-dark-group.png) | [after/dark](d2b-after-dark-group.png) | [before/light](d2b-before-light-group.png) | [after/light](d2b-after-light-group.png) |
| terminal | 전체 이름과 브랜치 보조 행, 테마 및 터미널 글꼴 | [before/dark](d2b-before-dark-terminal.png) | [after/dark](d2b-after-dark-terminal.png) | [before/light](d2b-before-light-terminal.png) | [after/light](d2b-after-light-terminal.png) |
| inspector | 오른쪽 상태·출처·신뢰도·갱신 시간 인스펙터 | [before/dark](d2b-before-dark-inspector.png) | [after/dark](d2b-after-dark-inspector.png) | [before/light](d2b-before-light-inspector.png) | [after/light](d2b-after-light-inspector.png) |


## 보조 증거

- [필터](d2b-after-dark-filter.png): 일치하는 작업과 조상을 유지합니다.
- [퍼지 검색](d2b-after-dark-fuzzy.png): `새비그`가 `새 비교 그룹`과 일치하며 매칭 글자를 강조합니다.
- [최근 항목](d2b-after-dark-recents.png): 실행한 저장소 열기가 먼저 나옵니다.
- [글꼴 설정](d2b-after-dark-settings-live.png), [기존 터미널 반영](d2b-after-dark-font-live.png): Consolas 18이 재시작 없이 적용됩니다.
- [종료 코드 7](d2b-after-dark-inspector-exit.png): 실제 코드와 상태를 표시합니다. 집계의 동일 코드/혼합/미상은 pod-ui 단위 테스트로 확인했습니다.
- 빈 Review: [수정 전 재현](d2b-empty-before-fix-dark.png), [최종 다크](d2b-after-dark-review-empty.png), [최종 라이트](d2b-after-light-review-empty.png). 빈 파일 목록이 중앙 내용을 밀어내지 않습니다.

## 구현과 경계

한국어 정적 문구와 포맷 템플릿은 `src/strings.rs`에 모았습니다. 이미 잠금 파일에
있던 MIT `rust-i18n`을 직접 연결해 툴킷 Dock/Input/Dialog/Command/Settings/Diff
번역 확장 훅도 등록했습니다. 경로·명령·브랜치·프로토콜 식별자와 외부 진단은 보존합니다.
공유 상태 배지의 한국어는 기존 pod-ui 소유입니다.

사이드바는 304px, 이름과 브랜치/상태를 두 행으로 표시하고 필터 중 조상 경로를 유지합니다.
설정은 중앙 탭, 상태/출처는 선택한 터미널을 따라가는 오른쪽 인스펙터입니다.
UI·터미널 글꼴/크기가 따로 저장되며 기존 터미널도 즉시 갱신합니다.
Ctrl+K / Ctrl+Shift+K 선호도도 즉시 적용됩니다.
팔레트는 유니코드 부분 순서 매칭, 굵기/밑줄 강조, 최근 20개 ID 저장을 사용합니다.
알림은 왼쪽 사이드바 하단 일반 레이아웃으로 배치하며 최대 3개/스크롤 영역을 사용합니다.
hover와 내부 키보드 focus 모두 타이머를 멈추는 이벤트 처리를 갖췄습니다.
로딩은 reduced-motion에서도 안전한 정적 3줄 placeholder입니다.

master의 부모 트리, OSC reply, focused-terminal 알림 억제, 패널 복원을 그대로 통합했습니다.
이 작업의 공유 파일 직접 수정은 main의 strings 모듈 등록, panel의 글꼴 초기값/문구,
tree/vm의 표시 문구, workspace의 UI·설정·알림 레이아웃입니다.
부모 필드/관계, OSC 전달, focused-terminal 억제 조건, layout 파일 및 복원 로직은 직접 수정하지 않았습니다.
podd/pod-cli/pod-status/pod-hook 변경은 master 병합분뿐입니다.

## 검사

[최종 요약](d2b-verification.txt), [check](d2b-check.txt), [release](d2b-release.txt),
[전체 테스트](d2b-tests.txt), [clippy](d2b-clippy.txt), [deny](d2b-deny.txt).

```text
cargo check -p pod-app
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.95s
cargo build --workspace --release
    Finished `release` profile [optimized + debuginfo] target(s) in 23.03s
cargo test --workspace --release
270 passed; 0 failed; 1 ignored (기존 terminal-view 문서 예제)
pod-app: 46 passed; pod-ui: 10 passed; pod-terminal-view: 28 passed
cargo clippy -p pod-app -p pod-ui -p pod-terminal-view --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.80s
cargo deny check
advisories ok, bans ok, licenses ok, sources ok
```

모든 명령 exit 0. clippy 경고 0개. deny에는 기존 ISC 허용 목록이 사용되지 않았다는
`license-not-encountered` 경고만 있습니다. deny 이후 변경은 UI 조건 한 줄이며 의존성 변경은 없습니다.
이전에 실패한 Group 테스트 2개는 영어 문구 기대값을 한국어로 갱신한 뒤 전체 재실행으로 통과했습니다.
[초기 실패 로그](d2b-tests-initial.txt)도 보존했습니다.
알려진 `slow_attach_client_is_dropped_and_can_reattach` 테스트는 최종 전체 실행에서 통과했습니다.
개인 target, sccache, 빌드 4 jobs, 테스트 4 threads, 모든 cargo/app 시작 전 CPU 게이트를 사용했습니다.

## 남은 D2 게이트와 D3

- 네이티브 키보드 전용 흐름 **통과 아님**: 최종 릴리스의 Tab 입력도
  [window_not_focused](d2b-native-keyboard-final.json)로 거절됐습니다.
  UIA/대상 창 메시지 기반 화면 검증을 네이티브 키보드 검증으로 계산하지 않았습니다.
- 키보드 focus로 알림 타이머가 멈추는 처리는 구현했지만 OS 포커스 차단으로 실제 Tab 흐름 검증은 남았습니다.
  모든 모달의 포커스 복귀, 한국어 IME, Narrator, caption drag/Snap도 미검증입니다.
- 툴킷 하드코딩 대화 상자 닫기 접근성 이름 `Close`는 영어가 남습니다.
  공개 API가 없어 툴킷 확장/상류 수정이 필요합니다. 일부 툴킷 탭의 접근성 이름도 비어 있습니다.
- 설정/상태 인스펙터 탭의 재시작 복원은 아직 지원하지 않습니다.
  master의 Review/Group/Gates/Agents 복원과 구분해야 합니다.
- 혼합 DPI, 고대비, 작은 창/깊은 트리, 실제 네이티브 단축키 전환과 IME/Narrator 매트릭스는 D3에서 확인해야 합니다.
- 제출 확인 표시는 master에서 통합하고 문구를 중앙화했지만 실제 에이전트 전송은 이 fixture QA 범위에서 실행하지 않았습니다.

구현과 자동 검사는 완료했으나 위 미검증 항목 때문에 전체 D2 접근성 게이트를 완료로 표시하지 않습니다.

## 정리

모든 QA 앱 창을 닫고 fixture 전용 소켓의 데몬에 server stop --kill을 실행해 실행 중인 holder 3개를 정리했습니다. 이 작업트리 target 아래 실행 파일 프로세스는 남지 않았습니다. [정리 기록](d2b-cleanup.txt). 다른 작업자의 데몬과 공유 sccache는 건드리지 않았습니다.

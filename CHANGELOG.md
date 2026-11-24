# 변경 기록

Pod 릴리스마다 사용자에게 보이는 변경을 짧게 적는다. 버전은 루트 `Cargo.toml`의
`[workspace.package] version`, 태그는 `v<버전>`이다(절차: [docs/14 §2.1](docs/14-install.md#21-릴리스)).
태그 없이 만든 설치 파일은 `<버전>-dev.<sha>` 이름표를 단다.

## 미출시

- 명령 팔레트에서 웹 브라우저를 열어 로컬 개발 서버와 웹페이지를 편집기 도크에서 볼 수 있다. [사용법](docs/13-editor-and-lsp.md#편집기-브라우저)
- 프로젝트 에이전트 그래프에서 저장소별 상태와 부모 관계를 보고 에이전트 터미널을 연다. [사용법](docs/09-pod-app.md#프로젝트-에이전트-그래프-project_graphrs-2026-11-24)
- `pod a2a`로 A2A 1.0 JSON-RPC 에이전트 카드와 메시지·작업을 다룬다. [지원 범위](docs/a2a-client.md)
- 상태 표시줄에 Codex와 Claude Code의 남은 한도와 초기화 카운트다운을 표시한다. [Claude Code 연결 방법](docs/usage/ai-usage-monitor.md)
- 열린 파일이나 부모 폴더의 이름을 바꾼 뒤에도 미저장 편집과 언어 서버의 새 경로를 유지한다. [검증 범위](docs/27-feature-validation-results.md#2026-11-24-추가-검증)
- 2026-11-10 리뷰 수정(`docs/review/2026-11-10`): 개발 빌드 이름표, 공개 스크립트 검사 강화, CI 워크플로 파일.

## 0.1.0 (태그 없음, 2026-11-08 ~ 2026-11-10)

첫 사전 알파. 데몬(`podd`)이 소유하는 ConPTY 터미널, 에이전트 상태 감지(훅·OSC 7501·추정),
작업별 git worktree, diff 리뷰와 줄 댓글, 그룹·게이트, LSP 편집기, GitHub PR, 샌드박스 JS 확장,
SSH 원격(일부), 세션 찾기, 워커·코디네이터 오케스트레이션, 사용자별 설치 프로그램.

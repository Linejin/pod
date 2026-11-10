# Security Policy

## 보고 방법 / Reporting a vulnerability

보안 취약점은 **공개 이슈로 올리지 말고** GitHub의 비공개 보고 기능으로 알려 주세요:
저장소의 **Security → Advisories → Report a vulnerability** (GitHub Security Advisories).

Please **do not open a public issue** for security problems. Report them privately through
GitHub Security Advisories: **Security → Advisories → Report a vulnerability** on this repository.

가능하면 다음을 포함해 주세요 / Please include:

- 영향받는 버전 또는 커밋, Windows 버전 / affected version or commit, Windows version
- 재현 방법과 영향 / steps to reproduce and impact
- 알려진 완화 방법 / any known mitigation

확인 후 가능한 한 빨리 답변하고, 수정이 배포될 때까지 내용을 비공개로 유지합니다.
We will acknowledge the report as soon as we can and keep it private until a fix is released.

## 범위 / Scope

Pod는 pre-alpha이며 기본 브랜치의 최신 상태만 지원합니다. 특히 관심 있는 영역:
Pod is pre-alpha; only the latest default branch is supported. Areas of particular interest:

- `podd` 로컬 named pipe와 훅 토큰 인증 / the `podd` named pipe and hook token authentication
- 에이전트 설정 파일(`~/.claude`, `~/.codex`)을 고치는 훅 설치기 / the hook installer that edits agent configuration
- 터미널 출력 처리(이스케이프 시퀀스, OSC 응답) / terminal output handling (escape sequences, OSC replies)
- 설치 프로그램과 업그레이드 경로 / the installer and upgrade path

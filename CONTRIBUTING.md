# Contributing to Pod

감사합니다! Pod는 pre-alpha라 구조가 자주 바뀝니다. 큰 변경은 먼저 이슈로 의견을 나눠 주세요.
(English is welcome in issues and pull requests.)

## 시작하기

- Windows 10/11 x64, Rust stable (msvc toolchain), Git.
- 번들 ConPTY: `powershell -ExecutionPolicy Bypass -File tools/fetch-conpty.ps1`
- 빌드: `cargo build`, 테스트: `cargo test` — 기본 멤버(출하 크레이트 전부)만. UI·diff 스파이크는 `-p pod-spike`/`-p diff-spike`로 따로.
- 설계 배경은 [docs/01-plan.md](docs/01-plan.md)와 각 영역 문서(목록은 [README의 문서 표](README.md#문서))를 보세요.

## Pull request 전에

1. `cargo fmt --all` 과 `cargo clippy --all-targets -- -D warnings`
2. `cargo build` 뒤 `cargo test` (pod-cli 테스트는 옆에 빌드된 podd·pod-hook을 씀)
3. 의존성을 추가했다면 `cargo deny check` — Apache-2.0/MIT 호환 라이선스만 허용합니다(GPL/AGPL 불가).
4. UI를 바꿨다면 바뀐 화면의 스크린샷을 PR에 붙여 주세요(가능하면 다크·라이트, 100%·150% 배율).
5. 동작이나 프로토콜을 바꿨다면 관련 `docs/` 문서도 고쳐 주세요.

## CI

`.github/workflows/ci.yml`(windows-latest): 저장소 검사 스크립트(`tools/check-*.ps1`, `tools/test-publish-public.ps1`) → `cargo fmt --check`(실패하면 CI 실패) → clippy `-D warnings` → build → test, 그리고 Linux 잡에서 `cargo deny check`.
**아직 동작하지 않는다**: 워크플로 파일을 푸시하려면 `workflow` 권한이 있는 GitHub 토큰이 필요한데 지금 토큰에는 없다. 그 전까지는 위 명령을 손으로 돌린다.

- `POD_REQUIRE_TOOLS=1`: 외부 도구(ssh, rust-analyzer, 옆에 빌드된 pod-hook.exe 등)가 없어 건너뛰던 테스트가 실패한다. CI는 이 값을 켜고, 러너에 없는 codex 테스트 하나만 이름으로 건너뛴다.
- 공개 저장소 반영(`tools/publish-public.ps1`)은 비공개 차단 목록이 비었거나 정규식이 잘못되면 거부하고, 일반 비밀 패턴도 검사하며, push 실패를 실패로 보고한다. 시험: `tools/test-publish-public.ps1`.

## 원칙

- **클린룸**: 다른 제품의 코드·에셋(아이콘, 이미지, CSS)을 복사하지 마세요. 공개 규격과 공개 문서만 참고합니다.
- 사용자 설정(`~/.claude`, `~/.codex` 등)을 쓰는 코드는 반드시 백업과 dry-run 경로를 둡니다. 테스트는 임시 디렉터리만 사용합니다.
- 커밋 메시지는 무엇을 왜 바꿨는지 짧게 적어 주세요.

## 라이선스

기여한 코드는 저장소와 같은 [Apache License 2.0](LICENSE)으로 배포됩니다(Apache-2.0 §5).

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
   - podd 통합 테스트 하나만 돌릴 때는 시험용 바이너리를 먼저 빌드한다: `cargo build -p podd --example fake-agent -p pod-hook -p pod-cli --bin pod --bin pod-hook` 뒤 `cargo test -p podd --test <이름>`, 또는 `tools/test-daemon-refactor.ps1`. 테스트는 cargo를 부르지 않으므로 pod-cli·pod-hook을 고쳤으면 다시 빌드할 것(오래된 바이너리는 감지하지 않음).
   - 경로를 다뤘다면 CI처럼 TEMP가 8.3 짧은 이름(`C:\Users\RUNNER~1`)일 때도 돌려 본다: `tools/test-shortpath.ps1 [명령]`(기본 `cargo test --workspace --no-fail-fast`, 긴 이름 폴더를 만들어 그 짧은 이름을 TEMP/TMP로 씀).
3. 의존성을 추가했다면 `cargo deny check` — Apache-2.0/MIT 호환 라이선스만 허용합니다(GPL/AGPL 불가).
4. UI를 바꿨다면 바뀐 화면의 스크린샷을 PR에 붙여 주세요(가능하면 다크·라이트, 100%·150% 배율).
5. [문서 관리](#문서-관리)에 따라 같은 변경에서 사용법과 검증 기록을 검토하고 PR에 문서 영향을 남겨 주세요.

## 문서 관리

1. **작업 시작** 때 작성자가 변경할 사용자 행동, 명령·옵션, 기본값, 데이터·권한, 지원 범위를 확인하고 아래 표에서 관련 문서를 찾는다.
2. **구현과 검증** 단계에서 같은 변경에 문서를 갱신한다. 기능별 검증 계획에는 재현 절차와 통과 기준을, 결과에는 시험 대상 커밋·환경·실행 결과와 미실행 범위를 적는다. 날짜가 붙은 이전 결과는 보존하고 새 결과를 추가한다.
3. **완료 보고와 리뷰**에서는 문서 경로 또는 문서 영향이 없는 구체적인 이유를 남긴다. Orca 워커는 담당 범위의 문서를 갱신하고 공용 문서 영향과 근거를 코디네이터에게 전달한다. 코디네이터는 통합 전에 공용 문서와 검증 기록을 갱신한다. 리뷰어는 설명을 동작·옵션·검증 근거와 대조한다.
4. **릴리스 후보 확인**에서는 통합 담당자가 릴리스 후보 소스와 변경 기록, 설치·서명·개인정보 안내의 대상과 검증 범위를 맞춘다.

| 바뀐 내용 | 갱신할 문서 |
|---|---|
| 사용자 흐름, 화면, 기능 추가·제거 | [README](README.md), 해당 사용 가이드, [변경 기록](CHANGELOG.md)의 미출시 항목 |
| CLI·설정 기본값·프로토콜·호환성 | 해당 명령·설정·프로토콜 가이드와 예제 |
| 설치 파일, 업그레이드, 배포·서명 | [설치 가이드](docs/14-install.md), 해당 [서명 정책](CODE_SIGNING_POLICY.md)과 배포 안내 |
| 네트워크 대상, 계정 정보, 데이터 보관, 권한 | [개인정보 초안](PRIVACY.md), 해당 [신뢰 경계](docs/22-threat-model.md)와 사용자 설정 안내 |
| 새 검증 절차, 결함 재검증, 지원 범위 확인 | [기능 검증 계획](docs/26-feature-validation.md), [날짜별 실행 결과](docs/27-feature-validation-results.md), 해당 [통합 기록](docs/25-improvement-execution.md) |

PR 본문에 아래 양식 중 한 줄을 쓴다. `updated`에는 실제 변경한 문서의 저장소 기준 경로를 쉼표로 나열하고, `none`에는 리뷰어가 확인할 수 있는 이유를 적는다.

```text
Docs: updated -- docs/a2a-client.md, CHANGELOG.md
Docs: none -- Internal refactor; CLI options, output and configuration are unchanged.
```

`Documentation impact` 검사는 제품 소스, 패키지·의존성 설정, 설치 도구가 바뀐 PR의 이 한 줄을 확인한다. `updated`에 나열한 경로는 각각 PR의 문서 변경에 있어야 한다. 리뷰에서는 설명이 기능과 검증 근거를 정확히 반영하는지 확인한다. 로컬에서는 다음 명령으로 문서 인덱스, 버전 표기, 파일·섹션 링크를 검사할 수 있다.

```powershell
powershell -NoProfile -File tools/check-docs.ps1
```

## CI

`.github/workflows/ci.yml`은 Windows에서 저장소 정책·문서 검사, fmt, Clippy, build, test를 실행하고 Linux에서 `cargo deny check`를 실행한다. `.github/workflows/docs-impact.yml`은 PR 생성·재열기·커밋 갱신·본문 편집 시 문서 영향 기록을 검사한다.

- `POD_REQUIRE_TOOLS=1`: 외부 도구(ssh, rust-analyzer, 옆에 빌드된 pod-hook.exe 등)가 없어 건너뛰던 테스트가 실패한다. CI는 이 값을 켜고, 러너에 없는 codex 테스트 하나만 이름으로 건너뛴다.
- 공개 저장소 반영(`tools/publish-public.ps1`)은 비공개 차단 목록이 비었거나 정규식이 잘못되면 거부하고, 일반 비밀 패턴도 검사하며, push 실패를 실패로 보고한다. 시험: `tools/test-publish-public.ps1`.

## 원칙

- **클린룸**: 다른 제품의 코드·에셋(아이콘, 이미지, CSS)을 복사하지 마세요. 공개 규격과 공개 문서만 참고합니다.
- 사용자 설정(`~/.claude`, `~/.codex` 등)을 쓰는 코드는 반드시 백업과 dry-run 경로를 둡니다. 테스트는 임시 디렉터리만 사용합니다.
- 커밋 메시지는 무엇을 왜 바꿨는지 짧게 적어 주세요.

## 라이선스

기여한 코드는 저장소와 같은 [Apache License 2.0](LICENSE)으로 배포됩니다(Apache-2.0 §5).

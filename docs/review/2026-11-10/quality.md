# Pod 리뷰 — 엔지니어링 품질·릴리스 준비도 (2026-11-10)

기준: master `47dff8a` · 검토 범위: 워크스페이스·의존성·라이선스, 테스트 건강도, CI, 설치 프로그램·버전, 문서 정합성, README/CONTRIBUTING, 공개 저장소 상태.
제품 코드는 바꾸지 않았다. 실행한 것: `cargo test -p pod-status -p pod-orch`(89개 통과, 빌드 포함 43 s), `cargo clippy -p pod-status -p pod-orch -p pod-forge --all-targets -- -D warnings`(통과), `cargo fmt --all -- --check`, `cargo deny check`(전부 ok), `cargo tree -i`, 직접 만든 마크다운 링크 검사기(53개 .md, 상대 링크 462개, 앵커 포함), `tools/publish-public.ps1 -DryRun`(메인 체크아웃에서, 읽기만), 레지스트리 원본과 `vendor/gpui-component` 비교. 워크스페이스 전체 빌드·테스트는 하지 않았다.

## 1. 점수표

| 기준 | 점수 | 근거 |
|---|---|---|
| 정확성(버그 없음) | 8 | 실행한 테스트·clippy·deny 모두 통과, 이 영역에서 제품 버그는 못 찾음(범위가 좁아 상한 있음) |
| 견고성(오류·경합·재시작) | 7 | 테스트 데몬이 파이프·상태 디렉터리·환경까지 격리, 설치 시험도 격리; 공개 스크립트는 push 실패를 확인하지 않음 |
| 보안·개인정보 | 7 | 공개 전 차단 패턴·noreply 작성자, 테스트가 실제 설정을 건드리지 않음; 일반 비밀 검사·코드 서명 없음 |
| 성능·자원 | 6 | 통합 테스트 고정 sleep 약 69 s, 스파이크 크레이트가 워크스페이스 빌드에 포함, 2 MB짜리 생성 JSON 커밋 |
| 테스트 품질 | 7 | `#[test]` 794개, 실제 podd·headless GPUI QA로 결함을 잡는 구조; 대기 후 "안 일어났음" 단언, 조용한 건너뛰기, 무시된 RA 테스트 2개 |
| 코드 구조·유지보수성 | 7 | 크레이트 분리 명확, vendor 패치 문서화; rustfmt 미준수 201/268 파일, 린트 opt-in 불일치 |
| 문서 정확성 | 6 | 깨진 링크 0, 프로토콜 18 일치; README·CONTRIBUTING 문서 목록, docs/19 머리말, docs/01 백로그, deny.toml 근거가 낡음 |
| 사용성·접근성(해당 시) | 7 | 사용자별 설치·한국어 설치 화면·README 빌드 안내 명확; 문서 대부분 한국어(영문 요약만) |
| 릴리스 준비도 | 4 | CI 없음, 태그·CHANGELOG 없음, 버전 0.1.0 고정, 서명 없음, 공개 저장소가 master보다 1083 파일 뒤 |

**종합 6.7 / 10** — 가중치: 정확성 15, 견고성 10, 보안 15, 성능 5, 테스트 20, 구조 10, 문서 10, 사용성 5, 릴리스 준비도 10 (합 100).

## 2. 발견 사항 (확인됨)

| id | 심각도 | 제목 | 증거 | 영향 | 수정 제안 | 크기 |
|---|---|---|---|---|---|---|
| quality-01 | 높음 | CI가 전혀 없음 | `.github/` 없음. CONTRIBUTING의 fmt·clippy·test·deny는 수동뿐. 공개 저장소에는 스냅샷 커밋만 올라감 | 회귀·라이선스 위반·fmt 드리프트(quality-02)가 자동으로 걸리지 않음; 외부 PR 검증 불가 | `windows-latest` 워크플로: `tools/fetch-conpty.ps1`(SHA 고정) → `cargo fmt --check` → `cargo clippy --workspace --all-targets --locked -D warnings` → `cargo build --workspace --locked`(pod-cli 테스트가 옆의 podd/pod-holder/pod-hook exe 필요) → `cargo test --workspace --locked`(작업 timeout, `POD_*` 격리는 테스트가 이미 함) → `cargo-deny-action`. 선택 잡: Inno Setup 설치 후 `tools/installer/build.ps1`·`notices.ps1` 산출물 업로드. rust-cache, concurrency 취소. 공개 저장소(main)에 두어야 함 | M · **해결(파일만)** `0246e9e` — `.github/workflows/ci.yml` 추가, 아직 실행 안 됨: 푸시에 GitHub 토큰 `workflow` 권한 필요(CONTRIBUTING). fmt 검사는 quality-02 뒤 실패로 전환 |
| quality-02 | 중간 | `cargo fmt --check` 실패: 201/268 파일 | `cargo fmt --all -- --check` → 14개 크레이트 201개 파일 diff(pod-app 54, podd 50, pod-editor 27 …), `rustfmt.toml` 없음. CONTRIBUTING.md:15는 PR 전 `cargo fmt --all` 요구 | 기여자가 fmt를 돌리면 무관한 대량 diff; CI 도입 즉시 실패 | 현재 스타일을 원하면 `rustfmt.toml`(예: `max_width`)을 정하고, 아니면 한 번에 `cargo fmt --all` 커밋. **orch-dogfood-fixes가 podd/pod-cli를 수정 중이므로 그 병합 뒤에** | S · **해결** `3122b7f` — 워커가 없을 때 `cargo fmt --all` 한 번(두 번 실행해야 안정), `.git-blame-ignore-revs`에 기록, CI fmt 검사를 실패로 전환 |
| quality-03 | 중간 | deny.toml 예외 근거가 사실과 다름 | deny.toml:31–35·46–56은 MPL-2.0 `option-ext`와 미관리 크레이트 4개(instant, paste, rustybuzz, ttf-parser)가 "pod-spike로만" 들어온다고 씀. `cargo tree -i option-ext -e normal` → pod-app·pod-diff·pod-editor·pod-terminal-view·pod-ui 경로. 나머지 4개도 같음 | 출하되는 pod-app에 MPL·미관리 크레이트가 들어가는데 `unmaintained = "workspace"`가 이를 숨김; 정책 결정이 잘못된 전제 위에 있음 | 주석을 사실대로 고치고, `unmaintained = "all"` + 크레이트별 `ignore`(사유·재검토일)로 바꿔 새 알림은 드러나게. MPL 고지는 notices.ps1이 저장소 URL을 남기므로 유지 | S · **해결** `813e9b8` — 주석을 실제 경로(pod-app)로, `unmaintained = "all"` + id별 ignore(사유·재검토 2027-02-10), 스파이크는 deny 그래프에서 제외 |
| quality-04 | 중간 | vendor 패치 파일에 변경 고지 누락 | 레지스트리 `gpui-component-0.9.0`과 `diff -rq --strip-trailing-cr` → 바뀐 파일 5개(dialog.rs, dock/tab_panel.rs, input/input.rs, root.rs, locales/ui.yml). "Pod patch" 표기는 dialog.rs에만 있음 | Apache-2.0 §4(b)("수정한 파일에 눈에 띄는 변경 고지") 미충족 | 나머지 4개 파일 머리에 한 줄 고지, POD-PATCHES.md에 파일 목록 추가 | S · **해결** `d6b70ed` — 4개 파일 고지 + POD-PATCHES.md 목록, `tools/check-vendor-notices.ps1` |
| quality-05 | 중간 | 공개 저장소가 master보다 크게 뒤처짐 | `publish-public.ps1 -DryRun` → "1083 files changed, 239421 insertions(+), 4792 deletions(-)", 공개 main `0aa5ba2`(2026-11-09). 늘어난 줄의 대부분은 생성 QA 파일(`crates/pod-app/qa/d3/contrast.json` 114k줄·2.2 MB 등) | 공개본에 docs/15–19, 오케스트레이션, 수정이 없음; 다음 공개 때 리뷰 불가능한 크기의 단일 커밋 | 공개 주기를 정하고(예: master 병합마다), 생성 JSON·대량 스크린샷은 `dist`/아티팩트로 빼거나 압축 요약만 커밋 | S · **보류** — 이 묶음 범위 밖(공개는 사용자 확인 뒤, README 수정 계획) |
| quality-06 | 중간 | 고정 sleep에 기댄 "없음" 단언, 테스트 시간 부풀림 | `crates/*/tests`의 sleep 83곳, 300 ms 이상 고정 대기 합 약 69 s. 예: orch_runs.rs:489·495·508·512(3 s×4)·548(4 s), orchestration.rs:323·346·801·964 | 대기 창 뒤에 일어나는 회귀(중복 깨우기·재전달)는 못 잡음(거짓 통과), 테스트 실행이 길어져 CI 비용 증가 | 데몬에 테스트 전용 "워커 한 바퀴 끝남" 카운터/RPC를 두고 그 순번까지 기다린 뒤 단언; 또는 이벤트 순번 비교. **podd 수정 중인 워커와 겹침** | M · **보류(다른 작업과 겹침)** — podd/pod-cli 수정 워커와 겹쳐 손대지 않음 |
| quality-07 | 낮음 | 도구가 없으면 조용히 통과하는 테스트 | pod-cli/tests/hooks.rs:291(codex 없으면 return), :258(`cargo test -p pod-cli`는 pod-hook.exe를 안 만들므로 버전 확인이 보통 건너뜀), pod-remote/tests/system_ssh.rs:16, podd/tests/gaps.rs:553, pod-editor lsp_lifecycle.rs:221·331(`#[ignore]` + 없으면 return) | CI 러너에서 "통과"로 보이지만 실제로는 검사하지 않음 | `POD_REQUIRE_TOOLS=1`이면 건너뛰지 말고 실패; CI에서는 설정하거나 명시적으로 ignored로 표시 | S · **해결** `7645bda` — `POD_REQUIRE_TOOLS=1`이면 건너뛰기 대신 실패(공용 `tools/test-support/require_tools.rs`), CI는 켜고 codex 테스트만 이름으로 건너뜀 |
| quality-08 | 낮음 | README·CONTRIBUTING의 문서 목록이 14에서 멈춤 | README.md:46–59 표는 01–14만, CONTRIBUTING.md:11 "docs/03–14". 실제 docs/15–19 존재 | 새 기여자가 PR·확장·원격·세션 찾기·오케스트레이션 문서를 못 찾음 | 표에 15–19 추가, CONTRIBUTING은 "docs/ 목록은 README 참고"로 | S · **해결** `c4a989c` — README 표 15–19, `tools/check-docs.ps1` |
| quality-09 | 낮음 | 진행 상태 문서가 낡음 | docs/19:3 "도그푸딩은 가짜 에이전트 드라이런만", "기준: 프로토콜 v15" ↔ 같은 문서 §17(503행) 실제 에이전트 시험 PASS, 코드 `PROTOCOL_VERSION = 18`(podd/src/protocol.rs:71). docs/01:245 "진행: 실제 Claude·Codex 도그푸딩". docs/01 "나중 \| P6-4 SSH 원격"이 "진행 \| P6-4 … R3(SSH) 완료"와 모순 | 계획 판단을 잘못된 상태로 하게 됨 | 머리말·백로그 줄 갱신, 중복 P6-4 줄 정리 | S · **해결** `f9e144d` — docs/19 머리말 v18·§17 PASS, docs/01 백로그 정리, check-docs가 프로토콜 번호도 검사 |
| quality-10 | 낮음 | 워크스페이스 린트가 출하 크레이트 일부에 빠짐 | Cargo.toml:22 주석은 "pod-spike만 제외"라 하지만 pod-hook, pod-status, pod-terminal-view, diff-spike에도 `[lints] workspace = true` 없음 | 출하 바이너리 pod-hook·pod-status에서 `unused_must_use = "deny"` 등이 적용 안 됨 | 세 크레이트에 opt-in 추가 후 clippy 확인 | S · **해결** `2b0e088` — 세 크레이트 opt-in(clippy 0), `tools/check-workspace.ps1` |
| quality-11 | 낮음 | 릴리스 버전 체계 없음 | `[workspace.package] version = "0.1.0"` 고정, `git tag` 결과 없음, CHANGELOG 없음, rust-toolchain/`rust-version` 없음(edition 2024) | 설치 파일이 빌드마다 `Pod-Setup-0.1.0-x64.exe`로 같은 이름; 사용자·버그 보고에서 빌드 구분 불가 | 릴리스마다 버전 올림 + 태그 + 짧은 CHANGELOG; 개발 빌드는 `0.1.0+<sha>`를 VersionInfo에 넣기; `rust-version` 명시 | S · **해결** `2b0e088`·`69ef546` — `rust-version = "1.99"`, CHANGELOG.md, 개발 빌드 이름표 `<버전>-dev.<sha>`, `-Release`는 태그 `v<버전>` 필요(docs/14 §2.1). 실제 태그는 첫 릴리스 때 |
| quality-12 | 낮음 | 공개 스크립트의 검사·오류 처리 빈틈 | tools/publish-public.ps1:6–7 "no secrets"라 하지만 검사는 비공개 패턴 목록뿐(일반 비밀 검사 없음); :37 `git push` 뒤 `$LASTEXITCODE` 확인 없이 :38 "published" 출력(PS 5.1은 네이티브 실패로 중단하지 않음) | 패턴에 없는 토큰이 공개될 수 있고, push 실패를 성공으로 보고 | push 뒤 종료 코드 확인; gitleaks 등 일반 비밀 검사를 트리에 한 번 돌리기 | S · **해결** `7b4a79f` — push 등 git 종료 코드 확인, 일반 비밀 패턴 + canary, `tools/test-publish-public.ps1`. gitleaks는 도입 안 함(새 도구) |
| quality-13 | 낮음 | 디자인 목업 아이콘 NOTICE의 MIT 문구가 잘림 | docs/design/d4-claude/src/icons/NOTICE.md: MIT 면책 문구가 "WITHOUT WARRANTY OF ANY KIND."에서 끊김 | 출하하지 않는 파일이지만 공개 저장소에 Fluent·Phosphor SVG를 재배포함 | MIT 전문(각 저작권자) 붙이기 | S · **해결** `b44ab70` — Fluent·Phosphor MIT 전문 |
| quality-14 | 낮음 | 스파이크 크레이트가 워크스페이스 멤버 | Cargo.toml:3 members에 pod-spike·diff-spike(테스트 1·0개); `--workspace` 빌드·clippy·fmt(7개 파일) 대상 | 빌드 시간·CPU, CI 비용 | `exclude`로 빼거나 examples로 이동 | S · **해결** `2b0e088` — `default-members`에서 스파이크 제외(`-p`로는 빌드 가능), CI도 제외 |

심각도별: 치명 0 · 높음 1 · 중간 5 · 낮음 8.

### 추정 (확인 못 함)

수정 작업(2026-11-10, 브랜치 `SJin-Choi/fix-quality`)에서 확인: `bin_current`는 설치된 pod-hook.exe와 원본을 바이트로 비교하므로(`hooks_install.rs` `status`) 같은 0.1.0의 다른 빌드도 구분한다 → 문제 아님. 무시된 rust-analyzer 테스트 2개는 CI에서도 돌지 않는다(러너에 rust-analyzer 없음, 무거움) → 그대로 둠. headless QA 글꼴 차이는 CI가 실제로 돌기 전에는 확인 불가. 전체 `cargo build` 뒤 `cargo test`(기본 멤버, jobs 2, 공유 PC): 802 통과·0 실패·3 무시, 테스트 단계 621 s(테스트 대상 컴파일 포함), 불안정 실패 없음(1회).

- 워크스페이스 전체 `cargo test` 시간과 불안정 테스트 비율은 재지 않았다(전체 빌드 금지). 고정 sleep만으로도 직렬 기준 1분 이상.
- pod-app headless QA(native_qa 등)는 GitHub `windows-latest`에서 한글 글꼴·DirectWrite 차이로 결과가 다를 수 있다.
- `pod hooks status`의 `bin_current`가 같은 0.1.0 버전의 다른 빌드를 구분하는지는 확인 안 함(quality-11과 연관).
- 무시된 rust-analyzer 테스트 2개는 어디서도 정기적으로 돌지 않는 것으로 보인다.

## 3. 잘 된 점

- `cargo deny` 정책(라이선스·출처·와일드카드 금지) 통과, 설치 패키지용 고지 생성기(notices.ps1)가 출하 바이너리의 normal 의존성만 따라가며 GPL 계열을 막음. Lucide 고정 버전·ISC+MIT 전문, ConPTY SHA-256 고정.
- 통합 테스트가 데몬마다 고유 파이프·상태 디렉터리·발견 경로를 쓰고, codex 테스트는 실제 `~/.codex/config.toml`이 안 바뀌었는지까지 확인. 설치 시험도 임시 prefix·별도 POD_SOCKET.
- 문서 상대 링크 462개 모두 유효(앵커 포함), 프로토콜 번호가 코드·docs/03에서 일치.
- vendor 패치를 레지스트리 캐시를 건드리지 않고 `[patch.crates-io]`로 관리하고 POD-PATCHES.md에 사유 기록.
- UI 테스트를 GPUI headless 플랫폼에서 돌려 CI에 올리기 좋은 구조.

## 4. 우선 수정 제안

1. **CI 추가**(quality-01) — fmt·clippy·build·test·deny를 windows 러너에서. fmt 정리(quality-02)와 같이, 병렬 podd/pod-cli 작업 병합 뒤.
2. **라이선스 정리**(quality-03, -04, -13) — deny.toml 근거 수정·미관리 알림 노출, vendor 수정 파일 고지.
3. **테스트 대기 개선**(quality-06, -07) — 데몬 "한 바퀴 끝남" 신호로 고정 sleep 대체, CI에서 건너뛰기를 실패로.
4. **릴리스 체계**(quality-11, -05, -12) — 버전·태그·CHANGELOG, 정기 공개, publish 스크립트 오류 처리.
5. **문서 동기화**(quality-08, -09) — README/CONTRIBUTING 목록, docs/19 머리말, docs/01 백로그.

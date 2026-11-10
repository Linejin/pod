# 오케스트레이션·세션 / 엔지니어링 품질 재평가

평가 대상: master `813589efd1afad38d6d87ad7b0e68b52027e0061` (2026-11-10). 이전 기준은 `47dff8a`, [orch.md](orch.md)·[quality.md](quality.md). 수정 상태 칸 대신 현재 코드와 테스트를 대조했다. 제품 코드 변경 없음.

## 1. 점수표

비교를 위해 **두 보고서의 기준·가중치를 각각 그대로 유지**했다. 아래 O=오케스트레이션, Q=품질·릴리스. 숫자는 이전 → 현재이며 각 점수는 0–10이다. 품질의 별도 「릴리스 준비도」도 삭제하지 않았다.

| 기준 | O 가중치 | O 전 → 후 | Q 가중치 | Q 전 → 후 | 현재 판단 근거 |
|---|---:|---:|---:|---:|---|
| 정확성(버그 없음) | 25% | 7 → 7 | 15% | 8 → 8 | done 재생·branch tip 검증은 개선됐으나 unreachable 워커 오판과 wake 입력 유실이 남음. |
| 견고성(오류·경합·재시작) | 20% | 5 → 7 | 10% | 7 → 7 | 저장 실패 주입·통지 예약 분리·재접속이 추가됨; holder 복구와 orch 상태의 연결 및 Job 실패 처리는 불완전. |
| 보안·개인정보 | 15% | 7 → 5 | 15% | 7 → 7 | discovery 오류 전달은 개선; 새 무승인 commit 경로가 Git filter 실행을 허용. 공개 검사는 강화됐지만 제한된 패턴 검사임. |
| 성능·자원 | 10% | 7 → 8 | 5% | 6 → 7 | 출력 tail/cap·프로세스 트리 관리, 기본 빌드/CI의 spike 제외가 유효; 고정 sleep과 제한 없는 commit Git 호출은 잔존. |
| 테스트 품질(결함 검출·불안정성) | 15% | 8 → 8 | 20% | 7 → 7 | 잠긴 파일·손자 프로세스·도구 누락 테스트가 실질적. 새 기능 간 교차 회귀와 sleep 기반 부재 단언은 잡지 못함. |
| 코드 구조·유지보수성 | 5% | 8 → 8 | 10% | 7 → 8 | 공용 bounded runner·순수 엔진 분리, fmt·lint opt-in·정책 스크립트 개선. |
| 문서 정확성 | 5% | 8 → 7 | 10% | 6 → 7 | 문서 목록·백로그는 개선; v19 병합 뒤 docs/19 헤더가 v18이며 설치 버전 예외 설명도 낡음. |
| 사용성·접근성 | 5% | 7 → 8 | 5% | 7 → 8 | 미확정 통지 안내, unreachable 사유, 의견 서랍·설정 컨트롤의 키보드 경로 개선. 이번에 GUI/스크린리더 실측은 하지 않음. |
| 릴리스 준비도 | — | — | 10% | 4 → 5 | 개발 설치 파일은 SHA로 구별됨. CI는 아직 공개되지 않았고 현재 정책 검사부터 실패; 릴리스 태그 없음. |
| **가중합** | **100%** | **6.85 → 7.05** | **100%** | **6.70 → 7.10** | 이전 O의 표시값 6.9는 6.85를 반올림한 값. |

**이 두 영역의 종합: 7.08 / 10**. O·Q를 50:50으로 합산: `(7.05 + 7.10)/2 = 7.075` → 7.08. 동일 방식의 이전 원점수는 6.775 → 6.78(이전 표시점수 6.9·6.7의 평균은 6.8). README의 **6.71은 여섯 영역 전체 평균**이므로 이번 두 영역 종합과 직접 비교하거나 전체 새 점수로 대체하면 안 된다.

## 2. 이전 발견 사항 전수 확인

해결 확인 13 / 부분 3 / 미해결 2 / 회귀 1. 「해결 확인」은 해당 원래 결함의 수정 확인이며 제품 전체 무결함이나 모든 테스트 재실행을 뜻하지 않는다.

| id | 이전 심각도 | 지금 | 현재 코드·테스트 근거 |
|---|---|---|---|
| orch-01 | 높음 | 해결 확인 | `pod-orch/src/engine.rs:1111` 우편 선기록, `:1211` 세대·reopen 일치 재생, `:1059` 동일 done 재시도; `cargo test -p pod-orch done --locked` **2 passed**(mail/run 쓰기 실패). |
| orch-02 | 높음 | 부분 | `engine.rs:1746` 비영속 예약 / `:1762` 전달 후 영속, 재시작 테스트 **1 passed**; 앱의 미확정 표시도 있음. 그러나 새 입력 backlog 거절을 무시함 → 발견 04. |
| orch-03 | 높음 | 부분 | `podd/src/server/repo_config.rs:377` 공용 bounded tail 사용, bounded 테스트 **7 passed**; Job 생성·할당 실패 시 직접 자식만 관리하는 경로는 남음 → 발견 05. |
| orch-04 | 중간 | 해결 확인 | `podd/src/server/task_archive.rs:78` base 포함 또는 `:87` merged head와 현재 tip 일치만 삭제 허용; `:631` 추가 커밋·head 미상 테스트, `server/pr.rs:94` head 저장 연결 확인. |
| orch-05 | 중간 | 해결 확인 | `podd/src/server/discovery.rs:190` 저장/삭제 오류 수집 → `:225` PERSIST_FAILED, `:612` 메모리 즉시 off; `podd/tests/discovery.rs:447` cache/settings 잠금·재시도·저장 후 재시작 검사. 실패한 설정 자체가 재시작에도 보존된다는 뜻은 아님. |
| quality-01 | 높음 | 부분 | `.github/workflows/ci.yml`에 Windows fmt/clippy/build/test와 Linux deny 존재. 원격 main에는 아직 없음; 현재 check-docs도 실패 → 발견 03. |
| quality-02 | 중간 | 해결 확인 | 현재 `cargo fmt --all -- --check` exit 0; CI에서 fmt 실패를 무시하지 않음. |
| quality-03 | 중간 | 해결 확인 | `deny.toml:33` 출하 GPUI 경로로 근거 수정, `:60` unmaintained=all 및 사유·재검토일 있는 ignore; 이번 deny도 4개 검사 모두 ok. |
| quality-04 | 중간 | 해결 확인 | vendor의 수정 5개 파일 고지·POD-PATCHES 목록 확인; `check-vendor-notices.ps1` **vendor notices ok (5 files)**. 이번에는 레지스트리 전체 diff를 재실행하지 않음. |
| quality-05 | 중간 | 미해결 | 읽기 전용 `git ls-remote origin refs/heads/main`도 `0aa5ba2`; 현재 master와 **1252 files changed**. 생성 QA 대량 산출물도 여전히 추적됨 → 발견 07. |
| quality-06 | 중간 | 미해결 | `podd/tests/orch_runs.rs:724,755` 3초 후 중복 없음 단언, `tests/orchestration.rs:323,346` 1.5/2초 후 부재 단언이 그대로임 → 발견 06. |
| quality-07 | 낮음 | 해결 확인 | 공용 `tools/test-support/require_tools.rs:24` + hooks/SSH/gaps/RA 호출부 확인. CI는 POD_REQUIRE_TOOLS=1; Codex는 명시 skip, RA 2개는 여전히 명시 ignore. |
| quality-08 | 낮음 | 해결 확인 | README 15–19 링크·CONTRIBUTING의 README 참조 확인; 문서 검사에서 목록 누락은 없고 프로토콜 헤더 오류만 발생. |
| quality-09 | 낮음 | 회귀 | docs/01의 중복 백로그는 정리됐지만 `docs/19-worktree-sessions-orchestration.md:3`은 v18, `podd/src/protocol.rs:75`는 19; 추가된 검사기가 실제 실패 → 발견 03. |
| quality-10 | 낮음 | 해결 확인 | pod-hook·pod-status·pod-terminal-view가 workspace lint 상속; `check-workspace.ps1` **16 members, 14 default** 통과. |
| quality-11 | 낮음 | 해결 확인 | Rust 최소 버전 상속·CHANGELOG·태그 요구 코드 존재. `build.ps1 -PrintVersion` → **0.1.0-dev.813589ef**; `:48` Release는 태그/추적 파일 청결 검사. 실제 릴리스 태그·서명 검증은 별도 미완료. |
| quality-12 | 낮음 | 해결 확인 | publish의 Invoke-Git가 종료 코드를 검사하고 push 성공 후에만 main 이동; generic 패턴·canary 추가. 임시 저장소 테스트 **8개 모두 통과**, 실패 push에서 성공 출력/main 이동 없음 확인. |
| quality-13 | 낮음 | 해결 확인 | `docs/design/d4-claude/src/icons/NOTICE.md:12,40` Fluent·Phosphor 각각 MIT 책임 제한 문구까지 포함. |
| quality-14 | 낮음 | 해결 확인 | Cargo 기본 멤버에서 두 spike 제외, CI build/clippy/test도 명시 exclude. `--workspace`를 직접 쓰면 포함되는 것은 그대로이며 fmt 대상이기도 함. |

표에서 생략한 crate 경로의 접두사는 `crates/`이다.

## 3. 발견 사항 — 확인됨

**치명 0 / 높음 3 / 중간 4 / 낮음 1 (8건)**. 기존 미해결과 새 교차 회귀를 함께 집계했다. S=국소 수정, M=여러 경로+회귀 테스트, L=실행/복구 설계 변경. 코드 경로로 확인한 조건부 결함과 실제 실행 재현을 구별했다.

| 심각도 | id | 제목 | 근거 | 영향 | 구체적 수정 제안 | 크기 |
|---|---|---|---|---|---|---|
| 높음 | orch-quality-01 | **해결** (`a5c7da3`, docs/19 §18.5: plumbing 커밋·filter 드라이버 비움·필터 경로 거부·bounded, facts/diff도 차단) — 새 무승인 commit이 저장소 filter를 실행 | `crates/podd/src/git.rs:126,134,139,141`은 hooks/fsmonitor만 끄고 일반 `git add/commit` 실행; `server/codex_allowance.rs:57`은 commit 무승인 허용. 격리 linked worktree에 `filter.demo.clean=sh clean.sh`를 설정하고 같은 옵션으로 실행한 결과 **commit succeeded, outside-worker.marker=True**. | 기존 filter가 작업 폴더의 스크립트를 부르면 워커가 바꾼 코드가 daemon 권한으로 실행됨. 「nothing the agent wrote may run」(`git.rs:104`) 보장 위반. 모든 저장소에 filter가 있다는 주장은 아님. | worker commit에서 filter/process·서명 프로그램 등 외부 실행 설정을 제거/거부하는 경계를 설계하고 실행 시간·출력도 제한. filter가 있는 repo는 안전한 경로가 마련될 때까지 명시 승인을 요구. tracked filter script를 바꾸는 회귀 테스트 추가. | L |
| 높음 | orch-quality-02 | **해결 (d846882)**: 시작 때 재시도 중인 unreachable holder의 열린 Attempt는 Lost가 아니라 대기(자원 유지), 재접속 시 turn 기준 재설정, 재시도 소진·holder 사라짐·`pod kill` 때만 Lost(`holder_unreachable`); 그 사이 stop은 Unknown. 늦게 응답/영영 무응답 가짜 holder 통합 시험. — unreachable holder 재접속 전에 워커를 Lost로 확정 | `crates/podd/src/server/mod.rs:277,278,289`는 rediscover→비동기 retry→orch start. `orch_runs.rs:499`는 연결된 terminals만 조회하고 `:2127` 미발견을 Lost 처리; supervisor `:2213`도 동일. `lifecycle.rs:458` 재연결은 terminal만 추가. | 살아 있는 worker도 일시 pipe timeout 후 attempt가 닫힘. 나중에 연결돼도 done은 fenced되고 item 재시도와 기존 worker 작업이 겹칠 수 있음. 현재 `tests/unreachable.rs:125`는 terminal 재연결만 시험하고 orch attempt를 만들지 않음. | unreachable/retrying 상태에서는 attempt와 자원 예약을 유지하고 최종 Gone/포기 시 종료. 재접속 때 seen_turn 재설정; active attempt + busy holder + daemon restart 교차 테스트 추가. | M |
| 높음 | orch-quality-03 | CI는 아직 비활성이고 현재 트리도 정책 검사에서 실패 | 원격 main=`0aa5ba2`; `CONTRIBUTING.md:25`도 미활성 명시. `.github/workflows/ci.yml:45` 정책 검사가 `check-docs` 비정상 종료 시 중단. 실행 결과 **FAIL: docs/19-worktree-sessions-orchestration.md says protocol v18, code is v19**, exit 1. | 자동 회귀 검증이 없다는 원래 위험이 남음. 파일만 게시해도 Windows 잡이 clippy/build/test 전에 실패함. | docs/19 헤더를 현재 프로토콜과 맞추고 정책 검사 통과 후 권한을 갖춰 공개 CI를 실행. 실제 Windows/deny 결과를 릴리스 증거로 보관. 이번 평가에서는 게시하지 않음. | M |
| 중간 | orch-quality-04 | **해결 (d846882)**: wake 본문은 `try_input`, 거절(backlog/Gone)이면 Enter 없이 미입력 처리 후 재시도; Unverifiable은 전달로 치지 않고 미확인. 단위 시험 `wake_outcome_needs_the_line_and_a_confirmed_enter`. — wake 본문 입력 거절을 무시하고 Enter 전송 | `crates/podd/src/server/orch_runs.rs:2386`은 `t2.input(...)`의 bool 무시. `terminal.rs:137`은 backlog 거절 가능. `submit.rs:186,197`은 Enter 성공만으로 Unverifiable 반환 가능, `orch_runs.rs:2300`은 이를 delivered 처리. | 입력 잔여 용량이 본문보다 작고 CR 1바이트 이상이면 본문은 누락되고 Enter만 들어감. 미확정 경로도 실제 입력되지 않은 통지로 사용자 개입을 기다릴 수 있음. 코드 조건으로 확인; PTY 재현은 미실행. | 본문에 try_input을 사용하고 거절 시 Enter를 보내지 말 것. Gone/Backlog를 구별하여 예약 해제·재시도; budget−1 상태와 ACK 사이 경합 테스트 추가. | M |
| 중간 | orch-quality-05 | **해결 (67616ac)**: daemon-security-03과 같은 수정(suspended→Job→resume, 실패는 Err, 스레드 join). — bounded runner가 Job 실패를 숨기고 실행을 계속 | `crates/pod-forge/src/bounded.rs:211` Job 생성, `:212` spawn, `:213` 할당 실패→None, `:258` None이면 직접 child만 kill. `:369,390` Win32 오류를 Option/bool로 버림. | Job 생성/할당 실패 환경에서는 손자 종료 보장이 사라짐. `:175` reader는 deadline 뒤 detach될 수 있어 반복 실행 시 남은 프로세스/스레드 누적 가능. spawn 후 할당 전 탈출 창도 코드 주석 `:9`가 인정함. 실패 경로는 코드 확인, OS 오류 주입은 미실행. | Windows에서 Job 생성·할당 실패를 실행 실패로 반환하고 child를 정리. suspended 생성→Job 할당→resume으로 탈출 창 제거; 실패 주입과 즉시 손자 생성 fixture 추가. | M |
| 중간 | orch-quality-06 | **해결 (d846882, 오케스트레이션 시험만)**: `orch.queue`에 `supervisor_passes`·`wakes_in_flight`; `tests/orch_runs.rs`의 sleep 8곳을 "두 번 더 돈 감독 루프 + 진행 중 알림 0" 장벽으로, `tests/orchestration.rs` 두 곳은 기록된 `delivered_state`와 제출 확인 정착으로 교체(나머지 tests 트리 sleep은 범위 밖). — 고정 sleep 뒤 「중복/전달 없음」 단언 잔존 | `crates/podd/tests/orch_runs.rs:710,724,751,755,839`, `tests/orchestration.rs:323,346`. 현재 tests 트리의 직접 Duration 상수 sleep **88곳**, 300ms 이상 **51곳·101.4초**(각 위치 1회 합, 폴링 포함 정적 통계). | scheduler가 해당 pass를 수행하기 전 단언하면 거짓 통과할 수 있음. 관측 창 뒤 중복은 놓침. 정적 합을 실측 실행 시간이나 flaky 발생률로 해석하면 안 됨. | supervisor/delivery pass 순번이나 테스트 barrier를 기다린 뒤 상태/이벤트 증가량 비교. 의도적으로 지연된 중복과 backlog 거절을 검출하는 테스트 추가. | M |
| 중간 | orch-quality-07 | 공개본 뒤처짐과 대량 생성 산출물 문제 지속 | 원격 main SHA를 읽기로 확인; `git diff --shortstat main master` → **1252 files changed, 290167 insertions(+), 10082 deletions(-)**. `crates/pod-app/qa/d3/contrast.json` 등 생성 QA 파일도 여전히 추적. | 공개본에 새 기능·수정·CI가 없고 다음 snapshot 리뷰 비용이 더 커짐. publish 스크립트의 안전성 개선과 공개 완료는 별개. | 공개 주기·릴리스 체크포인트를 정하고 대량 QA 결과는 아티팩트로 보관. 변경 요약과 검증 결과를 붙여 검토 후 게시. | M |
| 낮음 | orch-quality-08 | 설치 문서에 이미 없어진 버전 예외가 남음 | `docs/14-install.md:29`는 pod-app 독립 0.1.0·pod-terminal-view 0.0.1을 다음에 맞춘다고 설명. 현재 두 `Cargo.toml:3` 모두 `version.workspace = true`. | 릴리스 시 어느 버전을 수정해야 하는지 문서가 서로 모순됨. 실제 installer SHA 이름표 동작은 정상. | 예외 문장 제거하고 workspace 버전 상속을 문서 검사에 포함. | S |

### 검증 실행과 한계

- 모든 cargo 실행 전 요청된 CARGO_TARGET_DIR 제거, sccache 경로, jobs=2, `cpu-gate -MaxPct 70 -MaxCargo 1`, 여유 RAM ≥8GB 조건을 적용했다. 전체 workspace build/test는 실행하지 않았다.
- 실제 실행: `cargo test -p pod-orch done --locked` **2 passed**; `cargo test -p pod-orch wake_scheduled_delivered_failed_and_restart --locked` **1 passed**; `cargo test -p pod-forge bounded --locked` **7 passed**. fmt check exit 0. deny는 advisories/bans/licenses/sources 모두 ok(사용되지 않은 ISC 허용 경고 1개).
- PowerShell 검사: workspace·vendor notices·publish 테스트 통과, docs 실패. `build.ps1 -PrintVersion`만 실행했고 설치 파일을 빌드하거나 설치하지 않았다. publish 테스트는 자체 임시 저장소/존재하지 않는 로컬 origin을 사용했다. 실제 원격에는 `ls-remote` 읽기만 했다.
- filter 재현은 `%TEMP%/pod-rescore-git-6n6x572e`의 repo/linked worker/no-hooks 및 합성 marker만 사용했다. `commit_for_worker`와 같은 Git 옵션의 동작 재현이며 실제 Codex sandbox 공격이나 daemon RPC 종단 테스트를 했다는 뜻은 아니다.
- holder v1–3의 협상/ACK 분기(`holder_proto.rs:31`, `server/terminal.rs:345`), v18 list 응답 변환(`server/mod.rs:547`, `protocol.rs:471`), `tests/gaps.rs:228`·`tests/unreachable.rs:198`을 읽었다. 프로토콜 상수끼리 충돌하는 결함은 발견하지 않았고, holder 복구와 orch의 결합 결함은 02로 분리했다.
- 의견 서랍(`pod-app/src/review.rs:1655,1921`, `layouts/review_qa.rs:531`)과 설정 select/switch(`pod-ui/src/controls.rs:210,380,462`, `pod-app/src/chrome.rs:1385`)의 포커스·Esc·단일 키 활성화 경로를 읽었다. 이번에는 UI 테스트/실제 화면을 실행하지 않아 추가 UI 회귀를 확인했다고 주장하지 않는다.
- 추정(집계 제외): Codex allowance가 허용하는 `--body-file`/`--message-file`의 무제한 파일 읽기(`pod-cli/src/orch_run.rs:383`)와 실제 sandbox의 읽기 제한 간 차이는 별도 검증 필요. 실제 agent/config 접근 금지에 따라 실험하지 않았다. 장시간 부하·반복 flaky 비율·CI 러너 결과도 미확인.

## 4. 잘 된 점

- done을 재생 가능한 우편 기록에 연결하고 저장 실패를 직접 주입한 수정은 원래의 완료 유실 결함을 실질적으로 해결했다.
- bounded runner의 정상 Job 경로는 손자 파이프·timeout·출력 초과를 실제 테스트로 잡는다. discovery도 실패를 성공으로 숨기지 않는다.
- fmt·lint 상속·deny 근거·vendor/아이콘 고지·공개 실패 처리·개발 설치 버전 식별이 개선됐다. 새 정책 검사기는 실제 문서 회귀를 잡았다.

## 5. 우선 수정 제안

1. **01**: worker commit의 임의 filter/외부 프로그램 실행을 막고, 안전성이 보장되지 않은 repo의 무승인 commit을 제한한다.
2. **02**: unreachable holder 재시도 동안 orch attempt를 보존하는 교차 복구 경로와 테스트를 추가한다.
3. **03**: 현재 문서 검사 실패를 고친 다음 CI를 실제 활성화하고 성공 실행을 릴리스 조건으로 삼는다.
4. **04·05**: 입력 수락 여부를 확인한 뒤 Enter를 보내고, Job 관리 실패 시 제한 없는 실행을 계속하지 않는다.
5. **06·07·08**: sleep 부재 단언을 동기화 신호로 교체하고, 버전 문서·CHANGELOG·검증 아티팩트를 정리해 공개 갱신을 준비한다.

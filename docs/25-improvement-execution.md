# 개선 작업 실행 기록

시작일: 2026-11-13 · 최근 기록: 2026-11-24

## 작업 기준

- 통합 브랜치: `improvements/stability-foundation`
- 통합 worktree: `C:/Users/Sejin/orca/workspaces/pod/pod-improvements`
- 시작 기준: 설치 준비 브랜치 `SJin-Choi/pod-signed-installer`의 `ad0003e0495e509f361dda404622fedcda1d6c64`. 제품 소스는 검토 기준 `e3b4db8`과 같다.
- 요구사항: [개선 과제와 기능 로드맵](24-improvements.md).
- `main`/`master`에는 병합하지 않는다. 원격 브랜치 갱신·PR·릴리스도 이번 작업에 포함하지 않는다.
- 워커는 통합 브랜치의 초기 문서 커밋을 기준으로 별도 브랜치와 worktree에서 작업한다. 결과는 검토 후 통합 브랜치에만 반영한다.

## 2026-11-24 통합과 검증

| 변경 | 통합 커밋 | 확인 범위 |
|---|---|---|
| 설치 패키지 의존성 및 로컬 터미널·worktree 회귀 | `0b4c81a`, `0d59615` | 배포 정책 재검사와 로컬 daemon 집중 검사. 새 설치 패키지는 미생성. |
| 프로젝트 에이전트 그래프 | `9d0ada5`, `b76298e` | 좁은 저장소 행, 노드 선택, 저장소 전환 직후 종료·복원 결함을 수정하고 Windows 앱에서 재검증. |
| A2A 1.0 JSON-RPC 클라이언트와 CLI | `82833d7`, `afb7f9d` | 통합 코어·CLI 검사와 별도 워커의 표준 SDK 상호운용 시험. |
| AI 사용 한도 모니터, 좁은 창 카운트다운, Claude 설정 복사 | `a68f371`~`3f961aa` | 워커의 자동·셸 fixture 검사와 로그인된 Codex 읽기 확인. 통합 Windows 앱의 표시·접근성·캐시 흐름 검증 80개 통과. |

소스별 결과와 남은 범위는 [기능 검증 결과](27-feature-validation-results.md)에, 반복 절차는 [기능 검증 계획](26-feature-validation.md#후속-기능-검증-계획)에 기록한다.

### 통합 검증 브랜치

`improvements/combined-validation`에는 편집기 브라우저(`f4b66c9`), A2A 그래프 UI(`1346a88`, `4e65e19`), 문서 정합성 감사와 CI 검사(`687f639`, `d14af9e`)를 함께 반영했다. 사용량 모니터의 실제 Windows 검증은 기존 worktree의 `3f961aa`에서 마쳤다. [결합 검증 결과](27-feature-validation-results.md#2026-11-24-결합-검증)

## 1차 작업 배치

새 기능이 의존하는 데이터 보존·실행 수명 결함 B01–B05부터 시작한다. 나머지 결함과 기능 제안은 후속 배치이며, 이 문서의 시작만으로 전체 완료를 의미하지 않는다.

| 워커 | 과제 | 소유 범위 | 완료 조건 | 상태 |
|---|---|---|---|---|
| editor-rename | B01: rename 후 dirty·저장·LSP 경로 | `crates/pod-editor` | 파일·폴더 rename 후 편집과 저장·닫기·LSP 회귀 검사, 국소 수정 커밋 | 검토·통합 완료 |
| daemon-lifecycle | B02–B04: 잠금 순서, keep과 release 경합, 출력 보관 실패 | `crates/podd/src/server/orch_runs` 및 해당 데몬 회귀 검사 | 결정적 동시 실행·보관 실패 시험, 실패 시 데이터·프로세스 보존 | 검토·통합·결합 검사 완료 |
| orch-persistence | B05: 메모리 선변경과 저장 실패 | `crates/pod-orch` | 관련 전이에 저장 실패를 주입하고 메모리·디스크·재시작 일관성 확인 | 검토·통합 완료 |

모델은 앞서 요청한 `gpt-6-astra`를 사용한다. 워커끼리 공유 파일을 수정하지 않으며, 소유 범위를 넓혀야 하면 코디네이터에게 먼저 알린다. 공용 문서와 통합은 코디네이터가 담당한다.

## 검증과 통합

1. 각 워커가 현재 제어 흐름과 기존 회귀 시험을 확인하고 재현 가능한 실패 조건을 만든다.
2. 최소 수정 후 관련 검사 명령·종료 코드·실행하지 못한 이유를 보고한다.
3. 코디네이터가 diff·회귀 범위·호환성과 작업 간 충돌을 검토한다.
4. 통합 브랜치에만 워커 커밋을 반영하고 필요한 결합 검사를 수행한다.
5. 과제 ID별 구현 커밋·검증 결과·남은 제한을 기록한다. 구현과 검증 완료를 구분한다.

로컬 Rust 도구는 Smart App Control 오류 4551로 차단될 수 있다. 보안 설정을 바꾸지 않으며, 실행하지 못한 cargo 검사를 성공으로 기록하지 않는다. 기존 원격 CI의 성공은 새 변경 검증으로 재사용하지 않는다.

## 결과 기록

### 시작 기록

- Run: `run_b2d83d8d7f6a`
- 공통 기준 커밋: `d7d20cb17ce8156e8c4cad80d2fb41364ed32522`
- 세 워커 모두 `gpt-6-astra`, effort `high`로 시작 영수증의 `ready`를 확인했다.
- 세 워커 브랜치의 기준 커밋 포함 여부와 `main` ref가 기존 `e3b4db8`에서 바뀌지 않았음을 확인했다.

| 워커 | 브랜치 | worktree | Task / Dispatch |
|---|---|---|---|
| editor-rename | `SJin-Choi/pod-fix-editor-rename` | `C:/Users/Sejin/orca/workspaces/pod/pod-fix-editor-rename` | `task_53f3a981aea5` / `ctx_401f1f4a16b3` |
| daemon-lifecycle | `SJin-Choi/pod-fix-daemon-lifecycle` | `C:/Users/Sejin/orca/workspaces/pod/pod-fix-daemon-lifecycle` | `task_d952c97d0bc8` / `ctx_f4d52a9c1f98` |
| orch-persistence | `SJin-Choi/pod-fix-orch-persistence` | `C:/Users/Sejin/orca/workspaces/pod/pod-fix-orch-persistence` | `task_f2dbdfc36250` / `ctx_fa8fa5a5601b` |

### B05 상태 저장 실패 처리

- 워커 커밋: `f7995fa9b4a4ee50f0ad9cb650e62bc4dcdac8d0`
- 통합 커밋: `d8304406b810e1a9b9f1a1fa5b66c42f38a301b4` (`improvements/stability-foundation`에 cherry-pick).
- 변경: Attempt 관련 전이는 Run 복제본을 저장한 뒤 메모리에 반영한다. ID counter도 index 저장 후 확정한다. Queue 대기 판정을 먼저 저장하고 마지막에 시작을 승인하여 다른 Run의 저장 실패 때문에 실행 정보가 소모되지 않도록 했다.
- 검토: 코디네이터가 변경 제어 흐름과 실패 주입 검사를 확인했다. 기존 completion journal은 유지하며 mail·read receipt를 Run 교체 중 보존한다. 워커 소유 범위 밖 파일 변경은 없다.

워커 환경의 실제 검사 결과:

| 명령 | 결과 |
|---|---|
| `cargo test -p pod-orch --locked` | 통과: 단위 시험 50개(신규 실패 주입 회귀 10개 포함), wire snapshot 4개, doc test 0개 |
| `cargo clippy -p pod-orch --all-targets --locked -- -D warnings` | 통과 |
| `cargo fmt -p pod-orch -- --check` | 통과 |
| `git diff --check` | 통과 |

이 워커 실행에서는 SAC 차단이 발생하지 않았다. 초기 환경의 차단을 현재 검사의 미실행 사유로 재사용하지 않는다. 코드 변경 없이 동일 커밋을 통합했으므로 동일 crate 검사를 중복 실행하지 않았으며, 데몬 변경과 결합한 검사는 해당 워커 완료 후 판단한다.

통합 worktree에서 docs 색인, crate boundaries, workspace policy 검사를 모두 통과했다.

남은 제한: Run snapshot 복제 비용, Run 저장 실패 후 예약 ID의 빈 번호, 여러 Run의 대기 판정이 일부만 저장될 수 있는 동작은 허용한다. 기존 fsync 없는 저장의 전원 손실 보장은 바뀌지 않는다. Item·mail·wake 전체 트랜잭션과 B10 읽기 실패 복구는 이번 수정 범위 밖이다.

데몬 워커에는 `crates/podd/src/server/orch_runs.rs`의 최소 상태 필드 변경도 승인했다. 프로토콜 변경 없이 잠금·release claim을 조정하기 위한 범위 확장이며 다른 워커와 파일 소유가 겹치지 않는다.

### B01 rename 후 편집·저장·LSP 보존

- 워커 커밋: `08bdeb359ddc21d6945a9477876f28c7951ec204`
- 통합 커밋: `94db1bd`.
- 변경: 변경·편집 이벤트가 에디터 entity로 현재 경로를 찾도록 수정했다. Rename 때 같은 에디터 entity를 유지하며 LSP provider·언어 강조·진단·dirty 이벤트 경로를 갱신한다. 기존 예약 sync를 취소하고 미저장 본문으로 새 URI를 연다.
- 검토: 실제 rename 폼, 저장·닫기 경고, 파일·부모 폴더와 배경 버퍼, provider 4종, 확장자 전환, 옛 이름 재사용에 대한 회귀 범위를 확인했다. 새 회귀 5개는 기존 동작 검사를 포함하여 10개 시험으로 실행했다.

워커의 `cargo test -p pod-editor --test save_ui --test tabs_lsp`는 10개 모두 통과했다. 변경 파일 rustfmt와 `git diff --check`도 통과했다. 기존 소스를 임시 복원한 반증 검사에서는 dirty 누락과 LSP 본문·언어·옛 이름 재사용 관련 회귀 시험이 예상대로 실패했고, 구현 복원 후 전체 대상 시험을 통과했다.

SAC 차단은 없었다. 실제 GPUI workspace와 scripted LSP 서버를 이용한 시험이며, 네이티브 마우스 수동 검사와 실제 rust-analyzer 검증은 하지 않았다.

### B02–B04 데몬 잠금과 자동 정리

| 과제 | 워커 커밋 | 통합 커밋 |
|---|---|---|
| B02 | `7fc37f67489bce258aea7a6929e36936791b9167` | `6d70e82` |
| B03 | `7540354fc5f7d3556310d606e8cb7ea3d5a3fceb` | `b1267a7` |
| B04 | `8d48579877a8717e99b04a9f2ed71f62fa667060` | `615fcb9` |

- B02: RPC의 기존 ops 잠금으로 설정 쓰기를 직렬화한다. 설정 복제본을 별도로 저장한 뒤 engine→config 순으로 확정하여 반대 잠금 순서를 없앴다. 저장 실패는 유효 설정을 바꾸지 않는다.
- B03: 자동 정리 직전에 Finished·기한을 재검사하고 daemon 내 release claim을 설치한다. Keep·사람 입력은 같은 engine 잠금으로 조정한다. Claim 전 보존 요청은 프로세스를 보호하고, claim 이후 keep은 명시적으로 거부한다. 보관·제거 I/O 동안 상태 잠금을 잡지 않으며 오류·취소 시 claim을 해제한다.
- B04: 자동 보관 실패는 제거 전에 중단하고 기존 상태 문자열로 재시도 가능한 오류를 표시한다. 보관 경로를 제거 전에 저장하여 터미널이 사라진 뒤에도 최종 상태 저장을 재시도할 수 있다. 명시적 수동 release의 기존 보관 실패 시 강제 제거 동작은 유지한다.
- 검토: 코디네이터가 잠금 순서, RPC ops 직렬화, claim 생명주기, 오류 처리와 결정적 경합·실제 archive 쓰기 실패 검사를 확인했다. 프로토콜 스키마 변경은 없다.

워커 기준의 검사 결과:

| 명령 | 결과 |
|---|---|
| `cargo test -p podd --lib config_publication_does_not_block_engine_holder_reading_release_policy` | 통과: 1개 |
| `cargo build -p podd --example fake-agent -p pod-hook -p pod-cli --bin pod --bin pod-hook` | 통과: 통합 시험 도구 빌드 |
| `cargo test -p podd --test orch_runs automatic_release_ -- --nocapture` | B03 단계 통과: 2개 |
| `cargo test -p podd --test orch_runs archive_failure -- --nocapture` | 통과: 2개 |
| `cargo test -p podd --lib server::orch_runs -- --nocapture` | 최종 구현 통과: 10개 |
| `cargo test -p podd --test orch_runs` | **전체 실패: 21개 통과, 기존 waiting-worker escalation 시험 1개 시간 초과. 신규 회귀 4개는 통과** |
| `cargo test -p podd --test orch_runs a_waiting_worker_is_not_a_missed_done_and_is_woken -- --exact --nocapture` | 단독 재실행 통과: 1개. 전체 실행 실패의 원인은 미확정 |
| 변경 파일 rustfmt 및 `git diff --check` | 통과 |

단독 통과를 근거로 전체 시험을 통과 처리하지 않는다. SAC 차단은 없었고 vendor/conpty 부재에 따른 기존 inbox ConPTY fallback 경고가 있었다.

### 후속 결합 검증 완료

- 후속 Task: `task_9f8f8b8ec01a`, Dispatch: `ctx_21c6e5872f20`.
- 같은 데몬 워커의 별도 브랜치에 B05 커밋을 cherry-pick하여 결합 검사를 수행하도록 승인했다. 통합 worktree나 main/master는 수정하지 않는다.
- 보관 경로 저장 실패 시 제거 전 보존, 제거 후 Released 저장 실패 시 보관 경로·claim 해제·재시도/재시작 복구를 결정적으로 검사한다.
- 결합한 데몬 lib·전체 orch_runs 시험을 실행하고, 앞선 병렬 실행 시간 초과를 조사한다. 시간 제한 완화나 성공할 때까지 재실행하는 방식으로 결과를 숨기지 않는다.
- 보완 워커 커밋: `fe0683a12c9f54e91391c332824af9b3ec5cab7c`, 통합 커밋: `328ba1e7491282cd8aa3a9674a49fb8bb6b10329`.
- 보완 변경은 실제 저장 실패를 주입하는 회귀 시험 2개, 제거 직후의 debug 전용 rendezvous, 기존 시간 초과의 실패 시 진단 출력이다. 릴리스 빌드 동작과 기존 시간 제한·assertion을 바꾸지 않았다.

워커가 B02–B05를 결합한 환경에서 실행한 결과:

| 명령 | 결과 |
|---|---|
| `cargo test -p pod-orch --lib` | 통과: 50개 |
| `cargo test -p podd --lib server::orch_runs` | 통과: 10개 |
| `cargo test -p podd --test orch_runs` | 통과: 24개, 기본 병렬 실행, 앞서 시간 초과한 시험 포함 |
| `cargo test -p podd --test orch_runs persistence_failure -- --nocapture` | 최종 통과: 2개. 전체 대상 통과 후 claim 해제 검사의 결정적 rendezvous를 보강한 최종 파일로 실행 |
| 변경 파일 rustfmt 및 `git diff --check` | 통과 |

보관 경로 저장 실패는 터미널 제거 전에 멈추고 원래 출력·메모리·디스크 상태를 유지했다. 제거 후 Released 저장 실패는 유효한 archive 경로와 Finished·재시도 기한을 유지했다. 두 경우 모두 claim 해제를 확인했으며, 후자는 장애가 남은 상태의 데몬 재시작 이후에도 보관본으로 읽기와 재시도가 가능했다. 장애 제거 후 최종 Released 저장까지 완료했다.

초기 보완 시험 개발 중에는 Windows 경로 문자열 구분자 비교 오류와 fake-agent 결과 파일 기록 전에 제거된 시험 순서 오류가 있었다. Path 비교와 명시적 handshake로 수정했다. 직전 시험 프로세스가 실행 파일을 점유한 LNK1104 빌드 실패도 한 차례 있었고, 해당 프로세스 종료 후 검사를 실행했다. 이 실패들을 최종 제품 회귀 실패나 SAC 차단으로 잘못 분류하지 않는다.

코디네이터는 보완 diff와 실제 검사 로그를 검토한 뒤 보완 커밋만 반영했다. `git diff --exit-code`로 통합 브랜치의 `crates/podd`·`crates/pod-orch`가 결합 검증 워커와, `crates/pod-editor`가 편집기 워커와 정확히 일치함을 확인했다. 검증된 동일 소스의 시험을 중복 실행하지 않았다.

### 시간 초과 조사와 후속 과제 B20

최종 결합 실행에서는 과거 시간 초과가 재발하지 않았다. 이전의 21개 통과·1개 실패 결과는 위 기록에 유지한다. 새 실패 진단에는 관측된 턴·상태·터미널 출력·입력·메일을 포함한다.

진단용 임시 시험에서 같은 OSC working→idle 보고를 각각 입력하면 Idle·turn=1, 한 청크로 입력하면 Idle·turn=0이었다. `feed_output`이 청크 내 보고를 모두 반영한 뒤 한 번 재계산하고, `recompute`가 관측된 Working 상태를 떠날 때만 turn을 올리기 때문이다. 기존 fake-agent는 working·명령 실행·결과 파일 기록·idle 사이에 daemon 관측 확인을 하지 않는다.

이는 청크 경계에 따라 턴이 누락되는 메커니즘의 재현이다. 과거 실패의 원본 PTY 프레임은 남아 있지 않으므로 그 시간 초과의 직접 원인으로 확정하지 않는다. 이 문제는 [로드맵 B20](24-improvements.md)에 미수정 후속 과제로 추가했으며, `pod-status`나 알림 로직을 이번 배치에서 수정하지 않았다. 진단용 시험은 잘못된 동작을 정답으로 고정하지 않도록 제품 회귀 시험에 넣지 않았다.

### 1차 배치 완료 범위

B01–B05와 결합 회귀 검증까지 통합 브랜치에 반영했다. B06 이후 및 기능 제안 F01–F15는 미구현 후속 목록이다. 실제 rust-analyzer·수동 네이티브 UI·전체 workspace 시험·새 설치 EXE 생성은 이번 배치에서 검증하지 않았다. `main`/`master` 병합, 원격 push, PR·릴리스 발행은 하지 않는다.

최종 통합 문서 검사: docs 색인·crate boundaries·workspace policy·`git diff --check` 통과. 이 로드맵과 실행 기록의 상대 링크 84개가 존재하는 것도 확인했다.

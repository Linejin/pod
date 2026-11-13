# 개선 작업 실행 기록

시작일: 2026-11-13 · 상태: B05 통합 완료, 나머지 Astra 워커 2명 구현 진행 중

## 작업 기준

- 통합 브랜치: `improvements/stability-foundation`
- 통합 worktree: `C:/Users/Sejin/orca/workspaces/pod/pod-improvements`
- 시작 기준: 설치 준비 브랜치 `SJin-Choi/pod-signed-installer`의 `ad0003e0495e509f361dda404622fedcda1d6c64`. 제품 소스는 검토 기준 `e3b4db8`과 같다.
- 요구사항: [개선 과제와 기능 로드맵](24-improvements.md).
- `main`/`master`에는 병합하지 않는다. 원격 브랜치 갱신·PR·릴리스도 이번 작업에 포함하지 않는다.
- 워커는 통합 브랜치의 초기 문서 커밋을 기준으로 별도 브랜치와 worktree에서 작업한다. 결과는 검토 후 통합 브랜치에만 반영한다.

## 1차 작업 배치

새 기능이 의존하는 데이터 보존·실행 수명 결함 B01–B05부터 시작한다. 나머지 결함과 기능 제안은 후속 배치이며, 이 문서의 시작만으로 전체 완료를 의미하지 않는다.

| 워커 | 과제 | 소유 범위 | 완료 조건 | 상태 |
|---|---|---|---|---|
| editor-rename | B01: rename 후 dirty·저장·LSP 경로 | `crates/pod-editor` | 파일·폴더 rename 후 편집과 저장·닫기·LSP 회귀 검사, 국소 수정 커밋 | 시작 확인 |
| daemon-lifecycle | B02–B04: 잠금 순서, keep과 release 경합, 출력 보관 실패 | `crates/podd/src/server/orch_runs` 및 해당 데몬 회귀 검사 | 결정적 동시 실행·보관 실패 시험, 실패 시 데이터·프로세스 보존 | 시작 확인 |
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

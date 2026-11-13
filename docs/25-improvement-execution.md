# 개선 작업 실행 기록

시작일: 2026-11-13 · 상태: Astra 워커 3명 구현 진행 중

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
| orch-persistence | B05: 메모리 선변경과 저장 실패 | `crates/pod-orch` | 관련 전이에 저장 실패를 주입하고 메모리·디스크·재시작 일관성 확인 | 시작 확인 |

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

구현 커밋과 검증 결과는 아직 수신하지 않았다. 완료 보고를 받은 뒤 검토·통합 결과를 추가한다.

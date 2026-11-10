# orchestration·sessions 평가

기준: `47dff8a` (2026-11-10), 브랜치 `SJin-Choi/review-orch`. 제품 코드는 수정하지 않았다. 범위는 pod-orch, podd의 orchestration·sessions·설정/신뢰·worktree 수명, pod-discover, 관련 CLI/guide, 문서 08·11·18·19와 §17 dogfood 증거다. 아래 결함은 **코드 제어 흐름으로 확인**했으며 장애 주입 실행으로 재현했다는 뜻은 아니다.

## 1. 점수표

| 기준 | 점수 / 10 | 가중치 | 근거 |
|---|---:|---:|---|
| 정확성(버그 없음) | 7 | 25% | 정상 DAG·토큰 fencing·게이트 검증은 탄탄하나 완료 우편과 깨우기 유실 경로가 있다. |
| 견고성(오류·경합·재시작) | 5 | 20% | 큐 복구·깨진 JSONL 처리는 좋지만 여러 파일/외부 효과 사이 장애 복구가 불완전하다. |
| 보안·개인정보 | 7 | 15% | 명령 해시 승인, discovery 기본 꺼짐, 세션 UUID 검증이 있으나 개인정보 삭제 실패를 성공처럼 반환한다. |
| 성능·자원 | 7 | 10% | FIFO·주입 가능한 meter·스캔 예산은 좋고, archive 출력/프로세스 수명 제한은 부족하다. |
| 테스트 품질 | 8 | 15% | 토큰·세대·큐·재시작·선택지·실제 paste 지연 회귀를 검증한다. 저장 단계별 장애와 자식 프로세스 timeout 검증은 부족하다. |
| 코드 구조·유지보수성 | 8 | 5% | 순수 엔진/자원 판단과 daemon·meter 분리, 공통 decide_in이 명확하다. 부수 효과 트랜잭션은 분산돼 있다. |
| 문서 정확성 | 8 | 5% | dogfood의 수동 승인과 관찰 실패를 명시한다. timeout·개인정보 삭제 보장은 오류 경로에서 충족되지 않는다. |
| 사용성·접근성 | 7 | 5% | 양언어 guide·다음 행동·선택지 오류가 구체적이다. 실제 Codex 승인 부담과 대기 표현 문제가 남아 있다. GUI 접근성은 직접 검증하지 않았다. |
| **종합** | **6.9** | **100%** | 가중합 6.85를 소수 첫째 자리로 반올림. |

## 2. 발견 사항

치명 0 / 높음 3 / 중간 2 / 낮음 0. 크기: S=국소 변경, M=여러 경로 및 회귀 테스트, L=저장·복구 설계 변경.

| 심각도 | id | 제목 | 근거 | 영향 | 구체적 수정 제안 | 크기 |
|---|---|---|---|---|---|---|
| 높음 | orch-01 | 완료 상태 저장 후 우편 실패 시 재보고 불가 | `crates/pod-orch/src/engine.rs:981`에서 run 저장, `:983`에서 우편 전송; 실제 append는 `:1168`. 재호출은 `:939`의 Succeeded/Failed 검사로 거절. `crates/podd/src/server/orch_runs.rs:1394`의 request 결과 저장은 그 뒤다. | run 저장 후 index/mail 쓰기 실패 또는 프로세스 종료 시 Item은 완료되지만 코디네이터 우편은 없다. 동일 request_id도 아직 저장되지 않아 정상 재시도로 복구되지 않으며 의존 Item은 이미 ready일 수 있다. | 완료 전이와 우편을 하나의 journal/outbox 단위로 기록하고 재시작 시 미전송분을 재생한다. 이미 완료된 동일 보고의 재시도는 기존 결과를 반환한다. run 저장 직후·mail append 실패·remember 이전 종료를 각각 주입한다. | L · **해결** `d0c1314` — 완료 우편(세대·reopen 수·release 시각 포함)을 먼저 쓰고 run.json을 뒤에 저장, load 때 같은 열림의 완료 우편 재적용, 같은 내용의 done 재보고는 기록된 결과 반환. 잠긴 mail.jsonl·run.json 주입 테스트 |
| 높음 | orch-02 | 실제 입력 전에 깨웠다고 영속 기록 | `crates/podd/src/server/orch_runs.rs:1745`의 mark_notified가 `:1754` 이후 spawn/input보다 앞선다. unconfirmed는 `:1761`에서 로그만 남긴다. `crates/pod-orch/src/engine.rs:1325`는 notified가 있으면 깨우지 않고 `:1332`에서 저장한다. 시작 복구 `orch_runs.rs:1576`는 이를 초기화/재시도하지 않는다. | 기록 후 입력 전 daemon 종료 시 holder가 살아 있어도 unread 우편은 자동 통지되지 않는다. 제출 실패도 같은 상태에 남아, 폴링 없이 턴을 끝내라는 가이드와 결합하면 진행이 멈춘다. | pending/confirmed 통지 상태를 분리하고 미확정 건을 재시작 시 조정한다. 제출 실패에는 사용자에게 복구 행동을 노출한다. 중복 입력 방지와 함께 mark→input 사이 종료 및 unconfirmed 테스트를 추가한다. | M · **해결** `bd783fb` — 예약(메모리)과 전달 확인(영속)을 분리, Confirmed/Unverifiable에서만 `notified_ms` 저장, 입력 못 함은 다음 차례 재시도, 미확인은 중복 방지로 check·재시작 전까지 보류하고 로그에 복구 방법. 재시작 후 재깨우기 테스트. 앱 화면 표시는 **보류**(pod-app, fix-ui 영역) → **해결** `4d96fbd`, `8167717`, `9e2a313`(브랜치 `SJin-Choi/next-orch-ui`): `RunInfo`/`WorkerInfo.wake_unconfirmed_ms`(추가형 읽기 필드), 실행 보드에 누가·언제부터 + "다시 깨우기 (Enter)"(기존 `terminal.send {text:"", enter}`), check로 풀리면 `orch.changed`. podd·모델·fixture·네이티브 QA 테스트, 장면 `orch-wake` |
| 높음 | orch-03 | archive timeout이 프로세스 트리와 출력 수집을 제한하지 않음 | `crates/podd/src/server/repo_config.rs:349`는 stdout/stderr 전체를 read_to_end, `:361`은 부모 child만 kill, `:367`은 reader join을 무기한 대기한다. 4 KiB 절단은 `:371`에서 사후 수행. `crates/podd/src/server/task_archive.rs:98`의 전역 task_ops 잠금을 이 호출 내내 보유한다. | 대량 출력은 daemon 메모리를 소모한다. stdout/stderr를 상속한 장수 자식이 남으면 부모 timeout 이후에도 join이 반환하지 않아 다른 task 생성·삭제·재개가 막힐 수 있다. 문서 08 §8의 10분 제한이 전체 호출 제한이 아니다. | 기존 group check처럼 kill-on-close Job Object로 트리를 관리하고, 읽으면서 고정 크기 tail만 유지한다. 출력 수집에도 deadline/cancellation을 적용한다. 파이프를 상속한 자식과 대량 출력 테스트를 추가한다. | M · **해결** `25899a7`, `123c250` — 공용 `pod_forge::bounded`(kill-on-close Job Object 트리, drain 포함 전체 deadline, 읽으며 tail/상한). archive 스크립트·group check·gh runner가 사용. 손자가 파이프를 잡는 fixture·대량 출력 테스트 |
| 중간 | orch-04 | 과거 PR merged 상태만으로 현재 branch 삭제 허용 | `crates/podd/src/server/task_archive.rs:67`의 PR Merged 분기는 현재 tip 검사 전에 None 반환. `crates/podd/src/server/tasks.rs:947`이 이를 삭제 허가로 쓰고 `:1069`에서 branch_delete 실행. | PR 병합 후 같은 branch에 추가 커밋한 경우에도 merged로 간주한다. 추가 커밋을 push해 dirty/unpushed 검사를 통과하거나 force를 사용하면, base에 미병합인 현재 branch도 삭제된다. 원격 복사본 유무에 따라 복구 부담이 달라진다. | PR 병합 당시 head SHA와 현재 tip이 동일한지 검증한다. 다르면 현재 tip의 base 도달 가능성을 확인하고 증명 불가 시 branch를 보존한다. squash merge 이후 추가 커밋 사례도 테스트한다. | M · **해결** `b849977` — `TaskPr.head_oid`(gh `headRefOid`) 기록, 현재 tip이 base에 포함되거나 병합된 head와 같을 때만 삭제. 병합 뒤 커밋 추가·head 모름 테스트 |
| 중간 | orch-05 | discovery 끄기·미리보기 삭제의 저장 실패가 성공 응답 | `crates/podd/src/server/discovery.rs:94`의 write_json은 오류를 로그만 남기고 반환하지 않는다. `:361`의 cache 삭제 오류도 무시하고 `:376`에서 설정 저장, RPC `:541`은 정상 목록을 반환한다. 문서 18 §1은 끄면 cache를 지우고 preview off 시 저장 요약도 즉시 지운다고 명시한다. | 파일 잠금·권한·디스크 오류 시 개인정보가 남아도 사용자는 삭제 성공으로 이해한다. 설정 저장까지 실패하면 재시작 후 이전 enabled/preview 상태가 복원된다. | 저장/삭제 결과를 RPC까지 전달하고 개인정보 제거 실패를 명시한다. 메모리의 off 상태는 유지하되 영속 실패와 재시도를 관리한다. cache·settings 각각에 쓰기/삭제 실패를 주입해 디스크와 재시작 결과까지 검증한다. | M · **해결** `e54f279` — 메모리는 즉시 비공개, 디스크 반영은 `Result`, 실패 시 `-32043 PERSIST_FAILED {paths, error}`, 다음 `sessions.*` 호출·시작 때 재시도. `.tmp`는 실패 즉시 삭제, 깨진 cache는 삭제, 꺼질 때 `cache.corrupt-*`도 삭제. 잠긴 cache.json·sessions.json·재시작 테스트 |

### 검증 범위와 증거의 한계

- 새 테스트 실행 결과는 없다. CPU gate를 요구된 `-MaxPct 70 -MaxCargo 1`로 실행했으나 `cpu 1m=66%, cargo=1 (max 0 others)` 대기 상태여서 `cargo test -p pod-orch done` 시도를 빌드 전에 취소했다. 전체 workspace 빌드·실제 에이전트 호출·실제 Pod 상태 접근은 하지 않았다.
- 읽은 회귀 테스트에는 `engine::tests::ids_items_deps_and_reload`, `fences_token_and_generation`, `mail_check_ack_wake_and_reopen`, `resource::tests`, `podd/tests/orch_runs.rs`의 `resource_queue_fifo_meter_and_restart`, `gate_options_block_items_and_one_decision_path`, 세션/config 테스트와 `guide::tests::every_command_in_the_guide_parses`가 포함된다. 정상 reload 테스트 통과만으로 중간 저장 실패를 보장할 수 없다. flaky 여부는 반복 실행하지 않아 판정하지 않았다.
- §17 증거의 `scenario.log`는 두 Item 완료와 13.9분 뒤 두 branch 병합을 기록한다. `merged-test.txt`는 4 passed를 기록하지만 **기존 dogfood 산출물**이다. 코디네이터 최종 시험·보고는 하네스가 먼저 종료해 관찰하지 못했다는 문서 설명을 유지해야 한다. 로그 말미 unread 덤프의 0건을 실제 우편 교환 0건으로 해석하지 않는다(`mail-r1.txt` 별도 보관).
- 기존에 공개된 Codex sandbox 승인 4회, 워커 간 대기 시 조기 종료 escalation, 회색 prompt suggestion의 입력 오인은 §17.3의 알려진 문제다. 병렬 `orch-dogfood-fixes` 작업과 겹치므로 새 결함으로 중복 집계하지 않았고, 그 작업의 수정 결과는 본 기준 커밋에 포함하지 않았다.
- 미확인 추정은 결함 표에 포함하지 않았다. GUI/스크린리더 접근성, 장시간 부하, 신규 수정 브랜치의 실제 에이전트 재검증은 별도 평가 범위다.

## 3. 잘 된 점

- `decide_in`은 선택지·만료·scope·동시 결정·operation replay를 한 잠금 안에서 검증하고 저장 실패를 rollback한다. 이 패턴을 다른 상태 변경에도 적용할 수 있다.
- 토큰은 해시만 저장하고 세대가 지난 done은 fenced 기록으로 남긴다. 자원 큐는 FIFO, run별 제한의 예외, 한 pass당 한 worker 시작을 순수 함수로 검증한다.
- 설정 명령의 길이 구분 SHA-256 승인, 개인 설정의 명시적 우선순위, 잠긴/중첩 worktree 보호, UUID 제한, discovery 읽기 예산은 실질적인 방어다.
- review/group 결과를 코디네이터 우편으로 연결하고 guide 예제를 실제 clap 파서로 검사한다. 실제 dogfood가 드러낸 PATH·SessionStart·PowerShell 인자 문제를 회귀 테스트와 함께 기록한 점이 좋다.

## 4. 우선 수정 제안

1. orch-01: 완료 전이·우편·idempotency 결과를 복구 가능한 저장 단위로 묶는다.
2. orch-02: 통지 예약과 제출 확인을 구분하고 재시작/실패 시 깨우기 복구를 검증한다.
3. orch-03: archive 프로세스 트리·출력 메모리·전체 deadline을 제한한다.
4. orch-04: 삭제 판단을 과거 PR 상태가 아니라 현재 branch tip에 연결한다.
5. orch-05: 개인정보 삭제·설정 영속 실패를 사용자에게 전달하고 재시작 테스트를 추가한다.

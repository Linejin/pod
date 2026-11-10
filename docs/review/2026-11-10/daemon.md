# 데몬 코어 평가 — 2026-11-10

기준: `47dff8a17703608d2b73a278d9eb7abba0389129`, 브랜치 `SJin-Choi/review-daemon`. 제품 코드 변경 없음. `podd`(내부 `pod-holder` 바이너리 포함), `pod-status`, `pod-hook`, CLI 핵심·remote-stdio, `pod-remote` R3와 문서 03·04·07·17을 표본/제어 흐름 중심으로 검토했다. 별도 `crates/pod-holder`는 없다.

## 점수표

| 기준 | 점수 / 10 | 가중치 | 근거 |
|---|---:|---:|---|
| 정확성(버그 없음) | 7 | 25% | 정상 수명·상태 전이는 체계적이나 stop/run 경합과 재발견 실패 후 ID 재사용이 남음 |
| 견고성(오류·경합·재시작) | 6 | 25% | holder 독립 수명·stop 거절은 좋지만 busy 무한 재시도와 일부 복구 실패 처리 부족 |
| 보안·개인정보 | 8 | 15% | 사용자 SID DACL, 원격 pipe 차단, hook 토큰, 제한된 SSH 설정; 내부 큐 자원 고갈 위험 |
| 성능·자원 | 6 | 10% | 외부 attach 큐는 제한하지만 holder 입출력과 데몬→holder 큐는 무제한 |
| 테스트 품질 | 8 | 10% | 가상 시계 상태 전이·분할 프레임·fake SSH·실제 격리 pipe 회귀가 의미 있음; 아래 실패 경로는 공백 |
| 코드 구조·유지보수성 | 8 | 5% | 순수 상태 엔진/transport/holder 분리가 명확하고 프로토콜 타입이 공유됨 |
| 문서 정확성 | 8 | 5% | v18 및 R1–R3/실제 SSH 미검증 구분이 명확하나 holder 연결 15초 보장과 구현 불일치 |
| 사용성·접근성 | 8 | 5% | CLI JSON·SSH 한국어 오류·재연결 안내가 유용함; GUI 접근성은 평가 범위 밖 |
| **전체** | **7.05** | **100%** | 위 점수의 가중 산술평균; 정적 검토 평가이며 출시 인증 아님 |

## 검증 범위와 한계

- 아래 1–4는 실제 코드의 큐 종류·분기·await/잠금 순서를 추적해 확인한 결함이다. 장애 주입으로 실제 프로세스의 OOM·경합을 재현했다는 뜻은 아니다. 5는 경계 크기를 별도 계산했다.
- Cargo 빌드/테스트와 실제 SSH 접속은 실행하지 않았다. 기존 테스트를 읽은 사실과 실행 통과를 구분한다. 실사용 podd, 사용자 상태/에이전트 설정, 네트워크 쓰기는 건드리지 않았다.
- 프로토콜 v8–v18 변경 기록과 기본값/선택 필드, 현재 v18, hello 범위 11–18 및 원격 클라이언트 최소 12, `SERVER_STOPPING=-32050`, `PROTOCOL_MISMATCH=-32051`을 대조했다. 옛 바이너리 전체와의 실행 호환성을 증명하지는 않았다. 관련 근거: `protocol.rs:34`, `remote_protocol.rs:25`, `client.rs:194`, `tests/remote.rs`.
- `tests/stop_race.rs`는 기존 연결 거절·자동 재시작·holder 생존을 검사하지만, 진행 중인 `terminal.run`과 `kill:true`를 겹치지는 않는다. 300ms sleep으로 요청 도착을 가정하는 부분은 부하 의존 가능성이 있으나 불안정 실행은 관찰하지 않았다. `tests/gaps.rs:342`는 외부 attach 지연을 검사하며 holder 내부 큐를 검증하지 않는다. 상태 엔진은 가상 시간으로 우선순위·stale·취소·turn을 검사한다.
- 병행 작업 `orch-dogfood-fixes`의 podd/pod-cli 수정과 파일 영역은 겹친다. 이 보고서는 기준 커밋만 평가하며 좁은 Codex sandbox, worker waiting, grey suggestions 수정은 중복 구현하지 않았다.

## 발견 사항

경로는 저장소 루트 기준. 크기 S=국소 수정, M=여러 경로 및 회귀 테스트, L=구조 변경. **치명 0 / 높음 3 / 중간 2 / 낮음 0.**

| 심각도 | ID | 제목 | 확인 근거 | 영향 | 구체적 수정 제안 | 크기 |
|---|---|---|---|---|---|---|
| 높음 | daemon-01 | holder 내부 입출력 큐에 바이트 상한 없음 | `crates/podd/src/holder.rs:142`의 `std_mpsc::channel`, `:331`의 `unbounded_channel`, `:352`의 쓰기 대기, `:369` 입력 enqueue; `crates/podd/src/server/terminal.rs:218`도 무제한 | PTY 입력 소비가 멈추거나 연결된 데몬이 출력을 못 읽으면 생산이 계속되는 동안 메모리가 누적된다. 외부 attach 16MiB 제한으로 이 큐는 보호되지 않으며 holder OOM은 Job Object 내 프로그램 종료로 이어질 수 있음 | 입력은 바이트 예산과 명시적 backpressure/실패 응답, 출력은 한도 초과 시 느린 데몬 연결 종료 후 VT snapshot 재접속; Kill/Release 제어 경로는 입력 폭주와 분리. 소비 정지 시험 추가 | M · **해결** `b2e475c` — holder 출력은 연결별 8 MiB 예산(넘으면 PTY reader가 기다림=자식에 backpressure, 연결 종료·재접속 대신), 입력은 holder 프로토콜 3 `InputAck`로 데몬 미소비 4 MiB 한도(넘으면 `INPUT_BACKLOG` -32052로 거절), holder 입력 큐 16 MiB 상한(옛 데몬 대비). Resize/Kill/Release는 별도 우선 큐. 테스트: holder/terminal 단위 + `tests/input_budget.rs`. 남은 것: 출력이 실제로 멈춘 데몬을 상대로 한 장애 주입 E2E는 단위 테스트로만 확인 |
| 높음 | daemon-02 | `server.stop kill:true`가 시작 중인 터미널을 놓침 | `crates/podd/src/server/mod.rs:326`에서 terminals를 한 번 복제해 Kill/Release, `:349`에서야 in-flight 대기; `:940`에서 run은 연결을 await하고 `:970`에서 목록에 등록 | stop 전에 수락한 run이 kill 대상 snapshot 뒤에 등록되면 새 holder/자식은 kill 없이 살아남는다. 정상적으로 run을 완료한 경우에도 `stop --kill`의 전체 종료 의미가 깨짐 | stopping 전환 후 진행 중인 생성의 완료/취소를 먼저 확정하고 kill 집합 수집. run 등록 직전에 shutdown 상태를 검사해 시작한 holder를 정리. holder 준비 barrier로 결정적 경합 테스트 | M · **해결** `9765880` — run이 시작을 등록(`begin_start`, stop 뒤엔 거절)하고 kill stop은 시작 중인 run을 기다린 뒤 kill 대상을 모음. stop을 본 run은 자기 holder를 끝내고 `SERVER_STOPPING`. 회귀: `tests/stop_race.rs::stop_kill_also_ends_a_terminal_that_is_still_starting`(수정 전 holder·자식 생존 확인) |
| 높음 | daemon-03 | 재발견 실패한 holder의 ID를 다시 할당함 | `crates/podd/src/server/mod.rs:719` 레코드 파싱 후 `:742` 일반 연결 오류는 로그만 기록; `:746` 성공한 found의 최대 ID만 사용. `:911`/`:929` 같은 ID spec 생성, `crates/podd/src/server/status.rs:362` 토큰 덮어쓰기 | 태스크 참조가 없는 기존 t1이 handshake timeout/접근 오류를 내면 next_id가 t1로 돌아갈 수 있다. 아직 살아 있는 holder의 이름·토큰·상태 파일 공간과 새 실행이 충돌해 복구와 hook 인증을 손상시킴. 태스크 참조 ID에 대한 추가 보호(`mod.rs:284`)만으로 일반 터미널은 보호되지 않음 | 읽을 수 있는 모든 유효 HolderRecord ID를 연결 성공과 무관하게 예약하고 단조 next_id를 영속화. 연결 실패 레코드는 별도 상태로 유지. 실패 후 새 run이 기존 ID를 사용하지 않는 회귀 추가 | M · **해결** `9ab0566` — 모든 `t<n>.json` 기록(읽을 수 없는 것 포함)의 id를 연결 성공과 무관하게 예약, 실패 기록은 지우지 않음. 회귀: `tests/rediscover.rs`(수정 전 새 run이 t1). 보류: next_id 별도 영속화는 하지 않음(기록 파일이 그 역할). 후속 **해결** `313d0a0` — 연결 실패·읽을 수 없는 기록을 `unreachable`(사유 포함, 프로토콜 v19)로 목록에 두고 backoff로 6회 재시도, 연결되면 일반 터미널로 복귀, `pod kill <id>`로만 포기(기록·토큰 삭제로 id 예약 해제). hello < 19 연결엔 `exited`+`lost`. 회귀: `tests/unreachable.rs`(바쁘다가 응답하는 holder, 끝까지 안 되는 holder, 옛 hello), `lifecycle::tests` |
| 중간 | daemon-04 | busy holder 연결 재시도가 무기한이며 시작 전체를 막음 | `crates/podd/src/server/terminal.rs:166`의 `ERROR_PIPE_BUSY` loop에는 deadline 없음; `:212` timeout은 pipe open 이후 handshake에만 적용. `server/mod.rs:265` rediscover는 accept loop 이전, `:723` 순차 await. 문서 `docs/03-podd-p0.md:62`는 최대 15초라고 설명 | holder가 정지/과부하 등으로 새 pipe 인스턴스를 제공하지 못하면 run의 바깥 15초 deadline에 돌아오지 않는다. 재시작 때는 holder 한 개 때문에 전체 RPC 서비스 시작이 무기한 지연됨 | open 재시도와 handshake를 하나의 deadline에 포함하고 rediscovery를 개별 timeout으로 격리; busy를 계속 반환하는 double로 상한 검증 | S · **해결** `9765880`, `9ab0566` — `connect_within`: 바쁜 파이프 대기+handshake를 한 deadline(10 s)으로, rediscovery는 holder별 5 s로 동시에. 회귀: `terminal::tests::{busy,silent}_holder_*`, `tests/rediscover.rs`(첫 응답 시간). docs/03 갱신 |
| 중간 | daemon-05 | 4MiB 이하 hook payload가 envelope 때문에 통째로 유실됨 | `crates/pod-hook/src/main.rs:31`/`:171` stdin 한도 4MiB, `:147` 이후 HookMessage로 감싸 직렬화. `crates/podd/src/server/mod.rs:56`/`:507` 전체 요청도 4MiB. 계산 결과 `payload_bytes=4194304 wire_bytes=4194436 daemon_limit=4194304` | 허용된 크기의 유효 JSON hook도 연결이 닫히며 Stop/상태 이벤트 자체를 잃는다. 4MiB 초과 시 payload=null로 이벤트를 살리는 처리와 경계 동작이 불일치 | 직렬화한 최종 메시지 크기를 검사하고 초과 시 payload를 null 또는 필요한 식별 필드로 축약한 뒤 전송. 정확한 한도±1 및 envelope 추가 경계 테스트 | S · **해결** `34ca09f` — `HookMessage::to_wire_line`: 최종 줄 크기를 검사해 넘으면 작은 스칼라 필드 + `pod_truncated` 표시만 남김(이벤트 보존). podd는 `MAX_LINE >= MAX_HOOK_LINE` 컴파일 시 검사. 회귀: pod-status 경계(한도/한도+1, 4 MiB) + pod-hook 4 MiB E2E |

### 추정·미확인

실제 메모리 증가량/발생 빈도, stop/run 경합 발생률, 부하 중 테스트 불안정성은 측정하지 않았다. 실제 SSH 로그인·재연결 및 모든 과거 버전 바이너리의 호환성도 미검증이다. 이를 추가 결함 건수로 세지 않았다.

## 잘 된 점

- holder가 ConPTY·Job Object를 소유하고 snapshot과 구독을 같은 잠금에서 처리해 데몬 재시작과 출력 연속성을 설계에 반영했다.
- 외부 attach 큐 제한, 입력 프레임 한도, transport UTF-8/EOF/분할 읽기 검사, stop 이후 명확한 거절과 제한적 재전송은 좋은 방어다.
- SSH는 고정 원격 명령·별도 생성 config·엄격한 host key·BatchMode·forwarding 금지와 bounded stderr를 사용한다. R4 이후 설계를 R3 구현으로 과장하지 않는다.
- hook 토큰 검증·민감 토큰 비영속화, 순수 상태 엔진의 가상 시계, 재시작 시 turn/훅 복원 테스트가 실질적 회귀를 겨냥한다.

## 우선 수정 제안

1. stop/run을 하나의 생성 수명 규칙으로 묶고 종료 barrier 회귀부터 추가한다(daemon-02).
2. 실패한 holder ID 예약과 연결 전체 deadline을 함께 고쳐 재시작 복구를 보장한다(daemon-03/04).
3. holder 입출력에 바이트 예산을 적용하고 제어 프레임을 보호한다(daemon-01).
4. hook 최종 wire 크기를 검사해 상태 이벤트를 보존한다(daemon-05).
5. 위 실패 주입 회귀를 추가한 뒤 격리된 daemon에서 실행하고, 별도 허용된 환경에서 실제 SSH 검증을 진행한다.

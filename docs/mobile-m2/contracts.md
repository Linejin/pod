# M2 `/mobile/v1` 계약 (F1)

로컬 pipe는 v22를 유지한다. `GateInfo`, M1 `Mobile*` DTO와 관리 RPC는
변경하지 않는다. 아래 타입은 `pod_protocol::mobile_protocol::mobile_v1`
(또는 `protocol::mobile_v1`)이며 RPC schema에 등록하지 않는다.

| 요청 | 필수 필드 | 선택 필드 / 응답 |
|---|---|---|
| `tasks` | `params` 객체 | cursor, wait_ms / TaskPage |
| `task` | task | gate, cursor, operation, wait_ms / TaskDetail |
| `output` | task | OutputResult |
| `gate.decide` | machine, task, gate, revision, operation_id, decision, choice | choice는 null 가능하지만 생략 불가 / DecisionResult |
| `message` | task, operation_id, text, comments | comments는 선택한 코멘트 ID 배열 / MessageResult |
| `delivery` | task, delivery | DeliveryResult |

앱 요청은 `{op, params}`, 응답은 `{op, result}` 닫힌 union이다.
`operation.get`은 없다. `task.operation`은 kind(decision/message), id,
body_fingerprint를 필수로 받는다. 같은 인증 기기와 같은 Task·본문의
저장 결과만 반환하며 현재 Approve/Message 권한을 재검사한다(T1/P3/T3).
Cursor는 최대 512바이트 불투명 값이며 boot·권한 epoch 결합은 P2/T1 책임이다.

원본 바이트에 `parse_call`을 적용한다. Value로 먼저 변환하면 중복 키가
사라지므로 금지한다. 모든 객체는 unknown/중복 필드를 거부한다.
nullable choice 외 Option 필드는 생략/null을 허용한다. 요청 32 KiB를 파싱
전에 검사하고 `response_bytes`는 64 KiB가 넘기 전에 직렬화를 중단한다.
Text/List는 역직렬화 중 UTF-8 바이트/원소 예산을 검사한다.
ID 128바이트, operation ID 64바이트 ASCII 영숫자/밑줄/하이픈,
Task 페이지 20개, wait_ms 0–25000, 메시지 1000자/4096바이트/20줄,
코멘트 ID 20개다. 메시지 control/bidi는 거부하고 LF만 허용한다.
T3는 코멘트 snapshot과 최종 prompt 16 KiB 예산을 재검사한다.
OutputResult는 최대 100줄이며 T1은 합계 16 KiB와 제어문자 제거도 집행한다.

| 고정 lifecycle DTO (앱 op 아님) | 필드 |
|---|---|
| Challenge | purpose(claim/finish/session), machine, boot, subject, auth_epoch, nonce, csrf, expires_ms |
| Claim → Claimed | pairing_id, secret, code, nonce, public_key, signature, name, scopes → ticket, expires_ms |
| Finish → Session | ticket, nonce, signature, 선택 wait_ms |
| SessionOpen → Session | device, auth_epoch, nonce, signature |
| Session | device, boot, auth_epoch, bearer, csrf, expires_ms, scopes |
| Revoke (로컬만) | device, auth_epoch |

256비트 secret/nonce/ticket/bearer/CSRF는 소문자 hex 64자,
P-256 SPKI/DER 서명은 base64(각 최대 256/192바이트), 코드는 8자리다.
현재 DTO는 길이 예산을 강제하고 P2가 정확한 인코딩·서명·nonce 소비·
코드·수명·교집합을 검사한다. Scope는 task와 최대 네 caps, 한 요청 최대
20개 Task다. secret-bearing lifecycle 객체는 Debug를 구현하지 않는다.
인증 lifecycle 원본도 HTTP의 32 KiB 예산과 직접 serde 파싱을 적용해야 한다.

오류 객체는 `{code, retry_after_ms}`다. 입력이나 진단 문자열을 반사하지 않는다.
429는 HTTP Retry-After도 설정한다(T1).

| code | 예약 RPC 번호 | HTTP |
|---|---|---|
| invalid_request | -32130 | 400 |
| unauthenticated | -32131 | 401 |
| denied | -32132 | 403 |
| conflict | -32133 | 409 |
| stale | -32134 | 409 |
| too_large | -32135 | 413 |
| rate_limited | -32136 | 429 |
| unavailable | -32137 | 503 |
| storage | -32138 | 503 |
| unknown_operation | -32139 | 404 |

Gate 전용 projection에는 machine/boot/task/gate/revision, issued_ms,
expires_ms, 서버 origin(unknown/local_user/mod/orch), title/body/options/state,
complete와 body_fingerprint가 있다. `GateMetadata`는 기존 GateInfo 옆에
저장할 revision/origin 계약이다(P3). 구형 레코드는 revision 0/unknown으로
복원하여 complete=false다. 전체 상세가 필드/64 KiB 예산을 넘으면
`project_gate`는 too_large를 반환한다. T1은 불완전 상세 상태를 표시하고
읽음 기록/승인을 생성하지 않는다. 원문을 조용히 잘라 승인하지 않는다.

Fingerprint는 SHA-256(podd의 `sha2`, M1 메시지와 같음)의 소문자 hex다. 입력은 공백 없는 UTF-8 JSON이며
scope는 `["pod-mobile/1/scope", {정렬된 task: 정렬·중복제거 caps}]`,
decision은 `["pod-mobile/1/decision", machine, task, gate, revision, decision, choice]`다.
operation_id는 멱등 키여서 decision 본문 해시에 포함하지 않는다.
Gate 해시는 `gate_preimage`의 고정 순서로 boot·전체 원문·출처·상태도 결합한다.
`fixtures/mobile-m2-wire.json`은 독립 계산한 preimage/SHA-256 벡터다.

| 접합부 | 담당 / 현재 상태 |
|---|---|
| pairing::Pairing | P2: challenge/claim/finish, 현재 unavailable |
| auth::Auth, AuthenticatedDevice | P2: bearer+CSRF 인증·요청별 recheck·open/revoke. 신원 필드는 private, Deserialize/공개 생성자 없음 |
| Daemon::mobile_call | T1: 인증 context + Call → Reply. 현재 unavailable, M1 local_call로 fallback 없음 |
| approval::ApprovalBoundary, GateMetadata, project_gate | P3: 동기 결정·영속 메타데이터. 미구현 decide는 unavailable |
| session::Clock | 기존 시계 인터페이스 재사용. 두 락 획득 후와 commit 직전 새 시각 읽기 |
| listener::Listener | T1: enable/call은 unavailable, is_enabled=false, socket 없음 |

P3 경계는 mobile state → orch store 순서로 락을 잡고, 두 락 안에서
시계·인증·현재 범위를 재검사한다. replay 조회 후 신규 전이에만 CAS/읽음
기록을 검사하고 전이·revision·operation·감사를 단일 저장한다. 락 안에서
await/network/terminal I/O는 금지한다. 이 F1의 trait는 그 호출 계약이며
원자 저장·시계 경합 구현/검증을 완료했다는 뜻이 아니다.

서명 원문은 `claim_preimage`/`finish_preimage`/`session_preimage`가 고정한다.
셋 다 domain 문자열과 서버 Challenge 전체(purpose/machine/boot/subject/
auth_epoch/nonce/CSRF/만료)를 결합하고, claim은 pairing_id·secret·코드·
nonce·공개키·이름, finish는 ticket/nonce, session은 device/epoch/nonce를 더한다.
finish의 long-poll `wait_ms`는 서명하지 않는다. claim의 scope는 `scope_preimage`
바이트를 그대로 넣는다. fixture에는 현재 session 원문/SHA-256 벡터만 있으며
claim/finish 벡터와 실제 서명 검증·nonce 단일 소비는 P2가 추가한다.

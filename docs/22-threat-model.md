# 원격·모바일·에이전트 위협 모델

평가일: 2026-11-12 · 기준: master `2b8d5498c2289976580c6dfe98a910b07a8b69cb` · 설계/평가만, 제품 코드 변경 없음.

[17 원격](17-remote.md) §0의 결정과 단계 표, [19 오케스트레이션](19-worktree-sessions-orchestration.md), [기존 보안 평가](review/2026-11-10/security.md), [재평가](review/2026-11-10/rescore-daemon-security.md)를 현재 [21 구조](21-extending.md)에 대조했다. R4 병행 작업의 미완료 변경은 평가하지 않았다. **아래 22건은 출시 전 위협·요구사항이며 22개의 재현된 취약점을 뜻하지 않는다.**

## 1. 자산·공격자·신뢰 경계

| 자산 | 보호해야 할 성질 | 경계를 넘는 데이터/공격자 |
|---|---|---|
| 소스·diff·터미널 출력·미저장 버퍼·자격증명 | 기밀성, 올바른 머신/Task의 내용, 유실 방지 | 악성 저장소/에이전트/SSH 서버 → desktop; podd → 허용된 폰 |
| Gate·merge·명령 실행·워커 커밋 | 사용자 의도, 대상·내용·시점의 일치, 한 번만 반영 | 워커 보고/메일/브리프/터미널 → 코디네이터; 폰 → 제한 API → podd |
| SSH 프로필·known_hosts·TLS/Noise 키·QR·세션·route credential | 신원, 최소 권한, 철회, 비밀 비노출 | LAN MITM/악성 AP/웹사이트, QR 사진 보유자, 악성 relay, 분실 폰 |
| 작업·기기·epoch·operation·감사 저장소 | 재시작 후 동일한 권한과 결과, 실패 은폐 금지 | 네트워크 재전송/동시 요청, 디스크 실패; 같은 사용자 프로세스의 조작은 별도 한계 |
| daemon/holder·relay 메모리·CPU·네트워크 비용 | 유한 자원, 제어 경로 응답 | 느린 수신자, 인증 전 flood, 인증된 악성 기기/원격 서버 |

경계는 **① 로컬 OS 사용자 → pipe/UDS, ② SSH 계정 → 전체 RPC, ③ 모바일 인증 채널 → 닫힌 여섯 메서드, ④ 비신뢰 텍스트 → 에이전트의 도구 권한, ⑤ APK WebView → native 키/네트워크, ⑥ 암호화 종단 → 사용자 운영 relay**로 나눈다. 워크트리·머신 별칭·Gate·attempt 토큰은 OS 격리 경계가 아니다.

**확인된 기준:** M1은 listener 없는 정책 계층(`crates/podd/src/server/mobile/mod.rs:1`), 세션 ID는 비밀이 아닌 `s<boot>-<counter>`(`session.rs:7`, `:108`, 이 문단의 경로는 같은 mobile 디렉터리)다. 모바일 이름은 정확히 `tasks`, `task`, `output`, `gate.decide`, `message`, `delivery`(`crates/pod-protocol/src/mobile_protocol.rs:234`); 네트워크 어댑터는 `mobile_call`만 호출해야 한다(`crates/podd/src/server/mobile/api.rs:137`). pipe의 mobile 관리 RPC와 시험용 `mobile.test.*`는 폰 API가 아니다.

**가정/미검증:** R4–R6·M2–M6의 완료와 실제 SSH/Android/인터넷 동작은 가정하지 않는다. 신뢰된 PC·APK 설치/서명 경로와 정상 OS RNG를 전제로 하며, 관리자·종단 악성코드·동일 사용자 전체 권한 장악을 TLS/Noise로 격리한다고 주장하지 않는다. 악성 저장소가 모든 Git 설정을 바꿀 수 있다는 가정도 하지 않는다. 아래 공격 경로는 필요한 권한/조건을 적은 설계 분석이며 런타임 공격 성공의 증거와 구분한다.

심각도는 통제가 빠진 상태로 해당 기능을 출시했을 때의 영향이다. **높음**은 비밀/승인/실행 권한 침해, **중간**은 제한된 유출·가용성·감사 손실이다. 각 항목의 **사실**은 현 코드 또는 문서의 계약이고, **완료 시험**은 앞으로 해당 카드가 제출해야 할 증거다(이번에 실행한 시험이 아님).

## 2. 모바일 — M2부터 M6까지

### TM-01 · 높음 · M2 — LAN을 신뢰하거나 TLS 핀을 늦게 검사

- **사실:** 선택한 LAN IP 하나·기본 off·TLS 1.3·QR SPKI 핀·HTTP fallback 금지가 계약이다(`docs/17-remote.md:108`, `:110`). 구현된 네트워크 인증은 아직 아니다.
- **공격:** 악성 AP/MITM·DNS rebinding이 페어링 secret/토큰을 받거나 다른 PC에 승인을 보낸다. LAN 주소라는 이유로 인증을 생략하면 subnet의 다른 사용자도 공격자다.
- **필수 완화:** 첫 인증 데이터 전 native 핀/키 소유/기간/endpoint 검사; 구체 IPv4/IPv6 bind·peer/subnet·Host 허용 목록, IP 변경 시 닫기. 인증서 오류 예외·자동 방화벽/UPnP·원격 Gate의 묵시적 LAN 재노출 금지.
- **완료 시험:** loopback TLS double에서 위조/만료/다른 endpoint·핀·DNS/Host·IPv4-mapped IPv6 우회 거부, off에서 listener 0, 실패 연결에 secret 전송 0. 실제 LAN 확인은 M3에서 별도 수행.

### TM-02 · 높음 · M2 — QR 경쟁·시험 세션의 인증 토큰 오용

- **사실:** 실제 QR/토큰은 후속이고 M1의 ID는 예측 가능한 식별자다(`crates/podd/src/server/mobile/session.rs:108`). 등록 stub은 debug/test 경로다(`crates/podd/src/server/mobile/mod.rs:279`, `:333`). QR 2분·별도 8자리 코드·PC 최종 승인은 설계 계약(`docs/17-remote.md:114`).
- **공격:** QR 사진 탈취/코드 추측/동시 소비, URL·로그·JS에서 세션 탈취, polling으로 영구 로그인 유지.
- **필수 완화:** CSPRNG 256비트 일회 secret와 불투명 세션 토큰, 서버 해시 저장·원자 소비; 2분/5회 실패 폐기·IP 5회/분·전체 20회/분, 기기 키/이름/Task별 범위의 PC 확인. 토큰은 native 보안 저장소만; 8시간/유휴 15분, background는 유휴 갱신 불가. off/restart는 세션·미소비 pairing 폐기. release 네트워크에서 stub 접근 불가.
- **완료 시험:** 같은 QR의 병렬 등록은 한 건, 범위 확대/만료/재사용 거부; counter ID만으로 인증 불가; 로그/URL/JS/backup에 secret 없음; fake clock의 절대·유휴 만료와 off/restart 회귀.

### TM-03 · 높음 · M2, M3 — 출력 XSS·CSRF·WebView bridge 탈출

- **사실:** 출력 정리는 제어문자 제거와 100줄/16 KiB 제한이며 HTML escaping은 아니다(`crates/podd/src/server/mobile/api.rs:97`). APK 로컬 자산·좁은 main-frame bridge·native 최종 확인은 설계다(`docs/17-remote.md:190`).
- **공격:** 저장소/에이전트가 Gate 제목·출력·리뷰에 HTML/가짜 버튼을 넣어 승인 유도; iframe/외부 페이지가 native bridge를 호출하거나 웹사이트가 LAN API에 요청한다.
- **필수 완화:** 모든 DTO를 text로 렌더, 번들 자산만 허용하는 CSP; 외부 navigation/iframe/file/content URL/download/mixed content/debugging 차단. bridge의 정확한 origin·main-frame·intent schema 확인, 범용 fetch/RPC 금지. 변경은 POST+CSRF 값·Origin 검증(임의 null Origin 허용 금지), CORS off; 키/토큰은 JS에 전달하지 않는다. native 확인은 화면의 machine/Gate/digest와 결합한다.
- **완료 시험:** 모든 문자열 필드에 HTML/URL/이스케이프 payload, 잘못된 origin/서브프레임/CSRF 누락을 넣어 실행·네트워크·승인 0 확인. desktop double에 더해 M3 실기기의 bridge/navigation 검증 필요.

### TM-04 · 높음 · M2, M3 — license gate를 이유로 검증되지 않은 암호/자산 채택

- **사실:** 신규 의존성은 Apache-2.0/MIT/ISC만 허용하고 기존 deny 통과만으로 부족하다(`docs/17-remote.md:218`, `:232`, `:234`). 서명 APK·상향 versionCode·로컬 자산이 배포 계약이다(`:190`, `:196`). 이는 유료 기능 라이선스 검사가 아니라 **의존성 라이선스 출시 gate**다.
- **공격:** 허용 closure를 확보하지 못해 암호 검증을 끄거나 자체 구현/원격 JS로 우회; 변조 APK·업데이트가 키와 승인을 탈취한다.
- **필수 완화:** target/feature별 고정 lock과 전이 C/assembly/JNI/Android 자산까지 SBOM·원문·해시 검토; 불충족이면 출시 차단, 암호 downgrade 금지. 서명 키는 relay와 분리, release debug 서명 금지, 신뢰된 fingerprint와 업데이트 계보 확인.
- **완료 시험:** 금지 라이선스/누락 고지를 넣은 합성 closure는 CI 실패; APK 자산 해시·외부 코드 요청 0; 다른 서명/낮은 버전/변조 APK 거부. 빌드 도구 사용 조건과 번들 runtime을 따로 기록한다.

### TM-05 · 높음 · M2 — 제한 API가 전체 RPC·과도한 DTO로 확장

- **사실:** exact allowlist(`crates/podd/src/server/mobile/api.rs:48`)와 Task별 기본 거부(`crates/podd/src/server/mobile/registry.rs:214`), 전용 DTO(`crates/pod-protocol/src/mobile_protocol.rs:264`)가 있다. 출력 opt-in도 비밀 내용의 완전한 제거를 보장하지 않는다.
- **공격:** 메서드 prefix/중첩 JSON-RPC·다른 Task/delivery ID를 통해 파일·shell·merge·타 기기 결과에 접근하거나, 거대한 Gate/출력으로 폰/daemon을 소진한다.
- **필수 완화:** 네트워크 dispatcher를 전체 `server/rpc`와 분리하고 여섯 op만 매핑; Task·기기·delivery 소유권을 매번 검증. `terminal.send/input/keys`, shell, 파일, PR merge, `group.pick`, `mod.*`, `server.stop`, 기기 관리/stub 노출 금지. 요청 32 KiB·응답 64 KiB를 파싱/직렬화 전에 예산화하고 pagination, 읽기 60·결정 10·메시지 5회/분, 기기 3개를 유지한다. 출력·prompt·환경·경로의 포괄 직렬화 금지.
- **완료 시험:** 공개 RPC 전체 이름+prefix 변형·batch·다른 Task/기기 ID의 거부 행렬, 초과 크기의 사전 거부·정상 세션 응답 유지. 기존 `api.rs:609`의 allowlist 시험을 네트워크 진입점까지 확장한다.

### TM-06 · 높음 · M2, M4 — 오래된 승인 화면·시각·감사와 실제 커밋의 불일치

- **사실:** M1 DTO에는 revision/challenge/digest/origin이 없다(`crates/pod-protocol/src/mobile_protocol.rs:246`, `:300`). scope·pending·expiry·operation과 저장 rollback은 있다(`crates/podd/src/server/gates.rs:68`). 그러나 요청 시작의 `now`(`mobile/api.rs:168`)를 lock 획득 후에도 그대로 사용한다(`mobile/api.rs:413`, `:417`; 모두 `crates/podd/src/server/` 기준). **대기 중 만료를 새 시각으로 검사하지 않는 정적 공백**이며 실제 경합 재현은 하지 않았다.
- **공격:** lock 대기/절전·지연 뒤 만료 승인, 다른 revision/선택지에 승인 적용, 에이전트가 제목의 출처를 사칭, 감사 저장 실패를 성공처럼 표시.
- **필수 완화:** M2에서 서버 기록 origin/전체 원문·machine/task/gate/revision·서버 만료를 표시하고 stale/잘린 화면 승인 금지. 최종 lock 경계에서 신뢰된 시계를 새로 읽어 session/권한/expiry/CAS 재검사; 성공 전이와 operation/감사 원자 저장·실패 rollback. M4에서 challenge/digest 결합 추가. operation 조회는 같은 기기의 같은 본문 결과만 제공하고 여섯 API 계약에 명시적으로 통합한다(임의 프록시나 무심코 일곱 번째 op 추가 금지).
- **완료 시험:** fake clock+barrier로 요청 후 lock 대기 중 Gate/세션 만료를 넘겨 성공 0; PC/폰 동시 결정 한 건; revision 변경/원문 잘림 거부; 디스크 실패/restart/응답 유실에도 성공 기록 한 건. 30일 보존과 크기 상한 충돌은 명시 정책으로 해결하고, 이미 끝난 Gate를 기록 정리 뒤 다시 열지 않는다.

### TM-07 · 높음 · M4 — Noise 상호 신원·프로파일·키 수명 혼동

- **사실:** `Noise_XX_P256_ChaChaPoly_SHA256`는 검토 전 후보이며, QR v1과 transcript 계약도 설계다(`docs/17-remote.md:178`–`:181`). 구현/상호운용/독립 보안 검토 완료를 뜻하지 않는다.
- **공격:** 악성 relay가 종단 키/역할/머신/프로파일을 바꾸거나 downgrade·nonce 재사용으로 내용을 읽고 위조한다. relay TLS pin은 podd 신원 키 인증을 대신하지 못한다.
- **필수 완화:** 검증된 구현·양 종단 동일 벡터, QR의 machine/public key/key epoch 정확 대조; prologue의 버전/역할/머신/epoch와 프로파일 transcript 고정. 양쪽 등록 static key 비교, 세 메시지 완료 전 DTO·0-RTT 금지. 매 연결 새 ephemeral, 방향별 nonce 재사용 금지; 30분/방향당 2^20 프레임 전에 새 handshake, 키 폐기. QR에는 비밀키를 넣지 않는다.
- **완료 시험:** 알려진 벡터+반대 역할/키/epoch/프로파일/MITM/역순·중복·손상 tag 실패; nonce 한계와 rekey·폐기 경로 검토. 독립 암호 설계 리뷰 없이는 M4 완료 불가.

### TM-08 · 높음 · M4, M6 — 철회·restart·응답 유실 이후 승인 replay

- **사실:** channel/boot/device/auth_epoch/seq와 최대 60초 challenge, 영속 operation/CAS 계약(`docs/17-remote.md:182`–`:184`)은 M1의 세션 epoch 검사보다 강하다(`crates/podd/src/server/mobile/session.rs:124`).
- **공격:** relay가 철회 통지를 숨기거나 이전 암호문/동일 요청을 새 연결·다른 기기에 재생하고 오래된 offline 승인을 전달한다.
- **필수 완화:** envelope의 모든 식별자 검증; challenge를 기기·세션·revision·전체 본문 digest·만료에 결합하고 전이와 함께 원자 소비. 영속 auth_epoch 증가 후 연결 종료, commit 직전 로컬 저장소 재검사. off는 PC에서만 재개, restart는 새 세션/키; 승인 자동 재전송/offline 큐 금지, 결과 불명은 조회만.
- **완료 시험:** 철회 통지 전부 드롭·디스크 실패·재시작·boot 교체·두 기기·동일 op/다른 본문·60초 경계 행렬에서 추가 전이 0. 승인 후 응답 유실은 원래 결과만 반환; M6 실제 절전/네트워크 전환에서도 확인.

### TM-09 · 중간 · M5, M6 — 사용자 relay의 route credential·메타데이터

- **사실:** 사용자가 운영하는 relay는 rendezvous만 제공하며 IP·연결 관계·시각·크기·빈도를 본다(`docs/17-remote.md:159`, `:172`). E2EE도 이를 숨기지 않는다.
- **공격:** 유출 route credential로 접속 방해/트래픽 소모·기기 활동 추적; query/access 로그·백업에 credential이나 본문 보관. 사용자가 운영한다는 사실만으로 침해 가능성이 사라지지 않는다.
- **필수 완화:** 256비트 route와 별도 기기별 credential, pairing 창 전용 초회 credential·개별 회전/철회; QR secret/종단 키와 분리. 임의 URL/TCP 프록시·키 저장·viewer 코드 배포 금지. 본문/credential/query 로그 금지, 집계 기본·IP 진단 opt-in 24시간; 운영자/호스팅 계층의 메타데이터 접근·보존 책임 명시.
- **완료 시험:** credential만으로 mobile DTO/승인 불가, 한 기기 철회가 다른 기기를 끊지 않음; 합성 canary로 정상/오류/프록시 로그·crash dump 정책 점검. 로그가 없다는 시험도 트래픽 상관분석 방어 증거로 쓰지 않는다.

### TM-10 · 중간 · M5 — relay flood·분할 복원·지연/중복 전송

- **사실:** handshake 10초, frame 32 KiB·queue 256 KiB·TTL 30초, route/전역 예산이 계약이다(`docs/17-remote.md:174`); 오프라인 저장소가 아니다(`:183`).
- **공격:** 인증 전 handshake·많은 route·거짓 fragment 총길이·느린 수신자로 메모리/egress 비용 소진, 끊기기 직전 큐의 승인 암호문 재생.
- **필수 완화:** 인증 전 IP 10회/분·동시 5개, route 기기 3/연결 4·256 KiB/s와 burst 512 KiB, 전역 연결/메모리/egress·초대 예산·kill switch. 할당 전 길이/조각 수·복원 합계·TTL 검증, 단절 시 양방향 큐 폐기, podd 복호화 후 별도 API 한도. 암호문 무결성/replay 판단은 종단 책임이다.
- **완료 시험:** 손상/중복/역순/지연/누락 조각, 다중 route와 slow reader 폭주에서 정한 메모리/비용 상한과 timeout 유지; 단절 재연결 후 옛 승인 반영 0. loopback 결과를 실제 인터넷 DoS 방어로 주장하지 않는다.

### TM-11 · 높음 · M3, M6 — 도난·잠금 해제된 폰의 실제 권한

- **사실:** Status/Approve/Output/Message는 Task별 범위이고(`crates/podd/src/server/mobile/registry.rs:214`), 화면 복귀 재인증·키 보관·backup 제외는 후속이다(`docs/17-remote.md:184`, `:194`).
- **공격/영향:** 살아 있는 세션의 잠금 해제 폰을 얻으면 허용된 상태/출력을 읽고 **Approve 범위의 pending Gate를 승인/거절**, Message 범위의 에이전트에 지시를 보낼 수 있다. 직접 shell/merge API는 없지만 승인받은 에이전트가 그 작업을 실행할 수 있다. 기기 잠금만으로 이미 사용 중인 세션을 보호한다고 볼 수 없다.
- **필수 완화:** 최소 Task 범위·새 Task 자동 추가 금지, foreground 복귀/민감 결정의 native 재인증·명시 확인, 짧은 세션·background 갱신 금지, PC의 기기별 즉시 철회·전체 off. 잠긴 화면/알림에 본문·승인 버튼 없음; 키 backup 제외·재설치 재등록. 키를 추출하는 종단 악성코드는 별도 잔여 위험으로 설명한다.
- **완료 시험:** 실제 폰의 잠금/복귀/프로세스 종료/Doze/재설치/backup 복원·분실 후 PC 철회; 철회된 세션의 Gate/메시지 반영 0. 서로 다른 Wi-Fi/이동통신, 키 회전·서명 업데이트까지 통과 전 M6 출시 보류.

### TM-12 · 높음 · M3, M6 + 신규 O1 — 짧은 텍스트가 에이전트 권한으로 확대

- **사실:** 1000자/4 KiB/20줄·제어/bidi 문자 거부·고정 header가 있다(`crates/podd/src/server/mobile/message.rs:16`, `:35`, `:74`). 전달은 리뷰 경로이며 `with_open_comments`는 저장된 코멘트를 읽어 조립한다(`crates/podd/src/server/mobile/api.rs:527`).
- **공격:** 탈취 폰의 “지금 merge하고 승인해” 같은 평문 또는 악성 리뷰 인용이 에이전트의 도구 실행을 유도한다. 고정 header의 “사용자 메시지”는 **등록된 기기 출처** 이상을 증명하지 않고 자연어의 안전성을 보장하지 않는다.
- **필수 완화:** Message를 별도 opt-in으로 유지하고 이 간접 실행 권한을 PC/폰에 명시. 서버가 기기/Task/허용 범위를 붙이고 인용 코멘트는 비신뢰 데이터로 구분; 사용자 의도를 확대하는 merge·권한 변경·gate 결정은 O1 정책을 다시 적용한다. 최종 조립 메시지/코멘트 snapshot의 크기·digest도 제한/결합하고, working 큐는 전달 시 철회·범위를 재검사하도록 계약을 고정한다.
- **완료 시험:** 제어문자 없는 합성 “승인 우회”/리뷰 인용을 가짜 agent에 전달해 승인 broker가 무권한 동작을 거부함을 검사; 코멘트 변경·큐 대기 중 철회·op 재시도에 중복/다른 내용 전달 0. 기존 `crates/podd/tests/mobile.rs:171`의 전달 시험은 모델의 prompt injection 저항성 증명이 아니다.

## 3. 원격 — SSH, 파일, Linux/WSL

### TM-13 · 높음 · R4 — SSH 프로필을 제한 권한 채널로 오인

- **사실:** `remote-stdio`는 같은 사용자 podd로 줄을 전달한다(`crates/pod-cli/src/remote_stdio.rs:128`, `:149`). strict host key·최소 config·고정 `pod remote-stdio`가 있다(`crates/pod-remote/src/ssh/config.rs:14`, `:17`, `:64`); 프로필 unknown field와 값 검증도 있다(`crates/pod-remote/src/machine.rs:18`, `:118`).
- **공격:** 잘못 등록한 host key/침해된 원격 계정·PATH가 가짜 Pod를 실행하거나, 수정된 프로필/같은 별칭으로 다른 머신 작업에 명령을 보낸다. SSH 계정 탈취는 **전체 same-user RPC·프로세스 실행 권한** 탈취이지 모바일 여섯 메서드로 제한되지 않는다.
- **필수 완화:** UI에 전체 계정 권한 표시; host fingerprint를 별도 신뢰 경로에서 확인, 자동 accept/known_hosts 교체 금지. 프로필은 로컬 신뢰 설정, 임의 SSH 옵션/ProxyCommand/remote command 수입 금지; 절대 시스템 ssh·최소 `-F`와 argv·원격 신뢰된 Pod 설치 유지. machine_id+resource_id 구조체를 사용하고 별칭/host key/머신 변경은 재확인한다. 이 브리지를 relay/모바일 endpoint로 재사용하지 않는다.
- **완료 시험:** 악성 host/user/path/`%`/개행·unknown 옵션 거부, host key 변경·다른 machine_id·동명 Task 선택 시 변경 RPC 0. fake ssh 시험과 실제 이미 구성된 서버의 로그인 증거를 구분한다.

### TM-14 · 높음 · R4 — 재접속의 잘못된 대상·입력 중복·출력 자원 소진

- **사실:** hello/machine/boot·8 MiB 줄·32 KiB chunk·2 MiB 큐·16 MiB snapshot·변경 자동 재전송 금지는 계약이다(`docs/17-remote.md:69`, `:71`, `:75`). 현 브리지 framing 검사는 제한된 전송 증거이지 R4 완료가 아니다(`crates/pod-cli/src/remote_stdio.rs:72`, `:128`).
- **공격:** 악성/느린 서버가 과대 프레임·stdout 배너·snapshot 조각으로 고갈시키거나, 응답 유실 후 key/명령이 재전송돼 두 번 실행된다. boot 변경 뒤 옛 seq와 현재 화면을 합치면 잘못된 상태로 승인한다.
- **필수 완화:** 할당 전 바이트/조각/큐 상한, stdout 오염 즉시 중단; attach 최대 4·제어 RPC 독립, lagged/gap 명시. 재접속 시 machine/boot→전체 상태→attach snapshot 순서, 끊김 중 입력 비활성화·마지막 수신 표시. 변경/키 자동 replay 금지, 결과 불명은 조회로 해결한다.
- **완료 시험:** 20/100/300 ms RTT·throttle·4개 flood·60초 단절·boot 교체·유실 ACK 주입; 메모리 상한, snapshot/live 경계, 중복 입력 0·실제 소비 바이트/해시 검증(수락 횟수만 세지 않음), 제어 p95 목표 확인.

### TM-15 · 높음 · R5 — 경로 탈출·reparse/ADS·검사 후 교체

- **사실:** R5는 Task 상대 경로·component별 no-follow handle·ADS/장치 경로/링크 거부를 요구한다(`docs/17-remote.md:83`–`:86`). 현 worker commit의 `symlink_metadata` 검사를 R5의 안전한 파일 I/O 구현으로 재사용할 수는 없다(`crates/podd/src/git/worker_commit.rs:54`).
- **공격:** 악성 repo/원격 peer가 `..`, 절대/UNC/장치 경로, `C:foo`, ADS, 8.3 alias·대소문자·trailing dot/space, junction/symlink 교체로 Task 밖 비밀을 읽거나 덮어쓴다. hardlink도 no-follow만으로 격리되지 않는다.
- **필수 완화:** endpoint OS별 정규화·상대 경로 문법, 고정 Task root handle, 모든 component no-follow 및 같은 handle로 읽기/쓰기. Windows reparse tag 전부 정책화·ADS/예약 장치 거부, 최종 객체/volume 확인; 외부 inode 공유 hardlink는 v1 거부 등 명시 정책. Task root 삭제/교체·허용하지 않은 `.git`/상태/키 경로도 차단한다. 클라이언트가 원격 경로를 로컬 파일로 열지 않는다.
- **완료 시험:** Windows 실제 junction/reparse/ADS/8.3와 Linux symlink/hardlink fixture, 검사 직후 디렉터리 교체 barrier에서 외부 canary 읽기/쓰기 0. 문자열 fixture만 통과하면 OS별 완료로 보지 않는다.

### TM-16 · 높음 · R5 — diff 프로그램 실행·stale 저장·crash 중복

- **사실:** `review.diff`의 no-ext-diff/no-textconv, snapshot/revision, `fs.write`의 expected_revision/operation journal이 설계다(`docs/17-remote.md:85`, `:87`, `:91`). 현재 worker Git filter 차단은 별도 경로다(`crates/podd/src/git/command.rs:111`).
- **공격:** repo의 diff/textconv/filter/hook 설정으로 podd 권한 프로그램 실행, 다른 revision에 리뷰/저장 적용, 파일 교체와 결과 기록 사이 crash 후 중복·유실; 원격 소스의 묵시적 로컬 캐시 유출.
- **필수 완화:** 원격 Git도 고정 argv·깨끗한 환경·guarded/bounded 실행, ext-diff/textconv·필터/훅/fsmonitor 등 repo 실행 경로 차단. snapshot/revision 앵커·4 MiB 파일 상한, CAS+임시 파일 원자 교체+준비 journal/hash 복구, 복구 중 쓰기 금지. operation을 task/path/body hash에 결합하고 보관 종료 뒤 자동 재시도 금지; 영구 캐시는 opt-in.
- **완료 시험:** 합성 실행 marker가 ext-diff/textconv/clean/process 경로에서 0(활성 대조 포함); 동시 수정·stale hunk 거부; rename 전후/결과 기록 전 crash 지점마다 재시작 시 한 결과, CRLF/BOM·미저장 버퍼 보존.

### TM-17 · 높음 · R6a — Linux UDS·PTY·프로세스 수명 경계 누락

- **사실:** 현재 Windows pipe는 current-user DACL·remote client 거부를 쓴다(`crates/podd/src/server/runtime.rs:30`). Linux 0700/0600 UDS·setsid·별도 holder와 실 Linux 검증은 후속이다(`docs/17-remote.md:62`, `:247`).
- **공격:** 다른 로컬 사용자가 공용 runtime 경로/socket을 선점·연결하거나 symlink state에 키를 쓰게 함; 단절/kill 뒤 자손이 남거나 재attach가 다른 사용자의 holder로 연결된다.
- **필수 완화:** 소유 UID·0700 runtime/0600 UDS·peer credential 검사, 안전한 bind/lock/stale socket 삭제와 no-follow state, root/sudo 없는 사용자 서비스. stdio/FD 상속 최소화·setsid·PTY/process group 종료·holder 신원/버전 확인; Windows 경로/환경 가드를 Linux에 맞게 이식한다.
- **완료 시험:** 실 Linux의 다른 UID 접속 거부·socket/state symlink 선점·권한 느슨한 runtime 거부, daemon kill 뒤 holder 재attach, 즉시 자손·느린 reader·종료 경합에서 잔존 프로세스 없음. Windows fake backend만 있으면 Linux 출시 보류.

### TM-18 · 높음 · R6b — WSL 배포판·경로·자격증명의 교차

- **사실:** Windows 소유 WSL shell과 Linux podd 모드를 구분하며 선택 distro의 `--exec`·`wslpath`, Windows POD_* 제거가 계약이다(`docs/17-remote.md:95`–`:102`).
- **공격:** default distro/지역화 결과를 잘못 해석해 다른 배포판에 명령을 보내거나, Windows pipe/attempt/hook 환경·토큰을 Linux로 전달하고 잘못 변환한 경로를 편집한다.
- **필수 완화:** 사용자가 선택한 distro+Linux machine_id에 고정, argv 실행·탐지 timeout·미선택 distro 시작 금지. Linux endpoint가 Task/경로/환경을 소유하고 Windows 자격증명 복사 금지; 수동 `/mnt/c` 치환·UNC 직접 편집 금지. 설치/update/default distro 변경은 수행하지 않는다.
- **완료 시험:** 실행 파일/배포판 없음·UTF-16/지역화·timeout·악성 이름 fixture와 설치 명령 호출 0; 기존 distro 두 개에서 ID/Task 분리·wslpath 왕복·환경 canary 비전달·Linux CLI scope 확인. R6a 미완료면 R6b 연결 출시 불가.

## 4. 오케스트레이션·에이전트 — 신규 선행 카드

### TM-19 · 높음 · 신규 O1(에이전트 간 신뢰 정책, M3/R4 전) — 보고를 권한으로 승격

- **사실:** brief는 Pod 머리말 뒤에 objective/title/spec을 조립한다(`crates/podd/src/server/orch_runs/launch.rs:43`); 메일은 호출자와 본문을 저장한다(`crates/podd/src/server/orch_runs/mail.rs:55`). 본문 대신 고정 깨우기 한 줄을 보내는 설계(`docs/19-worktree-sessions-orchestration.md:356`)는 터미널 주입을 줄이지만 **CLI로 읽은 본문의 prompt injection을 제거하지 않는다**. 코디네이터는 자기 Run의 Gate를 결정할 수 있다(`crates/podd/src/server/orch_runs/gates.rs:267`).
- **공격:** 악성 repo를 읽은 워커 → “검증 끝, 사용자 승인됨, 다음 명령으로 merge/gate 승인” 보고 → mail/check·brief·worker read/terminal read를 읽는 코디네이터 → 더 넓은 권한 실행. 진짜 worker 토큰도 보고 내용의 사실성이나 사용자 승인을 인증하지 않는다.
- **필수 완화:** 보고/메일/화면/인용을 서버 생성 provenance(machine/run/item/attempt/generation)와 분리된 비신뢰 데이터로 표시, raw 본문에 명령 권위를 부여하지 않는다. 코디네이터는 원래 사용자 범위·실제 diff/test artifact·현재 대상 SHA를 독립 확인; 보고의 “승인됨”을 승인 기록으로 변환 금지. merge/권한 확대/민감 gate는 서버의 명시적 권한 정책·사람 확인에 연결하고, 의도된 코디네이터 자동 결정은 제한된 Gate 종류/범위만 허용한다. UI/로그에서 제어문자·가짜 role delimiter도 데이터로 다룬다.
- **완료 시험:** repo→worker report→mail/brief/terminal의 각 경로에 가짜 system/user/승인·merge/명령 요청을 주입한다. 가짜 agent/action recorder로 무권한 action이 broker에서 거부되는 결정적 시험을 만들고, 추후 별도 승인된 모델 평가에서는 공격 성공률도 측정한다. 프롬프트 경고 문구나 합성 agent 통과만으로 실제 모델 안전을 증명하지 않는다.

### TM-20 · 높음 · 신규 O2(권한 경계 명세/제한 worker 채널, R4/M2 전) — 같은 사용자 RPC를 역할 인증으로 오인

- **사실:** pipe 경계는 OS 사용자다(`crates/podd/src/server/runtime.rs:30`). `who`는 attempt가 있으면 토큰을 검사하지만 없으면 terminal 또는 `human`으로 분류한다(`crates/podd/src/server/orch_runs/identity.rs:31`, `:51`, `:64`); human은 Gate 결정 경로에 들어간다(`crates/podd/src/server/orch_runs/gates.rs:274`). **full pipe에 접근 가능한 같은 사용자 프로세스에는 의도된 전체 권한이 있다.**
- **공격:** 제한 토큰/attempt fence를 전체 RPC ACL로 착각해 악성 same-user 프로세스/샌드박스 밖 워커에 raw pipe를 넘긴다. caller 필드 생략/변조로 역할 표시를 바꾸거나 일반 RPC로 작업·Gate·터미널을 조작할 수 있다. 이는 추가 원격 취약점의 재현 주장이 아니다.
- **필수 완화:** UI/문서/transport API에서 full-user와 mobile/worker 권한을 명시적으로 구분. worker를 보안상 격리할 필요가 있으면 별도 capability broker/endpoint에서 server-derived identity·task/run/메서드 allowlist를 집행하고 caller 생략·terminal override를 권한 승격에 쓰지 않는다. 같은 사용자 full pipe 자체를 막아야 하는 요구는 별도 OS 계정/샌드박스 경계가 필요하다. SSH bridge는 full-user로 유지하되 제한 채널에 연결하지 않는다.
- **완료 시험:** 격리 daemon에서 same-user full RPC 가능성을 경계 계약으로 확인; 제한 worker 채널에서는 caller 없음/타 terminal/타 Run·임의 gate/merge/terminal RPC 모두 거부. full endpoint 접근 가능한 프로세스를 격리했다고 표시하는 UI/문서가 없어야 한다.

### TM-21 · 높음 · 신규 O3(worker allowance·커밋 경계 회귀, R4/R5/R6a 전) — 좁은 허용의 실행 우회

- **사실:** Pod가 쓰는 rules는 좁은 `pod orch` 목록이고 git/worker read/gate decide는 없다(`crates/podd/src/server/codex_allowance.rs:55`). `orch.commit`은 토큰·기록된 Task worktree를 사용한다(`crates/podd/src/server/orch_runs/workers.rs:359`, `:400`); plumbing·filter/hook/fsmonitor/signing/maintenance 차단이 있다(`crates/podd/src/git/command.rs:111`, `worker_commit.rs:85`, 같은 git 디렉터리). worker 환경 guard는 시작 시 수집한 driver를 비운다(`crates/pod-client/src/env.rs:172`, `crates/podd/src/server/orch_runs/launch.rs:647`).
- **공격/가정:** 허용 명령 prefix의 실행 파일/cwd/env 변조, 기존 rules 경로의 링크, 중첩 repo·Git 외부 프로그램·검사 후 링크/설정 교체로 sandbox 밖 daemon 권한을 빌린다. 현재 경로 검사는 metadata와 이후 Git 열기가 분리돼 있다(`crates/podd/src/git/worker_commit.rs:54`, `:215`); **경쟁 가능 조건/실행 파일 바꿔치기 성공은 미재현 가설**이다. 과거 clean filter 결함을 현재 미해결로 재보고하지 않는다.
- **필수 완화:** Pod 통합의 허용 목록/실행 파일 해석을 고정·검증, 사용자 전역 agent 권한 확대 금지. rules·exclude·no-hooks 경로도 링크/소유권 검사, 보호 파일이 외부 파일이면 덮어쓰지 않는다. worker commit은 지정 root·fresh guard·bounded plumbing만 유지; 검증한 파일 handle의 내용을 안전한 snapshot으로 hash하거나 동등한 방법으로 검사/사용 경합을 닫는다. 시작 뒤 새 filter/config·환경 override는 별도 정책화한다. 커밋은 merge/검사 통과가 아니며 hook 생략·LFS 제한을 표시한다.
- **완료 시험:** 기존 `crates/podd/tests/worker_commit_filters.rs:95`, `:171`의 활성 marker 대조를 유지하고 clean/process/textconv/signing·중첩 repo·ADS/reparse·경쟁 교체·새 driver·rules 링크·이름 같은 가짜 실행 파일 행렬을 격리 fixture로 추가. 외부 marker 0, 잘못된 경로 거부, 일반 비허용 명령의 승인 경계 유지. 실제 모델 프롬프트는 이 평가에서 실행하지 않는다.

### TM-22 · 높음 · 신규 O4(hook 신뢰/출처, R4/M3 전) — Claude hook 상태를 실행 승인으로 오인

- **사실:** Pod hook relay는 env의 terminal/socket/token과 stdin payload를 전달한다(`crates/pod-hook/src/relay.rs:32`), daemon은 terminal 토큰을 확인해 상태에 반영한다(`crates/podd/src/server/status/ingest.rs:93`, `:137`). SQOS identification 수정이 있다(`crates/pod-hook/src/relay.rs:97`). 이것은 **Pod의 hook 연동 계약**이며 다른 앱 내부 보안 구조에 대한 주장이 아니다.
- **공격:** token을 가진 자식/같은 사용자 프로세스가 가짜 idle/blocked/Stop을 보내 코디네이터의 신뢰·깨우기/입력 시점을 오염시킨다. 악성 hook 설정/실행 경로 교체는 사용자 권한 코드 실행이며, blocked:permission을 사람의 승인으로 해석하면 권한이 확대된다.
- **필수 완화:** hook은 상태 증거만; Gate/도구 권한 승인이나 완료 보고의 대체로 쓰지 않는다. 토큰은 terminal/수명에 결합·출력/brief/로그 비노출, malformed/큰 payload·빠른 flood 제한. stale/unknown 상태에서 자동 입력/완료 판정 금지. hook 설치는 사용자가 확인한 절대 실행 파일·좁은 설정 변경만, 기존 설정 보존·내용 변경 시 재확인; SQOS와 timeout 유지.
- **완료 시험:** 틀린/이전 terminal token·큰/손상 payload·위조 Stop/PermissionRequest로 Gate 전이/자동 승인 0; busy/draft 상태 깨우기 금지. `crates/pod-hook/tests/cli.rs:538`의 가짜 pipe impersonation 회귀와 `crates/podd/tests/orch_runs.rs:1853`의 SessionStart/완료 구분을 유지한다. token 보유 same-user 공격까지 인증으로 막았다고 주장하지 않는다.

## 5. 카드별 완료 문턱과 평가 기록

| 카드 | 필수 항목 / 완료 문턱 |
|---|---|
| R4 | TM-13/14, O1/O2/O3/O4 선행. 실제 SSH 미검증은 별도 표시 |
| R5 | TM-15/16, O3 경합·Git 외부 실행 회귀. OS 파일 객체 시험 필요 |
| R6a / R6b | TM-17 / TM-18. 실제 Linux/선택 WSL 미검증이면 해당 지원 출시 보류 |
| M2 | TM-01–06, O2. 인증·license·API·승인 원자성 완료 전 listener 출시 금지 |
| M3 | TM-03/04/11/12, O1/O4. 실 Android LAN·키/bridge/잠금 검증 필요 |
| M4 | TM-06–08. 암호 벡터·challenge/epoch·독립 검토 필요 |
| M5 | TM-09/10. relay는 권한 판단/키 보관 주체가 아니며 예산·로그 계약 필수 |
| M6 | TM-08/09/11/12 및 선행 M2–M5. 실제 폰+사용자 준비 relay의 인터넷/운영 검증 필요 |

기존 리뷰의 해결 표시는 그 자체로 새 시험 성공이 아니다. 이번에는 현재 파일에서 M1 allowlist/실패 감사, hook SQOS, worker commit의 filter 차단을 정적으로 대조했다. 과거 리뷰의 `server/mod.rs`, `git.rs`, hook `main.rs` 등 줄 번호를 그대로 복사하지 않고 리팩터 후 위치를 사용했다. 프로토콜/DTO 추가는 `pod-protocol`, transport는 `pod-client`/`pod-remote`, 권한 판단은 `podd/server`에 둔다(`docs/21-extending.md:14`, `:35`); 모바일의 제한 dispatch를 일반 RPC 등록 표와 합치지 않는다.

검증: 문서·코드·시험 소스 읽기와 `git diff --check`, `tools/check-docs.ps1`만 수행했다. Cargo/daemon/실제 agent/SSH/Android/네트워크 쓰기·사용자 Pod/agent 설정 조작은 수행하지 않았다. 위 완료 시험은 후속 카드의 요구사항이며, 이 문서는 출시 인증이나 실제 prompt injection 성공률 측정 결과가 아니다.

# 모바일 M2 상세 설계 — LAN TLS·QR·로컬 화면

작성: 2026-11-12 · 평가 기준: master `c933101`, 프로토콜 v21 · **설계만, 구현·출시 완료 아님**.

[17 원격](17-remote.md) §0의 Android 얇은 래퍼·사용자 운영 relay·foreground long-poll 결정을 유지한다. M2는 LAN 기반과 desktop double까지, M3는 Android/LAN 출시, M4–M6는 Noise·relay·실제 인터넷 승인이다. M2 성공을 인터넷 요구 완료로 표시하지 않는다. [22 위협 모델](22-threat-model.md)의 **TM-01–06과 O2**가 M2 완료 문턱이다. rm-r4ui/rm-r5의 진행 중 코드와 v22는 이 평가에 포함하지 않았다.

## 1. 현재 구현과 확장 경계

| 확인한 코드 | M2에서 보존·보강할 계약 |
|---|---|
| `podd/src/server/mobile/api.rs::{route,mobile_call}` | 정확히 `tasks`, `task`, `output`, `gate.decide`, `message`, `delivery`. 관리 RPC·일반 RPC와 분리. `message`가 이미 여섯 개 중 하나다. |
| `mobile/{registry,session}.rs` | Task별 Status/Approve/Output/Message 기본 거부, 기기 3개, 세션 8시간/유휴 15분, 기기별 rate limit. `s<boot>-<counter>`는 식별자일 뿐 인증 비밀이 아니다. |
| `mobile/{mod,audit,message}.rs`, `pod-cli/src/mobile.rs` | 로컬 기기/정책/철회/감사 CLI, debug stub, 짧은 메시지·리뷰 전달. 실제 listener·QR·토큰은 없음. |
| `server/{gates,orch_store}.rs` | Gate 전이+operation 기록의 단일 저장/rollback. revision·발행 origin과 최종 락 획득 후 새 시각 검사, 30일 보존은 보강 필요. |
| `server/rpc/worker.rs`, `server/{connection,runtime}.rs` | O2는 인증에서 도출한 신원·요청별 재검사·닫힌 allowlist의 선례. worker의 일반 dispatch 재사용을 mobile에 복제하지 않는다. |

표의 `podd/`, `pod-cli/`는 `crates/` 아래이고, 축약한 `mobile/`는 `crates/podd/src/server/mobile/`, `server/`는 `crates/podd/src/server/`다. 실제 근거 시험은 `crates/podd/tests/mobile.rs`와 mobile/gates 인라인 시험이며, 이번에는 소스를 읽었고 실행하지 않았다.

```text
APK 번들 HTML/CSS/JS → 좁은 native intent → 핀 검증 HTTPS
  → pod-mobile의 TLS/HTTP 경계 → podd의 인증 context → mobile_call 여섯 op
PC 앱/로컬 CLI → 사용자 pipe의 mobile 관리 → 설정·QR·PC 최종 허용
SSH remote-stdio → full-user pipe                 (mobile로 이어 붙이지 않음)
worker pipe → worker capability                  (mobile로 이어 붙이지 않음)
```

`pod-protocol`은 DTO만, 새 `pod-mobile`은 TLS·주소 검증·QR 인코딩·bounded HTTP와 시험 double만 갖는다. `podd`가 기기/Task 권한·승인·저장을 소유한다. `pod-mobile → podd/pod-client/GUI` 간선은 금지하고 typed callback으로 여섯 op만 넘긴다. 모듈·등록·호환·저장은 [21 확장 가이드](21-extending.md)를 따른다. 범용 RPC/URL/TCP 프록시, full pipe 연결, `remote-stdio` 호출 경로를 만들지 않는다.

새 모바일 HTTP 계약은 `/mobile/v1`로 별도 버전 협상한다. M1 시험 DTO와 구형 로컬 `gate.resolve`를 자동으로 HTTP에 올리지 않는다. 새 관리 RPC/DTO는 통합 시점의 **다음 미사용 protocol 번호**와 capability를 배정한다(v22를 선점하지 않음). 필수 revision/인증 필드가 없는 클라이언트는 거부하며 v21 로컬 계약의 golden은 보존한다.

## 2. Listener·TLS (TM-01)

- 빌드 feature `mobile-lan`은 기본 off, 배포 빌드에서도 런타임 기본 off다. OS 사용자별 `<state_dir>/mobile/`에 선택 주소/포트/subnet만 저장한다. **enable은 현재 daemon 수명에만 유효**하며 재시작·손상 설정·인증 저장 실패 뒤에는 PC에서 다시 켠다. off는 bind socket·진행 중 TLS/long-poll·세션·pairing을 모두 닫는다.
- PC에서 활성 NIC의 **구체 IPv4 RFC1918 또는 IPv6 ULA 주소 하나**와 직접 연결 subnet을 선택한다. 포트 기본 `0`(OS가 빈 포트 배정), 고정 포트는 1024–65535의 사용자 선택만; 실패 시 다른 포트로 조용히 이동하지 않는다. 실제 IP/포트를 QR에 넣는다. wildcard·공인/멀티캐스트·link-local·DNS hostname은 거부한다. loopback은 격리 시험 전용이며 제품 설정으로 우회할 수 없다.
- 해당 NIC/subnet과 socket peer를 모두 확인한다. IPv4-mapped IPv6는 canonical IPv4로 검사하고 family 불일치·zone id·IPv6 scope 우회는 거부한다. HTTP Host/authority는 QR의 IP와 실제 포트의 정확한 집합만 허용(IPv6 bracket 포함). proxy/forwarded 헤더는 신뢰하지 않으며 absolute-form URL·중복 Host·redirect는 거부한다. NIC/IP/subnet 변경·절전 복귀 시 listener를 닫고 재선택한다.
- LAN도 적대적이다. OS 방화벽·UPnP·포트 전달·서비스를 자동 변경하지 않는다. 다른 머신 Gate를 모아 LAN에 재노출하지 않는다. VPN peer/profile·relay는 후속 단계이며 private IP만 보고 VPN까지 자동 허용하지 않는다.
- TLS는 **플랫폼 스택**이다(사용자 결정 2026-11-13): Windows SChannel·CNG, Linux 시스템 OpenSSL 3(동적 링크, 번들 없음). rustls·ring·aws-lc·RustCrypto TLS는 쓰지 않는다. **TLS 1.3이 기본**이고, OS에 TLS 1.3 server가 없는 Windows 10만 TLS 1.2를 ECDHE-ECDSA + AEAD(AES-GCM, ChaCha20) suite로 허용한다. handshake 뒤 version·suite·ALPN을 다시 검사하고, 협상 version을 pairing 기록과 PC UI에 남긴다. 신뢰는 CA가 아니라 **QR의 SPKI 핀**이 운반하므로 TLS 1.2에서도 핀·CertificateVerify 검사는 같다. ALPN `http/1.1`, 0-RTT·TLS resumption·압축·renegotiation·HTTP fallback은 끈다. 구현과 closure는 [mobile-m2/dependencies.md](mobile-m2/dependencies.md). handshake 10초, header 8 KiB/5초, body 32 KiB/10초, 연결 총 16개·인증 전 IP별 동시 2개/10회·분, 인증된 기기별 연결 2개로 제한한다. 느린 수신에는 응답 송신 10초·연결당 큐 128 KiB 상한을 둔다.

최초 enable 때 OS CSPRNG로 **P-256 ECDSA 키와 자가서명 leaf 인증서**를 만든다. SAN은 선택한 IP 하나, EKU serverAuth, CA=false, 유효기간 90일(시작은 발급 시각보다 5분 앞), 무작위 serial이다. 생성·서명·검증은 OS 암호 API(Windows CNG·CryptoAPI, Linux OpenSSL)만 쓰며 ASN.1/암호를 직접 구현하지 않는다. certificate DER·SPKI SHA-256·key epoch·endpoint manifest를 한 generation으로 기록한다. 개인키는 Windows에서 현재 사용자 CNG 키 저장소(Microsoft Software KSP)의 영구·내보내기 불가 키로 두어 사용자 DPAPI가 보호하고, 인증서는 OS 인증서 저장소에 넣지 않는다. Linux는 OpenSSL PKCS#8 파일(0600)이다. 기록 디렉터리/파일 DACL은 해당 사용자 전용, reparse/잘못된 소유권 경로는 거부한다. 쓰기·동기화·원자 교체 성공 뒤만 enable하고 손상/부분 저장이면 off다. Linux 0700/0600 보장은 R6a 이후 별도 검증한다.

native 클라이언트는 **인증 데이터 전송 전에** QR의 SPKI와 실제 인증서 공개키를 상수시간 비교하고, TLS CertificateVerify 키 소유·인증서 서명/기간/SAN·접속 IP/포트·machine_id를 검증한다. 사설 QR 앵커 검증은 OS 신뢰 저장소에 CA를 설치하거나 모든 인증서 오류를 허용하는 TrustManager가 아니다. 토큰을 미리 버퍼링하지 않으며 핀/기간/endpoint 실패 시 secret·코드·토큰 전송은 0이다. machine_id는 핀 검증 이후 얻은 응답과 QR을 대조하고, 클라이언트 시계 오류는 안내 후 차단한다.

만료 14일 전 PC에 경고한다. 키/인증서 회전·주소/포트 변경은 listener를 멈추고 새 generation과 key epoch를 저장한 뒤 **PC QR 재등록**을 요구한다. 기존 기기 인증 epoch도 올려 세션·pairing·진행 요청을 무효화하고 등록을 `needs_repair`로 둔다. session 발급은 기기의 등록 TLS generation이 현재 것과 같아야 하므로 새 핀만 알아내도 옛 키로 재로그인할 수 없다. 옛 핀과 새 핀의 자동 겹침·서버가 보낸 핀 자동 신뢰·만료 grace는 없다. 저장 실패는 off를 유지하며 이전 키로 자동 복귀하지 않는다.

## 3. QR·기기 등록·세션 (TM-02)

QR은 열 수 있는 웹 URL이 아닌 `pod-mobile-pair/1` 데이터다. schema는 `version, machine_id, endpoint, spki_sha256, tls_key_epoch, pairing_id, secret, expires_at`이고 크기 2 KiB 이하, unknown/중복 필드·잘못된 IP/길이·미지원 버전은 거부한다. 공개 인증서 핀 외에 **256비트 일회 secret**만 담고 개인키·세션 토큰은 담지 않는다. PC의 **별도 8자리 코드**는 QR에 포함하지 않는다. 화면 캡처/클립보드/로그/URL query/분석 도구에 자동 저장하지 않는다.

1. PC 명시 동작으로 전역 pairing 창 하나를 2분 열고 이전 창을 폐기한다. secret은 메모리에 해시만, 코드는 무작위 salt를 붙인 해시만 보관한다. 응답/로그 오류에는 두 값과 요청 본문을 포함하지 않는다.
2. native가 키 저장소에서 새 P-256 서명 키를 만들고 핀을 검사한다. `/mobile/v1/auth/challenge`의 256비트 nonce(용도·machine/boot/pairing id·60초 수명)를 받아 secret+코드·기기 공개키/이름·요청 Task 범위를 `/pair/claim`에 POST한다. 공개키 소유 증명은 domain-separated 정형 데이터의 서명으로 검증한다. 비인증 challenge는 앱 데이터 접근권이 아니다.
3. 서버는 실패도 IP별 5회/분·전체 20회/분에 세고, 한 QR의 코드 실패 5회 또는 2분 경과 시 폐기한다. mapped IP를 합치고 제한 테이블도 유한하게 유지한다. 오류는 secret/코드 중 무엇이 틀렸는지 구별하지 않는다. 유효 요청은 lock 아래 `open → claimed`로 **한 번만 소비**하며 병렬 요청의 승자는 한 기기 키다.
4. PC에 서버 machine, 기기 이름(자기 주장임을 표시)·키 fingerprint·Task별 네 권한을 표시한다. 사용자 최종 허용 전에는 세션/DTO를 주지 않는다. 범위는 요청과 PC 선택의 교집합, 새 Task 자동 추가 없음. 거절·시간초과·off는 `closed`이며 같은 QR을 재개하지 않는다. 전체 PC 확인도 최초 2분 안에 끝나야 한다.
5. 허용은 기기·범위·auth_epoch를 원자 저장한 뒤 확정한다. `/pair/finish`는 pending ticket와 **claim 때 키에 대한 새 nonce 서명**으로 결과를 받는다(최대 25초 대기). ticket도 256비트/hash-only·2분 경계이며 QR 소유만으로 결과를 회수할 수 없다. 성공 토큰은 한 번 발급하고 claim/finish replay로 두 번째 기기나 세션을 만들지 않는다. 응답 유실이면 재등록을 자동 실행하지 않고, 승인된 키의 새 session 절차로 복구한다.

auth/pair 경로는 **고정 인증 lifecycle**이며 여섯 앱 op를 추가하는 일반 메서드가 아니다. 이 경로는 기기 목록·범위 변경·Task DTO·전체 RPC를 제공하지 않는다. 모든 nonce는 서버 발급·용도/키/boot/epoch 결합·단일 소비이며 정해진 정형 인코딩/서명 벡터를 wire 시험에 고정한다.

| 세션 계약 | 결정 |
|---|---|
| 재로그인 | 명시적인 native 연결/잠금 해제 후 `/auth/challenge` → `/session/open`으로 등록 기기 키 서명 검증. nonce에 machine/boot/device/auth_epoch·용도 결합, 60초·단일 소비. 자동 refresh/공유 기기 키 없음. |
| bearer | OS CSPRNG 256비트 불투명 토큰 → 서버 메모리에는 해시와 M1 session id 매핑만. 요청의 `Authorization`에서 인증하고 device/actor/session은 서버가 도출. `s…`·device id·body caller로 인증 불가. |
| 범위 | 현재 기기 Task 정책과 발급 시 scope snapshot의 교집합. 축소·철회·재키는 영속 auth_epoch 증가 후 즉시 연결/토큰 무효화, 확대는 PC 허용 후 새 세션 필요. Status가 다른 권한의 전제다. |
| 수명 | 절대 8시간/유휴 15분, 기기당 세션 최대 4개(초과 시 가장 오래된 것 폐기), 전체 기기 3개. 단조 경과 시간에 절전 시간을 반영하고 wall clock 역행/불확실한 복귀는 세션 종료. Gate 기간은 서버 UTC로 별도 검사. |
| 활동 | poll/heartbeat/output 자동 갱신은 유휴 연장 불가. native 사용자 intent만 활동으로 표기, body의 임의 `user_initiated`는 신뢰하지 않음. 탈취한 native 토큰 보유자의 활동 사칭은 별도 한계이며 절대 만료는 우회 불가. |
| 저장/철회 | Android Keystore로 보호한 native 저장소만, 토큰은 JS/localStorage/cookie/backup/로그에 없음. off/restart는 모든 세션·nonce·pairing 폐기; 등록/정책/철회 epoch·TLS 키만 유지. 철회 저장 실패 시 모바일 전체를 off로 두고 성공 표시 금지. |

debug/test `mobile.test.*`는 어떤 네트워크 빌드에서도 도달 불가다. release에서는 stub 코드도 제외한다. 같은 OS 사용자의 full pipe 권한까지 기기 토큰으로 격리한다고 주장하지 않는다.

## 4. 여섯 op·예산·메시지 (TM-05)

인증된 `POST /mobile/v1/call`은 `{op, params}`의 엄격한 닫힌 union을 파싱한다. JSON-RPC envelope/batch·중첩 method·중복/unknown 필드·prefix 변형은 거부한다. HTTP parser 앞에서 request body 32 KiB, DTO 구성/clone 전 필드 예산, bounded serializer로 response 64 KiB를 집행한다. 압축은 받지 않으며 과대 요청은 413, 속도 제한은 429+Retry-After, 과대 응답은 작은 명시 오류/페이지로 돌려준다.

| op | 권한·응답 |
|---|---|
| `tasks` | Status인 Task의 id/이름/상태/출처/수신 시각만. opaque cursor로 최대 20개와 64 KiB 중 먼저 도달한 페이지. |
| `task` | 선택 Task의 Status + 별도 Gate 상세 페이지. machine/boot/revision/전체 원문/출처를 제공. `operation` 선택자는 kind(결정/메시지)·id·본문 fingerprint로 **같은 기기의 같은 요청 결과**만 읽는다(해당 Approve/Message도 재검사). 메시지는 delivery id를 복구한다. 이것이 `operation.get`의 의미이며 일곱 번째 op는 없다. |
| `output` | Status+Output. 명시 요청 때만 마지막 100줄/16 KiB, ANSI/OSC/제어문자 제거, truncation 표시. HTML escape는 화면에서 별도로 수행. |
| `gate.decide` | Status+Approve. machine/task/gate/revision/operation_id/decision/choice를 필수화하고 §6의 원자 검사. |
| `message` | Status+Message 별도 opt-in. 정규화 뒤 1000자·4 KiB·20줄, 제어/bidi 문자 거부. 고정 출처 머리말의 plain text를 review delivery로만 보냄. |
| `delivery` | 현재 Status+Message 범위의 Task 및 인증된 기기가 소유한 delivery만. 다른 기기/Task는 존재 여부도 숨김. |

읽기 60·결정 10·메시지 5회/분은 **기기별** 집행해 세션 네 개로 한도를 곱할 수 없다(M1 구현 기준으로 [17]의 세션 표현을 구체화). 실패도 소모한다. `tasks`/`task`에 최대 25초 foreground long-poll 옵션, 기기당 하나, cursor는 boot·권한 epoch에 결합한다. 변경/종료/만료 시 깨우고 반환 직전 인증/범위를 재검사한다. background에서는 취소, 실패 2–30초 jitter backoff, 재접속은 snapshot부터, FCM/영구 이벤트 재생 없음.

full RPC 이름 전부와 `terminal.send/input/keys`, shell, 파일, PR merge, `group.pick`, `mod.*`, `server.stop`, 관리/stub는 항상 거부한다. Task 원본·prompt·argv·환경·절대 경로·전체 scrollback을 직렬화하지 않는다. 로컬 CLI의 `--machine`과 SSH 브리지는 모바일 설정/페어링을 전달하지 않는다. full pipe는 여전히 OS 사용자 전체 권한이며 협력 클라이언트의 원격 차단이 same-user 격리를 뜻하지 않는다.

Message는 에이전트에 지시하는 간접 권한임을 PC와 폰에 표시한다. 리뷰 첨부는 선택 코멘트/인용을 **한 snapshot으로 고정**, 비신뢰 인용 구분, 조립된 최종 prompt까지 16 KiB·코멘트 20개로 제한하고 초과는 잘라 보내지 않는다. operation은 task·정규화 텍스트·선택 코멘트 snapshot/hash에 결합한다. working 큐는 전송 직전 세션/epoch/정책·Task·agent 상태를 재검사해 철회/만료/blocked면 전달하지 않는다. 소켓 응답 유실 시 `delivery`/`task` 조회만 하고 자동 입력·오프라인 송신 큐는 만들지 않는다. 고정 머리말만으로 prompt injection이 해결되지는 않으며 O1/O4와 실기기 검증은 M3 선행이다.

## 5. 로컬 웹 자산·native 경계 (TM-03/04)

자체 HTML/CSS/JS를 `mobile/web/`에 두고 자산 manifest(SHA-256)와 함께 APK에 고정한다. M2 desktop harness가 같은 바이트를 사용한다. podd/relay는 화면 코드를 배포하지 않는다. 화면 runtime의 npm 의존성/CDN/원격 폰트·스크립트·service worker·원격 업데이트 코드 없음. M3는 이 자산을 Android WebViewAssetLoader의 정확한 origin `https://appassets.androidplatform.net` 아래 `/assets/pod/`에서만 연다. 이 origin의 **나머지 경로도 native에서 차단**하며 실패 시 실제 네트워크로 fallback하지 않는다. [Android 로컬 자산 안내](https://developer.android.com/develop/ui/views/layout/webapps/load-local-content)를 구현 검증 기준으로 삼는다.

고정 CSP: `default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; connect-src 'none'; font-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-src 'none'; frame-ancestors 'none'`. inline/eval/data/blob 스크립트는 금지하고 CSP를 native 자산 응답 헤더로 제공한다(meta만으로 frame-ancestors를 대체하지 않음). 외부 navigation/redirect/팝업/iframe/download, file/content/javascript URL, mixed content, WebView debugging을 차단한다. 정보 링크는 자동 열지 않고 M2에서는 텍스트로만 표시한다.

- JS bridge는 main-frame·정확 origin·현재 번들 페이지만 허용한다. `tasks/task/output/decide/message/delivery` intent의 typed schema·길이·요청 id를 검사하며 임의 URL/fetch/RPC/file/키 입력을 받지 않는다. native가 인증·네트워크·세션·foreground 수명을 소유하고 JS에는 bounded DTO만 돌려준다. 최종 결정은 native 확인창이 실제 DTO의 machine/task/gate/revision/decision/choice를 다시 보여준 뒤 보낸다.
- 모든 문자열(이름/상태/출처/옵션/오류/출력/리뷰 포함)을 `textContent`/text node로만 렌더한다. HTML/Markdown/ANSI 링크 자동 해석·`innerHTML`·동적 script/style/url 속성 금지. JSON escaping과 제어문자 제거는 HTML escaping의 대체가 아니다. bidi/control은 화면에서 가시적인 escape로 표시하고 원문 바이트/revision을 변경하지 않는다.
- HTTP는 쿠키 인증·CORS 허용을 사용하지 않는다. **모든 POST에 정확한 위 Origin + native 전용 CSRF 헤더**를 요구하고 없거나 null이면 거부한다. 유일한 bootstrap은 `GET /mobile/v1/auth/challenge`이며 정확 Origin·rate limit을 검사하고 비밀은 query에 넣지 않는다. 이 challenge가 별도 256비트 CSRF 값을 주고 이후 pairing/session에 결합하며, 인증 후 토큰과 독립적인 세션 CSRF 값으로 교체한다. 이 값도 JS에 노출하지 않는다. challenge는 앱 데이터/권한이 없고 Origin은 인증 대체가 아니다. native가 검증한 intent에서만 헤더를 붙이고 서버는 Host·Origin·CSRF·bearer를 각각 검사한다.
- 서버 연결 끊김/boot 변경/25초 갱신 실패·페이지 background·원문 일부 누락이면 stale 표시와 함께 승인/거절/송신을 막는다. native 복귀/재인증 후 새 snapshot을 읽는다. 출력 truncation은 출력에만 허용하며 승인 원문은 별도 완전한 상세 화면을 요구한다.

M2의 desktop WebView/bridge double은 정책·XSS payload·자산 검증 증거다. Android main-frame 판별·Keystore/backup 제외·잠금·서명 APK/업데이트·실제 navigation은 M3에서 실기기로 확인하며 double 성공으로 대체하지 않는다.

## 6. 승인 원자성·stale 화면 (TM-06)

M1의 `mobile_decide`는 요청 시작 시각을 `mobile` 락과 `orch` 락을 기다린 뒤에도 사용한다. 수정은 락 밖에서 시계 한 번을 더 읽는 것으로 끝내지 않는다. Gate의 서버 발행 provenance·revision을 orch 저장소에 추가하며, 구형 Gate는 `origin=unknown`으로 복원한다. 발행 경로가 local app/CLI를 구별할 인증 근거가 없으면 `local-user`로 표시하고 본문이 주장한 역할을 신뢰하지 않는다. mod는 서버가 확인한 mod id, orch agent는 run/attempt를 기록한다. full-user 프로세스의 역할 주장은 보안 증명 밖이다.

revision은 재사용하지 않는 Gate id와 함께 저장하는 단조 값이다. 제목/원문/옵션/Task/만료/origin/상태 변경마다 같은 저장 경계에서 증가한다. 모바일 projection에는 machine/task/gate/revision·발행/서버 만료 시각·전체 원문·출처·`complete`를 넣는다. 기존 로컬 GateInfo wire를 강제로 바꾸지 않고 persistence 메타데이터와 모바일 전용 DTO로 투영한다. 큰 목록은 페이지로 나누고 상세 하나가 64 KiB에 안 들어가면 incomplete로 표시하여 모바일 결정 자체를 거부한다.

1. 상세 `task` 읽기가 완전한 원문을 반환할 때 서버는 `(session, boot, task, gate, revision)`의 읽음 기록을 최대 30초만 메모리에 둔다(세션당 16개). 원문 미완성/페이지 목록은 기록하지 않는다. 이것은 오래된 화면 차단용이며 **M4의 요청 digest에 결합된 암호 challenge는 아직 아니다**. native는 확인창에 현재 상세 snapshot만 사용한다.
2. 결정은 고정 순서 **mobile state → orch store**로 락을 획득한다. 역순 경로를 제거하고 await/network/terminal I/O 중 락을 잡지 않는다. 두 락을 모두 얻은 뒤 신뢰된 시계를 새로 읽어 boot·활성 session·절대/유휴 만료·auth_epoch·현재 Status+Approve·Task 소유를 검사한다. 요청 입구의 인증은 idle을 갱신하지 않고 최종 유효성 검사 후에만 활동을 반영한다. 초기 인증 시각이나 idle 갱신으로 대기 중 만료를 구제하지 않는다.
3. 동일 `(device, operation_id)`가 있으면 **같은 machine/task/gate/revision/decision/choice 본문 fingerprint**인지 먼저 검사한다. 일치하면 새 전이 없이 저장 결과, 다르면 conflict다. 그 기록이 없는 **신규 전이만** Gate pending/실제 현재 만료·revision CAS·읽음 기록의 30초/complete를 이어 검사한다(이미 성공한 replay를 현재 revision/pending으로 막지 않음). 결과 조회도 유효 세션과 현재 범위를 요구하고 다른 기기는 읽을 수 없다. PC `gate.resolve`·expiry worker도 같은 Gate commit 함수를 쓰되 구형 PC 호출에 모바일 전용 읽음 기록을 요구하지 않는다.
4. Gate 전이·revision·operation 결과·성공 감사(actor/device/epoch/범위/이전/새 상태/서버 시각)를 **orch store의 단일 원자 저장**으로 커밋한다. 저장 직전에도 만료 시각을 다시 확인하고 admission/선형화 지점을 명시한다(그 이후 디스크 flush 도중 벽시계가 흐른 것과 락 대기 중 만료를 구별). 저장 실패는 메모리/읽음 소비/operation을 모두 rollback하며 성공 이벤트·응답을 내지 않는다. fsync/원자 교체의 실제 crash 내구성을 시험하고 재시작 복구 완료 전 변경 요청을 막는다.
5. JSONL 감사는 파생 뷰다. append 실패를 Gate 성공 기록 유실로 만들지 않고 orch 저장 원본으로 복구한다. 실패 감사는 최외곽에서 빠짐없이 남기되 입력/secret은 기록하지 않는다. 응답 유실은 `task`의 operation 조회로 해결하고 승인 자동 replay는 하지 않는다. 감사 저장 실패를 성공 UI로 바꾸지 않는다.

**보존과 용량 충돌:** 성공 operation/감사 원본은 최소 30일, 20 MiB 예약 예산을 둔다. 30일 이내 기록을 2,000개/크기 상한 때문에 먼저 버리는 M1 방식은 제거한다. 공간이 없으면 새 모바일 변경을 거부하고 로컬에 정리 필요를 표시한다. PC 결정도 필요한 원자 감사 공간이 없으면 실패를 명시한다. 보안 실패 JSONL은 별도 bounded 순환/집계로 원본 예산을 잠식하지 않는다. 30일 뒤 결과 정리는 가능하나 terminal Gate를 pending으로 되돌리지 않고 id를 재사용하지 않는다. 결과가 없어지면 unknown/expired이며 재실행 근거가 아니다. 기록/마이그레이션 손상은 복구 전 모바일 off다.

M4에서는 여기에 request digest·최대 60초 challenge·channel/boot/epoch/seq와 원자 소비를 추가한다. M2의 TLS/operation 멱등성만으로 relay replay·인터넷 E2EE 요구를 충족했다고 하지 않는다.

## 7. 의존성 license gate (TM-04)

유료 기능 검사가 아니라 **신규 직접·전이 의존성의 출시 gate**다. 허용 선택지는 Apache-2.0/MIT/ISC뿐이며 SPDX OR은 실제 허용 분기를 선택해 원문을 보관하고 AND는 모든 항목을 충족해야 한다. 기존 `deny.toml`의 BSD/MIT-0/기타 예외를 신규 모바일 closure에 상속하지 않는다.

**결정(2026-11-13): TLS·암호는 플랫폼 것만 쓴다.** 채택 closure와 근거·재현은 [mobile-m2/dependencies.md](mobile-m2/dependencies.md), 고정본은 [mobile-m2/sbom.json](mobile-m2/sbom.json), gate는 `tools/check-mobile-licenses.ps1`이다(카드 M2-F0b).

| 대상 | 배포물에 링크되는 closure | build host 전용 |
|---|---|---|
| Windows x64 (daemon·desktop double) | windows-sys 0.61.2, windows-link 0.2.1(MIT OR Apache-2.0) + OS의 SChannel·CNG·CryptoAPI | 없음 |
| Linux x64 (예정, 런타임 미검증) | openssl 0.10.82(Apache-2.0), openssl-sys 0.9.118(MIT), foreign-types 0.3.2·shared 0.1.1, bitflags, cfg-if, libc + 시스템 libssl/libcrypto 3 | cc, pkg-config, vcpkg, openssl-macros·syn·quote·proc-macro2, unicode-ident(`Unicode-3.0` 항: host 전용 pin 1개, 사용자 확인 대상) |

rustls·ring·aws-lc·RustCrypto는 쓰지 않는다(ring 경로는 rustls의 BSD-3-Clause `subtle` 예외가 필요했다). gate는 이들과 openssl-src(번들 OpenSSL)가 closure에 나타나면 실패한다. SPKI SHA-256·256비트 random·P-256 서명 검증도 OS API(BCrypt/NCrypt, OpenSSL)로 하므로 p256·sha2·getrandom을 새로 넣지 않았다. [qrcode 0.14.1](https://crates.io/api/v1/crates/qrcode/0.14.1)(MIT OR Apache-2.0)·[axum 0.8.9](https://crates.io/api/v1/crates/axum/0.8.9)(MIT)는 아직 후보이며 추가할 때 같은 gate를 통과해야 한다. 암호 직접 작성·인증서 검증 생략·조용한 라이선스 예외로 진행하지 않는다.

F0 산출물은 `Cargo.lock` checksum, feature/target graph, SBOM, SPDX 선택 근거·원문·NOTICE·자산 해시와 재현 명령이다. Windows x64 daemon/desktop double과 예정 Linux target은 각각 구분한다. M3 Android는 OS TLS + 앱의 SPKI 핀이며 이 Rust closure 밖의 별도 gate다. normal/build/dev 의존성을 따로 기록하며 실제 번들 C/assembly/JNI/폰트/JS는 빠짐없이 포함한다. JDK/SDK/NDK/Gradle 같은 외부 빌드 도구의 사용 조건은 별도로 기록한다. AndroidX·QR 스캔 SDK·APK 서명/backup/업데이트 검증은 M3 gate이며 아직 승인된 runtime으로 취급하지 않는다. 금지 라이선스/누락 원문/다른 자산 hash를 주입한 합성 manifest는 CI가 실패해야 한다.

문서 작성 검증: M1/O2 코드·시험 소스 대조와 후보 메타데이터 조회, `git diff --check`·`check-docs`·`check-workspace`·`check-boundaries` 수행. 제품 코드·Cargo/daemon/실제 agent/Android 실행·사용자 설정·네트워크 쓰기는 없었다. 아래 시험은 앞으로 제출할 증거이며 이 문서의 PASS 결과가 아니다.

## 8. 병렬 worktree 구현 카드

**총 10카드.** 형식은 [20-refactor](20-refactor/daemon.md) §3을 따른다. OWNED는 저장소 상대 경로의 완전한 파일 집합이며 `{a,b}`는 개별 파일, 새 경로는 제안이다. 동시 카드의 파일은 겹치지 않는다. 선행 병합 후 기준을 갱신하고 추가 파일이 필요하면 카드부터 재분할한다. Cargo manifest/lock 변경은 F0 한 담당자만 수행하며 후속 dependency 변경도 F0로 돌린다. rm-r4ui/rm-r5가 소유한 공용 파일은 해당 작업 병합 뒤 계약/번호를 재확인한다. 아래 카드는 이번 문서 작업에서 실행하지 않았다.

| wave | 순서 |
|---|---|
| 0 | F0 license → F1 계약/접합부. F0 미통과 시 후속 구현 차단 |
| 1 | P1 TLS, P2 인증, P3 승인, P4 웹 화면 병렬 |
| 2 | T1 제한 endpoint 통합(P1–P4 이후) |
| 3 | T2 PC 제어, T3 메시지 경계 병렬(T1 이후) |
| 4 | V1 TM 검증·출시 판정(모든 카드 이후) |

공통 완료 **B**: M1/로컬 계약 회귀, 실패·off·restart와 격리 double, `git diff --check`/문서·boundary 검사. **W**: protocol/schema/호환 golden 변경을 검토하고 구형 로컬 요청 보존; 네트워크 필수 필드 누락은 거부. **S**: 아래 TM 시험 증거·실행 target/filter·미검증 범위를 기록한다. Cargo는 [21 §4](21-extending.md#4-검증-순서)의 매 실행 전 CARGO_TARGET_DIR 제거·지정 sccache·jobs=2·CPU gate(`-MaxPct 70 -MaxCargo 1`)·free RAM ≥8 GB를 지키고 targeted crate/filter만 순차 실행한다. 실제 Pod/agent 설정·실제 모델·SSH/방화벽·외부 서비스는 건드리지 않는다.

### F0 — license closure·빌드 경계 · wave 0 · M · 의존 없음

**상태(2026-11-13):** F0b로 완료 — 플랫폼 TLS(SChannel/OpenSSL 3), `pod-mobile` TLS seam·플랫폼 암호, 엄격 gate·Linux CI 검사 → master `01663c4`. 이후 manifest 변경도 이 gate를 통과해야 한다.

OWNED (11): `Cargo.toml`, `Cargo.lock`, `crates/{podd,pod-app}/Cargo.toml`, `crates/pod-mobile/{Cargo.toml,src/lib.rs}`, `tools/{check-mobile-licenses.ps1,check-boundaries.ps1}`, `.github/workflows/ci.yml`, `docs/mobile-m2/{dependencies.md,sbom.json}`.

1. §7 후보의 실제 provider/target closure 승인 또는 차단 근거를 제출한다. 승인 후만 기본 off feature·최소 crate 골격/manifest를 추가한다.
2. protocol→runtime, mobile→daemon/GUI/full transport 금지 간선과 별도 엄격 license CI gate를 둔다.
3. 증명: **TM-04** 금지/누락/OR·AND fixture 실패, 잠금 graph·자산 고지 대조, feature off 빌드에 listener 없음. M3 Android gate는 미검증으로 분리한다.

### F1 — wire·공유 접합부 고정 · wave 0 · M · 의존 F0

OWNED (11): `crates/pod-protocol/src/{mobile_protocol.rs,protocol.rs,lib.rs}`, `crates/pod-mobile/src/lib.rs`, `crates/podd/src/server/mobile/{mod.rs,api.rs}`, `crates/podd/src/server/rpc/domains.rs`, `crates/podd/tests/{protocol_wire.rs,protocol_compat.rs}`, `crates/podd/tests/fixtures/mobile-m2-wire.json`, `docs/mobile-m2/contracts.md`.

1. §3–6 strict DTO·여섯 op·auth lifecycle·cursor/operation 조회·오류 계약을 고정한다. 공용 모듈 선언/호출 seam을 먼저 마련하되 listener는 off, 미구현 경로는 명시 거부한다.
2. 기존 `GateInfo`와 M1 시험 DTO를 보존하는 mobile projection·persistence metadata 인터페이스, 락/시계 호출 경계를 카드 담당자가 함께 참조하도록 명시한다. 다음 protocol 번호는 병합 기준에서 배정한다.
3. 증명: **W, TM-05/06** 여섯 이름 집합·필수 필드/unknown/duplicate 거부·구형 로컬 golden·scope/본문 fingerprint 벡터.

### P1 — TLS·주소·키 저장 · wave 1 · M · 의존 F1

**F0b 반영:** `tls.rs`·`identity.rs`의 TLS 정책·CNG 키·자체 서명 인증서는 이미 있다. P1은 generation 기록·원자 저장·DACL, 주소/peer/Host 정책, QR과 F0b 보안 검토 지적 수정으로 좁힌다.

OWNED (7): `crates/pod-mobile/src/{tls.rs,address.rs,identity.rs,qr.rs}`, `crates/pod-mobile/tests/{tls_pin.rs,address_policy.rs,qr.rs}`.

1. §2 TLS 1.3·주소/peer/Host 정책·DPAPI/DACL·인증서 generation 저장/회전과 §3 strict QR schema/행렬 인코더를 구현한다. 실제 방화벽 변경 없음.
2. 증명: **TM-01** loopback TLS double의 위조/만료/not-yet-valid/다른 SAN·핀·port·Host·mapped IPv6·DNS/redirect 우회 거부와 secret 송신 0; 회전/부분 저장/off 실패. 정상 인증서의 양성 대조도 포함한다.

### P2 — pairing·세션·철회 · wave 1 · L · 의존 F1

OWNED (6): `crates/podd/src/server/mobile/{pairing.rs,auth.rs,registry.rs,session.rs}`, `crates/podd/tests/{mobile_pairing.rs,mobile_sessions.rs}`.

1. §3 claim/PC 허용/finish와 기기 서명 nonce·hash-only bearer·CSRF·영속 epoch·세션 수명·기기/IP 예산을 구현한다. fail-closed off 상태를 T1에 제공한다.
2. 증명: **TM-02** 같은 QR 병렬 소비 한 건, 잘못된 서명·범위 확대·코드 5회/2분·IP/전역 한도·replay·counter-only 인증 거부; fake clock 8시간/15분/역행·절전·poll 미갱신·off/restart·철회 저장 실패. 비밀 canary가 로그/URL/JS/저장 원문에 0.

### P3 — Gate CAS·원자 감사 · wave 1 · L · 의존 F1

OWNED (8): `crates/podd/src/server/{gates.rs,orch_store.rs,mods.rs}`, `crates/podd/src/server/mobile/{approval.rs,audit.rs}`, `crates/podd/tests/{mobile_atomic.rs,mobile_retention.rs}`, `crates/podd/src/server/orch_runs/gates.rs`.

1. §6 서버 origin/revision·완전 상세 읽음 기록·최종 락 안 시계·권한 재검사·단일 저장/rollback을 구현한다. 기존 Gate 생성 호출의 출처를 서버가 증명하지 못하면 unknown/local-user로 유지한다.
2. 최소 30일과 20 MiB 충돌 시 admission 차단·파생 로그 복구·terminal Gate/id 비재사용을 고정한다.
3. 증명: **TM-06** fake clock+barrier로 mobile 락/이후 orch 락 대기 중 Gate/세션 만료 성공 0, 정책/철회 경합, PC/폰/expiry 한 전이, revision·잘린 원문·30초 경계 거부, 저장 실패 rollback·rename 전후 crash/restart·응답 유실 단일 기록·2,000개 초과/용량 한도. **TM-05** 다른 기기의 operation 조회 거부.

### P4 — 번들 화면·bridge double · wave 1 · M · 의존 F1

OWNED (8): `mobile/web/{index.html,app.js,style.css,assets.json}`, `mobile/tests/{bridge-double.mjs,security.test.mjs,fixtures.json}`, `tools/check-mobile-assets.ps1`.

1. §5 CSP/text renderer·typed intent·전체 Gate 상세/선택지·native 확인·stale/foreground/결과 불명 화면을 구현한다. Android 코드는 M3이며 double은 같은 계약을 구현한다. `security.test.mjs`는 설치된 desktop browser/WebView에서 실제 CSP/DOM/네트워크 차단을 관측한다(순수 JS mock만으로 통과 불가). 시험 도구/feature는 F0에서 잠그며 미설치 엔진을 자동 다운로드하지 않는다.
2. 증명: **TM-03** 모든 DTO 문자열의 HTML/URL/bidi payload, iframe/다른 origin/null Origin·CSRF 누락·외부 navigation/file/content/download/mixed content에서 script/네트워크/결정 0; 양성 클릭은 한 번. **TM-04** 외부 자산 요청 0·번들 hash 변조 실패. **TM-06** incomplete/stale/offline 화면 변경 intent 0.

### T1 — 제한 HTTP·daemon 수명 통합 · wave 2 · L · 의존 P1–P4

OWNED (9): `crates/pod-mobile/src/{lib.rs,http.rs}`, `crates/podd/src/server/mobile/{mod.rs,api.rs,listener.rs}`, `crates/podd/src/server/{mod.rs,runtime.rs}`, `crates/podd/tests/{mobile_network.rs,mobile_limits.rs}`.

1. typed 인증 context만 `mobile_call`에 전달하고 여섯 DTO/현재 정책/operation 조회·bounded pagination/long-poll·no bridge를 연결한다. enable/off·IP/절전·shutdown 시 모든 inflight를 취소한다.
2. 증명: **TM-01/02** off listener 0·재시작/회전 폐기·유효한 TLS 인증 성공. **TM-03** 실제 HTTP Host/Origin/CSRF 거부. **TM-05/O2** 공개 schema 모든 이름+prefix/batch/중첩·다른 Task/device/delivery·debug stub을 네트워크에서 거부, 32/64 KiB 경계·slow reader/flood에도 정상 세션 응답. **TM-06** 조회는 전이 없이 본래 결과만 반환.

### T2 — PC enable·QR·최종 확인 · wave 3 · M · 의존 T1

OWNED (12): `crates/pod-cli/src/{mobile.rs,remote_stdio.rs,context.rs}`, `crates/pod-cli/tests/command_contract.rs`, `crates/pod-app/src/{main.rs,source.rs}`, `crates/pod-app/src/chrome/{settings.rs,copy.rs}`, `crates/pod-app/src/mobile/{mod.rs,pairing.rs,tests.rs}`, `crates/podd/src/server/mobile/mod.rs`.

1. 로컬 사용자 제어 화면과 CLI 상태/철회·enable/off·pairing 시작/거절/승인 연결. QR은 PC UI에서만 그리고 별도 코드/기기 key fingerprint/Task 권한을 확인시킨다. secret을 CLI stdout/JSON/history에 출력하지 않는다.
2. `--machine`/remote-stdio는 모바일 관리 전달 거부, full pipe의 same-user 한계는 명시. 기존 source/UI 컴포넌트·키보드 focus/문구/취소 규칙을 재사용한다.
3. 증명: **TM-01/02** 사용자별 off 기본·두 계정 설정 분리·QR expiry·최종 확인 전 세션 0·철회/회전 실패 UI; **TM-03/05** 기기 이름 위장 text 처리·원격/worker 관리 거부·범위 자동 추가 0. PC UIA/키보드 fixture와 CLI 계약 회귀.

### T3 — 짧은 메시지·큐 권한 경계 · wave 3 · M · 의존 T1

OWNED (6): `crates/podd/src/server/mobile/{message.rs,api.rs}`, `crates/podd/src/server/{review.rs,orch_store.rs}`, `crates/podd/tests/{mobile.rs,mobile_message_scope.rs}`.

1. §4 최종 조립/리뷰 snapshot 한도·operation 본문 결합·delivery 현재 범위·큐 전송 직전 철회/만료 재검사를 적용한다. 키 입력 API를 추가하지 않는다.
2. 증명: **TM-05** 길이/UTF-8/control/bidi·코멘트 합계 초과, 타 Task/device, 큐 중 철회/만료/blocked와 코멘트 수정, 응답 유실/replay에서 추가 전달 0; 기존 fake-agent review 전달·submit 회귀. **TM-06 관련** 락 대기 중 세션 만료는 메시지에서도 같은 새 시각 규칙 적용. 실제 모델 안전성 평가는 하지 않는다.

### V1 — TM 수용 증거·출시 판정 · wave 4 · M · 의존 T2/T3

OWNED (6): `crates/podd/tests/mobile_m2_acceptance.rs`, `tools/{test-mobile-m2.ps1,check-mobile-licenses.ps1}`, `docs/mobile-m2/validation.md`, `docs/{17-remote.md,23-mobile-m2.md}`.

1. 격리 state/POD_SOCKET·TLS double·fake clock/agent·local web harness를 조립하고 TM-01–06 양성/음성 행렬을 실행한다. 테스트 본문에서 nested cargo를 시작하지 않는다. release stub 부재·기본 off와 실제 선택 feature graph를 다시 확인한다.
2. 증명: **TM-01–06/O2 전부** 대상별 artifact·실행 명령·실패/미실행·허용 dependency closure·한도 측정 기록. 단순 M1 시험 성공으로 네트워크/보안 통과를 주장하지 않는다.
3. 미해결 provider/license·원자성·endpoint 누락 하나라도 M2 미완료. 통과해도 M3 Android 실기기와 M4–M6 인터넷·독립 암호 검토는 남아 있다고 명시한다.

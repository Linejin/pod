# Pod 보안·개인정보 평가

검토일: 2026-11-10 · 기준: `47dff8a` · 범위: 파이프/훅, 모바일 M1, SSH, 세션 발견, mods, 저장소 설정·브리프·환경, forge, 공개 배포, 설치 프로그램. 제품 코드 변경 없음.

## 점수표

| 기준 | 점수 / 10 | 가중치 | 근거 |
|---|---:|---:|---|
| 정확성(버그 없음) | 7 | 15% | 핵심 권한 경계는 명확하나 훅 SQOS 누락과 설정 저장 실패의 성공 응답이 남음 |
| 견고성(오류·경합·재시작) | 6 | 15% | 모바일 결정의 원자성은 좋으나 외부 명령의 자손·출력 파이프 종료를 보장하지 않음 |
| 보안·개인정보 | 6 | 30% | 사용자 전용 DACL·닫힌 모바일 API·mods 권한 검사 대비 훅 impersonation과 개인정보 삭제 오류 처리 부족 |
| 성능·자원 | 7 | 10% | JS·전송·발견 스캔에는 예산이 있으나 archive/forge 출력은 무제한 적재 |
| 테스트 품질 | 8 | 15% | 실제 경계값·권한 회수·재시작·JS 폭주·경합을 검사; 아래 실패 경로의 회귀 검사는 부족 |
| 코드 구조·유지보수성 | 8 | 5% | 모바일 정책/세션/감사와 mods runtime/host 분리가 좋고 실행기 제한은 중복됨 |
| 문서 정확성 | 8 | 5% | 16–19의 구현 단계·미검증 범위 설명이 구체적; 삭제·timeout 보장에는 차이 존재 |
| 사용성·접근성 | 7 | 5% | 한국어 오류·명시적 권한 토글·접근성 라벨 확인, 삭제 실패 표시와 실사용 접근성은 미검증 |
| **전체** | **6.8** | **100%** | 위 가중평균; 보안 영역 평가이며 IDE 전체 제품 점수가 아님 |

## 검증된 발견 사항

높음 1 · 중간 3 · 낮음 2 · 치명 0. 정적 검증은 해당 분기의 실제 제어 흐름을 확인했다는 뜻이며 모든 항목을 실행 재현했다는 뜻은 아니다.

| 심각도 / id | 제목 | 근거 | 영향 | 구체적 수정 제안 | 크기 |
|---|---|---|---|---|---|
| 높음 / security-01 | 훅 파이프 클라이언트가 서버의 impersonation을 허용 | `crates/pod-hook/src/main.rs:184`는 SQOS 없는 `OpenOptions::open`; 일반 클라이언트는 `crates/podd/src/transport.rs:251`에서 `SECURITY_SQOS_PRESENT \| SECURITY_IDENTIFICATION` 사용. 임시 파이프 수신기의 `RunAsClient` 결과: `impersonation=Impersonation` | 실제 podd가 없는 틈에 예측 가능한 이름(`endpoint.rs:19`)을 선점한 서버에 훅이 연결하면 클라이언트 토큰을 impersonation 수준으로 제공. 다른 신원/상위 권한 악용은 공격자의 impersonation 권한 등 OS 조건에 좌우됨; 정상 podd의 DACL만으로 가짜 서버를 방어하지 못함 | 훅도 `OpenOptionsExt::security_qos_flags`로 identification 이하를 강제하고 가짜 파이프 서버 회귀 검사 추가. 같은 패턴의 파이프 클라이언트도 점검 | S · **해결** `85680f3`(+holder 클라이언트 `9765880`) — pod-hook, pod-app attach/status, podd plug·holder 연결 모두 `SECURITY_SQOS_PRESENT \| SECURITY_IDENTIFICATION`(`podd::transport::open_pipe`). 회귀: 가짜 파이프 서버가 `ImpersonateNamedPipeClient` 후 수준 확인(pod-hook 테스트, podd transport·terminal 테스트), 수정 전 2(impersonation) → 1 |
| 중간 / security-02 | archive timeout 이후에도 자손의 출력 핸들 때문에 무기한 대기 가능 | `crates/podd/src/server/repo_config.rs:361`은 직접 자식만 kill, `:367`은 EOF를 기다리는 reader를 무기한 join. `crates/pod-forge/src/runner.rs:157`도 직접 자식만 종료하고 timeout에서 reader 스레드를 남김; 정상 자식 종료 뒤 `:168` join 역시 deadline 밖 | 승인된 archive 스크립트/외부 도구의 자손이 stdout/stderr를 상속해 계속 살아 있으면 archive 요청이 설정된 timeout 후에도 끝나지 않음. forge는 자손·reader가 남거나 응답 대기 지속 | Job Object로 프로세스 트리 수명 관리, 출력 drain에도 종료 예산 적용. 부모 종료 후 자손만 출력 핸들을 보유하는 fixture로 검증 | M · **해결** `25899a7`, `123c250` — 공용 `pod_forge::bounded`: Job Object 트리 종료, 출력 drain까지 deadline, 부모 종료 뒤 손자가 파이프를 잡는 fixture로 검증(archive·gh·group check) |
| 중간 / security-03 | 외부 명령 출력 제한이 수집 후에만 적용 | `crates/podd/src/server/repo_config.rs:349`의 `read_to_end(Vec)` 뒤 `:371`에서야 4096바이트 tail; `crates/pod-forge/src/runner.rs:145`도 무제한 적재 | 승인된 스크립트의 실수/폭주 또는 대량 gh 응답이 podd 메모리를 소진할 수 있음. JS의 16 MiB 제한이나 파이프 프레임 제한은 이 호스트 메모리를 보호하지 않음 | archive는 읽으면서 고정 크기 tail 유지, forge는 응답 크기 상한·초과 오류 및 자식 트리 종료. 작은 상한 fixture로 검증 | M · **해결** `25899a7`, `123c250` — 읽는 동안 고정 tail(archive 4 KiB) 또는 상한(gh stdout 16 MiB 초과 시 트리 종료 + `output too large` 오류). 작은 상한 fixture로 검증 |
| 중간 / security-04 | 세션 발견 끄기·미리보기 삭제 실패를 성공으로 응답 | `crates/podd/src/server/discovery.rs:94`는 쓰기 오류를 로그만 남김, `:361`은 cache 삭제 오류 무시, `:376`은 설정 저장 실패 무시, `:537` RPC는 그대로 결과 반환. `docs/18-session-discovery.md:14`–`:16`은 저장 요약/캐시 즉시 삭제를 설명 | 파일 잠금·권한 오류·디스크 오류 시 개인 프롬프트 요약이 남아도 화면은 꺼짐으로 표시. 기존 enabled/preview 설정까지 저장되지 않으면 재시작 후 다시 읽고 표시할 수 있음 | 삭제/설정 저장을 `Result`로 전파하고 실패를 UI/CLI에 명시. 메모리는 계속 비공개로 유지하되 영속 실패를 숨기지 말고 `.tmp`·corrupt 캐시의 잔여 데이터 정책도 정리 | M · **해결** `e54f279` — orch-05와 같음: `Result` 전파, `PERSIST_FAILED`, 재시도, `.tmp`·corrupt cache 정책(docs/18 §1) |
| 낮음 / security-05 | 모바일 DTO 파싱 실패가 감사 경로를 건너뜀 | `crates/podd/src/server/mobile/api.rs:175`, `:190` 등의 `params(p)?`가 개별 결과 감사보다 앞에서 반환. 정상 스키마의 메시지 검증 실패에는 감사가 있지만 필수 필드 누락/잘못된 타입에는 없음 | 실패 시도 추적에 공백. M1은 리스너가 없고 테스트 훅에서만 호출되므로 현재 인터넷 공격 표면으로 평가하지 않음 | API 최외곽에서 파싱을 포함한 실패를 본문 없이 공통 감사. `{}` 및 잘못된 필드 타입 입력의 오류·감사 레코드를 함께 검사 | S · **해결** `f1a4572` — `mobile_call` 최외곽에서 처리기가 기록하지 않은 실패를 모두 감사, 파라미터 파싱 실패는 내용 없이 `invalid params (not recorded)`. 회귀: `tests/mobile.rs::failed_mobile_calls_are_audited_without_their_params`(`{}`·잘못된 타입 7건, 수정 전 0건 감사) |
| 낮음 / security-06 | 공개 배포 차단 목록이 비어도 검사 통과 | `tools/publish-public.ps1:24`는 존재만 확인, `:25`–`:28`에서 유효 패턴 0개면 내용 검사를 생략. 동일 스크립트의 임시 저장소 DryRun에서 주석만 있는 목록 + 합성 문자열 파일이 `dry run: would commit tree ...`, exit 0 | 필수 비공개 목록이 실수로 비거나 주석만 남으면 비밀/RE 내용 검사 없이 배포 가능. 실제 비밀 유출을 발견했다는 의미는 아님 | 유효 패턴 수 0이면 중단, 정규식 검증과 git 종료 코드 확인, 합성 canary로 검사기가 동작하는지 배포 전 검증 | S · **해결** `7b4a79f` — 빈/주석뿐인 목록·잘못된 정규식 거부, git 종료 코드 확인, canary, `tools/test-publish-public.ps1`(임시 저장소 8개 경우). 실제 비공개 목록으로 DryRun 재실행은 메인 체크아웃에서 사용자가 |

security-02는 종료/수명 문제, security-03은 종료 전 메모리 상한 문제로 수정·회귀 조건이 다르다. 둘 다 의도적으로 승인받은 명령을 실행하는 기능 자체를 취약점으로 본 것은 아니다.

## 확인한 경계와 잘 된 점

- **누가 podd를 호출하는가:** `winsec.rs:116`의 protected current-user DACL, `server/mod.rs:204`의 최초 인스턴스 선점 방지 및 remote-client 거절. 같은 사용자의 일반 프로세스는 전체 RPC 권한을 갖는다. 훅/attempt 토큰은 전체 RPC 인증이나 같은 사용자에 대한 샌드박스가 아니다. `docs/19`의 제한 토큰 Codex 문제는 이 경계와 일치하며 병행 수정 작업과 겹치므로 수정하지 않았다.
- **훅/환경/브리프:** OS RNG 128비트 훅 토큰, 비교·terminal id 검사, 저장 status에서 토큰 제거, 훅 입력 크기·시간 제한을 확인. registry 기반 환경 + launcher marker 제거, 훅 토큰의 마지막 주입, 고정 첫 줄과 별도 브리프 파일은 좋은 설계다. 환경 정리는 비밀값 전체를 제거하는 DLP 기능이 아니다. `.pod/config.toml`은 명령 SHA-256과 repo별 승인값을 비교하고 개인 override의 출처를 구분한다.
- **모바일 M1:** 리스너 off, debug+환경변수로 제한된 pairing stub, 정확한 여섯 메서드 allowlist, 기본 거부 task capability, 세션 만료·철회 epoch, commit 직전 재검사, 메시지 제어문자/길이 검사·고정 header·idempotency·본문 기본 비감사. M2 이후 TLS/QR/암호화·실제 인증 토큰을 완료된 보안으로 계산하지 않았다.
- **SSH:** 절대 경로 실행 파일, 최소 `-F` config, 고정 remote command와 argv, strict known_hosts·비밀번호/포워딩 차단, 경로 quote/control 검사 및 `%` escape, bounded stderr 확인. 실제 SSH 로그인은 이번 평가에서 수행하지 않았다. 현재 `remote-stdio`는 전체 사용자 RPC 브리지이므로 제한된 모바일 채널과 혼동하면 안 된다.
- **세션 발견/mods:** discovery 기본 off·읽기 전용·head/tail 예산·대상 프로세스만 상세 조회; mods는 기본 off, 파일/네트워크 모듈 없음, runtime별 16 MiB/stack/deadline·Promise job 제한, Rust host의 manifest∩grant·task scope 검사. 새 `orch` 권한은 기본 거부이고 `*` 이벤트도 grant 검사한다.
- **forge/설치:** gh에 repo 명시·argv 배열·PR 본문 stdin, branch option 검사 확인. 설치기는 per-user/HKCU, 훅 설치가 unchecked 및 silent에서 제외되고 제거 시 선택을 요구한다. 실제 설치·업데이트·제거는 실행하지 않았다. 경로의 특수문자, 파이프 선점 같은 OS 경계 회귀를 더할 가치가 있다.

## 검증 기록과 제한

1. `git log -5 --oneline`으로 기준 `47dff8a` 확인. 16/17/18/19 및 `SECURITY.md`, 관련 소스와 `podd/tests/{mobile,remote,discovery,sessions,mods,orch_runs}.rs`, mods runtime/manager·SSH/forge 테스트를 읽었다. 테스트 통과를 새로 주장하지 않는다. Cargo 실행·전체 빌드·실제 에이전트 호출 없음.
2. 훅 재현: 기존 `C:\ai\ide\target\debug\pod-hook.exe`를 임의 GUID의 새 named pipe에만 연결했다. 자식 환경에 합성 `POD_SOCKET`/terminal/token, stdin `{}`를 주고 .NET 서버가 한 바이트 읽은 후 `RunAsClient`의 token level만 확인했다. 결과 `Impersonation`; 실제 사용자 podd/state/agent 설정은 사용하지 않았다. 기존 바이너리의 정확한 빌드 SHA는 인증하지 않았으며 소스의 SQOS 누락을 별도로 확인했다.
3. 요청된 실제 작업 트리 검사: `powershell -NoProfile -File tools/publish-public.ps1 -Message security-review -DryRun` → **exit 1**, `missing private\publish-block.txt (blocked-pattern list); refusing to publish without it`. 안전한 중단이지만 **공개 가능 판정은 미완료**다. 목록을 우회하거나 원본에 생성하지 않았다.
4. security-06은 `%TEMP%\pod-security-publish-<GUID>`에 동일 스크립트를 복사하고 로컬 main/master 두 커밋·합성 문자열·주석 전용 차단 목록으로 재현했다. **DryRun만 실행**, remote 자체가 없고 push 없음. 실제 비밀값은 사용하지 않았다.
5. tracked `private/`, `vendor/conpty/`, `*.pem`, `*.key`, `.env*` 목록 및 대표 private-key/GitHub/Anthropic token 패턴 `git grep -l -I -E`에는 일치 없음. 이는 제한된 텍스트 점검일 뿐이며 비공개 RE 차단 목록과 이미지/바이너리까지 포함한 무유출 증명이 아니다.

**추정·미확인:** installer의 cmd 래퍼(`tools/installer/pod.iss:214`)가 `%` 포함 설치/임시 경로에서 환경변수 확장을 겪을 가능성은 있으나 설치 실행으로 확인하지 않아 결함 수에 넣지 않았다. custom state directory의 상속 ACL은 별도 시험하지 않았다. 현재 코드의 `create_dir_all`만으로 임의 부모 디렉터리를 항상 사용자 전용으로 만든다고 간주하면 안 된다.

## 우선 수정 제안

1. **security-01:** 훅의 SQOS를 일반 transport와 통일하고 가짜 파이프 서버를 회귀 fixture로 고정한다.
2. **security-02/03:** archive/forge 실행기를 프로세스 트리 종료·bounded output·drain deadline을 갖춘 공통 실행기로 정리한다.
3. **security-04:** 개인정보 숨김/삭제의 영속 실패를 사용자에게 전달하고 재시작·잠긴 cache 시나리오를 검사한다.
4. **security-05:** 네트워크 M2 연결 전에 파싱 실패까지 빠짐없이 감사하는 공통 경로를 만든다.
5. **security-06:** 빈 목록 거부와 canary를 추가한 뒤, 승인된 실제 비공개 목록이 있는 환경에서 publish DryRun을 다시 완료한다.

# P6-4 원격 머신 · WSL · 모바일 승인

작성일: 2026-11-09 · **설계만, 구현·시스템 설정 변경 없음** · 기준: master `550ebc3`, 프로토콜 v11.

**구현 상태 (2026-11-10):** R1·R2 완료(프로토콜 v12, 상세는 [03 §7.4](03-podd-p0.md)). R3는 SSH 전송·머신 프로필·CLI까지 완료(앱 UI는 후속, [03 §7.6](03-podd-p0.md)); **실제 SSH 서버 연결은 미검증**(이 PC에 OpenSSH Server 없음). R4 이후는 아직 설계뿐이다. 모바일은 M1 완료(프로토콜 v13, listener 없음, 상세는 [03 §7.5](03-podd-p0.md)). OpenSSH Server·WSL·서비스·방화벽은 건드리지 않았다.

## 0. 사용자 결정이 필요한 질문

**사용자 결정 (2026-11-09):** 모바일은 인증서/키 핀을 검사하는 얇은 앱 래퍼, 첫 OS는 Android, 승인 범위는 **외부 인터넷에서도 승인**(§4). 나머지는 권고안 채택: 원격 설치는 사용자 수동, 출력은 작업별 허용·기본 비공개, Windows SSH 먼저. 추가 결정: 중계는 **사용자 직접 운영**(VPN은 대체안), 알림은 **앱을 열어 확인(foreground long-poll) 먼저**. 모바일 범위 확장(2026-11-10): 승인·상태·허용된 출력 보기에 더해 **에이전트에게 짧은 텍스트 메시지 보내기**(리뷰 의견 전달 포함) — 작업별 명시적 허용, 길이 제한, 키 입력·제어 문자·임의 명령 불가, 감사 기록.

아래 권고는 설계의 기본안이며 사용자 동의로 확정된 제품 범위가 아니다. SSH 기반 작업은 모바일 결정과 독립적으로 진행할 수 있다.

| 질문 | 권고 / 선택에 따른 영향 |
|---|---|
| 릴레이 운영자는 사용자 VPS/컨테이너인가, 프로젝트 서비스인가? | **사용자 운영 기본**, 프로젝트 운영은 비용·악용 대응·개인정보 책임을 정한 뒤 별도 결정. 기존 VPN은 대체안 |
| 앱을 열어 확인하는 방식으로 먼저 출시해도 되는가? | **foreground long-poll 먼저**. 백그라운드 즉시 알림이 필수라면 Google 의존·메타데이터 노출을 수용하는 선택형 FCM을 후속 검토 |

LAN-only는 첫 출시 단계이며 인터넷 요구 완료를 뜻하지 않는다. iOS·일반 브라우저는 이번 범위가 아니다.

## 1. 기준과 경계

[01](01-plan.md)의 목표(현 문서 §1, §2는 동기), §5 원격 스택, §6 P6, §8 백로그를 구체화한다. [03](03-podd-p0.md)의 holder 영속성과 NDJSON JSON-RPC, [08](08-tasks.md)의 원격 worktree 소유권, [11](11-review-and-orchestration-backend.md)의 Gate 상태를 유지한다. [16](16-extensions.md)의 mod 권한을 원격·모바일로 넓히지 않는다. 취약점 보고는 [SECURITY.md](../SECURITY.md)의 비공개 절차를 따른다.

**현재 사실과 제안 구분:** (작성 당시) `PROTOCOL_VERSION = 11`, `client.rs`는 `BufReader<File>` 기반 Windows pipe 클라이언트였다. R1·R2 뒤로는 v12이고 클라이언트는 `transport::Transport`(pipe·stdio 공용)를 쓴다. attach는 연결을 스트림으로 전환하고 입력은 다른 연결로 보낸다. R3 뒤로 `pod --machine <name>`이 시스템 ssh로 원격 `pod remote-stdio`에 붙는다(`pod-remote` crate). Linux PTY/UDS, 원격 attach·재접속, 원격 파일 RPC, 모바일 인증은 아직 없다. 저장은 현재 JSON registry/store이며 SQLite라고 가정하지 않는다. docs/03·08·11의 과거 버전 표기보다 현 코드가 우선이다.

```
pod-app / pod --machine M
  ├─ local → 사용자 전용 named pipe → Windows podd → holder → ConPTY
  ├─ ssh   → 시스템 ssh -T → pod remote-stdio → 원격 pipe/UDS → 원격 podd
  └─ wsl   → wsl.exe -d D --exec pod remote-stdio → Linux UDS → Linux podd

Android APK 로컬 화면 → native 핀 검증 HTTPS → podd 제한 API (LAN/VPN)
                     └→ relay 경유 Noise E2EE → podd (인터넷)
```

머신은 UI 연결 프로필이며 작업 소유자는 각 podd다. 주소는 구조체 `{machine_id, resource_id}`로 보관한다(Task id 자체에 `/`가 있으므로 문자열을 단순 분할하지 않는다). 원격 `t1`과 로컬 `t1`은 다르다. 영속 `machine_id`와 매 시작 바뀌는 `boot_id`를 구분하며, SSH 호스트 키가 실제 머신 인증 경계다. 원격 머신 교체 시 동일 별칭이라도 자동으로 기존 작업에 연결하지 않는다. v1은 데몬 간 자동 연쇄 프록시·분산 그룹을 만들지 않는다.

## 2. SSH: 시스템 OpenSSH를 전송 계층으로

| 후보 | 장점 | 비용 / 결정 |
|---|---|---|
| `russh` | Rust 비동기 채널, 한 연결에서 다중 채널·정밀한 흐름 제어 | 키·agent·known_hosts·SSH 설정 호환을 Pod가 책임진다. Apache-2.0이지만 암호 의존성 전체 검토 필요. **보류** |
| 시스템 `ssh.exe` / `ssh` | 기존 키·ssh-agent·known_hosts와 운영 습관 재사용, SSH 암호 구현을 번들하지 않음 | 실행 파일 부재·버전 차이, 여러 스트림의 프로세스 비용, 원격 shell 인용을 검증해야 함. **v1 채택** |

이는 01 §5의 `russh + TLS/Noise` 후보를 수정하는 제안이다. SSH 경로에 TLS를 다시 겹치지 않는다. 시스템 OpenSSH는 외부 전제조건이며 재배포하지 않는다.

### 2.1 인증과 실행

- 관리자가 이미 구성한 SSH 서버와 사용자 계정을 사용한다. Pod가 서버 기능·서비스·방화벽·키·authorized_keys를 설치하거나 바꾸지 않는다. SSH가 없으면 설치 안내와 진단만 표시한다.
- `BatchMode=yes`, `StrictHostKeyChecking=yes`, `PasswordAuthentication=no`, `KbdInteractiveAuthentication=no`, `ForwardAgent=no`, `ForwardX11=no`, `ClearAllForwardings=yes`, `RequestTTY=no`를 강제한다. 키는 ssh-agent 또는 사용자가 지정한 IdentityFile로 사용한다. 잠긴 키는 사용자가 외부에서 agent에 해제한다. 비밀번호·키 암호를 Pod가 입력받거나 저장하지 않는다.
- 최초 키는 사용자가 별도 신뢰 경로에서 fingerprint를 확인하고 OpenSSH known_hosts에 등록한다. 미등록·변경 키는 연결 차단, 자동 accept/삭제 금지. `ssh-keyscan` 결과만으로 신원을 검증했다고 하지 않는다.
- v1은 Pod 전용 최소 SSH 설정을 생성하고 `-F`로 지정한다. 프로필은 host/user/port/key 경로/known_hosts 경로만 저장한다. 임의 SSH 옵션·사용자 config의 `ProxyCommand`, `LocalCommand`, `Match exec`를 자동 수입하지 않는다. ProxyJump·사용자 config 전체 지원은 후속이다.
- 절대 경로로 확인한 시스템 ssh를 argv 배열로 실행한다. host/user는 선행 `-`·제어문자를 거부한다. **로컬 argv 안전성만으로 원격 셸 문자열이 안전해지지는 않는다.** 원격 명령은 고정 `pod remote-stdio`이며 cwd·프롬프트·파일 경로는 JSON 요청에만 싣는다. 서버의 PATH에 신뢰된 Pod 설치가 있어야 한다. 사용자 정의 remote command는 받지 않는다.
- stdout은 프로토콜 전용, stderr는 길이를 제한한 진단만. 로그인 배너·profile 출력이 stdout에 섞이면 오류로 중단하며 JSON처럼 보이는 줄을 찾아 건너뛰지 않는다. `-T`로 SSH PTY는 만들지 않는다(PTY는 원격 holder가 소유).

옵션의 의미는 [OpenSSH 공식 ssh_config](https://man.openbsd.org/ssh_config)를 기준으로 하고 Windows 제공 버전별로 실제 지원을 검증한다.

### 2.2 원격 설치·시작·수명

사용자는 원격 OS/아키텍처에 맞는 고정 버전 패키지를 설치한다. Windows는 사용자 디렉터리의 pod/podd/pod-holder와 필요한 ConPTY 파일, Linux는 사용자 디렉터리의 실행 파일을 사용한다. 배포 manifest의 해시와 신뢰된 배포 경로를 확인하며 `curl | sh`, sudo, 시스템 서비스 등록은 하지 않는다. 해시만으로 배포자 인증이 생기지는 않는다. Linux 바이너리는 이식·검증 완료 전 배포하지 않는다.

새 `pod remote-stdio`는 같은 사용자 로컬 endpoint에 붙는 **짧은 수명의 브리지**다. 없을 때만 설치된 동일 패키지의 podd를 분리 실행하고, 단일 인스턴스 락과 시작 제한 시간으로 중복 시작을 막는다. `podd --stdio`를 SSH 세션의 직접 자식으로 오래 실행하지 않는다. SSH가 끊어지면 브리지만 종료하며 daemon/holder/agent는 유지한다. Windows의 상속 핸들 제한을 유지하고 Linux는 stdio 분리·setsid·사용자 소유 런타임 디렉터리/UDS(0700/0600)를 구현한다. 시스템 로그아웃 정책·재부팅·WSL 종료까지 생존을 보장하지 않는다.

버전 불일치 시 설치 안내만 하고 실행 중 daemon/holder를 자동 교체·종료하지 않는다. holder 프로토콜은 클라이언트 v12와 독립적으로 협상한다. 원격 에이전트 안의 `pod`는 원격 `POD_SOCKET`/`POD_TERMINAL_ID`를 사용하며 로컬 머신 토큰·환경을 복사하지 않는다. gh 인증·Git 자격증명·에이전트 설치도 원격 사용자 환경의 책임이다.

### 2.3 프레이밍·지연·재접속

- 기존 **UTF-8 NDJSON JSON-RPC 2.0**를 양방향 stdio에 재사용한다. holder의 바이너리 프로토콜을 인터넷에 노출하지 않는다. base64 터미널 바이트와 snapshot/seq 의미도 유지한다.
- 최초 `server.status`로 protocol을 확인한 뒤 v12 `connection.hello`로 지원 범위·capability·machine_id·boot_id·프레임 상한을 확인한다. v1 원격은 v12만 허용한다. v11은 버전 안내만 가능하고 원격 변경 작업은 차단한다. v12 daemon은 기존 로컬 v11 메서드를 유지한다. 미래 버전은 공통 버전과 필수 capability가 확인될 때만 허용한다.
- 현재 연결별 의미를 유지: 제어 RPC, status 구독, 보이는 터미널별 attach에 별도 SSH/브리지 연결. v1은 동시 attach 최대 4개, 나머지는 상태만 받는다. SSH 연결 다중화는 Windows 의존성 때문에 필수가 아니다. 성능 증거가 모이면 russh/단일 multiplex 채널을 다시 비교한다.
- 새 전송의 한 JSON 줄 상한 8 MiB, 일반 출력 chunk 32 KiB 이하, 연결별 대기 큐 2 MiB. 큰 snapshot은 v12 chunk 시작/조각/끝으로 나누고 전체 16 MiB 한도·스크롤백 생략 표시를 둔다. 줄 상한은 할당 전에 검사하고 UTF-8 분할·EOF·잘못된 base64·과대 payload를 거부한다.
- 출력은 16–33 ms 단위로 모으고 백그라운드 attach를 끈다. base64의 약 4/3 크기 비용을 인정한다. SSH 압축은 기본 끔; v1 별도 바이너리 포맷은 없다. 느린 클라이언트는 holder를 막지 않도록 `terminal.lagged`/재snapshot으로 복구한다. 바이트를 조용히 버린 채 seq를 이어 붙이지 않는다.
- keepalive 15초/3회 실패, 연결 시간 제한 10초, 재시도 1→2→4→… 최대 30초+jitter, 사용자 취소 가능. 재접속은 `연결 끊김·마지막 수신 시각`을 보이며 입력을 비활성화한다. 정상 입력에는 로컬 에코를 만들지 않는다.
- 재접속 후 machine/boot 확인 → 상태 전체 snapshot/구독 → 보이는 attach snapshot으로 재구성. 이벤트 seq는 boot 경계에서 초기화하며 누락 이벤트의 영구 재생을 약속하지 않는다. 재snapshot과 live 이벤트 사이의 원자적 경계를 유지한다.
- **변경 RPC·키 입력 자동 재전송 금지**: 응답 유실이면 결과 불명으로 표시하고 목록/작업 조회로 확인한다. 파일 저장과 모바일 Gate만 별도 멱등 키로 안전 재조회한다. `terminal.send`의 제출 확인은 실행 결과 보장이 아니다.
- v9의 색 응답 500 ms 창은 유지한다. 높은 RTT에서 늦은 색 응답은 버리고 앱에 진단한다. 원격이라고 무기한 허용하면 터미널 입력 오염이 재발한다.

### 2.4 파일 편집과 리뷰

원격 worktree가 유일한 원본이다. SMB/SSHFS 마운트나 로컬 복제 후 양방향 동기화는 하지 않는다. pod-editor의 파일 I/O를 `Local/RemoteFileService`로 분리하고 GPUI 편집 버퍼만 로컬에 둔다. 원격 파일 경로는 machine+task+상대 경로로 식별한다.

| 기능 | v1 계약 |
|---|---|
| 트리·파일 읽기 | `fs.list/read`는 task 범위, 페이지 처리, 파일당 4 MiB 제한. 읽기는 bytes/base64+revision(hash)+인코딩/BOM/개행 정보. 바이너리는 편집 불가 안내 |
| 저장 | `fs.write`에 expected_revision·operation_id, 원격 lock 아래 비교 후 임시 파일+원자 교체. 외부 수정이면 conflict와 최신 revision, 덮어쓰기 대신 사용자 재검토. CRLF/BOM 유지, 성공 뒤 task.touch 발행 |
| 경로 경계 | 절대 경로·`..`·Windows 장치/ADS·NUL 거부. symlink/reparse 경로는 v1 읽기·쓰기 모두 거부. 컴포넌트별 no-follow handle로 검사하고 같은 handle 기준 처리하여 검사/사용 사이 탈출을 막음 |
| 리뷰 diff | 기존 task.diff의 numstat만으로 본문 diff를 만들 수 없다. 새 `review.diff`가 원격 git으로 base/HEAD/worktree 내용을 읽어 snapshot_id·파일 revision·분할 hunk 제공. 기존 merge-base/base_commit fallback 의미 유지, no-ext-diff/no-textconv |
| 리뷰 코멘트 | snapshot_id·revision 기준 앵커 검사, 바뀌면 stale 표시 후 재검토. 코드 인용은 본 시점의 내용을 보존; 전달·제출 확인은 기존 review.* 사용 |
| 끊김·삭제 | 미저장 버퍼 보존, 연결 전 저장 불가. v1 원격 UI의 task.remove는 편집 탭 확인 뒤 수행. 외부 CLI 삭제는 300 ms grace로 네트워크 저장을 보장하지 못하므로 저장 실패 시 복구 버퍼를 남김 |

파일 저장의 operation_id는 task+path+본문 hash와 묶어 결과를 영속 기록한다. 파일 교체와 기록 사이의 crash는 준비 journal과 최종 파일 hash로 복구하며 복구 완료 전 해당 파일 쓰기를 막는다. 같은 키에 다른 본문은 거부하고 보관 기간(24시간)이 지난 결과 불명 요청은 자동 재시도하지 않는다. 캐시는 메모리가 기본이며 영구 원격 소스 캐시는 별도 opt-in. LSP 원격 프록시, 파일 생성/이름 변경/삭제, 대용량·바이너리 편집, 오프라인 저장/자동 병합은 v1 제외한다. Git·check·PR는 원격 podd에서 실행하고 UI는 원격 파일을 로컬 경로로 열지 않는다. 같은 OS 계정의 악성 프로세스와 경쟁하는 파일 접근은 완전한 샌드박스가 아님을 명시한다.

## 3. WSL: 두 모드를 구분

| 모드 | 소유권·경로·선택 |
|---|---|
| Windows podd가 `wsl.exe` 터미널 실행 | Windows Task/holder 안에 Linux shell만 띄움. 초기 보조 기능으로 가능하나 Linux 작업 환경과 동등하지 않다. Windows Git은 Windows worktree, Linux 도구는 명시 변환된 마운트 경로 사용. Windows pipe 주소를 Linux pod가 쓸 수 있다고 가정하지 않음; Linux CLI 오케스트레이션은 미지원 표시 |
| WSL 배포판 내부 Linux podd | **정식 WSL 작업 모드로 채택**. Linux Git/PTY/holder/UDS/CLI가 Linux filesystem의 worktree를 소유. 앱은 `wsl.exe -d <선택한 배포판> --exec pod remote-stdio`로 연결. SSH·방화벽 불필요, Linux 이식 완료가 선행 조건 |

`wsl.exe` 존재 확인만으로 설치 성공을 판단하지 않는다. 읽기 전용 `wsl --status`, `wsl --list --quiet`/`--verbose`에 제한 시간을 두고 실행 파일 없음·기능 없음·배포판 없음·오류를 구분한다. Windows 출력 인코딩(UTF-16 포함)과 지역화된 오류는 fixture로 검증한다. **설치는 사용자 몫**이라는 안내와 [공식 명령 문서](https://learn.microsoft.com/en-us/windows/wsl/basic-commands)만 제공하며 install/update/default distro 변경을 실행하지 않는다. 선택 전에는 배포판을 시작하지 않는다.

경로는 항상 endpoint OS 기준이다. `/home/u/repo`를 Windows 정규화 함수에 넣지 않는다. Windows→WSL 경로가 필요한 보조 모드에서만 선택 배포판의 `wslpath`로 변환하며 `C:`를 무조건 `/mnt/c`로 치환하지 않는다(사용자 automount 설정). UNC·매핑 불가 경로는 오류, `\\wsl.localhost` 파일 직접 편집은 하지 않는다. WSL 터미널에서 Windows POD_*를 제거하고 Linux podd가 자기 값으로 주입한다. WSL 종료/Windows 재부팅 시 작업 종료 가능성을 표시한다.

## 4. 모바일 승인: Android 래퍼·LAN·인터넷 E2EE

### 4.1 노출과 신뢰 부트스트랩

기본 off이며 off에서는 listener도 없다. 로컬 앱에서 켤 때 **선택한 사설 LAN IP 한 개**에만 HTTPS를 bind한다(0.0.0.0/:: 금지). 허용 subnet을 명시하고 IPv4/IPv6 모두 검사한다. IP가 바뀌면 listener를 닫고 다시 선택한다. LAN도 신뢰 경계가 아니므로 인증은 필수다. LAN 단계에는 UPnP·포트 개방·자동 방화벽 변경·공개 relay가 없다. 방화벽으로 막히면 실패를 설명한다. SSH 원격 머신의 모바일 서비스는 해당 머신에서 별도 설정하며 로컬 Pod가 원격 Gate를 몰래 LAN에 재노출하지 않는다.

**선택: Android APK에 고정된 자체 웹 화면과 native 네트워크 계층**(§4.7). LAN은 podd의 제한 API를 TLS 1.3으로 직접 호출하고 QR의 SPKI SHA-256 핀을 접속 전에 검사한다. OS 인증서 저장소 변경·인증서 오류 무시·HTTP fallback은 없다. 일반 브라우저 JS의 가짜 핀 검사도 지원하지 않는다. 자가서명 LAN 인증서는 QR 앵커·키 소유·유효기간·endpoint를 모두 확인한다. 인증서 핀 변경은 PC에서 재등록한다. [Android 공식 문서](https://developer.android.com/privacy-and-security/security-config)는 참고이며 동적 QR 핀은 native 계층에서 검증한다. 인터넷 경로는 §4.5–4.6의 별도 종단 키 인증을 추가한다.

### 4.2 페어링·세션·권한

1. 로컬 앱의 명시적 사용자 동작으로만 2분짜리 pairing 창을 연다. QR에는 endpoint, machine_id, 인증서 pin, 256비트 일회용 pairing secret을 담고 **별도 8자리 코드**는 PC에 표시한다. 둘 다 인증된 채널 밖 로그·URL query·분석 도구에 남기지 않는다.
2. 래퍼가 QR을 로컬에서 파싱하고 pin 검증 TLS로 secret+코드를 POST한다. secret은 해시만 저장하며 1회 소비를 원자 처리한다. 코드 5회 실패 또는 2분 경과 시 해당 pairing 폐기; IP당 5회/분·전체 20회/분 제한도 적용한다.
3. PC에 기기 이름과 요청 범위(선택 Task 읽기, Gate 결정, 출력 읽기)를 표시하여 최종 허용한다. QR을 본 것만으로 자동 권한 부여하지 않는다. 새 작업/그룹이 자동으로 범위에 들어오지 않는다.
4. 256비트 불투명 세션 토큰, 서버에는 해시만 보관. 래퍼 OS 보안 저장소에 보관하고 웹 localStorage/URL에는 넣지 않는다. native가 네트워크를 전담하며 토큰/쿠키를 JS에 주지 않는다. 절대 8시간·유휴 15분 만료, 자동 영구 refresh 없음. 백그라운드 polling으로 유휴 시간을 연장하지 않는다.
5. 로컬 앱에서 기기별 또는 전체 철회 가능. 세션·연결 즉시 폐기, 결정 요청마다 철회 여부 재검사. off 또는 daemon 재시작 시 세션/미사용 pairing 모두 무효; TLS 키와 승인된 기기 신원/범위·철회 기록은 사용자 전용 권한으로 보존(§4.6). 키 회전 시 재페어링. 인증 로그에도 토큰/코드/QR은 기록하지 않는다.

API는 `tasks`의 id/이름/상태/출처/갱신시각, 선택 작업의 pending Gate와 허용된 마지막 출력만 제공한다. Task 원본의 prompt·argv·환경변수·절대 경로·PR 토큰·전체 파일·전체 scrollback을 직렬화하지 않고 전용 DTO를 쓴다. 출력은 요청할 때만 100줄/16 KiB, ANSI/OSC·제어문자 제거 후 HTML text로 렌더한다. 비밀 패턴 마스킹은 보조 수단이며 완전한 제거를 보장하지 않는다. UI는 연결 시각과 오래된 상태를 표시한다.

모바일은 `terminal.send/input/keys`, shell, 파일, PR merge, group.pick, mod.*, server.stop을 **호출할 수 없다**. 임의 JSON-RPC 프록시 endpoint를 만들지 않는다. 읽기 60회/분·결정 10회/분/세션, 동시 기기 3개, 요청 32 KiB·응답 64 KiB 한도, 초과 시 429와 Retry-After. 상태 갱신은 §4.6의 foreground long-poll이며 이벤트 재생 저장소는 만들지 않는다.

### 4.3 Gate 결정의 의미와 감사

**승인 대상은 Pod Gate의 pending 상태뿐이다.** `blocked:permission/question/auth` 화면을 보고 임의 `y`/Enter를 보내는 기능은 없다. Gate는 협력하는 에이전트의 대기 규약이며 OS 권한 샌드박스가 아니다. 모드가 만든 Gate도 출처를 표시하고 모바일 접근 권한을 얻지 못한다. v12에서 서버가 기록한 origin(local-cli/mod/desktop) 메타데이터를 추가하고 이전 Gate는 unknown으로 표시한다. 제목의 모드 이름만으로 신원을 인증하지 않는다.

결정 화면에 machine/task/Gate id·제목·전체 요청·발행 시각·만료 시각·출처를 보여 주고 approve/reject를 구별한다. 잘린 요청·stale/offline 화면에서는 승인 비활성화하고 원문을 새로 읽게 한다. 새 `gate.decide`는 gate id, 예상 revision, operation_id, decision을 받고, 인증 actor는 서버가 세션에서 결정한다(본문 actor는 무시). scope·pending·실제 현재 시각의 만료·revision을 **같은 lock/commit 경계**에서 검사한다. 현재 v11 `gate_resolve`는 pending 검사 후 저장하므로 expiry worker와의 경합을 추가로 막아야 한다.

같은 operation_id+같은 본문 재시도는 저장된 결과를 반환하고 다른 본문은 conflict; PC와 모바일 동시 결정은 한 건만 성공한다. 응답 유실 시 operation 조회 뒤 표시하며 오프라인 승인 큐는 없다. 기존 로컬 gate.resolve도 같은 내부 결정 경로로 수렴시키되 새 필드 없는 로컬 요청은 현 계약을 유지한다.

결정·actor·기기 id·범위·이전/새 상태·서버 시각·operation_id·거부 이유를 감사한다. Gate 전이와 성공 감사 레코드는 기존 orch store의 단일 원자 교체에 함께 커밋하고 저장 실패 시 메모리 전이도 롤백한다. 그 뒤 이벤트/응답; 별도 JSONL은 재생 가능한 파생 로그다. pairing/로그인/철회/실패는 제한된 보안 로그에 남기고 본문·출력·토큰은 제외한다. 실패는 API 최외곽(`mobile_call`)에서 빠짐없이 감사한다: 처리기가 더 자세히 남기지 않은 실패와 매개변수 파싱 실패(`{}`, 잘못된 타입)도 포함하며, 파싱 실패는 매개변수 내용 없이 `invalid params (not recorded)`로만 남긴다(security-05). 기본 30일/20 MiB 순환, 접근은 로컬 사용자만. 성공 기록을 남길 수 없으면 모바일 결정은 거부한다. 같은 OS 사용자나 관리자에 의한 감사 파일 변조까지 방지하는 원격 증거 저장소는 v1 범위 밖이다.

### 4.4 위협 모델

| 공격자 / 경로 | 자산·위험 | 방어 / 잔여 한계 |
|---|---|---|
| LAN 도청·MITM·악성 AP | 소스/출력 유출, 승인 위조 | 최초 바이트부터 QR pin TLS, HTTP fallback 없음; QR을 보여 주는 PC가 신뢰 전제 |
| 인터넷 스캐너·DNS rebinding | API 접근·인증 시도 | 구체 LAN IP/subnet, Host/Origin allowlist, 인증·rate limit; LAN-only가 인증 대체 아님 |
| QR 사진·코드 추측 | 기기 무단 등록 | 짧은 수명·일회 사용·별도 코드·PC 최종 확인; PC/휴대폰 동시 장악은 제외 |
| 악성 웹사이트·출력 HTML | CSRF/XSS로 승인 | POST+CSRF 토큰+Origin 검사, CORS off, CSP default-src self/frame-ancestors none, 외부 자원 없음, text 렌더 |
| 탈취·분실 휴대폰 | 살아 있는 권한 사용 | 기기 잠금/OS 보안 저장소·짧은 만료·철회, foreground 복귀 재잠금; 잠금 해제된 기기 악성코드는 잔여 위험 |
| 재전송·동시 승인·시계 경계 | 중복/만료 Gate 승인 | 멱등 operation, revision CAS, 서버 만료 검사, 원자 저장; 클라이언트 시각 불신 |
| 악성 에이전트/모드 | Gate 사회공학·비밀 출력 | 출처·원문 표시, 최소 DTO·출력 opt-in, 모바일 권한을 mod API에 주지 않음; 사용자가 잘못 승인할 가능성은 남음 |
| 악성 SSH 서버·경로·대량 출력 | 로컬 파일 탈출·메모리 소진 | host key, OS별 경로 경계, 크기/큐 한도, 원격 데이터에서 로컬 명령 실행 금지 |
| 같은 사용자·관리자·감염된 podd | 키/작업/감사 조작 | 현 DACL/UDS 경계 유지; 동등 계정·종단 감염을 TLS로 막는다고 주장하지 않음 |
| 악성/침해된 relay·TLS 종료자 | 본문 탈취·키 바꿔치기·재전송 | APK 로컬 자산, QR 종단 키 핀·Noise 상호 인증·challenge/CAS; IP/시간/크기 상관관계와 가용성은 보호 불가 |
| 인터넷 flood·route credential 유출 | 비용·메모리 소진·연결 방해 | 초대/기기별 credential·사전 크기 검사·IP/route/전역 예산·철회; credential만으로 승인 불가, 분산 DoS는 운영 대응 필요 |
| 오래된 암호문·철회 통지 차단 | 재시작/철회 뒤 승인 | 새 channel/boot/epoch·nonce·서버 만료·영속 operation/CAS; relay 통지에 권한 판단을 의존하지 않음 |
| APK 공급망·WebView bridge | 키/승인 탈취 | 서명 업데이트·번들 자산·origin/main-frame 검사·native 최종 확인·비밀 JS 전달 금지; 서명 키/종단 침해는 잔여 위험 |
| 선택형 미래 FCM | 기기 연결/시점 노출 | v1 미사용, 내용 없는 wake-up도 metadata 노출·Google 의존을 제거하지 못함 |

### 4.5 인터넷 경로와 릴레이 운영

이 절과 §4.6–4.7은 앞의 LAN 설계를 인터넷으로 확장한다. **M1–M3 LAN 먼저, M4–M6 인터넷 완료**로 나눈다.

| 경로 | 비용·운영·신뢰 / 선택 |
|---|---|
| 작은 Pod 릴레이, 사용자 VPS/컨테이너 | **인터넷 기본 권고**. 양쪽 outbound HTTPS로 NAT 뒤 podd에 연결. 사용자가 호스팅·도메인·TLS·패치·대역폭 비용 부담. 릴레이는 E2EE blob만 전달하지만 가용성과 메타데이터는 운영자에게 의존 |
| 같은 릴레이, 프로젝트 운영 | 사용자 설정은 쉽지만 프로젝트가 상시 비용·계정/초대·악용 신고·DDoS·로그 보존을 책임짐. 본문 E2EE여도 IP·접속 관계는 보임. 무료 무제한 서비스를 약속하지 않으며 초기 제외 |
| 사용자 제공 Tailscale/WireGuard 등 overlay VPN | **대체안**. LAN API·핀·권한 설계 그대로, 사용자가 지정한 VPN 인터페이스 주소/peer allowlist만 추가. Pod가 VPN 설치·서비스·계정·라우팅을 관리하지 않음. 공급자 제어면/요금과 VPN 운영 책임은 별도. [Tailscale DERP](https://tailscale.com/kb/1232/derp-servers)도 암호화 패킷 중계 가능 |
| 공인 IP/포트 전달, reverse tunnel, WebRTC | 직접 공개는 NAT·방화벽·인증서 운영과 공격면 증가. TLS 종료 터널만으로는 E2EE 아님. WebRTC는 signaling/STUN/TURN 운영 추가. 초기 제외; 도입 시에도 종단 키 인증·Gate 규칙 유지 |

```
Android APK 로컬 화면 → native TLS pin → podd 제한 API (LAN/VPN)
                     └→ TLS → relay ← outbound TLS ← podd
                         └──── Noise E2EE 종단 ─────┘
```

인터넷 모드는 podd inbound 포트 없이 릴레이로만 나가며 LAN listener와 독립적으로 끌 수 있다. off는 listener·relay 연결 모두 없음. VPN 주소는 LAN 판별을 느슨하게 하지 않고 명시 프로필로 허용한다. UPnP·서비스 설치·자동 방화벽 변경은 없다. 릴레이 TLS는 CA/hostname/기간 검사와 설정된 SPKI 핀을 함께 사용하며 **relay TLS 핀과 podd E2EE 신원 키 핀을 구분**한다. LAN 자가서명 인증서는 QR 앵커·키 소유·기간·endpoint 검사 유지. 핀 불일치는 차단; 계획된 relay 현재/다음 핀 겹침은 PC에서 미리 승인하고 relay가 보낸 새 핀은 자동 신뢰하지 않는다.

릴레이는 고정 rendezvous만 제공하고 임의 URL/TCP 프록시·Gate 결정·키 보관·viewer 코드 배포 기능은 없다. route는 무작위 256비트 값, 별도 기기별 relay credential은 로컬 등록/철회로 관리한다. 초회 credential은 pairing 창에만 유효하며 QR secret과 분리한다. credential을 알아도 mobile 권한은 얻지 못한다. 운영자는 IP·route 연결 관계·시각·지속시간·크기·빈도를 볼 수 있고 지연/차단할 수 있으나 task/Gate 이름·출력·결정·비밀키는 볼 수 없다. route 회전도 traffic analysis를 없애지는 못한다. 본문/credential/query 로그 금지, 기본 운영 로그는 집계만; IP 진단은 명시적 opt-in 24시간 보존.

초기 한도: TLS/handshake 10초, 인증 전 IP당 10회/분·동시 5개, 인증 후 route당 기기 3개·연결 4개, 프레임 32 KiB·큐 256 KiB·TTL 30초, route당 256 KiB/초·burst 512 KiB. 64 KiB 응답은 암호화된 메시지 id/총길이/조각 수로 분할하며 복원 상한을 먼저 검사한다. 초과는 429/Retry-After 또는 연결 종료, jitter backoff. 전역 연결/메모리/egress 예산·초대 발급·credential 폐기·운영 kill switch 필수. IP 제한만으로 NAT 사용자/분산 DoS를 해결하지 못하며 큰 공격에는 호스팅 계층 대응 또는 비용 한도에서 서비스 중단. podd는 별도로 복호화 후 API 한도를 집행한다.

### 4.6 E2EE·수명·알림 계약

**프로파일 후보는 `Noise_XX_P256_ChaChaPoly_SHA256`**이다. P256은 snow 지원 확장으로 양쪽 동일 구현/테스트 벡터를 검증하며 표준 25519 구현과 호환된다고 주장하지 않는다. 선택 이유는 §5의 BSD 의존성 제약; 자체 암호 프리미티브는 만들지 않는다. 패턴·nonce 규칙은 [Noise 공식 명세](https://noiseprotocol.org/noise.html)를 따른다. 아래는 Pod의 설계이며 구현 전 보안 검토가 필요하다.

1. **페어링:** podd와 기기마다 독립 장기 DH 신원 키·무작위 device_id. QR v1은 machine_id, podd 공개키/키 epoch, endpoint·TLS 핀, 256비트 일회 secret·pairing_id·2분 만료를 담는다(LAN과 동일 길이). 비밀키는 QR에 넣지 않는다. 앱 initiator/podd responder로 XX를 완료하고 앱은 QR 공개키를 정확히 대조한 뒤 암호화 채널로 secret·별도 PC 코드·기기 공개키·범위를 제출한다. PC는 기기 키 fingerprint와 범위를 최종 확인한다. 등록 전에는 pairing만 허용하며 일회 소비·실패 한도 유지. QR 탈취만으로 자동 권한 부여 없음.
2. **채널:** prologue에 `pod-mobile-e2ee/v1`, machine_id, key epoch, 역할을 묶고 프로파일을 transcript에 고정. 등록 후 양쪽 static key를 등록값과 비교하고 세 메시지 완료 후에만 DTO 전송; 0-RTT·downgrade 없음. 연결마다 OS CSPRNG의 새 ephemeral 키, 종료 시 ephemeral/transport 키 폐기. 30분 또는 방향당 2^20 프레임 전에 새 handshake; 재접속도 새 키. 장기 키 사후 유출에 대한 순방향 비밀성을 목표로 하며 종단/RNG 침해는 방어 밖. 방향별 단조 nonce 재사용 금지, 중복/역순/인증 실패는 채널 종료 후 snapshot 재조회.
3. **승인 replay 방지:** 암호화 envelope는 서버 발급 channel_id, boot_id, device_id, auth_epoch, seq, request_id와 DTO. `gate.decide`는 machine/task/gate id·revision·operation_id·decision·request_digest·서버 발급 challenge/expiry를 포함한다. challenge는 기기/세션/revision/본문 digest에 결합, 최대 60초 또는 Gate 만료 중 먼저 만료; 성공 시 원자 소비. 철회/세션/boot/범위/서버 시각/CAS를 commit 직전 검사. 같은 기기의 같은 operation/body만 저장 결과 조회 가능; 기록은 Gate와 함께 최소 30일 보존, 기록 만료 후에도 terminal Gate를 재개하지 않음.
4. **단절:** relay는 오프라인 요청 저장소가 아니다. 큐는 양쪽이 연결된 동안의 backpressure용이며 어느 쪽이 끊겨도 즉시 폐기한다. podd 절전/폰 오프라인이면 승인 불가. stale·만료 요청은 읽기만, 재연결 후 원문/revision/challenge를 새로 읽고 사용자 재확인. 결과 불명은 `operation.get`, 승인 자동 재전송 없음. 서버 커밋 확인 전 성공 표시 금지.
5. **키/세션:** 장기 등록·범위·철회 epoch는 재시작 후 유지하되 8시간/15분 유휴 세션은 소멸. 새 handshake 뒤 기기 잠금 해제와 명시적 연결로 등록 범위 내 새 세션 발급, 백그라운드 자동 refresh 없음. 기기 최대 3개, 각각 키/범위/감사/철회; 그룹 공유키 없음. off는 연결/세션 폐기, 다시 켜기는 PC에서만. 로컬 철회는 기기별 영속 auth_epoch 증가와 해당 연결 종료, 결정마다 재검사하므로 relay가 철회 통지를 숨겨도 무효. 장기 키 교체·앱 재설치·분실은 이전 등록 철회 후 QR 재등록. TLS 키와 E2EE 키 회전은 독립, 자동 키 복구/클라우드 백업 없음.

**알림 결정: foreground 25초 long-poll, 실패 시 2–30초 jitter backoff; 백그라운드 중지, FCM 없음.** polling으로 유휴 만료를 연장하지 않는다. Android 백그라운드 제한 때문에 앱을 닫은 동안 즉시 알림은 보장하지 않는다. 후속 FCM opt-in은 내용 없는 wake-up만, Gate id/이름/본문/승인 버튼 없음. 앱을 열어 E2EE 재조회해야 한다. 그래도 Google은 앱/기기 토큰·전송 시점을 알 수 있고 Google Play services 의존이 생긴다([FCM 전제조건](https://firebase.google.com/docs/cloud-messaging/android/client)). SDK 라이선스·사용자 동의는 별도이며 현재 승인된 의존성이 아니다.

### 4.7 Android 래퍼·빌드·배포

**Kotlin 최소 Activity + APK에 고정된 자체 HTML/CSS/JS WebView**를 선택한다. 전체 native UI보다 화면 재사용이 쉽지만 bridge 경계를 검증해야 한다. relay/podd에서 실행 코드를 받지 않는다. 로컬 자산은 APK 서명으로 고정; [WebViewAssetLoader](https://developer.android.com/develop/ui/views/layout/webapps/load-local-content)의 로컬 HTTPS origin, CSP `default-src 'none'`에 번들 script/style만 허용한다. iframe·외부 navigation·다운로드·mixed content·file/content URL·release debugging 차단. text DTO와 origin/main-frame을 확인하는 좁은 message bridge로 목록/조회/결정 intent만 전달한다. 일반 `addJavascriptInterface`·fetch proxy 금지. TLS pin·Noise·키·토큰은 native/Rust JNI가 소유하고 쿠키 교환 없이 JS에 주지 않는다. 최종 승인은 native 확인 화면에서 machine/Gate/digest와 challenge를 다시 결합한다.

Windows 빌드는 고정 JDK/Gradle wrapper/AGP·Android SDK Build Tools와 Rust Android target/NDK로 APK 생성([공식 명령행 빌드](https://developer.android.com/build/building-cmdline)). JNI용 arm64-v8a 실기기와 x86_64 emulator를 구분하고 버전·Gradle/Cargo lock·자산 해시를 기록한다. 이번 작업은 설치/빌드 없음; 도구가 없으면 사용자 수동 준비 후 진행. emulator는 **이미 설치·실행 가능한 경우만**, 서비스·가상화 기능·방화벽 변경 금지. 없으면 Windows native core/desktop wrapper double과 합성 UI로 검증하고 Android 검증으로 간주하지 않는다.

기기 신원 키는 [Android Keystore](https://developer.android.com/privacy-and-security/keystore)의 wrapping key로 암호화 저장한다. 선택 Noise 키의 하드웨어 비추출 지원을 가정하지 않으며 JNI 사용 중 메모리 잔여 위험을 인정한다. 잠금 복귀 재인증·백업 제외, podd 키는 사용자 DACL·OS 보호 저장소. 앱 서명 키·TLS 키·기기 신원 키는 서로 다르다.

**서명 APK sideload 먼저, Play 후속**. 개인 빌드는 사용자, 프로젝트 릴리스는 지정 릴리스 관리자가 서명 키와 오프라인 백업을 보유하며 relay에는 주지 않는다. 키/비밀번호를 repo·로그에 넣지 않는다. 신뢰된 다운로드 채널과 서명 fingerprint를 확인하고 debug 서명은 제품 배포 금지. 업데이트는 같은 서명 계보·상향 versionCode 검사. Play 도입 시 [Play App Signing](https://developer.android.com/studio/publish/app-signing)의 키 보관 주체와 sideload 업데이트 호환성을 별도 결정한다.

실제 폰에서는 QR 카메라·인증서/핀 불일치·키 저장/복원 제외·잠금/복귀·철회·APK 업데이트와 bridge 탈출을 확인해야 한다. 서로 다른 Wi-Fi/이동통신에서 NAT·네트워크 전환·Doze/프로세스 종료·PC 절전/복귀·배터리 소모를 검사하고 지연/중복/만료 승인이 커밋되지 않아야 인터넷 출시 완료다.

## 5. 프로토콜·코드 경계·의존성

아래 이름은 **제안**이다. 그중 `connection.hello`만 R2에서 v12로 구현됐다(오류 코드 `PROTOCOL_MISMATCH` -32051, capability 이름은 예약만 하고 아직 광고하지 않음). 나머지는 구현 시 다음 번호를 쓴다. 모바일 HTTP는 별도 `/mobile/v1`이며 일반 RPC 전체를 공개하지 않는다.

| 추가 계약 | 내용 |
|---|---|
| `connection.hello` | min/max protocol, 선택 버전, capabilities(`remote.files`, `remote.diff`, `attach.chunks`, `gate.cas`), machine/boot id, OS/path 규칙, 한도 |
| `fs.list/read/write`, `review.diff` | task 범위, paging/chunk, revision·snapshot·멱등 저장; 오류 `UNSUPPORTED`, `CONFLICT`, `TOO_LARGE`, `PATH_DENIED`는 구현 시 충돌 없는 코드 배정 |
| attach snapshot chunks | 시작 seq와 snapshot id, 순서 있는 조각·완료, 이후 기존 output seq; v11 로컬 attach 응답은 변경하지 않음 |
| `gate.decide`, `operation.get` | 예상 revision+멱등 키, 충돌·만료·철회 결과; mobile adapter에서 인증 context 전달, 기존 gate.resolve는 호환 경로 |
| 로컬 전용 mobile 관리 | enable/disable, pairing 발급, session 목록/철회. HTTP/모드에는 노출하지 않음. 신뢰된 전체 권한 SSH 클라이언트도 v1에서는 모바일 설정 변경 불가 |
| mobile E2EE v1 | QR schema·Noise profile·channel/boot/epoch/seq·challenge/digest·인증 context, `/mobile/v1` DTO만 암호화. 비호환 버전 fail closed, 평문/구버전 fallback 없음 |
| relay wire v1 | register/connect/close와 bounded blob framing만. 기기별 credential·route quota; 범용 JSON-RPC/HTTP proxy 아님 |

새 workspace crate는 `pod-remote`(transport/머신 프로필/재접속), `pod-mobile`(feature off 기본인 TLS·웹 DTO·페어링·정책) 두 개로 제한한다. 프로토콜 타입은 우선 podd의 client-only 경로를 재사용하고 필요할 때만 별도 crate로 추출한다. pod-cli에는 `remote-stdio`, `--machine`을 추가한다. Linux의 PTY·UDS·환경·git 경로 처리는 podd의 OS별 모듈이며 독립 daemon 제품을 만들지 않는다. 모바일 래퍼와 relay 실행 파일/컨테이너는 별도 배포물이다. 초기 암호 코어는 pod-mobile의 제한 JNI 경계로 재사용하고 relay는 별도 binary target으로 둔다.

### 의존성 라이선스 확인 (2026-11-09)

crates.io의 아래 **명시 버전 license 메타데이터**를 조회한 후보 목록이다. 실제 lockfile/전이 의존성 승인이나 보안 감사를 끝냈다는 뜻이 아니다. 새 직접·전이 의존성에는 이번 요청의 **Apache-2.0/MIT/ISC만** 적용한다. 기존 deny.toml은 BSD 등을 허용하고 Windows만 검사하므로 그것의 통과만으로 이번 제약을 만족했다고 하지 않는다.

| crate / 확인 버전 | SPDX | 사용 결정·공식 근거 |
|---|---|---|
| russh 0.65.1 | Apache-2.0 | 비교만, 추가 안 함 · [metadata](https://crates.io/api/v1/crates/russh/0.65.1) |
| axum 0.8.9 | MIT | 모바일 최소 HTTP routing 후보 · [metadata](https://crates.io/api/v1/crates/axum/0.8.9) |
| rustls 0.23.46 | Apache-2.0 OR ISC OR MIT | TLS 후보 · [metadata](https://crates.io/api/v1/crates/rustls/0.23.46) |
| rcgen 0.14.10 | MIT OR Apache-2.0 | 자체 인증서 후보 · [metadata](https://crates.io/api/v1/crates/rcgen/0.14.10) |
| qrcode 0.14.1 | MIT OR Apache-2.0 | QR 행렬 생성만, image/svg 기본 feature 끔 · [metadata](https://crates.io/api/v1/crates/qrcode/0.14.1) |
| snow 0.10.0 | Apache-2.0 OR MIT | Noise 후보. [Cargo manifest](https://docs.rs/crate/snow/0.10.0/source/Cargo.toml), default-features 끄고 P256/ChaChaPoly/SHA256/getrandom resolver만 검토 |
| p256 0.13.2 | Apache-2.0 OR MIT | DH 후보 · [metadata](https://crates.io/api/v1/crates/p256/0.13.2) |
| chacha20poly1305 0.10.1 | Apache-2.0 OR MIT | AEAD 후보 · [metadata](https://crates.io/api/v1/crates/chacha20poly1305/0.10.1) |
| curve25519-dalek 4.1.3 / x25519-dalek 2.0.1 | BSD-3-Clause | **제외**. [curve metadata](https://crates.io/api/v1/crates/curve25519-dalek/4.1.3), [x25519 metadata](https://crates.io/api/v1/crates/x25519-dalek/2.0.1); snow 기본 25519 경로를 그대로 채택할 수 없음 |

snow의 [의존성 메타데이터](https://crates.io/api/v1/crates/snow/0.10.0/dependencies)에서 curve25519는 optional normal, x25519는 upstream dev dependency다. 제품 default feature를 끄는 것과 upstream 전체 테스트의 라이선스 적합성은 별개다. P256 대안도 승인 완료가 아니다: elliptic-curve/crypto-bigint·digest/sha2·AEAD·getrandom의 target별 OS/WASI·build/dev 도구 및 JNI/AndroidX/Gradle의 실제 고정 closure와 원문을 검사한다. 전이 항목 하나라도 Apache/MIT/ISC로 충족하지 못하면 해당 조합/출시를 막는다. 자체 암호 작성이나 조용한 BSD 예외로 우회하지 않는다. SDK/JDK/NDK는 외부 빌드 도구로 별도 사용 조건을 기록하며 APK 번들 runtime/native 파일은 엄격 정책 대상이다.

TLS provider의 C/assembly 포함 전이 라이선스는 특히 별도 gate다(ring/aws-lc의 crate SPDX만 보고 전체를 허용하지 않음). 엄격 정책에 맞는 feature/버전 closure를 확보하지 못하면 모바일 의존성 추가·출시를 막고 대안을 검토한다. Windows·Linux target 및 모바일 래퍼/SBOM을 각각 확인하고 라이선스 원문·고지를 보관한다. 지금은 의존성을 추가하지 않으므로 cargo-deny 실행 결과를 주장하지 않는다. UI 자산은 자체 작성하며 다른 앱 내부 구현·코드·에셋을 참고하지 않는다.

## 6. 작은 병합 단계와 이 PC에서의 검증

각 행은 독립 커밋/PR 크기로 더 나눌 수 있다. 이후 구현자가 실행할 검증 계획이며 **이번 설계 작성에서 수행한 테스트가 아니다**. 기본 테스트는 고유 pipe·임시 state/worktree·가짜 인증/시계로 사용자 데이터와 격리한다.

| 단계 | 결과물 / 선행 | 테스트·단일 Windows PC 검증 / 완료 기준 |
|---|---|---|
| R1 **완료** | File에 묶인 client를 duplex transport로 분리, v11 유지 | 실제 named pipe 회귀+메모리/자식 stdio test double; 분할 JSON·EOF·큰 줄·attach 전환. 기존 CLI 결과 동일. → `podd/src/transport.rs`, `tests/remote.rs` |
| R2 **완료** | v12 hello·machine/boot·한도·remote-stdio | 같은 PC의 격리 podd에 브리지 두 개 연결. v11 거절/v12 성공/미래 비호환, 중복 시작, stdout 오염, 브리지 종료 후 holder 생존. → `pod-cli/src/remote_stdio.rs`, `pod-cli/tests/remote_stdio.rs`. 남은 것: 출력 chunk는 아직 holder 읽기 단위 64 KiB(32 KiB·snapshot chunk는 R4), Linux UDS 쪽 브리지는 R6a |
| R3 **전송·CLI 완료** | 시스템 SSH 프로필·오류 UI | fake ssh 실행 파일로 argv·배너·키 오류·시간 초과 검사. 실제 SSH는 **OpenSSH Server가 이미 설치되고 실행 중이며 localhost 인증도 구성된 경우에만** localhost 사용. 아니면 test double로 대체·실제 SSH 미검증 표기. → `crates/pod-remote`, `pod machine`, `pod --machine`, `pod-cli/tests/remote_ssh.rs`(가짜 ssh `pod-fake-ssh`), `pod-remote/tests/system_ssh.rs`(실제 ssh.exe의 `-G` 설정 해석·닫힌 포트). 2026-11-10 이 PC에는 sshd가 없어 **실제 SSH 로그인 미검증**. 남은 것: 앱 UI(오류 표시·프로필 편집), 원격 attach·재접속(R4) |
| R4 | 머신별 보드·터미널·재접속 | loopback 프록시에 RTT 20/100/300 ms·끊김·throttle 주입. 4개 attach 폭주에서 큐 상한 유지, 제어 RPC p95가 설정 RTT+250 ms 이내 목표; 60초 단절 뒤 상태 수렴·입력 중복 0, snapshot gap 표시 |
| R5 | 원격 파일·diff adapter | 로컬 격리 worktree를 원격으로 취급. 한글/공백/CRLF/BOM, 동시 수정 conflict, symlink/reparse fixture·ADS·경로 탈출, 큰 파일·stale diff, 끊김 뒤 버퍼 보존. 저장 재시도 1회만 반영 |
| R6a | Linux 경로/UDS/PTY·holder 이식 | Windows에서 Linux 경로 fixture·가짜 프로세스 backend 검사. CI 또는 **이미 설치된** WSL이 있으면 Linux build/test·daemon kill 후 holder 재attach·프로세스 그룹 종료 확인. 없으면 미검증 명시, Linux 지원 출시 보류 |
| R6b | WSL 탐지·정식 연결 | 설치 없음/빈 배포판/지역화/timeout fixture, 설치 명령 호출 0 확인. 기존 배포판에서만 사용자가 선택한 distro로 연결, wslpath 왕복·Linux CLI scope 검증 |
| M1 **완료** | mobile 정책·Gate 원자 결정, listener off | fake clock 만료 경계, PC/mobile 경합 한 건, 철회 경합·저장 실패 롤백·감사 복구, 범위 밖/임의 RPC 거부. 메시지(2026-11-10 범위): 길이·제어 문자·권한·한도·작업 없음·working 큐/blocked 거절, 리뷰 전달 경로·제출 확인. → `podd/src/server/mobile/`, `tests/mobile.rs`, `pod mobile`. 남은 것: 실제 페어링·세션 토큰(M2/M4), 감사의 30일 보존은 크기 상한(약 2 MiB)으로 대신, Gate origin(local-cli/mod) 메타데이터는 아직 없음 |
| M2 | TLS·QR·로컬 자산 UI·license gate | loopback double로 pin mismatch·만료·위조 인증서·QR 재사용·무차별 대입·CSRF/XSS·큰 출력·세션 만료·외부 navigation 차단. 실제 target dependency closure 검토 |
| M3 | Android 래퍼·LAN 첫 출시 | 기존 emulator만 사용, 없으면 desktop double. APK 자산/bridge·JNI·서명 업데이트 검증. 실기기 QR·잠금/복귀·기기 철회·핀 오류·LAN 승인 통과 전 Android 출시 보류 |
| M4 | Noise 코어·QR v1·키/epoch·challenge | Windows loopback 두 종단과 알려진 벡터, 잘못된 신원/프로파일·MITM·nonce/replay/역순·키 회전·다중 기기·restart/철회·expiry·operation 응답 유실. FS 키 폐기 경로 검토; 독립 보안 검토 통과 |
| M5 | 최소 relay·인터넷 전송 | 서비스 설치 없이 사용자 프로세스 loopback relay double: 암호문 변조/지연/중복/단절·큰 frame·느린 수신·quota·메모리 상한·본문 로그 없음·route credential 철회. VPN 대체안 주소/peer allowlist fixture; 실제 인터넷 NAT 검증으로 간주하지 않음 |
| M6 | 인터넷 출시 검증·운영/배포 | 실제 폰+사용자 준비 relay로 서로 다른 Wi-Fi/이동통신·Doze·네트워크 전환·PC 절전·재시작·키 회전·철회·배터리·서명 APK 업데이트 확인. 운영 예산/로그 정책 문서화, 미검증이면 인터넷 완료 표시 금지 |

시험 도중 OpenSSH Server 설치/서비스 시작·활성화, WSL 설치, 방화벽 변경은 하지 않는다. loopback double 통과는 실제 SSH 서버·Linux·모바일 검증의 대체 증거가 아니며 각 미검증 항목을 릴리스 노트에 남긴다. 기존 테스트는 관련 crate의 cargo test, 새 feature/target별 cargo check·clippy·엄격 license 검사로 확인한다.

## 7. v1 제외와 주요 위험

첫 LAN 단계는 인터넷 미완료다. 인터넷 단계에서도 제외: 프로젝트 운영 공개 relay 서비스, podd 직접 인터넷 포트 공개, 임의 셸 승인 입력, 다중 사용자 권한/조직 SSO, 비밀번호 로그인, SSH agent forwarding·ProxyJump·자동 원격 설치/업그레이드, 머신 간 fan-out/자동 merge, 원격 LSP·포트 포워딩, 파일 동기화·대용량/바이너리 편집, 오프라인 승인, 모바일 확장·푸시·임의 명령. Windows PC 절전·원격 재부팅·WSL 종료를 견디는 실행 보장은 없다.

가장 큰 위험은 Linux가 단순 전송 추가가 아니라 ConPTY/Job Object·경로·환경까지 이식하는 일이라는 점, 네트워크 단절에서 변경 RPC 결과가 불명확해지는 점, 릴레이 가용성·메타데이터 노출과 Android 배포/키 관리 부담이다. 각각 Linux 출시 조건 분리, 자동 변경 재전송 금지/CAS, §0 사용자 결정으로 관리한다. 엄격 라이선스 조건과 TLS provider의 전이 의존성이 충돌할 수 있으며 예외를 조용히 추가하지 않는다. 수치 한도와 성능 목표는 초깃값으로 측정 후 조정하되 인증·권한·감사 실패 시 거부 원칙은 유지한다.

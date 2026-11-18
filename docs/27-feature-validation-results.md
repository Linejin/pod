# 기능 검증과 수정 실행 결과

실행: 2026-11-18-01 · 상태: 실행 중 · 기준: improvements/stability-foundation의 fd9a1f04f66864cdeadfe41bc703ad3a02cf5792

[기능 목록과 검증 계획](26-feature-validation.md)의 91개 기능을 대상으로 자동 검사, 실제 UI와 도구 검사를 실행한다. 기능별 결과는 [결과 레코드](validation/2026-11-18/results.json)에 저장하고, 재현된 결함은 담당 worker branch에서 수정한 뒤 통합 브랜치에서 검토한다.

## 환경과 실행 규칙

- Windows 11 Home, OS build 10.0.26200, x86_64-pc-windows-msvc.
- rustc 1.100.0 (0ef1bc386), cargo 1.100.0 (3acc79c0c). 이번 실행에서는 Rust 버전 조회가 성공했다.
- 최초 여유 메모리 약 13.6 GiB. 세 영역의 분석은 병렬로 수행하되 빌드·시험은 공통 named mutex로 직렬화하고 cargo jobs=2를 사용한다.
- 각 worker는 같은 기준 commit에서 별도 branch/worktree를 사용하고 자체 target을 쓴다. 사용자 daemon·작업·agent 설정을 시험 fixture로 사용하지 않는다.
- 원격 push·PR·릴리스·main/master 변경은 하지 않는다. 실제 외부 쓰기·설치는 별도 지정된 시험 환경이 있을 때만 실행 범위에 포함한다.
- 로그와 command별 JSON은 통합 worktree의 target/feature-validation/20261118-085848와 worker 보고 디렉터리에 보존한다. 실행 레코드에는 실제 증거 경로·exit code·소스 commit을 남긴다.
- 자동 suite의 PASS는 그 안의 실제 assertion 범위만 증명한다. 미실행 UI·실제 도구·설치 case를 함께 PASS로 바꾸지 않는다.

## 사전 검사

| 검사 | 결과 |
|---|---|
| check-workspace | PASS |
| check-docs | PASS |
| check-boundaries | PASS |
| check-vendor-notices | PASS |
| test-publish-public | PASS |
| 전체 포맷 검사 | v1 로그 보존. 실제 종료 코드 수집 결함으로 v2에서 재검증 예정 |
| 서명 경로 검증 스크립트 | 최초 FAIL: 번들 ConPTY 미준비. 고정 SHA-256 패키지 준비 후 v2 실행 PASS. 실제 Pod 서명 증명과 별개 |

### 실행 기록 도구 수정

초기 임시 executor가 자식 명령 실패에도 exit code 0과 PASS를 남기는 문제가 있었다. 실제 core 시험 로그와 기록이 불일치하여 발견했고, exit 7 자식 프로세스로 오판을 재현했다. 기존 v1 기록은 삭제하지 않으며 PASS 판정의 근거로 사용하지 않는다. 앞서 안내한 사전 검사 7개 통과도 이 수정 결과로 정정한다.

[공통 실행기](../tools/qa/invoke-check.ps1)는 Process.ExitCode를 직접 읽고 raw stdout/stderr, 인자 배열, 실행기 hash, 실행 전후 소스 fingerprint를 기록하도록 수정했다. 실행 중 소스가 바뀌면 BLOCKED로 남긴다. [자체 회귀 검사](../tools/qa/test-check.ps1)에서 exit 0/PASS, exit 7/FAIL, 한글·공백·따옴표·끝 역슬래시 인자, 두 출력 스트림, 실행 중 소스 변경 감지를 통과했다.

정책 검사 5개는 v2로 재실행하여 실제 종료 코드 0을 확인했다. 서명 검사는 원래 로그에도 ConPTY 미준비 오류가 있었으므로 최초 실패를 유지한다. tools/fetch-conpty.ps1의 고정 버전·SHA-256 검증을 통과한 뒤 서명 검사를 다시 실행해 통과했다. 모든 결과가 같아질 때까지 반복한 것이 아니라 도구 결함과 명시적인 누락 선행 조건을 해결한 재검증이다.

제품 검증 중간 결과: core worker가 B20 청크별 턴 누락과 B10 index/read_dir 오류·기존 Run 디렉터리 ID 보호 실패를 재현하여 수정 중이다. 앱 worker의 실제 Windows input desktop은 접근 가능하지만 rust-analyzer는 설치된 구성요소가 없는 rustup proxy여서 해당 실제 도구 case는 BLOCKED다.

## 병렬 검증과 수정 배치

| 영역 | 담당 범위 | 상태 |
|---|---|---|
| core-status | 상태·영속 저장·프로토콜·client·모드·발견·forge 라이브러리, STATE04/B20와 관련 결함 | Astra 실행 중 |
| app-editor | 앱·터미널 뷰·편집기·LSP·리뷰 UI, 실제 native 검사와 B06–B09 | Astra 실행 중 |
| daemon-cli | daemon/CLI 전체 도메인과 원격·모바일·권한·프로세스 수명 통합 검사 | Astra 실행 중 |

Run은 run_b2d83d8d7f6a이며, 세 worker 모두 d9a230d6541a02ac638b0d269dd7225a890acad8에서 gpt-6-astra/high로 시작했다.

| 영역 | branch / worktree | Task / Dispatch |
|---|---|---|
| core-status | Linejin/pod-qa-core-20261118 · C:/Users/Sejin/orca/workspaces/pod/pod-qa-core-20261118 | task_6dba5c7fe1ce / ctx_0e30d9f36c3a |
| app-editor | Linejin/pod-qa-ui-20261118 · C:/Users/Sejin/orca/workspaces/pod/pod-qa-ui-20261118 | task_8b1fb33ed1e1 / ctx_e9c80f5dca26 |
| daemon-cli | Linejin/pod-qa-daemon-20261118 · C:/Users/Sejin/orca/workspaces/pod/pod-qa-daemon-20261118 | task_6448805f10b5 / ctx_0ba5bb1420e4 |

완료 보고의 소스·명령·로그·회귀 범위를 검토한 뒤 commit과 기능 case 결과를 여기에 갱신한다. 모든 worker가 끝나면 결합 소스 검증과 남은 BLOCKED/NOT_RUN 범위를 확인한다.

### B11 의존 계약 조정

데몬 worker는 완료된 check/review의 durable 재조정과 수신 Run별 재시도를 맡는다. core worker는 send_link_once로 Pod 출처·Status 종류·대상 Run·source event에 묶인 idempotent mail을 제공한다. 기존 send는 partial/complete append 후 오류에서 메모리에 반영되지 않을 수 있으므로, journal 재읽기·복구 또는 후속 쓰기 fence 없이는 단순 mail_of 중복 검사만으로 보장하지 않는다. 구현과 결합 회귀는 진행 중이다.

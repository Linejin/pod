# 기능 검증과 수정 실행 결과

브랜치 `improvements/stability-foundation` · 최초 감사 `2026-11-18-01`의 통합 소스 `2405f2f58f3cbedb29b35c4a421b9f02c7a550ab` · 최근 기록 2026-11-24

[기능 목록과 검증 계획](26-feature-validation.md)의 91개 기능을 기준으로 자동 검사, 실제 UI, 도구 및 패키징 경로를 검증했다. 재현된 결함은 전용 worker branch에서 수정하고 통합 branch에서 확인했다. 당시 기능별 판정과 범위 제한은 [11월 18일 기록](validation/2026-11-18/results.json)에 보존한다.

## 2026-11-24 추가 검증

[추가 검증 기록](validation/2026-11-24/results.json)에 이번 실행의 소스와 증거 경로를 기록했다. 아래 91개 기능 집계는 11월 18일 감사 결과다.

| 기능 | 통과한 검증 | 남은 범위 |
|---|---|---|
| EDIT07 | 실제 Windows 앱에서 열린 파일과 부모 폴더의 이름을 바꿨다. 설치된 rust-analyzer의 이전 URI 닫기와 새 URI 열기·자동 완성·저장, 수정 중인 파일의 전체 저장과 옛 이름 재사용을 확인했다. URI 검증 18개 통과. | 배경에 수정 중인 여러 파일을 둔 조합, 대소문자만 바꾸는 이름 변경, 유니코드 이름과 동시 변경 및 충돌 복구. |
| UI03 | 앱을 정상 종료하고 같은 daemon에 다시 연결했다. 터미널·편집기·설정·상태 패널, 활성 탭과 커서가 복원되고 터미널과 프로세스가 중복되지 않았다. 레이아웃 검증 7개 통과. | 여러 터미널, 다른 도크 조합 및 텍스트 범위 선택 복원. |
| TASK06 | sparse checkout 명령에 실패를 주입했다. 새 Task·worktree·브랜치·터미널이 정리됐고, 동일 이름으로 다시 생성한 뒤 기존 Task의 sparse 설정·수정 파일·프로세스가 유지됐다. | 기존 브랜치를 재사용하는 실패, 디스크·권한 오류와 롤백 자체의 실패. |
| TERM04 | 두 클라이언트가 같은 터미널의 출력 40줄을 한 번씩 순서대로 받고, 하나를 분리해도 다른 쪽 입력과 원래 프로세스가 유지됐다. | 실제 앱에서 두 뷰를 조작하고 하나를 닫는 검증. |
| TERM09 | 격리된 daemon의 정상 중지와 강제 종료 뒤 원래 shell·holder PID, 중지 중 발생한 출력, 재시작 후 입력을 확인했다. | 출력 중 앱 종료와 복원, 실제 에이전트 및 원격 세션. |

native 검증은 `24c4396` 기준으로 제품 코드가 같은 `2405f2f` 빌드를 사용했다. 로컬 daemon worker의 관련 테스트 20개가 통과했다. 새 회귀 테스트 3개를 반영하고 임시 폴더와 파이프 이름을 병렬 실행에 맞게 고친 뒤, 통합 `0d59615`에서 `cargo test --locked -p podd --test local_validation -- --nocapture`가 기본 병렬 설정으로 통과했다. 재실행한 3개는 worker의 20개에 다시 합산하지 않는다.

`0b4c81a`에서 `url`을 2.5.9로, 필요한 `form_urlencoded`를 1.2.3으로 업데이트했다. worker의 잠금파일 확인과 앱·편집기 컴파일 검사가 통과했다. 통합 `0d59615`의 배포 정책 재검사에서 `RUSTSEC-2026-0592`가 사라졌다. `chrono 0.4.45`의 `RUSTSEC-2026-0543`으로 검사 종료 코드는 1이었다. 새 설치 EXE와 포터블 ZIP은 생성하지 않았다.

## 2026-11-24 후속 기능 검증

| ID | 확인한 소스와 결과 | 남은 범위 |
|---|---|---|
| GRAPH01 | 통합 `b76298e`의 Windows 앱에서 17개 시나리오, 검증 항목 53개 통과. 작업대와 고밀도 + 세부 열의 좁은 행에서 버튼과 노드를 선택했다. 저장소 전환 후 즉시 정상 종료해도 같은 패널·저장소가 복원되고 터미널과 미저장 확인이 유지됐다. | 집중 레일 조작, 다른 배율과 종료·복구 조합은 해당 검증 범위 밖이다. |
| A2A01 | 통합 `afb7f9d`에서 코어 42개와 CLI 8개 통과. 워커가 공식 a2a-python SDK 1.3.1의 loopback HTTP와 테스트 CA를 사용한 TLS에서 27단계와 wire 검증 11개를 통과했다. | 외부 서비스·공인 CA 상호운용은 미검증이다. [지원하는 표준 범위](a2a-client.md#what-is-supported) |
| A2A02 | 워커 `639f294`의 A2A 섹션 검사 21개와 공식 SDK 루프백 UI 시나리오 통과. 결합 소스에서도 SDK 연동을 재실행했다. | 실제 Windows 창의 입력·표시 검증이 남아 있다. [외부 피어 사용법](09-pod-app.md#외부-a2a-피어-project_a2ars-2026-11-24) |
| USAGE01 | 통합 `3f961aa`의 실제 Windows 앱에서 작업 영역 1,440×760 및 822×760, 96 DPI로 7개 레이아웃과 3개 테마의 상태 표시줄·세부 정보와 격리된 Claude 수집기·캐시 흐름을 검사했다. 검증 항목 80개 통과. | 해당 UI 검증은 격리된 보고값을 사용했다. 실제 Claude Code의 한도 보고와 같은 앱에서의 실시간 Codex 조회는 미실행이다. [사용량 상세 검증](#2026-11-24-사용량-모니터-windows-검증) |

원본 보고서는 로컬 `coordination-validation/reports/2026-11-24/`의 `project-agent-graph-native-fixed/report.md`, `a2a-standard-client/report.txt`, `a2a-integration/`, `ai-usage-monitor/`에 있다.

## 2026-11-24 사용량 모니터 Windows 검증

- `3f961aa`에서 `cargo test --locked -p pod-app usage:: -- --test-threads=1`은 24개 통과, 3개 기본 제외였다. `cargo test --locked -p pod-hook --test statusline -- --test-threads=1`은 7개 통과했다.
- 실제 앱에서 좁은 상태 표시줄의 남은 비율·초기화 시간, 키보드로 연 세부 정보와 접근성 이름, 다음 보고 반영, 오래된 값, 초기화 예정 시각 경과, 프로필 분리, 사용량 소진 표시를 확인했다. 로그와 화면은 로컬 `coordination-validation/reports/2026-11-24/ai-usage-monitor-native/REPORT.md`에 있다.
- 별도 워커 `7613ba4`에서는 Git Bash·Windows PowerShell 5.1의 Claude 수집 명령 5개를 확인했다. `85b5370`에서는 로그인된 Codex의 읽기 전용 한도 조회가 성공했다. [연결 방법과 fixture 범위](usage/ai-usage-monitor.md)

## 2026-11-24 편집기 브라우저 검증

`BROWSER01`은 `3f961aa`에서 분기한 `improvements/editor-browser`에서 확인했다. [검증 절차](26-feature-validation.md#편집기-브라우저-검증)

- Windows x64 빌드 후 실제 앱의 명령 팔레트에서 브라우저 탭을 열었다. WebView2 Runtime `157.0.4341.60`에서 로컬 HTTP 페이지 두 개의 콘텐츠, 뒤로·앞으로 이동과 새로고침 요청을 확인했다. 일반 창의 폭을 1,233px에서 1,433px로 늘리면 웹뷰 폭도 837px에서 1,037px로 늘어났다. 명령 팔레트가 열릴 때 웹뷰가 숨고 닫히면 같은 페이지가 다시 보였다.
- `cargo test --locked -p pod-app -j 4 -- --test-threads=4`에서 481개 통과, 5개 기본 제외였다. 새 테스트는 HTTP·HTTPS 주소 판정, 125% 좌표, 접근성 탭 이름, 크기 변경, 동일 탭 재사용, 다른 탭 및 대화상자에서 웹뷰 표면 숨김과 복원을 확인한다.
- 설치된 WebView2로 실행하는 기본 제외 테스트 `webview2_navigates_loopback_pages_and_reloads`를 별도로 세 번 연속 통과했다. 실제 창의 배율 변경 조합과 런타임 미설치 환경은 남은 검증 범위다.

## 2026-11-24 결합 검증

`improvements/combined-validation`의 제품 코드 `395882f`를 대상으로 했다. 이 브랜치의 앱은 편집기 브라우저, 외부 A2A 피어, AI 사용량 항목을 함께 포함한다.

- `cargo test --locked -p pod-app -j 3 -- --test-threads=4`에서 502개 통과, 5개 기본 제외였다. `cargo clippy --locked -p pod-app --all-targets -- -D warnings`와 문서·워크스페이스·크레이트 경계·vendor 고지 검사도 통과했다.
- 로컬 공식 a2a-python SDK 1.3.1을 연결해 프로젝트 그래프의 선택·검토·전송, 작업 상태 확인, 이어 보내기와 직접 답장 표시를 헤드리스 UI로 재검증했다. wire 기록에서 `SendMessage` 6회, `GetTask` 5회와 `A2A-Version: 1.0`을 확인했다.
- Windows 개발 빌드(`pod-app`, `podd`, `pod-cli`, `pod-hook`)와 설치된 WebView2를 사용한 루프백 탐색·새로고침 시험도 통과했다.

## 최종 기능 커버리지

11월 18일 감사의 엄격한 기능 절차 기준으로 **14 PASS · 21 BLOCKED · 56 NOT_RUN · 0 미해결 FAIL**이다. EDIT05와 EDIT06은 읽기 전용 하위 사례가 통과했고, 전체 기능 판정은 NOT_RUN이다.

| 판정 | 기능 ID | 설명 |
|---|---|---|
| PASS (14) | SYS06, SYS07, SYS08, TASK04, STATE04, UI05, REVIEW03, ORCH03, ORCH10, ORCH11, ORCH12, REMOTE02, MOBILE01, MOBILE02 | 정의된 기능 절차 범위에서 완료 |
| BLOCKED (21) | SYS01–SYS05, TASK03, TASK08, STATE03, REVIEW04, GROUP01, GATE02, ORCH02, ORCH05, ORCH08, PR02, PR03, DISC03, REMOTE01, REMOTE03, REMOTE04, REMOTE05 | 별도 설치/SSH/GitHub 환경, 허용 정책 또는 권한이 없어 핵심 경로를 증명하지 못함 |
| NOT_RUN (56) | 위 두 행 이외의 기능 | 자동 assertion 일부가 있어도 전체 문서 절차나 native 조합이 미완료 |

커버리지 감사는 947개 고유 시나리오와 1,248개 기능 연결을 정리했고, 근거가 약한 worker 연결 288개는 제외했다. 전체 목록·누락 절차·상태 근거는 ledger를 기준으로 한다. `MOBILE01/02` PASS는 문서에 적힌 로컬 M1 API 범위이며 phone/QR/listener UI를 구현하거나 검증했다는 뜻이 아니다.

## 통합·자동 검사 최종 상태

이 절은 11월 18일 감사의 실행 환경과 범위를 기록한다. 이후 실행 결과는 위의 날짜별 절을 참고한다.

기존 전체 자동 profile의 합성 결과는 **1,451 PASS · 0 FAIL · 4 ignored · 1 filtered**다. 이 수치는 `02177f7`에서 다시 실행한 25개와 바뀌지 않은 `779456e` 단계에서 이월한 1,426개를 합친 것이며, 단일 명령으로 최종 `2405f2f`에서 1,451개를 실행한 결과가 아니다. 초기 attach 검사의 마지막 줄 실패는 짧고 고정된 prompt와 결정적 raw replay를 추가한 tests-only 수정으로 해결했다. 과거 실패 로그는 남겨 두었다.

최종 `2405f2f` 통합 worktree에서 `cargo test -p pod-editor --test save_ui`와 `cargo build -p pod-app`가 통과했다. 읽기 전용 수정 worker는 관련 editor/app/toolkit 범위에서 557 PASS·2 ignored, app/editor와 toolkit Clippy, vendor notice/provenance 확인을 완료했다. 통합본의 관련 editor/app/vendor 경로가 worker 검증 트리와 같음을 확인했다. 전체 자동 profile의 기존 수치와 최신 focused 확인은 구분해 해석한다.

실제 Windows native UIA 확인은 새 앱 SHA-256 `28A0DCE852A785AA4D85A42BB6DA190586C26A78A905657DCE21BFAFFEB47680`에서 6개 시나리오를 통과했다. 3 MiB와 디코딩 불가 파일은 두 UIA `IsReadOnly` 조회가 모두 true였고, UIA 쓰기·실제 입력이 내용을 바꾸지 않았으며 tab은 깨끗했고 disk byte가 보존됐다. 안전한 작은 파일로 reload하면 두 flag가 false로 바뀌고 UIA/실제 편집과 저장이 가능했다. 자세한 stage 기록은 아래의 최종 native 검증 절에 정리한다.

별도 native 검증에서는 설정 진입 세 경로와 font/theme/display/behavior restart 저장, 리뷰 초안 이동과 daemon 저장 응답 대기 중 새 입력 보존도 확인했다. 한글 IME 선언 범위, rust-analyzer smoke/code-action 2개, UIA scene 13개, 7개 레이아웃의 dark/light 화면 28장 검토 등 이전 증거는 해당 절의 제한과 함께 유지한다. Narrator, 높은 DPI 전체 조합, 전체 글꼴 후보/옵션, 실제 native LSP rename URI는 완료로 계산하지 않는다.

실제 Codex와 Claude 수정 요청은 유효한 격리 fixture에서도 현재 정책상 거부되어 BLOCKED다. GitHub는 read-only 조회만 통과했고 fixture에 PR/check가 없어 write 경로를 증명하지 않았다. authorized SSH host, disposable install user/VM, phone UI fixture도 없어 각각 BLOCKED 또는 NOT_RUN다. 실행 중 Claude 설정 파일의 metadata 변화는 관찰했으나 원인과 내용은 확인하지 않았다.

패키지 gate는 최종 `2405f2f`에서 다시 실행했고 `cargo-deny`가 exit 1을 반환했다. `chrono 0.4.45`의 `RUSTSEC-2026-0543` unmaintained advisory가 원인이며 bans/licenses/sources 검사는 통과했다. 승인된 예외나 정책 완화는 하지 않았다. release compile, 새 EXE/ZIP, extraction 및 launch smoke는 실행되지 않았다. REMOTE05의 native file/directory symlink 생성은 Win32 1314 `ERROR_PRIVILEGE_NOT_HELD`로 막혔다. 필요한 권한·Developer Mode 변경 없이 남은 symlink boundary assertions는 BLOCKED다. 이 범위 제한은 해결된 제품 오류와 분리해 기록한다.

모든 결과·case 이력·missing matrix·명령과 증거 경로는 ledger와 `C:/Users/Sejin/orca/workspaces/pod/coordination-validation/reports/` 아래 보고서에 보존한다. 당시 원래 프로젝트의 main commit은 `e3b4db818afd62b7ad9eff3fd8ae06166d15f51b`이었다. 아래 세부 실행 기록은 11월 18일 실행 이력이다.

## 최종 native 데이터·접근성 결과

| 기능 | 11월 18일 최종 결과 | 범위 및 남은 부분 |
|---|---|---|
| UI05 | 설정 창 세 경로와 font/display/behavior 값의 재시작 보존 PASS | 96 DPI fixture 범위. 모든 후보 값, 고 DPI, Narrator, 저장 실패 주입은 미실행 |
| EDIT05 | save-all byte/인코딩 사례와 마지막 3 MiB·undecodable 읽기 전용 UIA 시나리오 PASS | 최종 UIA 수정 포함 source `2405f2f`. 전체 feature procedure의 모든 저장·거부 matrix를 전부 실행한 것은 아니므로 overall `NOT_RUN` |
| EDIT06 | external conflict/reload/diff·overwrite와 read-only 재로드 UIA 사례 PASS | 실제 권한/ACL 거부 및 고급 provider/LSP 경계 조합이 남아 overall `NOT_RUN` |
| EDIT07 | dirty file/parent rename, background save-all, 옛 경로 재사용, saved tab close PASS | 당시 실제 rust-analyzer URI 변경은 미실행이었다. 11월 24일 [추가 검증](#2026-11-24-추가-검증)에서 새 URI 열기·자동 완성·저장을 확인했다. |
| REVIEW03 | UIA 초안 A/B 이동과 actual isolated daemon 응답 지연 중 새 입력 보존 PASS | agent 전송은 하지 않음. restart draft recovery는 별도 범위 |
| REMOTE05 | junction/hardlink 경로 검사 1건과 loopback remote-files 4건 PASS | native file/directory symlink 생성이 Win32 1314로 거부되어 핵심 경계 검증 BLOCKED |

읽기 전용 결함은 두 단계로 수정했다. 통합 `c7a5f50`의 `EditorWorkspace`가 실제 렌더링 `Editor`에도 readonly 정책을 넘기게 했고, 통합 `2405f2f`에는 `gpui-base 0.9.0`/`gpui-pre 0.3.11`의 저장소 관리형 패치를 더해 기존 AccessKit 입력 노드가 현재 readonly 값을 내보내게 했다. 후속 reload가 같은 node를 editable로 돌리는 것도 확인했다.

검증은 worker branch `9b3ca5e`에서 editor/app 557 tests PASS·2 ignored, app/editor와 toolkit Clippy, vendor notice/provenance PASS였고 관련 source subtree가 통합본과 같음을 확인했다. 통합 `2405f2f`에서는 `cargo test -p pod-editor --test save_ui`와 `cargo build -p pod-app`를 각각 재실행해 통과했다. 실제 UIA 재검증은 `pod-app.exe` SHA-256 `28A0DCE852A785AA4D85A42BB6DA190586C26A78A905657DCE21BFAFFEB47680`에서 6개 scenario·9개 case mapping이 모두 PASS였다. 초기/재로드 대용량 및 undecodable buffer의 두 `IsReadOnly` 조회, UIA SetValue 거부, native input 차단, clean tab, exact disk bytes와 안전한 작은 파일 복귀를 검사했다.

증거는 `C:/Users/Sejin/orca/workspaces/pod/coordination-validation/reports/native-data-followup/report.md`, `uia-readonly-fix/report.md`, `native-uia-readonly-final/report.md`에 있고, final UIA report 아래 case assertions·input log·actual/expected byte artifacts·hashes·cleanup audit가 보존돼 있다. 통합 editor suite와 app build 로그는 `reports/uia-readonly-fix/integration/`에 보존한다. 역사적 UIA FAIL은 지우지 않고 final cases에 별도 PASS phase를 추가했다.

최종 integration HEAD `2405f2f`의 `cargo-deny --log-level error check`는 FAIL이다. locked `chrono 0.4.45`에 대한 `RUSTSEC-2026-0543` unmaintained advisory가 mandatory policy를 막고, bans/licenses/sources는 통과했다. 이 gate 이후 release compile은 시작하지 않았으며 새 installer/portable ZIP은 없다. 현재 HEAD의 정확한 결과는 `C:/Users/Sejin/orca/workspaces/pod/coordination-validation/reports/package-followup/current-integration/cargo-deny-current-integration-20261118-145857-707.log`에 있다. 기존 package follow-up report에는 더 이른 source의 각 시도와 no-artifact audit가 기록돼 있다.

## 실행 환경과 당시 진행 기록

아래 실행 메모는 단계별로 쓴 이력이라 당시의 `PENDING`/`진행 중` 상태를 그대로 포함한다. 현재 결과는 이 문서의 최종 요약과 ledger를 우선한다.

- Windows 11 Home, OS build 10.0.26200, x86_64-pc-windows-msvc.
- rustc 1.100.0 (0ef1bc386), cargo 1.100.0 (3acc79c0c). 이번 실행에서는 Rust 버전 조회가 성공했다.
- 최초 여유 메모리 약 13.6 GiB. 기본 빌드·시험은 공통 named mutex와 cargo jobs=2로 직렬화한다. 긴 UI 최초 빌드가 core API 검증을 막아 09:22에 여유 메모리 10.89 GiB·CPU 6%를 확인한 뒤, core worker의 짧은 의존 검사만 jobs=1의 별도 core-dependency 잠금에 허용했다. 해당 경로는 여유 8 GiB 미만·CPU 80% 초과 또는 jobs=1 미설정 시 시작하지 않는다. 다른 worker와 광범위한 빌드는 기본 잠금을 유지한다.
- 각 worker는 같은 기준 commit에서 별도 branch/worktree를 사용하고 자체 target을 쓴다. 사용자 daemon·작업을 시험 fixture로 사용하지 않는다. 실제 agent 실행만 기존 인증 경로를 허용한 예외 범위이며, 설정 변경·credential 복사와 구분하고 관찰한 메타데이터 변화도 기록한다.
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
| 전체 포맷 검사 | 초기 v1 기록은 무효로 보존. 최종 통합본의 v3 전체 검사 PASS |
| 서명 경로 검증 스크립트 | 최초 FAIL: 번들 ConPTY 미준비. 고정 SHA-256 패키지 준비 후 v2 실행 PASS. 실제 Pod 서명 증명과 별개 |

### 실행 기록 도구 수정

초기 임시 executor가 자식 명령 실패에도 exit code 0과 PASS를 남기는 문제가 있었다. 실제 core 시험 로그와 기록이 불일치하여 발견했고, exit 7 자식 프로세스로 오판을 재현했다. 기존 v1 기록은 삭제하지 않으며 PASS 판정의 근거로 사용하지 않는다. 앞서 안내한 사전 검사 7개 통과도 이 수정 결과로 정정한다.

[공통 실행기](../tools/qa/invoke-check.ps1)는 Process.ExitCode를 직접 읽고 raw stdout/stderr, 인자 배열, 실행기 hash, 실행 전후 소스 fingerprint를 기록하도록 수정했다. 실행 중 소스가 바뀌면 BLOCKED로 남긴다. [자체 회귀 검사](../tools/qa/test-check.ps1)에서 exit 0/PASS, exit 7/FAIL, 한글·공백·따옴표·끝 역슬래시 인자, 두 출력 스트림, 실행 중 소스 변경 감지를 통과했다.

정책 검사 5개는 v2로 재실행하여 실제 종료 코드 0을 확인했다. 서명 검사는 원래 로그에도 ConPTY 미준비 오류가 있었으므로 최초 실패를 유지한다. tools/fetch-conpty.ps1의 고정 버전·SHA-256 검증을 통과한 뒤 서명 검사를 다시 실행해 통과했다. 모든 결과가 같아질 때까지 반복한 것이 아니라 도구 결함과 명시적인 누락 선행 조건을 해결한 재검증이다.

제품 검증 중간 결과: B20·B10·B11 core 집중 검사 150개와 앱·편집기 4개 package의 640개 시험을 통과했다. B06–B09 baseline 실패와 수정 후 결과는 아래에 구분한다. Windows input desktop은 접근 가능하다. 처음 rust-analyzer 구성요소가 없어 BLOCKED였으나, 09:44 기존 stable 도구체인에 구성요소를 추가하여 실제 1.100.0 버전 조회가 성공했다. Rust compiler 버전은 그대로이며 실제 언어 서버 검사는 다시 진행한다.

편집기 baseline의 신규 회귀 3개도 모두 실패했다. B08 파일 증가 후 읽기 전용 정책 누락, 처음부터 큰 파일의 축소 후 읽기 전용 해제 누락, B09 Windows 공유 잠금에 따른 reload 실패 후 충돌 표시 유실이다. 해당 실행도 이미 시작된 v1 executor였으므로 잘못된 PASS/0 기록은 보존하되 무효로 표시했다. 원래 제품 코드는 바꾸지 않고 회귀를 추가한 baseline patch와 로그를 worker가 보존했으며, 수정 후 동결 소스에서 재검증한다.

실행기 v3는 실제 종료 코드 수집을 유지하면서 위 자원 제한 core-dependency 실행 경로를 추가했다. 정상 자원의 jobs=1 실행과 jobs=2 거부를 별도로 확인한다. 기존 v2 실행 결과의 종료 코드 근거는 유효하다.

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

데몬 worker는 완료된 check/review의 durable 재조정과 수신 Run별 재시도를 맡는다. core의 send_link_once는 Pod 출처·Status 종류·대상 Run·source event에 묶인 idempotent mail을 제공한다. 기존 send는 partial/complete append 후 오류에서 메모리에 반영되지 않을 수 있으므로 journal 재읽기·복구 또는 후속 쓰기 fence가 필요하다. core 구현은 집중 검사 후 통합했고, 데몬 구현·결합 회귀는 진행 중이다. 기존 marker 없는 알림은 원래 payload와 Pod 출처·Run을 대조하여 업그레이드 중복도 방지하도록 요구했다.

## 검토하고 통합한 core 수정

| 과제 | worker commit | 통합 commit | 검증 범위 |
|---|---|---|---|
| B10 | a13ed3709bc8c560d9580eeff449db044d66acb3 | eea94ca | read/read_dir·Windows 공유 위반·기존 ID 예약·복구 중 journal 쓰기 보호 5개 신규 회귀 |
| B11 core | b4ac5b12ad8872afc8894ba673e44b771819a1bd | 5a81a29 | 부분 append·완전 append 후 오류·재시작·읽음 유지·수신 Run·위조 marker·복구 불가 fence 5개 신규 회귀 |
| B20 | ab931609a22ff005a52a6c596a84b1111bc32c6a | d0f6e24 | 모든 청크 크기·분할 OSC·여러 턴·hook 우선순위·프로그램 교체 4개 신규 회귀 |

동결 소스에서 cargo test -p pod-status -p pod-orch --locked가 실제 exit 0, 150개 통과·실패/ignored 0이었다. 실행기의 전후 fingerprint가 일치했으며, 그 내용을 보존한 위 commit들과 통합 브랜치의 pod-orch/pod-status 트리가 같음을 확인했다. 기존 baseline 실패 로그는 유지한다.

이 결과는 두 core crate의 시험 범위이며 daemon source outbox, 전체 worker/앱 경로, 나머지 라이브러리, 실제 도구·설치 검증을 대신하지 않는다. 세 worker는 나머지 배치를 계속 진행한다.

## 앱과 편집기 수정 통합

worker commit 6ef0455a18930f06e0a7860a96495e85d55abdb4를 검토하여 cb1ec04로 통합했다. 위치별 리뷰 초안, 전송 원문과 현재 입력의 비교, reload 크기·강조·provider/LSP 정책, reload 실패 시 충돌 보존을 수정했다. 신규 회귀 7개와 기존 B01 인접 회귀가 포함된다.

- 집중 검사: review QA 7개, save_ui 8개, tabs_lsp 6개 통과.
- 동일하게 동결한 수정 소스에서 4개 package all-target Clippy가 -D warnings로 통과.
- 깨끗한 6ef0455 commit에서 pod-app/pod-editor/pod-terminal-view/pod-ui의 결합 검사 640개 통과·3개 ignored. 앱은 431개 모두 통과했다.
- ignored 3개는 실제 rust-analyzer 2개와 terminal-view doc 예제 1개다. rust-analyzer 설치 조건을 해결했으므로 2개는 명시적 실행으로 이어가고, doc 예제는 별도 생략 사유로 남긴다.
- 통합한 앱·편집기 파일이 검증된 commit과 같음을 확인했다. 전체 통합 브랜치의 새 core와 결합한 최종 검사는 별도로 수행한다.

## 번들 ConPTY 빌드 수정

처음 provider 회귀는 fetch가 성공했는데도 inbox를 선택하여 실패했다. 0d7f6de·e6bc59b에서 경로 회귀와 수정을 만들었고, f4c120f·2cddfc8로 통합했다. 이어 dd163b6에서 같은 길이의 다른 DLL과 잠긴 오래된 payload의 baseline 실패를 재현하고, 7603eeb에서 바이트 비교·원자적 교체·명시적 오류로 수정했다. 후속 통합 commit은 266776d·87cd4dc다.

최종 holder_contract는 두 Cargo 배치·같은 길이 교체·잠금 보존/재시도·동일 내용 잠금과 실제 bundled provider를 포함하여 7개 모두 통과했다. 검사 중 source fingerprint가 같았으며, daemon outbox 작업 diff가 포함된 실행이었다는 점을 보존한다. build helper·holder 시험 파일은 통합본과 같고 전체 결합 검증은 남아 있다. 잠긴 stale bundle을 조용히 최신으로 취급하지 않으므로 갱신이 불가능하면 빌드가 명시적으로 실패할 수 있으며, 실행 중인 holder를 강제로 종료하지 않는다. 설치 stage의 별도 vendor 복사는 이 개발 빌드 결함과 구분한다.

## 추가 profile 결과와 진행 중인 결합 실패

core worker의 깨끗한 ab931609 기준 CORE 157개, ORCH 64개, MODS 28개, DISCOVERY 36개가 통과했다. 앞선 집중 검사와 겹치는 시험이 있으므로 이를 전부 고유 시험 수로 합산하지 않는다. FORGE·owned clippy/fmt와 daemon·native/실제 도구 검사는 계속 진행한다.

B11의 원래 daemon baseline은 실제 exit 1로 재현했다. check는 Finished/all_passed=true였지만 mail 파일 OS32 잠금 해제 후 알림을 재시도하지 않았다. durable outbox draft와 core API를 연결한 다음 실행도 exit 1이었다. API preflight 읽기 실패가 startup과 같은 영구 fence를 설정하여 잠금 해제 후에도 재시도하지 못하는 새 결합 실패다. 성공한 check와 pending 알림은 보존됐으며, core는 startup 복구 보호와 일시적인 link-journal 복구 보호를 분리하는 보완을 진행한다. 원래 retry assertion은 완화하지 않는다.

이 결합 실패에 대한 보완 worker commit 661a9f0ff4bc8032628fdb906fddcf1e52c60d14를 검토하여 4ef5745로 통합했다. runtime의 pending journal과 시작 시 불완전 복구 fence를 분리하고, 검증된 Pod link 재시도만 pending journal을 재조정한다. storage를 읽지 못하는 동안 일반 쓰기는 계속 차단하며, startup fence는 전체 reload 전까지 유지한다. 새 probe 2개는 수정 전 실제 exit 101로 실패했고, 수정 후 집중 probe와 pod-orch 전체 63 unit·4 wire 검사가 통과했다. daemon의 원래 재시도 회귀는 보완 commit을 가져가 다시 실행하는 단계다.

## core-status 최종 인계 검토

core worker는 깨끗한 661a9f0에서 담당 9개 crate의 결합 검증을 완료했다. 총 321개 통과, 실패·ignored 0개이며, 같은 commit의 all-target Clippy(-D warnings)와 포맷 검사도 실제 exit 0이다. 실행기 v3의 전후 source fingerprint가 일치하고, 통합 브랜치의 해당 9개 crate 내용도 검증본과 동일하다.

| 범위 | 통과 수 |
|---|---:|
| CORE: client/process/protocol/status/storage | 157 |
| ORCH | 67 |
| MODS | 28 |
| DISCOVERY | 36 |
| FORGE | 33 |

근거는 coordination-validation/reports/core-status의 report.md, feature-results.json, test-inventory.json, binary-hashes.json과 commands/owned-tests-final-20261118-095232-971, owned-clippy-after-retry-20261118-095626-889, owned-fmt-after-retry-20261118-100033-957 기록이다. 기존 150개 집중 검증 및 318개 중간 profile과 겹치므로 합산하지 않는다. feature-results의 21개 라이브러리 assertion group을 중앙 결과에 추가하되, 21개 기능의 전체 사용자 흐름 통과로 판정하지 않는다. daemon B11 source 보존·legacy 중복 방지와 native UI·실제 도구 검증, 최종 결합 브랜치 검증은 남아 있다.

## daemon·CLI 중간 검증

CLI 전체 profile은 61개 통과, 실패·ignored 0개다. 실제 도구를 쓰는 codex_hooks_trusted_via_app_server_in_temp_home 1개를 명시적으로 제외했으므로 해당 기능까지 통과한 것은 아니다. daemon의 terminal/lifecycle 13개 target에서는 36개가 통과했고, ignored·filtered·skip 출력이나 fixture 프로세스 잔류 경고는 없었다. 이로써 daemon 37개 integration target 중 기존 contract·holder를 포함한 20개 target의 실행을 마쳤다.

근거는 daemon-cli/commands/cli-all-20261118-095243-658 및 daemon-terminal-20261118-100245-638-20261118-100245-664의 JSON·raw log다. 두 실행은 7603eeb와 아직 commit하지 않은 B11 outbox diff를 포함했으며 전후 fingerprint는 동일하다. 따라서 깨끗한 최종 commit이나 통합 브랜치 전체의 검증으로 해석하지 않는다. core 보완 661a9f0는 이 실행들이 끝난 뒤 daemon 브랜치에 a943764로 적용됐고, 변경하지 않은 원래 B11 재시도 회귀 검증을 이어간다.

B11의 변경하지 않은 원래 linked_check_mail_retries_after_save_failure_and_restart는 보완 적용 후 실제 exit 0으로 통과했다. daemon 라이브러리도 legacy markerless 중복 방지·위조 출처 거부·source 보존·부분 수신자 실패·재시작을 포함하여 182개 통과했다. 두 실행 모두 아직 commit 전 outbox diff를 포함하며 실행 중 fingerprint는 동일했다.

추가 waiter 회귀에서는 실제 exit 101, 182개 통과·1개 실패로 새 경계 문제가 재현됐다. journal에서 복구된 기존 mail은 fresh=false이므로 공개 이벤트 중복을 피하는 분기가 check --wait 대기자 알림까지 생략한다. 수정은 공개 이벤트를 재발행하거나 mail·읽음 표시를 바꾸지 않고 로컬 대기자가 복구된 미확인 mail을 다시 조회하게 하는 범위다. 근거는 daemon-cli/commands/b11-core-supplement-retry-20261118-100523-396, podd-lib-b11-20261118-100705-902, podd-lib-b11-waiter-baseline-20261118-101004-759의 JSON·raw log이며, 새 실패가 해소되고 결합 검증을 마칠 때까지 ORCH04 전체 판정은 FAIL을 유지한다.

waiter 수정 후 podd-lib-b11-waiter-fixed-20261118-101051-792는 실제 exit 0, 183개 통과·실패/ignored 0개다. 공개 이벤트 재발행 없이 로컬 Notify만 깨우며, 테스트가 중복 발행 0회도 확인한다. 검증은 동결된 diff에서 실행됐고, 이를 보존한 worker commit 72b9c54015e5e3764f8124b310e82293cd67f29b를 검토하여 a90b913으로 통합했다. 원래 재현 테스트 cf634616은 5b3109a로 통합했다. podd·pod-orch·pod-status의 통합본과 worker commit 내용은 동일하다. 나머지 17개 daemon integration target, Clippy·포맷 및 전체 결합 검증은 남아 있으므로 최종 통과 판정은 보류한다.

## 실제 Windows 설정·IME 중간 결과

설정 진입 경로를 분리한 native-settings-final-20261118-100321-972는 실제 exit 1이다. 96 DPI에서 Ctrl+,와 UIA Invoke는 설정 탭을 열지만 실제 마우스 입력은 열지 못했다. foreground와 클릭 지점의 창 소유권을 확인했으며, 초기의 다른 창 가림·준비 대기 문제와 구별되는 재현이다. UI05는 FAIL로 기록하고 앱 내부 제목 표시줄 hit-test를 조사한다.

Microsoft Korean IME의 실제 키 입력에서 조합·수정 중 터미널 입력은 없었으나 Escape 때 한글 음절과 Escape가 전달됐다. native-ime-results.json은 이 기대값 불일치를 FAIL로 기록했다. native-ime-isolated-20261118-100938-304의 wrapper가 내부 script의 종료 코드를 전달하지 않아 PASS/0으로 남긴 결과는 IME 통과 근거에서 제외한다. 실행기 v3가 관찰한 wrapper 종료 코드 자체와 내부 기능 판정을 구별한다. Windows 기본 text box 비교로 운영체제 동작인지 확인하기 전에는 Pod 결함으로 확정하지 않는다.

별도로 PR 버튼의 한국어 접근성 이름 보완과 stale Task fixture 기대값 수정인 worker commit 016b8b1bed083384a0c7caf1c856aedbd6332c4c를 검토하여 558bcd5로 통합했다. 동일 commit의 APP 431개와 Clippy는 통과했고, 앱·편집기·UI·terminal-view 내용은 통합본과 같다. 13개 native UIA scene 재실행, 설치된 rust-analyzer의 실제 검사 2개와 설정·IME 후속 검증은 진행 중이다. 이 접근성 수정이 설정 클릭 실패를 해결했다고 판정하지 않는다.

후속 on-display 검증은 실제 커서 위치까지 확인하여 설정 버튼에서 HTCAPTION=2를 측정했다. 앱의 title-actions에 occlude를 적용한 worker a2951e2와 화면 밖 클릭을 BLOCKED 처리하는 회귀 보완 e225e6c를 검토하여 f06ee08·94c4d7a로 통합했다. 수정 후 HTCLIENT=1이며 실제 마우스·Ctrl+,·UIA Invoke 세 경로가 96 DPI에서 모두 통과했다. 같은 앱 수정의 APP 431개와 Clippy도 통과했다. 세 경로 열기와 설정값 저장·재시작 유지 여부는 구분하고, 후자는 별도 검증 전까지 전체 UI05를 통과 처리하지 않는다.

설치된 rust-analyzer의 smoke와 code-actions를 각각 --ignored --exact로 명시 실행하여 각 1개 통과·ignored 0개를 확인했다. 진단·cross-file 정의·hover·종료와 UTF-32·import quick fix·completion 추가 edit·snippet을 실제 서버에서 검사했다. 이는 native 앱 provider 전체 흐름을 대신하지 않는다. 원래 결합 profile의 ignored 이력은 유지한다.

표준 Windows TextBox에서도 Microsoft IME가 Escape 전에 조합 음절을 확정하는 것을 확인하여 앞선 Escape=취소 기대값은 fixture 오류로 분류했다. 조합 중 음절을 삭제하여 취소하고 space로 확정하는 실제 IME 재검증은 조합·수정·취소 중 입력 0회, 확정 후 한글 음절 1회와 입력한 공백만 수신하여 통과했다. Pod IME 구현은 변경하지 않았다. 근거는 app-editor/commands의 native-settings-on-display-20261118-102319-177, native-settings-repaired-20261118-102825-195, native-ime-final-20261118-102905-112, ra-smoke-20261118-101749-637, ra-code-actions-20261118-101941-934와 ime-reference-result.json이다. 13개 UIA scene·7종 레이아웃의 dark/light 검증은 계속한다.

## daemon 통합 target 전체 실행 완료

깨끗한 72b9c54에서 daemon-remaining-full-20261118-101137-920가 실제 exit 0으로 끝났고, 나머지 17개 target의 94개 테스트가 통과했다. orch_runs 25개 전체에는 변경하지 않은 B11 재시도 회귀와 B03–B05 인접 회귀, worker 권한·수동 release 검사가 포함된다. B02 lock 회귀는 앞선 183개 라이브러리 검사에서 통과했다.

지금까지의 target별 최신 결과를 모으면 열거된 daemon integration target 37개에서 176개 통과·실패 0개다. protocol_wire::write_golden 1개만 의도적으로 ignored이며 조건부 skip 출력이나 fixture 프로세스 잔류 경고는 없었다. 이는 서로 다른 단계의 검증 결과를 모은 수치이고 전부 최종 commit에서 재실행했다는 의미는 아니다. 이번 94개는 최종 worker commit에서 실행했으며 관련 podd/CLI/hook/remote/orch/status 파일은 통합본과 같다. pod-hook·pod-remote·owned Clippy/포맷·실제 도구 및 전체 결합 검증은 계속한다. B11의 기존 실패 기록은 보존하고 최신 라이브러리·daemon integration 결과는 PASS로 구분한다.

후속 pod-hook 18개와 pod-remote 21개도 깨끗한 72b9c54에서 통과했다. 이 worker의 최신 자동 profile 결과는 daemon integration 176 + daemon lib 183 + CLI 61 + hook 18 + remote 21 = 459개 통과다. golden 생성 ignored 1개와 실제 Codex 연계 검사 filtered 1개는 통과 수에 넣지 않는다. owned-all-target-clippy도 같은 commit에서 실제 exit 0이며 source fingerprint가 일치했다. 근거는 daemon-cli/commands/pod-hook-all-20261118-101921-763, pod-remote-all-20261118-102032-300, owned-all-target-clippy-20261118-102203-436 기록이다. 포맷·실제 도구·최종 결합 source 검증은 아직 완료되지 않았다.

포맷 검사는 새 B11 integration 회귀의 배치만 지적하여 처음 exit 1이었다. 동작·assertion을 바꾸지 않은 worker 후속 commit a21e5e8c140cb201bb47f04214e0cc78105fbcd2에서 재검사 exit 0을 확인하고 b7f1346으로 통합했다. 관련 daemon/CLI/hook/remote/orch/status 내용은 worker와 같다. owned-format-check-20261118-102312-125의 실패와 owned-format-fixed-20261118-102335-225의 통과 기록을 함께 보존한다. 서식만 바뀌어 이미 유효한 제품 테스트 전체를 중복 실행하지 않으며, 실제 도구와 최종 결합 검증은 계속한다.

## 실제 에이전트 중간 검증

기존 인증 경로 사용과 ephemeral 실행 범위를 coordinator가 승인한 뒤, Codex CLI 0.167.2가 Pod의 독립 fixture task에서 gpt-6.2-sol로 실제 실행됐다. argv에는 workspace-write가 있으나 실제 banner는 read-only였고, calc.py 읽기와 python unittest가 blocked by policy로 거부됐다. 파일 수정이 없고 독립 검사도 기존 2개 실패를 유지했다. 따라서 agent 종료 코드 0은 작업 성공이 아니며, 실제 수정 시나리오는 외부 Codex 정책으로 BLOCKED다. 정책 우회는 하지 않았다.

Pod가 관찰한 상태는 unknown/process → idle/heuristic → exited/process이며, argv로 prompt를 주는 noninteractive 실행이므로 Enter/submit 경로는 N/A다. 실제 에이전트가 작업했다는 이유만으로 working/done 훅 전환이나 대화 resume까지 통과 처리하지 않는다. 근거는 daemon-cli/fixtures/real-codex-3afcc820f5a54b589237d8328f4e49e2의 agent-output.txt, pod-timeline.json과 테스트·파일 기록이다.

이후 외부 보고 wrapper가 Get-Content의 확장 provider metadata를 깊게 직렬화하다 멈춰, 식별한 보고 프로세스만 중지했다. real-codex-fixture-20261118-103130-370의 executor exit -1/FAIL은 이 보고 도구 실패이며, 이미 끝난 Codex의 exit 0 및 과제 BLOCKED와 구분한다. 원래 기록을 유지하고 기존 artifact로 결과를 복원한다. Claude는 별도 fixture에서 검증 중이다. 실제 GitHub의 Pod read-only 상태 조회는 github-read-result.json에서 PASS지만 fixture branch의 PR·check 수는 0이므로 실제 PR 생성·CI 체크·병합 통과로 확대하지 않는다.

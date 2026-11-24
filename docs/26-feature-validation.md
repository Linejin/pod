# 기능 목록과 검증 계획

작성일: 2026-11-18 · 대상: Windows 10/11 x64 · 기준 브랜치: improvements/stability-foundation · 제품 코드 기준: ad8f572

11월 18일 기준 기능 91개의 반복 검증 절차와 기준을 정한다. 이후 통합한 그래프·A2A·사용량 모니터는 [후속 기능 검증 계획](#후속-기능-검증-계획)에 있다. 검증·수정 루프는 기능 ID를 유지하며 실패 재현, 최소 수정, 영향받는 기능의 재검증을 이어간다. 실행 범위와 판정은 [기능 검증 결과](27-feature-validation-results.md)에서 확인한다.

## 1 기존 문서와 이 목록의 역할

| 기존 자료 | 담고 있는 내용 | 이 목록에서 보완하는 부분 |
|---|---|---|
| [README](../README.md), [앱 설명](09-pod-app.md), [편집기 설명](13-editor-and-lsp.md) | 주요 기능, 조작, 구조, 시점별 변경 | 현재 코드에 존재하는 기능별 검증 항목과 통과 조건 |
| [실제 Codex E2E](12-e2e-verification.md) | 특정 과거 commit과 환경에서 실행한 시나리오·실패·재검증 | 같은 시나리오를 새 제품 버전에서 반복할 때의 입력과 판정 |
| [데몬 검증](20-refactor/daemon-validation.md), [라이브러리 검증](20-refactor/libs-validation.md) | 리팩터링 계약과 과거 검증 기록 | UI·실제 도구·설치까지 연결한 제품 기능 범위 |
| [시각 QA](../tools/visual/README.md), [네이티브 QA 범위](../crates/pod-app/qa/native/README.md) | 이미지·접근성 트리와 GPUI 시험의 실행 방법·한계 | 실제 마우스·IME·Narrator와의 구분 |
| [개선 과제](24-improvements.md), [작업 실행 기록](25-improvement-execution.md) | 결함·품질 과제, F01–F15 제안, 구현·통합·검증 | 기존 기능 ID와 알려진 문제의 연결 |

기능 설명과 검증 자료는 이미 있지만, 이 기준 브랜치의 전체 기능을 동일한 ID와 판정 방식으로 묶은 목록은 별도로 필요하다. 이 문서는 제품 코드, CLI 명령 정의, RPC 경로, 기존 시험을 기준으로 그 목록을 제공한다. 다른 브랜치나 최신 원격 main에만 있는 기능은 이 버전의 구현으로 계산하지 않는다.

기능 하나는 독립적으로 실패·수정·재검증할 수 있는 사용자 행동이다. CLI 별칭, 같은 RPC의 앱 버튼, 내부 helper를 각각 별도 제품 기능으로 중복 집계하지 않는다. 앱·CLI·API 노출 차이는 각 행에 적고, UI와 백엔드 판정은 따로 기록한다.

## 2 판정과 실행 환경

### 결과 상태

각 기능에는 자동·UI·실제 도구 결과를 각각 남긴다. 예를 들어 API 시험 PASS와 실제 마우스 NOT_RUN이 함께 있으면 전체 기능을 검증 완료로 닫지 않는다.

| 상태 | 의미 |
|---|---|
| NOT_RUN | 이 commit과 환경에서는 아직 실행하지 않음 |
| PASS | 명시한 절차와 통과 기준을 충족했고 증거가 있음 |
| FAIL | 기대 결과와 다른 동작을 재현함 |
| BLOCKED | 필수 도구·데스크톱·계정·시험 장비가 없어 해당 검사를 수행하지 못함 |
| NOT_IMPLEMENTED | 계획만 있거나 해당 제품 경로가 아직 구현되지 않음 |
| NOT_APPLICABLE | 선언한 제품 범위에 해당하지 않음. 사유와 범위를 기록함 |

기존 시험 파일의 존재, 컴파일 성공, 과거 버전의 PASS, 재실행 한 번의 성공은 현재 버전의 모든 기능 PASS를 뜻하지 않는다. ignored·skip·조건부 조기 반환은 실행 개수와 이유를 남긴다. 알려진 결함은 원래 실패 기록을 유지한다.

### 공통 시험 데이터

- 제품 소스는 전용 worktree와 브랜치에서 고정한다. 실행마다 commit, dirty diff, 빌드 프로필, 바이너리 SHA-256, Windows·DPI·폰트·도구 버전을 기록한다.
- 로컬 fixture 저장소에 단순한 계산 함수, 의도적으로 실패하는 시험, 한글·공백 경로, LF/CRLF·BOM 파일, 변경·삭제·rename·이진·큰 파일을 준비한다. 사용자 작업 저장소는 fixture로 쓰지 않는다.
- 실행별 고유 pipe, daemon state, worktree 루트, 앱 설정, agent session store, hooks 설정, mod 경로를 사용한다. 데몬 runner가 임시 profile을 구성하더라도 테스트 터미널이 레지스트리 환경을 다시 읽을 수 있으므로, 실행되는 프로그램에도 fixture 경로를 명시한다.
- fake-agent·fake LSP·fake gh·fake SSH는 결정적 회귀 검사에 사용하고, 실제 도구 검사는 별도 결과로 기록한다. 가짜 서버 통과를 실제 로그인·네트워크·모델 실행 통과로 표시하지 않는다.
- 같은 worktree에서는 빌드와 시험 바이너리를 겹쳐 실행하지 않는다. worktree 간 CARGO_TARGET_DIR은 공유하지 않는다. 프로세스·출력·저장 파일의 최종 상태까지 확인한다.
- 초기 격리 fixture는 [데몬 시험 runner](../tools/test-daemon-refactor.ps1)와 [공용 fixture](../crates/podd/tests/common/fixtures.rs)를 재사용한다. 설치·GitHub 쓰기·실제 SSH·실제 agent 실행은 각각 별도 fixture 환경과 실행 범위로 분리한다.

### 검증 층

A는 상태·저장·프로토콜·입력 바이트 자동 검사, U는 GPUI와 실제 native UI 검사, R은 실제 agent·언어 서버·SSH·GitHub 연동 검사, P는 설치 패키지 검사다. 각 기능은 해당하는 층을 모두 확인한다. UI가 없는 API 기능에 UI 통과를 요구하지 않지만, API만 있는 경로를 완성된 앱 기능으로 표시하지 않는다.

아래 표의 자동 검사 열은 관련 시험이 들어 있는 진입점이다. 한 시험이 행의 모든 조건을 보장한다는 뜻은 아니다. 절차와 기존 assertion을 대조하여 빠진 조건에는 회귀 시험을 추가한다. 공통 실행 명령은 4절에 있다.

## 3 기능별 검증 목록

### 설치와 실행 기반

근거: [설치](14-install.md), [daemon](03-podd-p0.md), [CLI 명령](../crates/pod-cli/src/args.rs), [wire 등록](../crates/podd/src/server/rpc/registry.rs).

| ID | 기능과 범위 | 검증 절차와 통과 기준 | 자동 검사와 추가 층 |
|---|---|---|---|
| SYS01 | 사용자별 EXE 설치, PATH·바로가기 | 빈 Windows 사용자에 설치→새 터미널에서 버전·실행 경로 확인→앱 시작. 관리자 권한 없이 선택한 위치에 설치되고 배포 파일·고지가 일치한다. | [test-install](../tools/installer/test-install.ps1), P |
| SYS02 | 포터블 ZIP | 공백·한글 경로에 풀고 앱·CLI·holder를 실행한다. 설치본 없이 동작하고 번들 파일이 누락되지 않는다. | [패키지 빌드](../tools/installer/build.ps1), P. ZIP 실행 시나리오는 별도 추가 |
| SYS03 | 실행 중 업그레이드 | 실제 이전 버전 설치→터미널 출력·PID 기록→새 버전 설치→재연결한다. 기존 프로세스·출력은 유지되고 새 터미널은 새 바이너리를 사용한다. | test-install은 같은 EXE 재설치도 포함하므로 구버전 fixture 추가 필요. P, B14 |
| SYS04 | 제거와 데이터 보존 선택 | 실행 중 제거 거부→명시적 종료 후 제거→기본 데이터 보존과 별도 purge를 각각 검사한다. PATH·바로가기와 대상 데이터만 의도대로 처리한다. | test-install, P |
| SYS05 | 서명·패키지 무결성 | EXE/DLL/내부 바이너리의 hash·Authenticode·서명 체인·타임스탬프와 고지를 검사한다. unsigned는 unsigned로 기록하고 signed 배포 조건과 구분한다. | [test-signing](../tools/installer/test-signing.ps1), P. 스크립트 통과는 실제 서명 증명과 별개 |
| SYS06 | daemon 시작·상태·중지 | daemon 없는 상태에서 status/help/schema는 자동 시작하지 않는지 검사하고, run은 연결·시작한다. stop은 holder를 유지하고 stop --kill은 시험 프로세스를 끝낸다. | [daemon](../crates/podd/tests/daemon.rs), [CLI 계약](../crates/pod-cli/tests/refactor_acceptance.rs), A |
| SYS07 | CLI 텍스트·JSON·종료 코드와 가이드 | 명령 도움말·guide·schema, 잘못된 인자·대상·연결 실패를 검사한다. stdout JSON은 파싱 가능하고 stderr·exit code·미지원 기능 안내가 계약과 일치한다. | [command_contract](../crates/pod-cli/tests/command_contract.rs), A |
| SYS08 | 프로토콜 버전·클라이언트 호환 | 구버전·동일·미래 버전 hello, 모르는 메서드·필드·잘못된 params·알림을 검사한다. 공통 버전만 사용하고 비호환은 실행 전에 명시적으로 거절한다. | [protocol_wire](../crates/podd/tests/protocol_wire.rs), [protocol_compat](../crates/podd/tests/protocol_compat.rs), [rpc_routes](../crates/podd/tests/rpc_routes.rs), [client_compat](../crates/podd/tests/client_compat.rs), A |

### 저장소와 Task

근거: [작업](08-tasks.md), [세션 자동화](19-worktree-sessions-orchestration.md), [Task 구현](../crates/podd/src/server/tasks.rs), [저장소 설정](../crates/podd/src/server/repo_config.rs).

| ID | 기능과 범위 | 검증 절차와 통과 기준 | 자동 검사와 추가 층 |
|---|---|---|---|
| TASK01 | 저장소 등록·목록·등록 해제 | 일반·linked worktree·한글/공백 경로와 중복 이름으로 등록한다. 같은 저장소를 일관되게 식별하고 Task가 있으면 해제 규칙을 지키며 파일을 지우지 않는다. | [tasks](../crates/podd/tests/tasks.rs), A+U |
| TASK02 | 이름·기준 ref·worktree 생성 | 기본/명시 base, 이름 충돌·잘못된 ref·제안 이름을 검사한다. 정확한 commit에서 독립 브랜치·worktree가 생기고 실패 시 반쪽 Task가 남지 않는다. | [sessions](../crates/podd/tests/sessions.rs) S1, tasks, A+U |
| TASK03 | agent 선택·인자·브리프 전달 | Shell·Claude·Codex·argv fixture로 시작한다. cwd·환경·부모·프롬프트가 일치하고 브리프 파일 전달과 위험한 cmd shim 인자 거절이 유지된다. 실제 CLI 각각에서도 작은 작업 1회를 확인한다. | sessions S1, [agent 시작](../crates/podd/src/server/agent_launch.rs), A+U+R |
| TASK04 | 저장소 설정·개인 override·신뢰 | .pod/config.toml의 setup/탭/archive와 개인 override를 읽는다. 미승인 명령은 실행하지 않고 승인 hash 변경 시 다시 차단하며 다른 설정을 덮지 않는다. | sessions S2, [repo_config](../crates/podd/src/server/repo_config.rs), A. 프로젝트 설정 편집 UI는 F02 제안 |
| TASK05 | setup·기본 탭·순차 시작 | setup 성공·실패·대기 모드, 여러 기본 탭과 agent 시작 순서를 검사한다. 실패를 숨기지 않고 지정 cwd·이름·환경으로 한 번씩 실행한다. | sessions S2, A+U |
| TASK06 | sparse checkout | 지정 패턴으로 새 worktree를 만들고 허용·제외 파일과 실패 복구를 검사한다. 기존 저장소의 sparse 설정과 다른 Task는 바뀌지 않는다. | sessions S8, A. 새 브랜치의 명령 실패·정리·재시도는 [11월 24일 추가 검증](27-feature-validation-results.md#2026-11-24-추가-검증)에서 확인. |
| TASK07 | Task 목록·git 사실·touch | committed/uncommitted/untracked 변경과 commit·외부 저장 후 목록·diff를 확인한다. ahead/behind/dirty·최신 commit과 변경 이벤트가 실제 git 상태와 일치한다. | tasks, [파일 이벤트](../crates/podd/tests/files.rs), A+U |
| TASK08 | 보관·보관 해제·agent 재개 | archive 명령 성공·실패·dirty/unpushed 보호→보관→재개를 검사한다. worktree·브랜치·세션은 남고 기록한 cwd·agent session으로 재개한다. | sessions S3, [task_archive](../crates/podd/src/server/task_archive.rs), A+R |
| TASK09 | 정리 등급과 안전한 삭제 | protected/review/ready 분류, dirty·unpushed·미등록 디렉터리·명시적 force를 검사한다. 기본 삭제는 위험 대상을 거부하고 터미널·worktree·브랜치 선택을 정확히 처리한다. | sessions S3, tasks, A+U |

### 터미널과 상태 감지

근거: [터미널 뷰](06-terminal-view.md), [상태 엔진](07-pod-status.md), [터미널 조작](../crates/podd/src/server/terminal_ops.rs).

| ID | 기능과 범위 | 검증 절차와 통과 기준 | 자동 검사와 추가 층 |
|---|---|---|---|
| TERM01 | 프로세스 실행·목록·부모 트리 | fixture 부모가 자식을 실행하고 정상/오류 종료한다. cwd·env·task/parent 연결과 종료 코드가 CLI·앱 트리에서 일치한다. | [orchestration](../crates/podd/tests/orchestration.rs), [lifecycle](../crates/podd/tests/lifecycle.rs), A+U |
| TERM02 | 텍스트·키 전송·실제 제출 | 한글·공백·줄바꿈·bracketed paste, Enter/Tab/Esc/Ctrl+C를 보낸다. echo를 처리 완료로 오인하지 않고 agent 입력을 한 번 제출하며 미확인 상태를 표시한다. | [submit](../crates/podd/tests/submit.rs), [input_budget](../crates/podd/tests/input_budget.rs), [reply_budget](../crates/podd/tests/reply_budget.rs), A+R |
| TERM03 | 화면·scrollback·출력 대기 | 화면/이력 읽기와 문자열·종료·상태 대기를 검사한다. 보낸 입력의 echo를 새 결과로 세지 않고 timeout·exit code를 구분한다. | [daemon](../crates/podd/tests/daemon.rs), [status](../crates/podd/tests/status.rs), A+U |
| TERM04 | attach·detach·다중 뷰 | 같은 터미널에 두 뷰를 붙여 입력·출력을 교환하고 한 뷰만 닫는다. 출력 순서와 입력이 유지되고 detach가 프로세스를 종료하지 않는다. | [detach](../crates/podd/tests/detach.rs), [stream_order](../crates/podd/tests/stream_order.rs), A+U. 두 daemon 클라이언트의 출력 순서·분리·입력은 [추가 검증](27-feature-validation-results.md#2026-11-24-추가-검증)에서 확인. |
| TERM05 | resize·렌더링·마우스 보고 | 줄바꿈·대체 화면·마우스 모드·최근 resize 우선순위를 검사한다. 드래그 선택과 앱 마우스 보고가 충돌하지 않고 크기 변경 후 셀·커서가 일치한다. | [terminal-view 시험](../crates/pod-terminal-view/src/view/tests.rs), [mouse](../crates/pod-terminal-view/src/mouse.rs), A+U |
| TERM06 | 선택·복사·붙여넣기·링크 이동 | 다중 행 선택·복사/붙여넣기와 URL·파일:줄 링크를 검사한다. 선택한 텍스트가 정확하고 파일 링크는 올바른 위치로 이동한다. URL 열기는 지정 fixture URL로 확인한다. | [selection](../crates/pod-terminal-view/src/selection.rs), [links](../crates/pod-terminal-view/src/links.rs), [앱 링크](../crates/pod-app/src/term_links.rs), A+U |
| TERM07 | 한글 IME·넓은 글자·폰트 | 실제 Microsoft IME로 조합→수정→확정/취소, 한글·가나·한자·이모지를 입력한다. 조합 중 바이트·명령을 보내지 않고 후보창·커서·선택 폭과 확정 바이트가 맞는다. | [ime](../crates/pod-terminal-view/src/view/ime.rs), native_qa, [CJK 도구](../tools/visual/cjk-compare.ps1), A+U. Unicode SendInput만으로 IME 통과 불가 |
| TERM08 | kill·rm·종료 터미널 보존 | 자식 프로세스가 있는 터미널을 종료·보존·unkeep·삭제한다. 종료된 대상만 기본 rm하고 keep된 터미널은 retention에서 제외되며 전체 시험 프로세스 트리가 정리된다. | [retention](../crates/podd/tests/retention.rs), [stop_race](../crates/podd/tests/stop_race.rs), A+U |
| TERM09 | 앱·daemon 재시작과 holder 재발견 | 출력 중 앱 종료·daemon stop/crash·재시작을 각각 수행한다. 앱/daemon 수명과 프로세스 수명을 구분하고 원래 holder/PID·출력·입력 연결을 복구한다. | [rediscover](../crates/podd/tests/rediscover.rs), [holder_contract](../crates/podd/tests/holder_contract.rs), [unreachable](../crates/podd/tests/unreachable.rs), A+U. 격리 daemon의 정상·강제 종료와 재연결은 [추가 검증](27-feature-validation-results.md#2026-11-24-추가-검증)에서 확인. |
| STATE01 | 훅·OSC·휴리스틱 상태와 출처 | working/permission/question/idle/done/error/exit를 출처별로 주입한다. 우선순위·신뢰도·완료 확인 ack·stale 처리와 앱 표시가 일치한다. | [pod-status](../crates/pod-status/src/engine.rs), status, A+U |
| STATE02 | 훅 설치·상태·제거와 격리 | 임시 agent 설정에서 설치→재설치→status→제거한다. 다른 훅과 설정·백업을 보존하고 정확한 설치 계획 승인과 Pod 밖 no-op을 확인한다. | [CLI hooks](../crates/pod-cli/tests/hooks.rs), [pod-hook CLI](../crates/pod-hook/tests/cli.rs), A+R |
| STATE03 | 실제 agent 승인·질문·취소 상태 | Claude/Codex 각각 짧은 작업에서 권한 요청·승인·거절·질문·취소·완료를 관측한다. 승인 후 실행 중을 blocked로 고정하지 않고 toast와 상태 출처가 실제 행동과 맞는다. | orchestration의 permission 회귀, [실제 E2E](12-e2e-verification.md), U+R |
| STATE04 | 청크와 무관한 완료 턴 기록 | 동일 OSC working→idle 바이트를 한 청크·여러 청크·부분 시퀀스로 입력한다. 최종 상태와 완료 턴 수가 같아야 한다. 여러 턴·hook 우선순위·프로그램 교체도 검사한다. | [청크 입력](../crates/pod-status/src/engine/input.rs), [state_machine](../crates/pod-status/tests/state_machine.rs). B20 후속 수정·집중 회귀 결과는 27 문서 |

### 앱 화면과 조작

근거: [앱](09-pod-app.md), [레이아웃](../crates/pod-app/src/layout_mode.rs), [설정](../crates/pod-app/src/chrome/settings.rs), [native_qa](../crates/pod-app/src/native_qa.rs).

| ID | 기능과 범위 | 검증 절차와 통과 기준 | 자동 검사와 추가 층 |
|---|---|---|---|
| UI01 | 저장소·Task·터미널 사이드바와 필터 | 빈 상태·부모/자식·완료·오류·한글 검색·선택을 검사한다. 선택한 리소스와 실제 열린 패널·상태가 맞고 stale 항목을 정상 처리한다. | [workspace 시험](../crates/pod-app/src/workspace/tests.rs), native_qa, A+U |
| UI02 | 패널 열기·분할·복제·이름·닫기 | 터미널·편집기·리뷰·그룹·Gate·Agents·Run 패널을 조합한다. 패널 닫기는 실행을 유지하고 별도 kill만 프로세스를 끝내며 포커스를 회복한다. | workspace 시험, native_qa, A+U |
| UI03 | 레이아웃 저장·복원 | 탭·도크·비터미널 패널·편집기 위치를 바꾼 뒤 앱을 재시작한다. 원래 리소스를 중복 생성하지 않고 패널·커서·활성 선택을 복원한다. 미저장 본문 복구는 F08 제안이다. | [layout](../crates/pod-app/src/layout.rs), [restore-panels](../tools/visual/restore-panels.ps1), A+U. 정상 종료 후 4개 패널·탭·커서 복원을 [추가 검증](27-feature-validation-results.md#2026-11-24-추가-검증)에서 확인. |
| UI04 | 7종 레이아웃과 테마 | workbench/focus-rail/queue/kanban/block-terminal/review/dense-details를 순환하며 같은 작업·응답·선택을 조작한다. 사용자 스타일·시스템 테마·실시간 reload와 잘못된 스타일도 검사한다. 모달·키·상태·저장값이 유실되지 않는다. | [layouts](../crates/pod-app/src/layouts), [설정](../crates/pod-app/src/chrome/settings.rs), capture -Layout, A+U |
| UI05 | 설정 버튼·설정 저장 | 실제 마우스, Ctrl+,와 UIA Invoke로 설정을 각각 연다. 같은 화면·값이 열리고 font/테마/표시·동작 설정은 재시작 후 유지된다. 버튼 무응답 제보는 이 세 경로로 재현한다. | [chrome 시험](../crates/pod-app/src/chrome/tests.rs), [native 클릭 회귀](../crates/pod-app/qa/native/settings-mouse.ps1), U. 96 DPI 진입 경로와 font/display/behavior 재시작 저장 PASS; 전체 DPI·후보값·save-error matrix는 미실행. [결과](27-feature-validation-results.md#최종-기능-커버리지) |
| UI06 | 팔레트·단축키·모달·포커스 | 시작 직후·패널 닫기 후·모달 안에서 명령·Tab/Shift+Tab/Enter/Esc를 검사한다. 활성 문맥에만 실행되고 Esc는 열기 전 위치로 돌아가며 IME 확정이 이중 실행되지 않는다. | native_qa, [chrome commands](../crates/pod-app/src/chrome/commands.rs), A+U |
| UI07 | 알림·응답함·완료 확인 | 다른 터미널·Review·Group을 보는 동안 blocked/done/error·Gate를 발생시킨다. 필요한 toast·소리·응답 이동과 ack가 일관되고 중복 알림을 남기지 않는다. | [notifications](../crates/pod-app/src/workspace/notifications.rs), native_qa, A+U |
| UI08 | DPI·작은 창·접근성 | 100/125/150/175/200%와 모니터 이동, 작은 창·고대비·키보드·UIA·Narrator를 검사한다. 클릭 위치·IME 후보·필수 컨트롤·접근성 이름과 읽기 순서가 일치한다. | [mixed-dpi](../tools/visual/mixed-dpi.ps1), UIA selftest/capture, U. 이미지와 UIA 트리만으로 실제 낭독 통과 불가 |

### 편집기와 LSP

근거: [편집기](13-editor-and-lsp.md), [workspace](../crates/pod-editor/src/ui/workspace.rs), [LSP store](../crates/pod-editor/src/lsp/store.rs).

| ID | 기능과 범위 | 검증 절차와 통과 기준 | 자동 검사와 추가 층 |
|---|---|---|---|
| EDIT01 | 파일 트리·생성·삭제·이름 변경 | .gitignore·중첩 폴더·git 배지·한글 경로를 열고 파일/폴더를 생성·rename·삭제한다. 다른 Task나 루트 밖 경로를 바꾸지 않고 트리와 실제 파일이 일치한다. | pod-editor --lib, [tree](../crates/pod-editor/src/ui/workspace/tree.rs), A+U |
| EDIT02 | 파일 색인·빠른 열기·줄 이동 | Ctrl+P로 파일명 검색→선택→Ctrl+G 줄:열로 이동한다. 생성/삭제 뒤 색인이 갱신되고 한글·동명 파일·없는 줄·큰 색인을 안전하게 처리한다. | [index](../crates/pod-editor/src/index.rs), [quick_open](../crates/pod-editor/src/ui/quick_open.rs), A+U |
| EDIT03 | 탭·미리 보기·고정·MRU·재열기 | 미리 보기→편집 고정, 재정렬·동명 파일·overflow·닫기·Ctrl+Tab·Ctrl+Shift+T를 검사한다. 탭 순서와 실제 파일·커서가 일치하고 dirty 닫기는 확인한다. | [tabs_ui](../crates/pod-editor/tests/tabs_ui.rs), A+U |
| EDIT04 | 코드 편집·강조·찾기·치환·undo | 지원 언어와 일반 텍스트에서 입력·선택·한글·Ctrl+F/H·undo/redo를 검사한다. 바뀐 본문·위치·강조가 일치하고 파일 내 치환이 다른 파일에 적용되지 않는다. | [buffers](../crates/pod-editor/src/ui/workspace/buffers.rs), native_qa, U |
| EDIT05 | 저장·전체 저장·인코딩·줄 끝 | UTF-8/BOM/CRLF·혼합 줄 끝과 지원 재해석 인코딩을 편집·저장한다. 기대 바이트를 비교하고 표현 불가 문자·읽기 전용·이진·대형 파일을 정책대로 거부한다. | [save_ui](../crates/pod-editor/tests/save_ui.rs), [text](../crates/pod-editor/src/text.rs), A+U. 실제 save-all byte, 인코딩, UIA 읽기 전용·대형/디코딩 불가 보호 확인. 전체 기능 procedure는 NOT_RUN. [결과](27-feature-validation-results.md#최종-기능-커버리지) |
| EDIT06 | 외부 변경·충돌·저장 실패 | clean/dirty 각각에 agent 수정·삭제·권한/공유 잠금·reload 실패를 주입한다. 자동 reload/경고/차이/덮어쓰기 선택이 정확하고 실패 시 원문·충돌 상태를 보존한다. 크기 경계를 왕복 검사한다. | save_ui, [files](../crates/pod-editor/src/ui/workspace/files.rs), A+U. 충돌·reload·diff/overwrite·UIA 보호 범위 native PASS; 권한/ACL 및 full provider matrix 미완료. 전체 기능 procedure는 NOT_RUN. [결과](27-feature-validation-results.md#최종-기능-커버리지) |
| EDIT07 | rename 이후 dirty·저장·LSP | 열린 clean/dirty·배경 파일과 부모 폴더를 rename한 뒤 편집→전체 저장→닫기를 검사한다. 새 경로 dirty·경고·저장과 LSP URI가 유지되고 옛 이름 재사용도 안전하다. | save_ui, [tabs_lsp](../crates/pod-editor/tests/tabs_lsp.rs), A+U. 실제 rust-analyzer에서 이전 URI 닫기, 새 URI 열기·자동 완성·저장 확인. 여러 파일·대소문자·동시 변경 조합은 미실행. [추가 검증](27-feature-validation-results.md#2026-11-24-추가-검증) |
| EDIT08 | 탭·커서 복원과 LSP 지연 시작 | 여러 탭으로 재시작하고 비활성 탭을 차례로 연다. 파일·탭 순서·커서를 복원하고 보여 주기 전 배경 탭은 불필요한 서버를 시작하지 않는다. | tabs_lsp, [session](../crates/pod-editor/src/ui/workspace/session.rs), A+U |
| LSP01 | 서버 설정·저장소 신뢰 | 사용자·저장소 설정의 command/args/probe를 바꾼다. 미승인 저장소 설정은 실행하지 않고 hash 변경 재승인, enable/ignore·없는 서버 안내를 검사한다. | pod-editor --lib, [registry](../crates/pod-editor/src/lsp/registry.rs), U+R |
| LSP02 | 서버 시작·재시작·종료·상한 | initialize→crash→backoff→재시작 한도, 작업 전환 LRU, 앱 종료를 검사한다. pending 요청은 실패를 알리고 서버 자식이 남지 않으며 다시 사용하면 재개한다. | [lsp_lifecycle](../crates/pod-editor/tests/lsp_lifecycle.rs), A+U+R |
| LSP03 | 문서 sync·진단·문자 위치 | 열기·증분/전체 수정·저장·닫기, UTF-16/32·한글·이모지·구버전 진단을 검사한다. 서버 본문과 버퍼가 같고 오래된 진단은 현재 줄에 붙지 않는다. | lsp_lifecycle, tabs_lsp, A+R |
| LSP04 | 완성·스니펫·hover·정의·참조 | 실제 provider에서 완성 선택→placeholder 이동·추가 import→hover→정의/참조 이동을 검사한다. 새 URI·정확한 위치·미저장 본문을 사용하며 undo가 가능하다. | lsp_lifecycle, tabs_lsp, [providers](../crates/pod-editor/src/lsp/providers.rs), A+U+R |
| LSP05 | 심볼 rename·코드 액션·applyEdit | 심볼 rename은 preview→적용/취소, 코드 액션은 선택→다중 파일 적용, 서버 applyEdit는 요청·응답을 검사한다. 위치 인코딩·version·저장 충돌을 확인하고 의도한 파일·범위의 수정만 dirty 버퍼로 반영한다. | lsp_lifecycle, [lsp_actions](../crates/pod-editor/src/ui/workspace/lsp_actions.rs), A+U+R |

### 리뷰와 그룹과 Gate

근거: [리뷰·오케스트레이션](11-review-and-orchestration-backend.md), [리뷰 UI](../crates/pod-app/src/review.rs), [그룹](../crates/pod-app/src/groups.rs).

| ID | 기능과 범위 | 검증 절차와 통과 기준 | 자동 검사와 추가 층 |
|---|---|---|---|
| REVIEW01 | 변경 파일·diff·에디터 연결 | staged/unstaged/committed/untracked·rename/삭제/이진·큰 diff를 확인하고 줄에서 편집기로 이동한다. 기준 base와 before/after·줄 번호가 맞고 저장소 실행 훅을 호출하지 않는다. | [pod-diff](../crates/pod-diff/src/lib.rs), [files](../crates/podd/tests/files.rs), U |
| REVIEW02 | 줄·범위 의견·해결·삭제·재열기 | 양쪽 diff에 의견을 저장·해결·재열기·삭제하고 앱/daemon을 재시작한다. 위치·작성 내용·상태가 유지되며 변경 후 stale 위치를 안전하게 표시한다. | [orchestration](../crates/podd/tests/orchestration.rs), [review 시험](../crates/pod-app/src/review/tests.rs), A+U |
| REVIEW03 | 미저장 초안과 저장 중 입력 | A 의견 작성→B 위치 이동→복귀, 저장 응답을 지연시키고 추가 입력한다. 작성 중 내용이 확인 없이 교체되거나 늦은 성공 응답으로 지워지지 않아야 한다. | [review QA](../crates/pod-app/src/review/qa.rs), A+U. 실제 UIA navigation과 isolated real-daemon delayed response PASS; agent 전송 없음. F08 restart recovery는 별도 제안. [결과](27-feature-validation-results.md#최종-기능-커버리지) |
| REVIEW04 | 의견 전달·큐·취소·만료 | idle/working/blocked/종료 agent에 의견을 전송하고 취소·만료·daemon 재시작을 주입한다. 큐는 유지되고 실제 제출은 한 번이며 전달 실패를 표시한다. | orchestration, A+U+R |
| GROUP01 | 같은 요청의 다중 agent 비교 | 같은 base·프롬프트로 서로 다른 agent 2개를 시작한다. 독립 worktree·멤버 상태·변경 요약이 분리되고 비교 화면이 실제 결과와 일치한다. | orchestration의 group_fan_out_compare_check_pick, [groups 시험](../crates/pod-app/src/groups/tests.rs), A+U+R |
| GROUP02 | 멤버별 check 실행·중지·결과 | 한 멤버는 성공, 다른 멤버는 실패/timeout하도록 검사 명령을 실행·취소한다. 각 exit/log/진행 상태를 구분하고 살아 있는 검사 프로세스를 남기지 않는다. | orchestration, [groups 구현](../crates/podd/src/server/groups.rs), A+U |
| GROUP03 | 승자 선택·나머지 정리·그룹 해제 | 승자 선택과 dirty 패자 삭제를 시도한다. 기본 보호가 작동하고 명시적 정리만 수행하며 그룹 해제만으로 멤버 Task를 삭제하지 않는다. | orchestration, groups 시험, A+U |
| GATE01 | 승인·거절·선택지·기한 | Task/Group/Run 문맥에서 Gate 생성→앱/CLI 응답→만료·중복 응답을 검사한다. 올바른 대상·선택지·응답자만 기록하고 만료 후 승인하지 않는다. | orchestration, [orch_runs](../crates/podd/tests/orch_runs.rs), [gates](../crates/podd/src/server/gates.rs), A+U |
| GATE02 | Gate 대기와 재시작 | agent가 Gate를 기다리는 동안 daemon 연결을 끊고 재시작한 뒤 응답한다. 같은 Gate를 계속 기다리고 거절·기한·종료를 올바른 exit로 반환한다. | orchestration의 waits_survive_a_daemon_restart, A+R |

### Run과 워커 오케스트레이션

근거: [워크트리 세션과 오케스트레이션](19-worktree-sessions-orchestration.md), [엔진](../crates/pod-orch/src/engine.rs), [데몬](../crates/podd/src/server/orch_runs.rs).

| ID | 기능과 범위 | 검증 절차와 통과 기준 | 자동 검사와 추가 층 |
|---|---|---|---|
| ORCH01 | Run·Item·의존관계·조회 | 선행→후행, 독립·차단 Item과 잘못된 참조를 만들고 완료/재개한다. 후행은 조건이 충족될 때만 준비되고 UI 보드와 CLI 상태가 일치한다. | pod-orch --lib, orch_runs, [orch UI 모델 시험](../crates/pod-app/src/orch_ui/model/tests.rs), A+U |
| ORCH02 | worker 시작·배치·부모·브리프 | 현재/새 worktree worker를 시작하고 Task·Run·Item·부모·환경·brief를 확인한다. 반쪽 시작 실패와 queue 상태를 구분하고 다른 worker의 작업 공간을 침범하지 않는다. | orch_runs, sessions, A+U+R |
| ORCH03 | 완료 보고·중복·재시도·generation | 성공·실패·동일/다른 중복 보고·옛 token·retry-of를 보낸다. 유효한 generation만 Item을 바꾸고 중복 결과는 중복 실행·메일을 만들지 않는다. | pod-orch --lib, orch_runs, A |
| ORCH04 | 메일·그룹 주소·질문·응답·읽음 | 개인·coordinator·그룹에 send/ask/check/reply하고 restart한다. 수신자·thread·질문 상태·read receipt를 보존하고 임의 본문을 권한 지시로 승격하지 않는다. | orch_runs, [mail](../crates/podd/src/server/orch_runs/mail.rs), A+U |
| ORCH05 | 깨우기·전달 불확실·재시작 | working/idle/unknown 대상과 입력 거절·응답 유실·재시작을 주입한다. 확인된 전달만 기록하고 불확실 입력을 무조건 재전송하지 않으며 앱 표시가 일치한다. | pod-orch wake 시험, orch_runs, [wake](../crates/podd/src/server/orch_runs/wake.rs), A+U+R |
| ORCH06 | Run Gate·의존 차단·권한 | person/coordinator/worker 결정과 선택지를 검사한다. 정책에 맞는 응답만 실행을 풀고 워커가 자신의 권한을 확대할 수 없다. | orch_runs, [worker_endpoint](../crates/podd/tests/worker_endpoint.rs), A+U |
| ORCH07 | 동시성·CPU·메모리·디스크 큐 | 한도를 넘는 worker를 요청하고 자원 표본을 바꾼다. queue 이유·순서·admission이 일치하고 저장 실패 시 시작 데이터나 token을 잃지 않는다. | [resource](../crates/pod-orch/src/resource.rs), orch_runs, A+U |
| ORCH08 | 감독·대기·lost·회로 차단·중첩 | report 없는 완료·정상 질문 대기·활성 출력·프로세스 소실·반복 start 실패·깊이 제한을 검사한다. 정당한 대기를 lost로 오인하지 않고 정확한 이유로 알린다. | orch_runs, [supervisor](../crates/podd/src/server/orch_runs/supervisor.rs), A+R. STATE04 영향 포함 |
| ORCH09 | keep·사람 입력·자동 release 경합 | due 선정 직후와 release claim 직후 keep/입력을 넣는다. 선정 후 claim 전에는 프로세스를 보존하고 claim 후에는 보존을 거짓 약속하지 않으며 실패 시 claim을 해제한다. | orch_runs automatic_release_, A+U. B03 수정 이력 |
| ORCH10 | 출력 archive·release·재시작 복구 | archive 쓰기·경로 저장·최종 Released 저장을 각각 실패시킨다. 제거 전에는 출력/프로세스를 유지하고 제거 후에는 보관본과 재시도 상태를 보존하며 재시작 후 완료한다. | orch_runs archive_failure/persistence_failure, A. B04·B05 결합 이력 |
| ORCH11 | 설정·limits·영속 상태 | config 변경과 감독을 barrier로 겹치고 snapshot/index 저장 실패 후 같은 요청을 재시도·restart한다. 교착·유령 Attempt·중복 세대가 없어야 한다. 읽기 실패도 진단해야 한다. | [attempt_persistence](../crates/pod-orch/src/engine/tests/attempt_persistence.rs), [recovery_reads](../crates/pod-orch/src/engine/tests/recovery_reads.rs), orch_runs, A. B10 core 수정 후 daemon 결합 검증 중 |
| ORCH12 | worker endpoint·token·허용 커밋 | 인증 없는 호출·다른 worker/Task·옛 token·권한 확대·저장소 hook/filter 실행 시도를 검사한다. 닫힌 허용 목록을 지키고 커밋은 승인된 대상·동작만 수행한다. | worker_endpoint, [worker_commit_boundary](../crates/podd/tests/worker_commit_boundary.rs), [worker_commit_filters](../crates/podd/tests/worker_commit_filters.rs), [worker_commit_env](../crates/podd/tests/worker_commit_env.rs), A |
| ORCH13 | 다른 worker 기다리기·정리 UI | worker 간 wait·완료·취소와 cleanup protected/review/ready 선택을 검사한다. 교착 없이 결과를 관측하고 보존 대상은 기본 bulk 정리에서 빠지며 새로운 활동을 재검사한다. | orch_runs, [cleanup UI](../crates/pod-app/src/orch_ui/cleanup.rs), A+U. 일반 Task 통합 계획은 F15 |
| ORCH14 | worker stop·abandon·수동 release | 실행 중·종료·도달 불가 worker에 각 동작을 요청한다. stop은 종료를 증명할 때만 Stopped로 확정하고, abandon은 기록을 닫되 프로세스를 유지하며, 수동 release는 보관·강제 종료 정책을 명시적으로 따른다. | orch_runs, [workers](../crates/podd/src/server/orch_runs/workers.rs), A+U |

### GitHub와 확장

근거: [PR](15-forge-and-pr.md), [확장](16-extensions.md).

| ID | 기능과 범위 | 검증 절차와 통과 기준 | 자동 검사와 추가 층 |
|---|---|---|---|
| PR01 | 원격 인식·상태·checks·리뷰 결정 | GitHub/비지원/remote 없음·인증 없음·gh 실패·pending/fail/pass check를 검사한다. 오류 종류와 cached/최신 결과를 구분하고 PR 탭을 보지 않을 때 불필요한 요청을 하지 않는다. | [pr](../crates/podd/tests/pr.rs), [pr_panel](../crates/pod-app/src/pr_panel.rs), A+U+R |
| PR02 | 작업 브랜치 push·PR 초안 생성·열기 | fake forge에서 새 PR·기존 PR·dirty·앞선 commit 없음·API 오류를 검사한다. worker PR의 Item 제목·완료 보고 본문도 확인한다. 실제 쓰기는 지정 시험 저장소에서 대상 브랜치·제목·본문·draft를 확인한다. | pr, orch_runs, [pod-forge](../crates/pod-forge/src/lib.rs), A+U+R |
| PR03 | merge 방식·확인·실패 처리 | fake forge에서 merge/squash/rebase 선택과 미통과 check·인증 오류·중복을 검사한다. 확인한 대상·방식만 요청하며 실패를 성공으로 표시하지 않는다. 실제 쓰기 검사는 별도 fixture 범위다. | pr, [PR QA](../crates/pod-app/src/pr_qa.rs), A+U+R |
| MOD01 | 모드 목록·설치 경로·enable·reload | fixture manifest·JS를 준비해 목록/켜기/끄기/reload/오류를 검사한다. 사용자·프로젝트 모드를 구분하고 승인하지 않은 capability를 부여하지 않는다. | [mods manager](../crates/pod-mods/tests/manager.rs), [mods RPC](../crates/podd/tests/mods.rs), [mods UI 시험](../crates/pod-app/src/mods_ui/tests.rs), A+U |
| MOD02 | JS sandbox·타이머·예산·오류 격리 | 무한 루프·메모리·이벤트 폭주·거부된 API·잘못된 payload를 실행한다. 예산 위반 모드만 중지하고 daemon·다른 모드·사용자 입력은 유지한다. | pod-mods --lib, manager, mods RPC, A |
| MOD03 | 상태 detector·알림·레이아웃 preset | 허용된 terminal 이벤트로 상태·toast와 preset을 발행한다. 범위·TTL·우선순위·표시와 유효성 검사를 지키고 native shell·임의 네트워크 권한으로 확장되지 않는다. | mods RPC, [모드 QA](../crates/pod-app/tools/mods-qa.ps1), A+U |
| MOD04 | 모드 명령·터미널 전송·check·Gate | fixture 모드가 팔레트 명령 등록·Task/terminal 조회·허용 전송·manifest check·Gate 요청을 수행한다. 올바른 문맥·승인 capability·argv만 사용하고 거부·timeout·check.finished 결과를 표시한다. | pod-mods --lib, manager, mods RPC, A+U |

### 외부 세션과 원격과 모바일

근거: [세션 찾기](18-session-discovery.md), [원격](17-remote.md), [모바일 단계 경계](23-mobile-m2.md).

| ID | 기능과 범위 | 검증 절차와 통과 기준 | 자동 검사와 추가 층 |
|---|---|---|---|
| DISC01 | 외부 Claude·Codex 세션 탐색 | fixture session store로 off/on·수동 scan·preview off·숨김·오래됨·손상·대용량·예산 초과를 검사한다. 제한된 정보만 읽고 truncated·시각·출처를 표시한다. | [discovery](../crates/podd/tests/discovery.rs), [parsers](../crates/pod-discover/tests/parsers.rs), [세션 UI 시험](../crates/pod-app/src/sessions_ui/tests.rs), A+U |
| DISC02 | 세션 연결·저장소 등록·숨김 | cwd가 등록 Task·하위 폴더·외부 저장소인 세션을 연결·등록·숨긴다. longest path 경계를 지키고 기존 cwd를 유지하며 새 worktree를 임의 생성하지 않는다. | discovery, A+U |
| DISC03 | 외부 세션 재개·실행 중 보호 | UUID·없는 cwd·설치 안 된 CLI·이미 실행 중을 검사한다. 재개 인자와 cwd가 맞고 중복 실행을 거부한다. 실제 CLI에서 기존 대화가 이어지는지는 별도 확인한다. | discovery, sessions, A+U+R |
| REMOTE01 | 머신 프로필·연결·오류 진단 | 잘못된 host·실행 경로·중복 이름·인증/버전 실패를 검사한다. local/remote 대상을 혼동하지 않고 시스템 SSH의 기존 신뢰 정책을 완화하지 않는다. | [remote_ssh](../crates/pod-cli/tests/remote_ssh.rs), [machine UI 시험](../crates/pod-app/src/machines_ui/tests.rs), A+U+R |
| REMOTE02 | stdio 전송·프레이밍·버전·backpressure | UTF-8 분할·반쪽 줄·과대 응답·stdout 배너·동시 read/write·EOF를 주입한다. bounded 버퍼를 유지하고 프로토콜 아닌 출력은 명시적으로 실패한다. | [remote](../crates/podd/tests/remote.rs), [remote_stdio](../crates/pod-cli/tests/remote_stdio.rs), A |
| REMOTE03 | 원격 attach·재접속·입력 중복 방지 | 지연·60초 단절·응답 유실·원격 daemon 재시작·같은 프로필 뒤 다른 머신을 주입한다. snapshot/stream 순서를 복구하고 입력을 무조건 재전송하지 않는다. | [remote_attach](../crates/podd/tests/remote_attach.rs), [remote_resilience](../crates/pod-cli/tests/remote_resilience.rs), A+U+R |
| REMOTE04 | 앱 머신 보드·host key 확인·원격 터미널 | 로컬·원격에 같은 ID를 가진 fixture를 표시하고 최초 신뢰·불일치·재연결을 조작한다. 정확한 머신의 터미널만 제어하며 불일치는 연결 전에 거부한다. | machine UI 시험, [host_keys](../crates/pod-app/src/machines_ui/host_keys.rs), A+U+R |
| REMOTE05 | 원격 파일 읽기·저장·diff API | traversal·symlink/reparse·큰 파일·stale token·응답 유실·저장 중 crash를 검사한다. 허용 Task 경계와 CAS·복구 계약을 지킨다. 이 기준에서는 에디터/diff seam과 앱의 실제 원격 편집 완성을 구분한다. | [files](../crates/podd/tests/files.rs), [remote_files](../crates/pod-cli/tests/remote_files.rs), A. junction/hardlink 및 loopback API 5 checks PASS. native symlink 생성은 Win32 1314로 막혀 boundary assertions BLOCKED; 앱의 실제 원격 편집 별도. [결과](27-feature-validation-results.md#최종-기능-커버리지) |
| MOBILE01 | M1 기기·Task 권한·철회·감사 API | 로컬 debug stub에서 기본 off·기기 등록·task별 status/approve/output/message·세션 만료·rate limit·철회를 검사한다. 기본 거부·즉시 철회·비밀 없는 감사와 허용 메서드 경계를 지킨다. | [mobile](../crates/podd/tests/mobile.rs), A. 실제 listener·QR·휴대전화 연결 아님 |
| MOBILE02 | M1 Gate 결정·짧은 메시지·delivery API | 만료/선택지/중복 Gate, blocked/working agent 메시지, 길이·제어 문자·권한 철회를 검사한다. 승인 범위 안에서만 처리하고 키 입력·범용 RPC·임의 명령으로 바꾸지 않는다. | mobile, [mobile API](../crates/podd/src/server/mobile/api.rs), A. 실제 휴대전화 UI·전송은 미구현 단계 |

### 후속 기능 검증 계획

| ID | 기능 | 절차와 통과 기준 |
|---|---|---|
| GRAPH01 | 프로젝트 에이전트 그래프 | 저장소 두 개에 Task·자식 에이전트·셸을 구성한다. 저장소 행에서 마우스와 키보드로 열고, 해당 프로젝트의 에이전트 상태와 명시적 부모 관계를 확인한다. 좁은 창과 고대비에서 버튼과 선택 노드가 보이고 기존 터미널이 열려야 한다. 다른 저장소로 전환한 직후 정상 종료한 뒤 선택과 패널이 복원되는지 확인한다. U+A. |
| A2A01 | 표준 A2A CLI | 허가된 로컬 테스트 피어에서 카드·전송·조회·목록·취소와 이어지는 입력을 실행한다. JSON-RPC의 `A2A-Version: 1.0`과 표준 필드, 오류 및 종료 코드를 확인한다. HTTP loopback은 명시 옵션을 켠 경우에만 허용하고, TLS fixture에서 토큰의 대상과 실패 후 재전송 여부를 검사한다. A+R. |
| A2A02 | 프로젝트 그래프의 외부 A2A 피어 | 피어 설정이 다른 저장소 두 개와 격리된 SDK fixture를 준비한다. 피어 선택, 카드·기술 확인, 메시지 검토와 전송, 원격 작업·답장·계속 보내기를 검사한다. 프로젝트 전환과 접기에서 상태·조회 범위가 정확하고, 결과가 불명확한 전송은 명시적 재검토를 거쳐 같은 ID로 재전송되는지 확인한다. A+U+R. |
| USAGE01 | AI 사용 한도 | 고정된 한도와 초기화 시각 fixture로 일곱 레이아웃의 상태 표시줄과 세부 정보, 800px·확대 배율·고대비·키보드를 검사한다. 초기화 경계·오래된 값·실패·계정 전환에서 표시와 보관 규칙을 확인한다. 별도로 로그인된 Codex의 읽기 전용 조회와 Claude Code 수집기의 Git Bash·PowerShell 입력 전달을 검증한다. 실제 Claude Code 보고는 fixture와 별도로 기록한다. A+U+R. |

자동 검사 진입점은 GRAPH01의 `cargo test --locked -p pod-app project_graph`, A2A01의 `cargo test --locked -p pod-a2a`와 `cargo test --locked -p pod-cli a2a`, A2A02의 `cargo test --locked -p pod-app project_a2a`, USAGE01의 [수집기·앱 검증 명령](usage/ai-usage-monitor.md#확인-방법)이다. A2A02에서 실제 SDK를 사용할 때는 격리된 fixture의 주소를 `POD_A2A_UI_FIXTURE`로 지정한다. 실행 범위는 [11월 24일 후속 기능 결과](27-feature-validation-results.md#2026-11-24-후속-기능-검증)에 기록한다.

## 4 자동 검사의 실행 진입점

아래는 검증 루프가 사용할 실행 프로필이다. 기존 스크립트와 cargo 명령이며, 새 루프 구현체가 이미 있다는 뜻은 아니다. 정확한 실행 대상은 각 기능 행과 실제 assertion에 연결한다.

공통 명령 실행은 [invoke-check.ps1](../tools/qa/invoke-check.ps1)로 실제 프로세스 종료 코드·원본 출력·소스 fingerprint를 수집한다. 실행기 자체의 [회귀 검사](../tools/qa/test-check.ps1)를 먼저 통과시킨다. 기능별 실행·수정 결과는 [실행 기록](27-feature-validation-results.md)에서 추적한다.

### 정책과 빌드

저장소 루트에서 실행하며 빌드는 해당 worktree의 target만 사용한다. Rust·Git·필수 Windows 빌드 도구와 번들 ConPTY 준비 여부를 먼저 기록한다. 설치 자료와 프로젝트의 기존 도구 버전 기준을 따른다.

~~~powershell
powershell -NoProfile -File tools/check-workspace.ps1
powershell -NoProfile -File tools/check-docs.ps1
powershell -NoProfile -File tools/check-boundaries.ps1
powershell -NoProfile -File tools/check-vendor-notices.ps1
powershell -NoProfile -File tools/test-publish-public.ps1
powershell -NoProfile -File tools/installer/test-signing.ps1
cargo fmt --all -- --check
cargo build --workspace --exclude pod-spike --exclude diff-spike --locked
~~~

각 native 명령 직후 종료 코드를 검사한다. 마지막 명령 성공으로 앞선 실패를 덮지 않는다. 위 정책 검사만 통과한 상태는 기능 PASS가 아니다.

### 자동 시험 프로필

| 프로필 | 명령 또는 runner 대상 | 연결 기능 |
|---|---|---|
| CORE | cargo test에 -p pod-status, -p pod-storage, -p pod-process, -p pod-protocol, -p pod-client를 각각 지정하고 --locked | STATE01/04, SYS08, 저장·프로세스 공통 계약 |
| TERMINAL | cargo test -p pod-terminal-view --locked; daemon runner의 daemon, detach, retention, submit, status, stop_race, rediscover, stream_order, holder_contract, unreachable, input_budget, reply_budget | TERM01–09, STATE01 |
| TASK | daemon runner의 tasks, sessions, hook_trust | TASK01–09, STATE02 |
| APP | cargo test -p pod-app --locked | UI01–08, 앱의 Review/Group/Run/세션/머신/모드 시험. 실제 native 조작은 별도 |
| EDITOR | cargo test -p pod-editor --locked | EDIT01–08, fake LSP 기반 LSP01–05 |
| REVIEW | daemon runner의 orchestration, files | REVIEW01–04, GROUP01–03, GATE01–02 |
| ORCH | cargo test -p pod-orch --locked; daemon runner의 orch_runs, worker_endpoint, worker_commit_boundary, worker_commit_filters, worker_commit_env | ORCH01–14 |
| FORGE | cargo test -p pod-forge --locked; daemon runner의 pr | PR01–03, fake gh 기반 |
| MODS | cargo test -p pod-mods --locked; daemon runner의 mods | MOD01–04 |
| DISCOVERY | cargo test -p pod-discover --locked; daemon runner의 discovery | DISC01–03 |
| REMOTE | cargo test -p pod-remote --locked; pod-cli의 remote_stdio, remote_ssh, remote_resilience, remote_files; daemon runner의 remote, remote_attach, files | REMOTE01–05, fake 전송과 fault proxy 기반 |
| MOBILE | daemon runner의 mobile | MOBILE01–02, M1 로컬 API 범위 |
| CONTRACT | cargo test -p pod-cli --test command_contract --test refactor_acceptance --locked; daemon runner의 protocol_wire, protocol_compat, rpc_routes, client_compat, refactor_acceptance | SYS06–08와 전체 공개 계약 |
| HOOKS | cargo test -p pod-hook --locked; cargo test -p pod-cli --test hooks --locked -- --skip codex_hooks_trusted_via_app_server_in_temp_home | STATE02. 실제 Codex 신뢰 시험은 R 층으로 별도 선택 |

예를 들어 daemon runner는 PowerShell에서 다음과 같이 사용한다.

~~~powershell
& ./tools/test-daemon-refactor.ps1 -List
& ./tools/test-daemon-refactor.ps1 -Test @("tasks", "sessions", "orch_runs") -Keep
& ./tools/test-daemon-refactor.ps1 -Test @("orch_runs") -Filter persistence_failure -Keep
~~~

runner는 선택한 시험 바이너리와 fixture를 먼저 빌드하고 임시 profile에서 실행한다. 최초 실행부터 -SkipBuild를 사용하지 않는다. 실패한 실행의 격리 디렉터리·로그를 보존하고, 관련 프로세스가 끝난 것을 확인한 뒤 같은 바이너리를 다시 빌드한다.

변경 기능과 인접 기능을 통과한 후 전체 기본 제품 회귀를 실행한다.

~~~powershell
cargo clippy --workspace --exclude pod-spike --exclude diff-spike --all-targets --locked -- -D warnings
cargo test --workspace --exclude pod-spike --exclude diff-spike --locked -- --skip codex_hooks_trusted_via_app_server_in_temp_home
~~~

전체 명령도 앞의 격리 환경과 fixture 빌드를 전제로 한다. POD_REQUIRE_TOOLS=1은 필수 도구가 없어 조건부 생략되는 검사를 실패로 드러내는 데 쓴다. 이 값이 #[ignore]를 해제하지는 않는다. 현재 CI 정의의 push 대상은 main이므로 개선 브랜치의 로컬 commit만으로 CI가 실행됐다고 간주하지 않는다.

## 5 실제 UI와 외부 도구 검사

### 편집기 브라우저 검증

`BROWSER01` (A+U). Windows에서 설치된 WebView2와 격리된 앱 프로필을 사용한다. loopback 테스트 서버에서 제목이 다른 두 페이지를 제공한다. **Ctrl+K → 웹 브라우저 열기**로 탭을 만들고 주소 입력·Enter·링크 이동·뒤로·앞으로·새로고침을 검사한다. 서버 요청과 페이지 내용을 확인한다. 편집기 탭 전환, 창 리사이즈·DPI, 대화상자 열기·닫기, 브라우저 닫기·재실행 시 웹뷰가 현재 도크 영역에 맞고 숨겨진 탭의 웹뷰가 남지 않는지 확인한다. `file:` 같은 비웹 주소의 거부, 서버 오류, WebView2 시작 오류는 각각 따로 기록한다.

자동 검사 진입점은 `cargo test --locked -p pod-app browser_panel`이다. 설치된 WebView2의 별도 루프백 연동 검사는 `cargo test --locked -p pod-app webview2_navigates_loopback_pages_and_reloads -- --ignored`로 실행한다.

### UI 시각과 native 동작

- [capture](../tools/visual/capture.ps1)와 비교 모드로 같은 DPI·폰트·창 크기의 dark/light·고대비·7개 레이아웃을 검사한다. 모든 차이를 검토한 후에만 baseline을 갱신한다.
- 필수 UIA 노드·이름·focus·Invoke와 실제 마우스·키보드 검사를 분리한다. 설정 버튼은 UI05의 세 진입 경로를 모두 검사한다.
- TERM07/UI06/UI08의 실제 Microsoft IME·후보창·Esc와 Narrator 검사는 잠금 해제된 Windows 세션에서 수행한다. 잠긴 데스크톱은 BLOCKED로 남긴다.
- 기존 --ui-fixture는 실제 daemon·저장소·agent가 없는 장면을 사용한다. populated diff·리뷰 전달·Gate·터미널 입력은 실제 격리 daemon과 fixture 저장소를 연결한 앱에서 검사한다.

### 실제 도구 연동

| 대상 | 검증 방법과 완료 조건 |
|---|---|
| rust-analyzer | 설치·버전을 확인한 뒤 아래 ignored 시험을 명시적으로 실행하고, 앱에서 진단→완성→정의→rename/코드 액션→저장까지 확인한다. 다른 지원 언어 서버도 사용 버전·지원 기능을 명시해 각각 검사한다. |
| Claude·Codex | 각각의 실제 CLI 버전·실제 선택 모델·훅 버전·fixture cwd를 기록한다. TASK03→STATE03→REVIEW04→GATE02→TERM09의 작은 수정 시나리오를 실행하고, 생성 코드의 시험 결과와 Pod 상태를 따로 검증한다. |
| SSH | 시험용 Windows 원격 호스트에서 host key·인증→remote-stdio→터미널 입력→단절/재접속을 확인한다. fake SSH 성공과 실제 SSH 성공을 별도 판정하고 WSL/Linux 지원으로 확대 해석하지 않는다. |
| GitHub | fake forge로 실패 조건을 먼저 검사한다. 실제 읽기/생성/merge는 지정한 시험 저장소와 허용 동작 범위에서만 실행하고 생성 리소스·대상 브랜치·결과를 기록한다. 현재 제품 저장소 main/master는 시험 대상으로 쓰지 않는다. |
| 설치·업그레이드 | 기존 사용자 설치가 없는 별도 Windows 사용자/VM에서 SYS01–05를 실행한다. SAC 설정과 배포본 서명 상태를 기록하고 실행 차단을 해제해 통과로 만들지 않는다. 이전 버전과 새 버전의 바이너리 hash를 함께 기록한다. |

~~~powershell
cargo test -p pod-editor --test lsp_lifecycle rust_analyzer_smoke --locked -- --ignored --exact
cargo test -p pod-editor --test lsp_lifecycle rust_analyzer_code_actions --locked -- --ignored --exact
~~~

실제 Codex app-server 훅 신뢰 시험은 [hooks 시험](../crates/pod-cli/tests/hooks.rs)의 임시 HOME 조건과 설치 계획을 확인한 별도 R 층 실행으로 분류한다. 기본 자동 프로필의 skip을 해제했다고 다른 실제 agent 검사가 자동 완료되는 것은 아니다.

## 6 연결 시나리오와 알려진 검증 공백

### 기능 사이의 연결

| 시나리오 | 기능 ID와 절차 | 최종 판정 |
|---|---|---|
| FLOW01 첫 작업 완료 | SYS06 → TASK01/02/03 → TERM01/02 → STATE03 → EDIT05 → REVIEW01 | 독립 worktree의 실제 수정과 시험 결과, 앱·CLI 상태, diff가 일치 |
| FLOW02 리뷰 후 재수정 | REVIEW02/03/04 → 실제 agent 새 턴 → EDIT06 → REVIEW01 | 정확한 의견 한 번 전달, 추가 입력 보존, 새 변경과 최종 파일 일치 |
| FLOW03 사람 승인 후 계속 | ORCH02/04 → GATE01/02 → STATE03 → ORCH03 | 승인 전 대기, 올바른 사람이 응답한 뒤 계속, 정확한 보고와 후행 실행 |
| FLOW04 재시작 복구 | TERM09 → UI03 → REVIEW04 → ORCH10/11 | holder·출력·큐·보관 경로·상태가 restart 경계에서 일관됨 |
| FLOW05 다중 결과 비교 | GROUP01/02/03 → REVIEW01 → PR01 | 같은 base의 독립 결과·검사 실패/성공·선택·안전한 정리가 일치 |
| FLOW06 원격 장애 복구 | REMOTE01/02/03/04 → TERM02/03 | 다른 머신의 같은 ID를 혼동하지 않고 단절 후 출력 복구·입력 중복 방지 |

### 알려진 문제와 별도 범위

- B01–B05는 [2026-11-13 범위 내 검증](25-improvement-execution.md)이 있다. 새 루프에서는 선택한 실행 commit에서 결과를 다시 기록한다.
- B06·B07은 REVIEW03, B08·B09는 EDIT06, B10은 ORCH11, B11은 ORCH04/05의 사건 전달·메일 장애 주입에 연결한다. 미수정 과제를 정상 동작의 기준으로 고정하지 않는다.
- B20은 STATE04와 ORCH08에 연결한다. 청크 차이에 따른 턴 누락은 재현됐지만 과거 통합 시험 시간 초과의 직접 원인은 확정되지 않았다.
- B13/B19는 UI05–08·실제 native 검사, B14는 SYS03, B15는 R 층 실제 도구 검사에 연결한다. 성능 B18은 아래 별도 측정으로 관리한다.
- 원격 파일 앱 연결, Linux PTY/UDS·WSL, 모바일 TLS/QR·Android·인터넷 relay/E2EE는 현재 코드의 구현 경계를 다시 확인한 뒤 별도 기능으로 추가한다. M1 API PASS를 휴대전화 전체 기능 PASS로 집계하지 않는다.
- LSP 문서 formatting 명령은 이 기준의 UI·LSP 구현에서 확인되지 않아 기존 기능으로 집계하지 않는다. 코드 액션이 제공하는 수정과 독립적인 문서 format 기능은 구분한다.
- F01–F15 개선 제안은 [로드맵](24-improvements.md)에 있다. 미구현 기능은 현재 기능 통과율의 분모에 숨겨 넣거나 구현된 것으로 표시하지 않는다. 구현할 때 해당 요구사항과 새 기능 ID·회귀 검사를 연결한다.

### 부하와 시간과 보존

기능 정상 동작 외에 다음 행렬을 추가한다. 합의한 수치 기준이 없는 항목은 첫 측정을 근거로 기준을 고정하고, 수정 후 통과시키기 위해 기준을 느슨하게 바꾸지 않는다.

| 측정 | 입력과 관측 |
|---|---|
| UI와 편집기 지연 | 탭 1/20/100개 복원·외부 일괄 변경, p95 입력 지연·최대 frame time·LSP 수·메모리 |
| 터미널 폭주 | 1/4/16 터미널 동시 출력, frame·입력 응답·drop·queue·메모리, attach/control 독립성 |
| 장기 유지와 복구 | 다수 Task·Run·review queue, daemon/app 재시작과 버전 교체, 남은 holder·임시 파일·모드·LSP 프로세스 |
| 영속 저장 실패 | 쓰기 거부·rename 실패·읽기 실패·중간 crash, 메모리/디스크 상태·원문·재시도·중복 효과 |
| 보안 경계 | 신뢰하지 않은 설정·brief/메일·경로 이탈·옛 token·잘못된 worker·권한 철회, 거절 응답과 실제 부작용 부재 |

## 7 검증과 수정 루프에 사용할 기록

이 문서의 기능 ID는 테스트 파일 재배치나 UI 문구 변경에도 유지한다. 구체적인 테스트 case는 EDIT07-01처럼 기능 ID 아래에 붙이고, 경로·환경·정상·오류·취소·재시작 변형을 cases로 구분한다.

검증 루프는 다음 순서로 동작하도록 작성한다.

1. 기준 commit과 fixture·도구·실행할 기능/층을 고정하고 해당 행의 필수 case 목록을 만든다.
2. 검사를 실행하여 실제 결과·exit code·assertion·화면·RPC·파일/프로세스 상태를 기록한다. 준비 실패와 제품 실패를 구분한다.
3. 실패한 원본 입력과 로그를 보존하고 가능한 한 결정적으로 재현한다. timeout에는 마지막 상태와 입력/출력·대기 중인 조건을 남긴다.
4. feature branch의 담당 worktree에서 최소 수정과 회귀 시험을 추가한다. 현재 실패 조건이 수정 전에는 실패하는지 확인한다.
5. 실패 case→같은 기능의 모든 case→의존·인접 기능→전체 자동 회귀 순으로 재검사한다. 변경된 UI·외부 연동은 해당 층도 다시 검사한다.
6. 결과와 실패 이력을 함께 갱신한다. 같은 원인에 진전 없이 반복되면 BLOCKED로 중단하고 원인·필요한 입력을 기록한다. 시험 삭제·timeout 증가·baseline 자동 갱신으로 루프를 끝내지 않는다.
7. 선언한 기능·층의 필수 case가 모두 PASS이거나 사유 있는 범위 제외로 분리됐을 때 그 범위만 완료 처리한다. main/master 병합과 외부 배포는 이 루프의 자동 완료 동작에 넣지 않는다.

예시 결과 레코드의 값은 실제 실행 값으로 채운다. 아래 예시는 실행 결과가 아니다.

~~~json
{
  "feature_id": "EDIT07",
  "case_id": "EDIT07-01",
  "source_commit": "<full commit SHA>",
  "dirty_patch_hash": null,
  "layer": "A",
  "environment": "<Windows build, DPI, tool versions, fixture id>",
  "binary_sha256": "<built executable hash>",
  "command": "<exact command>",
  "expected": "rename 후 입력이 새 경로의 dirty와 저장에 반영됨",
  "actual": "<observed result>",
  "status": "NOT_RUN",
  "exit_code": null,
  "evidence": [],
  "issue_ids": ["B01"],
  "fix_commit": null,
  "started_at": null,
  "finished_at": null
}
~~~

단순한 시험 개수 합계 대신 기능별 필수 case와 층의 실행률·PASS/FAIL/BLOCKED/NOT_RUN을 집계한다. 한 기능의 여러 실패를 수십 개의 별도 제품 결함으로 부풀리지 않고, 하나의 전체 suite PASS로 미실행 UI·실제 도구 검사를 덮지 않는다.

## 8 유지와 범위 변경

새 명령·RPC·UI 동작을 추가할 때 기능 ID와 노출 범위·검증 절차·기존 시험 또는 새 case를 함께 갱신한다. [확장 가이드](21-extending.md)의 공개 계약 검사를 연결하고 README의 기능 요약도 대조한다.

실제 검증·수정 루프를 실행할 때는 실행별 결과를 별도 디렉터리에 저장하고 이 목록은 요구 동작의 기준으로 유지한다. 과거 PASS·실패 보고는 실행 SHA와 함께 보존한다. 원격 main이 바뀌면 이 로컬 기준에 결과를 섞지 말고 새 기준 SHA와 기능 차이를 먼저 기록한다.

# daemon·보안 재평가 — 2026-11-10

대상: 현재 master `813589efd1afad38d6d87ad7b0e68b52027e0061` (이 작업 브랜치 HEAD와 동일). 비교 기준 `47dff8a`, 이전 `daemon.md`·`security.md`. 제품 코드 변경 없음. **기존 11건: 해결 확인 9, 부분 2, 미해결 0, 회귀 0.** 원래 결함의 해결 여부와 새 경로에서 생긴 결함은 구분했다. 새 보고서의 확인된 잔존·신규 사항은 **치명 0 / 높음 2 / 중간 2 / 낮음 1**이다.

## 1. 점수표

이전 두 보고서의 기준·가중치를 각각 그대로 적용했다. 통합 점수는 두 영역 가중평균의 50:50 평균이다. 기존 전체 IDE 평균 **6.71과 이 두 영역 평균을 혼동하지 않는다**. GUI는 이번 범위와 연결되는 설정·의견 drawer만 정적으로 확인했다.

| 기준 | daemon 가중치 | daemon 전 → 후 | 보안 가중치 | 보안 전 → 후 | 근거 |
|---|---:|---:|---:|---:|---|
| 정확성(버그 없음) | 25% | 7 → 7.5 | 15% | 7 → 7 | ID·hook 경계 수정; 새 재접속 종료 경합과 commit 실행 경계가 남음 |
| 견고성(오류·경합·재시작) | 25% | 6 → 7 | 15% | 6 → 7 | connect deadline·시작 barrier·영속 오류 전파 개선; Job 할당 실패와 retry 종료 조율 미완 |
| 보안·개인정보 | 15% | 8 → 8 | 30% | 6 → 6.5 | SQOS·삭제 실패 표시·LSP 승인은 개선, 새 unsandboxed commit의 filter 실행이 상승폭 제한 |
| 성능·자원 | 10% | 6 → 7 | 10% | 7 → 8 | 일반 입력·출력/forge 수집 상한 도입; holder 자동 응답은 여전히 무제한 |
| 테스트 품질 | 10% | 8 → 8 | 15% | 8 → 8 | 경계·busy pipe·잠긴 파일·감사 테스트 추가; 실제 소비 검증과 격리 실패/경합 테스트 공백 |
| 코드 구조·유지보수성 | 5% | 8 → 8 | 5% | 8 → 8 | 공통 bounded runner·HolderTx 분리는 유익; 종료 수명 관리는 여러 경로에 분산 |
| 문서 정확성 | 5% | 8 → 8 | 5% | 8 → 7 | v19·프로토콜 3 설명 갱신; runner의 트리 보장 및 worker commit의 실행 차단 설명은 과함 |
| 사용성·접근성 | 5% | 8 → 8 | 5% | 7 → 7 | unreachable 사유·오류 표시, drawer 포커스 및 설정 키보드 처리 개선; 실기 접근성 재검증 없음 |
| **영역 전체** | **100%** | **7.05 → 7.525** | **100%** | **6.80 → 7.15** | 동일 산식 |

**통합: 6.925 → 7.3375, 표시 점수 6.93 → 7.34 / 10.** 출하 인증이나 전체 IDE의 새 점수가 아니다.

## 2. 이전 발견 사항 전수 확인

아래 “해결 확인”은 현재 제어 흐름과 회귀 테스트 내용을 대조한 판정이다. 모든 테스트를 실행했다는 뜻은 아니며 실제 실행은 §4에 한정한다.

| 이전 ID | 이전 심각도 | 현재 | 한 줄 근거 |
|---|---|---|---|
| daemon-01 | 높음 | **부분** | `holder.rs:32` 출력 8 MiB, `server/terminal.rs:53` 입력 4 MiB ACK 예산은 추가됐지만 `holder.rs:139,427` 자동 응답은 cap을 우회한다(새 02); `tests/input_budget.rs`의 소비 검증도 불충분(새 05). |
| daemon-02 | 높음 | **해결 확인** | `server/mod.rs:466,1071,1111,1147` 시작 등록·stop 재검사 및 `:346` barrier; `tests/stop_race.rs:135` 시작 중 holder/자식 생존까지 검사. 별개 retry 경합은 새 04. |
| daemon-03 | 높음 | **해결 확인** | `server/mod.rs:813,839,900` 파일명/record ID를 연결 성공과 무관하게 예약; `tests/rediscover.rs:26` 깨진 t4·busy t7 후 t8을 요구, unreachable 재시도/명시적 폐기 추가. 별도 counter 영속화 대신 record를 쓰는 설계는 원래 실패 경로를 막음. |
| daemon-04 | 중간 | **해결 확인** | `server/terminal.rs:286` open 재시도와 handshake에 동일 deadline, `server/mod.rs:848` 동시 5초 rediscovery; `tests/rediscover.rs:80` 첫 응답 9초 이내 단언 및 terminal 단위 busy/silent fixture. |
| daemon-05 | 중간 | **해결 확인** | `pod-hook/src/main.rs:152` 최종 wire 제한, `server/mod.rs:59` 크기 상수 관계 검사; `pod-status` hook 경계 3개 실제 통과, `pod-hook/tests/cli.rs:316` pipe 수신 테스트도 존재. |
| security-01 | 높음 | **해결 확인** | `pod-hook/src/main.rs:197`, `podd/src/transport.rs:38`, `server/terminal.rs:31` identification SQOS; `pod-hook/tests/cli.rs:352` fake 서버의 토큰 수준 검사. 앱/plug는 공통 open_pipe 사용. |
| security-02 | 중간 | **부분** | archive/forge가 `pod-forge/src/bounded.rs:195` 공통 실행기로 전환, drain은 유한하지만 `:212` spawn 후 Job 할당 및 실패 시 직접 자식 kill로 후퇴(새 03). |
| security-03 | 중간 | **해결 확인** | `bounded.rs:137` 읽는 동안 Tail/Cap 적용; `server/repo_config.rs:377` archive 4 KiB tail, `pod-forge/src/runner.rs:165` stdout 16 MiB cap·stderr tail. 과다 출력 fixture 존재. |
| security-04 | 중간 | **해결 확인** | `server/discovery.rs:187` dirty 상태 유지·실패 수집·PERSIST_FAILED, tmp/corrupt 정리; `tests/discovery.rs:447` 잠금→off/preview 오류→재시도·재시작 검증. |
| security-05 | 낮음 | **해결 확인** | `server/mobile/api.rs:128,137` 파싱 실패까지 바깥 감사 경로로, 본문 대신 고정 문구; `tests/mobile.rs:924` 7종 실패의 기록 및 내용 비노출 검사. |
| security-06 | 낮음 | **해결 확인** | `tools/publish-public.ps1:46` 빈/주석 목록 거절, Invoke-Git 종료 코드 검사와 합성 canary; `tools/test-publish-public.ps1` 8종 fixture. 실제 비공개 목록이나 publish는 실행하지 않음. |

경로 축약: 위 `server/*`, `holder.rs`는 `crates/podd/src/`, `tests/*`는 `crates/podd/tests/`; 다른 crate는 표기한 이름 아래이다.

## 3. 발견 사항 — 확인된 결함만

정적 검증은 조건과 실행 순서를 코드로 확인했다는 뜻이며 OOM·경합을 실제 발생시켰다는 뜻이 아니다. S=국소, M=복수 경로·회귀 테스트, L=구조 변경.

| 심각도 | ID | 제목 | 증거 | 영향 | 구체적 수정 제안 | 크기 |
|---|---|---|---|---|---|---|
| 높음 | daemon-security-01 | **해결** (`a5c7da3`, docs/19 §18.5: plumbing 커밋·filter 드라이버 비움·필터 경로 거부·bounded, facts/diff도 차단) — 새 worker commit이 Git clean filter를 샌드박스 밖에서 실행 | `crates/podd/src/git.rs:126,134,139` 차단은 hooks/fsmonitor뿐이며 add는 설정된 filter 실행; `server/orch_runs.rs:1856` daemon에서 호출. 동일 git 옵션의 격리 fixture 결과 `git_exit=0; filter_executed=True` | 기존 repo 설정에 `filter.<name>.clean/process`가 있으면 worker가 `.gitattributes`·해당 filter가 읽는 스크립트를 바꿔 daemon 권한으로 실행시킬 수 있음. **filter 설정 존재가 전제**이며 모든 저장소에서 무조건 탈출한다는 주장은 아님 | 허용된 config만 사용하는 staging 경로 또는 외부 filter 실행을 차단하는 검증·격리 도입. clean/process와 signing 프로그램까지 점검하고 외부 실행 fixture 추가 | M |
| 높음 | daemon-security-02 | **해결 (83fab6d)**: 응답 전용 예산 256 KiB/4096개, 넘으면 버리고 로그. `tests/reply_budget.rs`. — holder의 자동 터미널 응답 큐는 예산 밖 | `crates/podd/src/holder.rs:133,139` push_reply는 무제한 channel에 직접 추가, `:313,427` VT/OSC 응답이 호출; daemon 입력만 `push_from` cap 검사 | 자식이 입력을 읽지 않고 질의 escape를 계속 출력하면 응답이 계속 쌓임. daemon이 출력을 읽거나 연결이 없으면 출력 예산은 이 입력 큐를 막지 못해 holder 메모리 고갈 가능. 기존 daemon-01의 잔여 결함 | 자동 응답에도 총 바이트·개수 예산, 중복 질의 병합/명시적 초과 정책 적용. 읽지 않는 자식+질의 flood 시험 | M |
| 중간 | daemon-security-03 | **해결 (67616ac)**: CREATE_SUSPENDED로 생성→Job 할당→resume, Job 생성/할당 실패는 자식 kill 후 Err, reader/stdin 스레드는 CancelSynchronousIo 후 join. 테스트: 할당 실패 주입·즉시 손자·막힌 stdin. — bounded runner의 Job 격리가 원자적이지 않고 실패를 숨김 | `crates/pod-forge/src/bounded.rs:212` 실행 중인 자식을 spawn한 **뒤** assign; `:213` 실패를 None으로 바꿈, `:261` None이면 직접 자식만 kill. Tree 생성 실패도 동일 | 할당 전 생성된 자손 또는 Job 할당 실패 시 자손이 살아남을 수 있음. drain 제한은 응답 지연을 줄이지만 트리 종료 보장은 아님; 남은 reader/stdin 스레드도 종료 확정 못함 | suspended 생성→Job 할당 성공→resume, 또는 생성 시 Job 지정. 생성/할당 실패는 child 정리 후 오류로 반환. 실패 주입·즉시 자손 생성 fixture | M |
| 중간 | daemon-security-04 | **해결 (d582494)**: retry round를 시작으로 세어 stop barrier에 포함, stop 뒤 붙으면 kill·release. `tests/unreachable.rs::stop_kill_waits_for_a_retry_round_and_ends_its_holder`. — unreachable 재접속이 stop --kill의 snapshot 뒤에 등록될 수 있음 | `server/mod.rs:346,349` starting만 기다린 뒤 terminals 복제; `server/lifecycle.rs:410,439,458,479` retry는 starting에 포함되지 않고 shutdown 확인 없이 terminals 삽입 | kill 대상을 수집한 뒤 handshake가 끝나면 새로 연결된 살아 있는 holder가 Kill/Release 대상에서 빠짐. 예전 run 경합은 고쳤지만 새 retry 경로에 같은 종료 구멍이 생김 | retry를 shutdown barrier에 포함하고 재접속 성공 시 stop 상태에서 Kill/Release 처리. snapshot 직후 handshake를 푸는 결정적 시험 | M |
| 낮음 | daemon-security-05 | 입력 예산 E2E가 실제 입력 전달을 증명하지 않음 | `crates/podd/tests/input_budget.rs:27,43,47,57`: 자식은 출력 없는 findstr, 성공 RPC마다 sent만 증가시키고 바로 kill; 소비한 바이트/내용을 확인하지 않음 | 입력 누락·ACK 조기 발행·남은 큐 폐기를 놓칠 수 있음. “6 MiB reaches”라는 테스트 이름·주석보다 단언이 약함 | 읽은 바이트 수/해시를 돌려주는 fixture로 전체 전달 확인 후 kill; 별도 non-reading fixture로 거절·제어 경로 검사 | S |

01·04·05는 새 경로/테스트의 결함, 02·03은 기존 수정의 잔여 공백이다. 04의 증거 파일은 모두 `crates/podd/src/` 아래이다. 개별 시스템에서의 공격 성공률·OOM 발생량·경합 빈도는 측정하지 않았다.

## 4. 검증 기록·범위

- `git rev-parse HEAD`와 `git rev-parse master` 모두 위 `813589e…`; 시작 시 작업 트리는 깨끗했다. 수정 완료 표의 커밋 표기를 해결 증거로 사용하지 않았다.
- **실행:** 요구된 CARGO_TARGET_DIR 제거, 지정 sccache, jobs=2, `cpu-gate.ps1 -MaxPct 70 -MaxCargo 1`, 가용 RAM ≥8 GiB 확인 후 `cargo test -p pod-status hook::tests --lib` → **3 passed / 0 failed**. 전체 빌드·다른 cargo 테스트는 수행하지 않았다.
- **격리 실행:** `%TEMP%/pod-rescore-filter-<GUID>`에서 새 저장소에 합성 clean filter(`sh probe.sh`, marker 생성 후 cat)와 `*.txt filter=probe`를 두고 `git -c core.hooksPath=disabled-hooks -c core.fsmonitor=false add -- a.txt` 실행. 위 01의 결과를 얻었다. 이는 daemon RPC/Codex 전체 탈출 실험이 아니라 **현재 방어 옵션이 filter를 막지 못한다는 직접 재현**이다. 사용자 저장소 설정·실제 agent·daemon은 사용하지 않았다.
- **테스트 코드 검토:** stop_race, rediscover, unreachable, input_budget, discovery, mobile, hook CLI, bounded/runner, LSP registry, worker allowance/commit 테스트. 실행하지 않은 테스트는 통과했다고 계산하지 않았다. stop_race의 기존 sleep 기반 대기와 시간 상한 테스트는 부하 의존 여지가 있으며 실제 flaky 발생은 관찰하지 않았다.
- **호환성:** `holder_proto.rs:31,37` 지원 1–3, `server/terminal.rs:346` v3 ACK/v2 hello 분기, `server/mod.rs:547,1551` negotiated terminal.list를 `TerminalInfo::for_protocol`로 변환해 v19 unreachable을 구형에 exited+lost로 제공. 모든 구형 바이너리 조합 실행 검증은 하지 않았다.
- **승인 경계:** `.pod/config.toml`은 `server/repo_config.rs:228,435` command hash 대조; `.pod/lsp.json`은 `pod-editor/src/lsp/registry.rs:218,233,295,322` 읽은 동일 bytes의 repo+SHA-256 승인 뒤 merge, `lsp/store.rs:334` 승인 시 재확인. 신규 LSP 실행 차단은 코드/테스트로 확인했으며 이전 두 보고서의 11건과 별개인 editor-01 교차 항목이다.
- **그 밖의 보안 범위:** `pod-remote/src/ssh.rs:53` strict host key/BatchMode/forwarding 금지·고정 명령 및 bounded stderr, `pod-mods/src/runtime.rs:151,232` 메모리/CPU 제한과 manager의 manifest/grant 검사, mobile M1의 allowlist·세션/commit 재검증, `tools/installer/pod.iss:32,90` 사용자 설치 및 unchecked/skipifsilent 훅 설치 경계를 읽었다. 이 범위에서 추가로 확인한 회귀는 없으며 SSH 로그인·설치·모바일 네트워크 실기는 수행하지 않았다.
- **allowance·UI:** `server/codex_allowance.rs:55` 제한된 prefix 목록과 repo opt-out, 사용자 전역 rules를 쓰지 않는 경로 확인. `pod-app/src/review.rs:1656,1666` drawer 포커스 이동/복귀, `pod-ui/src/controls.rs:177,405` 키 처리·Switch 역할, `pod-app/src/chrome.rs:1237` 설정 저장 성공 후 적용 확인. 실제 GUI/스크린리더를 조작하지 않았으며 baseline 이미지를 실행 검증으로 대체하지 않았다.

### 추정·미확인 — 결함 수 제외

Codex의 명령 해석/실행 파일 탐색에 따라 같은 `pod` prefix가 다른 실행 파일로 해석될 가능성, installer cmd wrapper의 `%` 포함 경로 처리, custom state directory의 상속 ACL은 이번에 재현하지 않았다. 기존 리뷰의 가설을 확인된 취약점으로 승격하지 않는다. bounded runner의 실제 Job 실패/할당 전 자손 탈출과 retry-stop 경합도 런타임 주입은 하지 않았으나, 표에는 실패 분기·가능한 순서가 코드로 확정되는 보장 결함으로 기록했다.

## 5. 잘 된 점·우선 수정

- hook wire 제한과 SQOS는 생산 경로·공통 상수·회귀 fixture가 맞물린 수정이다.
- 연결 실패를 숨기거나 ID를 재사용하는 대신 unreachable과 제한된 재시도로 드러낸 점이 좋다.
- 개인정보 삭제 실패 전파, LSP 내용 승인, 읽는 동안의 출력 상한은 이전보다 명확한 방어다.

우선순위:

1. **01:** worker commit의 외부 프로그램 실행 경계를 먼저 닫고 clean/process filter 회귀 시험을 추가한다.
2. **02:** 자동 응답을 포함한 holder 입력 전체에 예산을 적용한다.
3. **03:** 프로세스 생성과 Job 격리를 원자화하고 격리 실패를 오류로 반환한다.
4. **04:** run과 retry 모두 같은 shutdown 수명 규칙을 따르게 한다.
5. **05:** RPC 수락량 대신 실제 소비량을 검증하고, 느린 소비자·종료 경합 시험을 강화한다.

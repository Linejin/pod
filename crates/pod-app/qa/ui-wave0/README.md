# UI wave 0 — dock·branch·select (2026-11-11)

브랜치 `Linejin/astra-ui`. fixture와 별도 `target/ui-wave0/<uuid>` 설정만 사용한다.
실제 podd·Claude·Codex·사용자 설정은 사용하지 않는다.

- `dock-activation.ps1`: release fixture의 설정 장면을 보조 모니터에 띄우고,
  별도 fixture 창으로 포커스를 옮겼다 돌아오는 3회마다 UIA 설정 내용과
  `상태` 탭 부재를 검사한다(`tools/visual/uia-required.ps1`).
  초기 UIA 연결 뒤 1초 대기 외에는 누락 트리를 재시도로 숨기지 않는다.
  실행할 수 있을 때 DPI-aware PrintWindow 이미지와 각 회차 트리를 같은
  디렉터리에 저장한다. `uia-check-selftest.ps1`은 누락·뜻밖의 인스펙터를 거부하는
  판정 함수를 합성 트리로 검사한다.
- `native_qa::focus::dock_activation_does_not_open_inspector`: headless 활성화 및
  부분 프레임에서도 인스펙터가 생성되지 않는지 검사한다.
  GPUI test platform에는 UIA adapter가 없어 UIA 검사는 위 실제 창에서 한다.
- `native_qa::keyboard::status_branch_tracks_task_and_hides_before_totals`: 포커스한 작업을
  따라 branch가 바뀌고, 긴 이름은 원문을 유지하며, branch가 없는 작업에서는
  숨는다. 1399px에서는 branch부터 숨고 기존 집계가 남는다.
- `pod-ui --test select_scroll`: Alt+Tab, 프로그램으로 옮긴 포커스, blur,
  부모 Escape에서 선택값·포커스를 빼앗지 않고 닫는다. 기존 스크롤·Tab 검사도 유지한다.

실행 예시(각 cargo/앱 실행 전 CPU gate와 여유 메모리 8GB 확인):

```powershell
cargo test -p pod-ui --test select_scroll
cargo test -p pod-app native_qa::keyboard::status_branch
cargo test -p pod-app native_qa::focus::dock_activation
cargo build --release -p pod-app
./crates/pod-app/qa/ui-wave0/dock-activation.ps1 -Theme dark
./crates/pod-app/qa/ui-wave0/dock-activation.ps1 -Theme light
```

## 실제 전경 검사: NOT RUN / 보류

Windows 세션이 잠겨 있다(`OpenInputDesktop` error 5, `LogonUI=True`).
수정 전 release의 초기 UIA 설정 트리 100개 노드는 읽었지만 전경 활성화가
거부되어 비교를 완료하지 못했다. Coordinator 지시대로 잠금 해제가 필요한
검사와 스크린샷은 다음 세션으로 미룬다. **실제 전경 재현·수정 확인이나 시각
검증을 통과했다고 주장하지 않는다.** `native-activation-not-run.txt` 참조.

dock 패치의 근거: GPUI 0.3.11의 캐시 재사용 경로는 paint·hitbox·dispatch를
재사용하지만 접근성 노드는 옮기지 않는다. 접근성 클라이언트가 연결된 동안
dock 내용의 view cache를 끄고 기존 absolute/full-size 배치를 유지한다.
접근성 비활성 시 캐시는 유지한다. Apache-2.0 변경 고지는 vendor에 기록했다.
headless 활성화 검사에서는 `상태` 인스펙터가 생성되지 않는다. 과거의 자동 열림
관찰은 잠금 해제 뒤 위 UIA 스크립트로 별도 확인해야 한다.

기준 이미지는 변경하지 않았다. 예상 시각 변경은 **1400 logical px 이상에서
작업에 포커스했을 때 상태 표시줄 branch 추가**다. 기존 1440×960 physical,
150% 장면에서는 branch가 숨는다. 픽셀 비교·재캡처는 coordinator가 담당한다.

## 변경·검사 결과

| 항목 | 커밋 | 원인과 수정 |
|---|---|---|
| select-blur | `9cde4d9` | Tab 처리만으로는 포커스 경로 변경을 잡지 못했다. 수명이 유지되는 focus-out·창 활성화 구독으로 닫고, mouse-down-out을 유지했다. |
| status-branch | `00c0a70` | 기존 task.branch를 현재 패널 문맥에서 읽어 아이콘·전체 이름 tooltip/UIA를 표시한다. branch가 없으면 숨기고, 폭이 줄면 집계보다 먼저 숨긴다. |
| dock-uia | `76a6b39` | 접근성 노드를 재사용하지 않는 view cache를 UIA 활성 중 우회한다. UIA 회귀 스크립트와 인스펙터 자동 생성 방지 headless 검사를 추가했다. |

전체 workspace 검사 1회: **947 성공, 0 실패, 3 무시**. 마지막 부모 Escape
회귀 검사를 포함한 select 테스트 4건도 별도로 통과했다. pod-app·pod-ui
전체 타깃과 vendor dock 라이브러리의 clippy는 경고 0건이다. cargo deny는
`advisories ok, bans ok, licenses ok, sources ok`로 통과했다(기존 미사용 ISC
허용 항목 경고). 정확한 명령·결과는 [verification.txt](verification.txt)에 있다.

남은 작업은 잠금 해제 뒤 dark/light 전경 UIA·스크린샷 확인과 coordinator의
기준 이미지 검토다. master 병합·push는 하지 않았다.

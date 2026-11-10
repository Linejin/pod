# 집중 레일 (프리셋 02) — 시각 QA

`tools/visual/capture.ps1 -Layout focus-rail -Style <style> -Scenes rail,rail-attention[,rail-switcher,rail-workspace]`
로 보조 모니터에서 캡처(DPI 인식, 1440×960). 파일 이름은 `<dark|light>-<장면>-<스타일>-focus-rail.png`, 옆의 `.tree.txt`는 UIA 트리.

| 장면 | 내용 |
|---|---|
| `rail` | task-000 현재 작업, 기본 배치(넓은 Worker 0 + 오른쪽 스택), 탭 `+1` 넘침 |
| `rail-attention` | 알림 서랍: 다른 작업의 승인 요청 → 실패 순 |
| `rail-switcher` | 작업 전환 목록(모든 작업, 현재 작업 선택) |
| `rail-workspace` | 작업 공간 서랍(기존 사이드바 재사용) |

스타일: workbench · quiet · islands · mono · warm · access, 각각 다크/라이트. 모두 테마 토큰·메트릭만 사용(islands는 레일·독이 섬 모양, access는 큰 글자와 강한 테두리).

## 갤러리 목업(`docs/design/gallery/images/02-codex-b-*.png`)과 비교

같은 점: 44px 아이콘 레일(작업 공간·터미널·파일·에이전트, 아래쪽 알림 배지), 위쪽 현재 작업 탭(상태 점·개수 배지·`+N`·작업 전환), 현재 작업만 보이는 독.

의도한 차이:
- 픽스처에는 데몬이 없어 터미널에 "연결 끊김 · 다시 연결 중"이 보임.
- 목업의 작업 헤더 줄·파일 탭은 없음. 작업 정보는 탭과 상태 표시줄로 충분하고 독 공간을 아낌.
- 픽스처 작업 제목이 길어 탭이 말줄임되고 보이는 탭 수가 적음(현재 탭은 항상 보임).
- 다른 작업의 승인/실패는 목업처럼 배지만이 아니라 서랍 목록(Ctrl+Alt+A)으로도 바로 이동 가능.

## 키보드 QA

`../../native/rail-keyboard.txt` — 헤드리스 GPUI 흐름(`layouts::rail::tests`) 실행 기록.

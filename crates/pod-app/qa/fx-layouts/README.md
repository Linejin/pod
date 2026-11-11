# fx-layouts 증거 (2026-11-11)

리팩터 뒤 남은 레이아웃 버그 세 개(docs/20-refactor/app.md "후속으로 남김")를 고친 기록.

| 버그 | 고침 | 회귀 테스트(고치기 전 실패) |
|---|---|---|
| 블록 터미널에 dock이 없어 설정·승인함·에이전트·상태 검사·리뷰·편집기·오케스트레이션 패널이 보이지 않음 | 오른쪽 패널 서랍이 kit `Slot::Dock`을 그린다. 패널이 열리거나 작업 공간이 dock 패널에 포커스를 주면 열림, F6 순환에 포함, Esc·닫기로 원래 영역에 포커스 복귀 | `blocks_qa::a_panel_opened_in_the_block_terminal_is_drawn_with_the_keyboard_in_it`(전: focus "workspace root"), `panels_opened_…_esc_returns`(7종), `f6_cycles_through_the_open_drawer_…`, `entering_with_a_focused_panel_…`, `header_buttons_stay_beside_the_open_drawer_in_a_narrow_window`(전: 단추 x 736 > 서랍 x 609), `blocks::tests::the_panel_drawer_…` |
| 칸반·리뷰에서 연 오케스트레이션 화면이 숨은 "패널" 탭 뒤에 열림 | kit `other_panels()`에 orch 화면, `PanelWatch`(포커스를 받은 dock 패널 = 요청) | `kanban_qa::orch_pages_opened_on_the_board_show_in_the_drawer`, `review_qa::orch_pages_opened_meanwhile_show_in_the_panel_view`, `kit::tests::other_panels_…`(+1) |
| 칸반 서랍의 dock에서 F6이 보드로 돌아가지 않음 | `switch_region`이 dock focus를 먼저 확인 | `kanban_qa::f6_from_the_drawers_dock_returns_to_the_board` |

## V

릴리스 빌드(`79cacfe`)로 refactor 8개 행렬(`qa/refactor/contract.json` `V.matrices`)을 `a-f0/before-12e3c76`과 비교: **56/56, 바뀐 픽셀 0, PNG SHA-256 56/56 동일**. 패널이 없는 장면은 단추·서랍이 없어 그대로다. 그래서 baseline을 다시 찍지 않았다.

## 새 동작 캡처 (fixture, 1440×960, 144 DPI)

- `blocks/`: 블록 터미널 × settings · orch-board · gates, 다크·라이트. 오른쪽 서랍에 패널, 머리줄 제목은 말줄임, 원본 터미널·패널 서랍 단추는 보임.
- `kanban/`: orch-board 장면에서 서랍이 "패널" 보기(실행 보드).
- `review/`: orch-board 장면에서 캔버스가 "패널" 탭(실행 보드).

남은 것: 좁은 서랍(960px 창에서 약 350px)에서 승인함의 메모 입력 등 일부 패널 내용이 오른쪽에서 잘린다(패널 쪽 최소 폭 문제). `tools/visual/uia-required.ps1`의 "block-terminal draws no dock" 주석은 이 카드 소유가 아니라 그대로 두었다.

# 칸반 보드 캡처 (docs/10 §D5.5, 프리셋 12)

```powershell
foreach ($s in 'kanban','quiet','islands','mono','warm','access') {
  & tools/visual/capture.ps1 -Output crates/pod-app/qa/layouts/kanban -Scenes @('workspace') -Style $s -Layout kanban
}
```

fixture `workspace` 장면(데몬 없음, 100작업/300터미널), 보조 모니터 1440×960 physical @150%, 다크·라이트.
파일: `<dark|light>-workspace-<style>-kanban.png` + UIA 트리. 비교 대상: `docs/design/gallery/images/12-kanban-<dark|light>.png`.
모든 캡처를 목업과 나란히 놓고 직접 확인했다(2026-11-10).

## 목업과 같은 점

열 3개(아이콘·이름·개수), 카드: 상태 아이콘 + `에이전트 · 이름`, 작업 제목, 고정폭 마지막 줄 상자, 동작 단추, 오른쪽 상태 문구, 응답 필요 카드의 왼쪽 띠(주황=응답, 빨강=실패), 선택 카드의 강조 테두리, 끌기 손잡이, 하단 서랍, 머리줄의 “모든 작업 · 에이전트 N · 작업 M”과 필터.

## 의도한 차이

- **서랍이 비어 있다**: 캡처는 카드를 열지 않은 첫 화면이라 서랍은 안내 한 줄 높이(카드를 Enter로 열면 터미널 높이 32%/60%). fixture에는 데몬이 없어 터미널 내용을 보여 줄 수 없다.
- **승인/거부 단추 없음**: fixture에는 대기 중 gate가 없다 → gate 없는 질문 카드는 “응답하기”(서랍 터미널). 승인/거부는 headless 테스트(`kanban_qa.rs`)로 확인.
- **목업의 “이번만 허용”** 대신 gate 경로의 **승인/거부**(podd가 지원하는 결정만).
- **경과 시간(12초) 대신 상태 문구**(`승인 필요`, `실패 종료 (1)`): 경과 시간을 갱신하려면 타이머가 필요하다(폴링 없음 규칙).
- **필터**: 목업의 저장소/에이전트 알약 대신 드롭다운 단추 `저장소: …`, `에이전트: …`. 힌트 문구는 상태줄이 아니라 머리줄에 둔다(상태줄은 작업 공간 공용).
- **제목줄**: 목업의 보드/터미널/리뷰 탭은 없다(레이아웃 전환은 설정 › 모양과 명령 팔레트).
- **열 머리 “…” 메뉴 없음**, 끌기 손잡이 아이콘은 Lucide `chevrons-up-down`(세로 점 6개 아이콘이 아이콘 세트에 없다).
- 카드 이름이 `Powershell · Worker N`: fixture 터미널이 모두 powershell이다.

## 스타일별

| 스타일 | 확인 |
|---|---|
| `kanban` (12) | 청록 강조, layered 면(열=surface, 카드=raised), 반경 8 |
| `quiet` (04) | flat: 열·카드 1px 경계, 그림자 없음 |
| `islands` (08) | 열이 반경 12 섬 + 그림자, 넓은 간격 |
| `mono` (09) | 경계 없음, 흑백 선택 테두리, 단추는 면으로 구분 |
| `warm` (10) | 웜 그레이 면, 녹 갈색 선택 테두리 |
| `access` (14) | 2px 경계, UI 15px — 카드 높이가 글자에 맞춰 커지고 잘리지 않음(마지막 줄은 말줄임) |

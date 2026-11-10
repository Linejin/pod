# 설정 category 열·select·switch (ui-10 후속, 2026-11-10)

- `before/`: master `88cf7f4`의 기준 캡처(모든 행이 segment, category 열 없음).
- `after/`: 이 브랜치의 release 빌드, 보조 모니터, 144 DPI. `capture.ps1 -Scenes settings`와 `behavior.ps1`.
- `behavior.ps1`: 창에 입력을 보내(전경·포인터 변경 없음) category ↓, select 열기·↓·Enter·닫힌 상태 ↓, switch 클릭·Space를 확인하고 UIA 상태를 `after/<theme>-behavior.txt`에 남긴다.
- `uia-probe.ps1`: TabItem `IsSelected`, ComboBox `ExpandCollapse`·값, ListItem `IsSelected`, Toggle 상태를 출력한다.

결과(dark·light 같음): category ↓로 `글꼴` 선택, select가 `Expanded`로 열리고 Enter 뒤 `Collapsed` + 값 저장(`Consolas`), 닫힌 select의 ↓는 값을 바로 바꾼다(`D2Coding`), switch는 클릭과 Space에 각각 한 번씩 바뀐다.

알려진 기존 문제(master도 같음): 창이 전경으로 활성화되면 dock 패널 내용이 UIA 트리에서 빠지고 `상태` 패널이 열린다. 그래서 이 스크립트는 창을 활성화하지 않는다.

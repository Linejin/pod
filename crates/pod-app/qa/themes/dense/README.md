# 고밀도 + 세부 열 레이아웃 캡처 (THEMES 2단계, docs/10 §D5.10)

`--ui-fixture --fixture-scene dense`(패널 4개 격자, `fixture-1`에 gate `g1`, `fixture-3` 질문, `fixture-2` 실패 종료, 완료 알림 1개, 세부 정보 = `fixture-1`). 1440×960 물리 px, 보조 모니터(DPI 144 = 150%), Malgun Gothic / Cascadia Mono.

```powershell
powershell -NoProfile -File tools/visual/capture.ps1 -Scenes dense -Layout dense-details -Style dense -Output crates/pod-app/qa/themes/dense/07
# 스타일 확인: quiet islands mono warm access → styles/<style>/
```

| 폴더 | 비교 대상 |
|---|---|
| `07/` | `docs/design/gallery/images/07-dense-{dark,light}.png` |
| `styles/` | 다른 스타일 + 이 레이아웃: 토큰·면 모델·밀도만 바뀌는지 |

## 갤러리와 의도적으로 다른 점

- **트리**: 시안의 트리(“작업 · pod”, 역할 이름 `Claude · 구현`)와 그 아래 “최근 이벤트”는 만들지 않았다. 트리는 모든 레이아웃이 같은 작업 공간 sidebar(fixture 이름 Worker N)이고, 최근 이벤트는 아래 **출력** 패널에 있다(같은 정보를 두 곳에 두지 않음).
- **dock 머리**: 시안의 `터미널 4 · 리뷰 · terminal.rs` 탭과 터미널 머리의 상태 문구는 dock 쪽 기능이라 이 레이아웃에서 만들지 않았다(모든 레이아웃이 같은 dock, 상태는 트리·세부 정보에 있음).
- **출력 머리**: 시안의 “문제 0 · 로그” 대신 “출력 이벤트 N” + 지우기 · 닫기. 문제(진단) 목록은 아직 없다.
- **출력 줄**: 시안 `hook Codex · 리뷰 → approval_requested`의 영어 이벤트 이름 대신 한국어 상태 문구(“승인 필요”, “실패 종료 (1)”, “승인 요청 · 제목: 명령”). 출처 칸은 podd가 준 값(hook/osc/gate/pod/알림).
- **세부 정보**: 시안의 이름·상태·명령·프로그램·브랜치·경과·출처에 작업·작업 폴더·PR·마지막 줄·상태 기록을 더했다. 동작은 시안의 `이번만 허용 (y) / 항상 허용 (a) / 거부 (n)` 중 gate 모델에 있는 이번만 허용·거부만 단추로(“항상 허용”은 gate에 없음, 터미널 입력을 대신 보내지 않음) + 터미널 열기·변경 검토·파일 열기·PR 보기.
- **활동 막대**: 시안의 4개(작업 · 파일 · 에이전트 · 검색)에 승인 요청과 아래쪽 출력/세부 정보 보이기 단추를 더했다(키보드 사용자를 위해 숨긴 패널을 다시 여는 길).
- **알림 토스트**: 트리 아래(작업대와 같은 자리).
- **터미널 내용**: fixture는 daemon이 없어 빈 터미널과 “연결 끊김 · 다시 연결 중…”(다른 baseline과 같음). 마지막 줄은 fixture 응답.
- **시각**: 출력은 기록을 시작한 때의 상태만 넣으므로 fixture에서는 모두 실행 시각.

## 확인한 것

- 에이전트 표시(§D5.11): `07/`은 master 병합 뒤 다시 캡처(출력 줄·세부 정보 “에이전트”·트리에 mark). `styles/`는 그 전 캡처라 mark가 없다(레이아웃·토큰 확인용).

- 07 다크/라이트: 22px 행·12px UI, 글자 잘림 없음(긴 이름·명령만 말줄임), 선택 행·gate 단추 대비, 무채색 바탕에 응답/실패만 색.
- 스타일 × 다크/라이트(`styles/`):

| 스타일 | 다크 | 라이트 | 메모 |
|---|---|---|---|
| quiet (04) | 확인 | 확인 | 1px 선, 행 28 |
| islands (08) | 확인 | 확인 | 트리·가운데(dock+출력)·세부 정보가 각각 둥근 섬, 10px 간격 |
| mono (09) | 확인 | 확인 | 선 없음, 출력·세부 정보는 면과 여백으로 구분 |
| warm (10) | 확인 | 확인 | 녹 갈색 강조가 활동 막대 선택·링크에 |
| access (14) | 확인 | 확인 | 40px 행·15px 글자: 출력 줄은 말줄임, 세부 정보는 세로 스크롤(잘림 없음) |

- 100/125/150/175%와 글자 20px 배치는 headless 테스트 `korean_text_fits_at_100_to_175_percent_and_large_fonts`(DirectWrite 측정)로 확인. 실제 125/175% 화면 캡처는 하지 않았다(OS 배율을 바꾸지 않음).

키보드 흐름 증거(headless, 실제 키 바인딩): `crates/pod-app/qa/native/dense.txt`.

# 응답 대기열·관제 레이아웃 캡처 (THEMES 2단계, docs/10 §D5.7)

`--ui-fixture --fixture-scene queue`(gate 1개 `g1`이 blocked `fixture-1`에, `fixture-3`은 질문, 패널 4개 격자, gate 카드 선택 + 목록 focus). 1440×960 물리 px, 보조 모니터(DPI 144 = 150%), Malgun Gothic.

```powershell
powershell -NoProfile -File tools/visual/capture.ps1 -Scenes queue -Layout queue -Style workbench -Output crates/pod-app/qa/themes/queue/03
powershell -NoProfile -File tools/visual/capture.ps1 -Scenes queue -Layout queue -Style mission   -Output crates/pod-app/qa/themes/queue/06
# 스타일 확인: quiet islands mono warm access → styles/
```

| 폴더 | 비교 대상 |
|---|---|
| `03/` | `docs/design/gallery/images/03-codex-c-{dark,light}.png` |
| `06/` | `docs/design/gallery/images/06-claude-c-{dark,light}.png` |
| `styles/` | 다른 스타일 + 대기열(03 구조): 토큰·면 모델·밀도만 바뀌는지 |

## 갤러리와 의도적으로 다른 점

- **집계 4개**(03): 시안은 작업 중/응답 필요/완료 3개. 과제 요구대로 `실패`를 더했다(0이 아니면 danger 색).
- **응답 필요 수**: 실패는 `응답 필요` 수에 넣지 않고 따로 센다(06 시안의 “응답 필요 2 · 카드 3장”과 같은 규칙). 카드는 요청 → 실패 순.
- **인라인 승인**: 시안 06의 `이번만 허용 / 거부 / 열기`는 Pod의 승인 gate(`gate.resolve`, 한 번 결정)가 있을 때만 보인다. 에이전트 자체 권한 질문(gate 없음)은 터미널에서 답하므로 `열기`만 — 터미널 입력을 대신 보내지 않는다. 시안의 “항상 허용”은 gate 모델에 없어 뺐다.
- **03 dock 머리**: 시안의 “한글 작업과 레이아웃 · pod/task-000” 작업 머리줄과 탭(터미널/layout.rs)은 dock 쪽 기능이라 이 레이아웃에서 만들지 않았다(모든 레이아웃이 같은 dock).
- **06 제목줄**: 시안의 “관제 pod · 작업 6”, 알림 종 대신 기존 제목줄(Pod · 맥락 · 검색 · 설정)에 상태 알약만 더했다. 알림은 상태줄과 Ctrl+Shift+B.
- **06 선택 카드**: 시안처럼 채운 청록 면 대신 raised 면 + 강조색 2px 테두리. 채우면 라이트에서 `이번만 허용`(success 면)과 대비가 부족했다(첫 캡처에서 확인 후 수정).
- **06 활동 막대 아이콘**: 대기열 · 에이전트 트리 · 파일 · 변경 검토 · 승인 요청 · (아래) 설정. 시안의 4개 아이콘 뜻이 정해져 있지 않아 기존 명령에 맞췄다.
- **카드 내용**: fixture 작업 제목(“Task 000 - 한글 작업과 레이아웃 🔎”)과 Worker 이름을 그대로 쓴다. 시안의 `Codex · 리뷰` 같은 역할 이름은 podd가 주지 않는다.
- **터미널 내용**: fixture는 daemon이 없어 빈 터미널과 “연결 끊김 · 다시 연결 중…”이 보인다(다른 baseline과 같음).
- **경과 시간**: 타이머가 없어(폴링 금지) 다음 변경 알림 때 갱신된다.

## 확인한 것

- 03 다크/라이트, 06 다크/라이트: 글자 잘림 없음(카드 제목만 말줄임), 선택·focus 막대, gate 버튼 대비.
- 스타일 `quiet` `islands` `mono` `warm` `access` × 다크/라이트(`styles/`): 아래 표.
- 100/125/150/175% 배치는 headless 테스트 `korean_text_fits_at_100_to_175_percent`(실제 DirectWrite 글꼴 측정)로 확인. 실제 125/175% 화면 캡처는 하지 않았다(OS 배율을 바꾸지 않음).

| 스타일 | 다크 | 라이트 | 메모 |
|---|---|---|---|
| quiet (04) | 확인 | 확인 | 기본 토큰, 1px 선 |
| islands (08) | 확인 | 확인 | 대기열과 dock이 각각 둥근 섬, 10px 간격 |
| mono (09) | 확인 | 확인 | 선 없음, 선택은 면 + 막대 |
| warm (10) | 확인 | 확인 | 녹 갈색 강조가 선택 막대·열기에 |
| access (14) | 확인 | 확인 | 2px 선, 15px 글자·40px 행에서도 집계·버튼 안 잘림 |

키보드 흐름 증거(headless, 실제 키 바인딩·DirectWrite): `crates/pod-app/qa/native/queue.txt`.

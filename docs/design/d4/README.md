# Pod D4 디자인 검토

사양: [docs/10 §D4](../../10-ui-design.md#d4-세련됨--밀도위계정밀도-재설계).

세 안의 정보 구조를 먼저 비교한다. A는 기존 트리를 정돈한 기본 권장안이며 아직 최종 선택은 아니다.

| 방향 | 다크 작업 화면 | 라이트 작업 화면 |
|---|---|---|
| A 정밀한 작업대 | [보기](dark-workspace.png) | [보기](light-workspace.png) |
| B 한 작업에 집중 | [보기](b-dark-workspace.png) | [보기](b-light-workspace.png) |
| C 응답 중심 관제 | [보기](c-dark-workspace.png) | [보기](c-light-workspace.png) |

- A: `{dark,light}-{workspace,review,agents}.png` 및 같은 이름 HTML.
- B/C: `{b,c}-{dark,light}-{workspace,review,agents}.png` 및 HTML.
- A 기존 상태 비교: `{dark,light}-workspace-empty.png`, `{dark,light}-review-error.png`.
- Before/after 6장: `{dark,light}-{workspace,review,agents}-before-after.png`. 왼쪽은 기존 fixture, 오른쪽은 합성 데이터가 들어간 제안이다. 동일한 live state의 성능/QA 비교가 아니다.
- `tokens.json`: dark/light/HC-dark/HC-light의 정확한 색값.
- `contrast.json`: D4 356쌍 모두 통과. 현행 contrast.py 72쌍도 통과.

재현: 저장소 root에서 `python docs/design/d4/build.py`. Python의 Pillow/Playwright와 설치된 Windows Edge가 필요하다. 네트워크를 사용하지 않는다. 기존 `tools/visual/contrast.py`를 실행하므로 기존 D3 contrast 결과 파일을 동일 값으로 다시 쓴다. 앱/기준 screenshot은 수정하지 않는다.

전체 28개 baseline을 4장씩 나눈 contact sheet로 모두 검토했다. 관찰 결과는 D4.1에 화면별로 기록했다. 새 목업은 960×640 logical, 1.5 device scale, 1440×960 PNG이며 HTML의 self-contained CSS/SVG는 직접 작성했다. 목업 선 아이콘은 외부 자산이 아닌 schematic placeholder이고 제품에는 고지 포함 Lucide를 사용한다. 한국어 문장과 comment wrapping을 확인했지만 실제 GPUI, IME, Narrator, DPI별 raster QA를 대신하지 않는다.

기준 commit: `6365f51`; 검토일: 2026-11-09. 외부 조사 출처와 라이선스 링크는 사양에 기재했다. 앱 코드 변경·merge·push는 하지 않았다.

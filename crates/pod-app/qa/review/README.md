# Review 패널: 의견 서랍(ui-05)과 어절 줄바꿈

fixture만 사용(`--ui-fixture --fixture-scene review`, 레이아웃 `review`, dark). 보조 모니터 144 DPI(150%),
DPI-aware `PrintWindow`. `before-*`/`after-*`는 `tools/visual/capture.ps1 -Scenes review -Layout review`,
800은 `-Width 1200 -Height 960`(800 logical), 960은 기본 1440×960. 모든 사진은 직접 보았다.

| 사진 | 확인한 것 |
|---|---|
| `before-800` | 의견 열이 캔버스(560 미만) 밖으로 나가 안내문·입력·보내기가 오른쪽에서 잘린다. |
| `after-800` | 캔버스 < 560이라 의견 열이 서랍으로 바뀐다. diff가 폭을 다 쓰고 가로 스크롤 없음. |
| `after-800-drawer-open` | 툴바 `의견`(UIA 클릭)으로 연 서랍. 캔버스 안, 입력·`미해결 0개 보내기` 온전. UIA: `대화 상자 의견 서랍`. |
| `before-960` | 안내문 `누르세`/`요.`가 어절 중간에서 끊긴다. |
| `after-960` | `+를`/`누르세요.`처럼 어절 사이에서만 줄바꿈. |

`drawer.ps1 -Start`로 띄우고 `-AppPid PID -Name NAME`으로 찍는다.
오류 배너(`작업 폴`/`더가 없습니다`)는 `pod_ui::banner`라 next-settings 담당(pod-ui keep-all)으로 넘겼다.
실제 앱 Esc는 `orca computer press-key`가 창 포커스를 얻지 못해 확인하지 못했다. Esc·Alt+C·포커스 이동은
헤드리스 테스트 `layouts::review_qa::comments_column_fits_or_is_a_drawer_at_800_960_1440`(800/960/1440)이 확인한다.

## 긴 어절(ui-editor-05)

의견·PR 본문의 `words()`가 열보다 긴 경로·식별자를 말줄임하던 것을 고쳤다. 열에 들어가는 어절은 그대로 한 줄,
열보다 긴 어절 하나만 열 폭에서 글자 단위로 줄바꿈한다. 헤드리스 테스트
`layouts::review_qa::long_comment_tokens_wrap_inside_the_column_at_800_960`이 800(서랍)·960(열)에서
긴 경로가 여러 줄로 열 안에 모두 보이고 짧은 어절은 한 줄인지 확인한다(옛 truncate로 되돌리면 실패).
fixture 장면에 긴 의견이 없어 새 사진은 찍지 않았다. 기존 사진(`after-800-drawer-open` 등)은 의견이 없어 바뀌지 않는다.

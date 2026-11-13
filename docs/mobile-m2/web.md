# M2 모바일 번들 화면

Apache-2.0 자체 HTML/CSS/JS만 사용한다. 작업 목록, 전체 Gate 상세,
출력(생략 표시), 결정 선택, 짧은 메시지, 전달 상태를 제공한다.
모든 값은 텍스트이며 제어·bidi 문자는 가시적인 escape로 표시한다.
미완성 상세, 오프라인, 백그라운드, native stale 알림, 25초 경과는
결정과 메시지를 막는다. 복귀 후 새 상세를 읽어야 한다.

`PodNative.request({schema: 'pod-bridge/1', id, intent, params})`는
`tasks/task/output/decide/message/delivery`만 받는다. 응답은
`{schema, id, result}`이며 [DTO 계약](contracts.md)의 예산을 검사한다.
`contract.js`는 화면과 desktop double이 공유하는 닫힌 검사기다.
결정은 typed intent일 뿐이며 최종 확인·operation ID·인증·CSRF·통신은
native 소유다. JS에는 키나 토큰을 주지 않는다. 메시지 코멘트는 현재 빈 배열이다.

저장소 루트에서 실행:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools/check-mobile-assets.ps1 -SelfTest
powershell -NoProfile -ExecutionPolicy Bypass -File tools/check-mobile-assets.ps1
node --test mobile/tests/
```

Node 내장 모듈과 설치된 Microsoft Edge만 사용하며 다운로드는 없다.
CPU gate 후 임시 프로필을 만들고 DevTools pipe로 실행한다. 디버깅
포트·HTTP 서버는 열지 않는다. Fetch interception은 정확한 번들 경로만
로컬 바이트와 고정 CSP 헤더로 응답하고 나머지는 실패시킨다.
종료 시 Edge 프로세스 트리와 프로필을 정리한다.

double의 Origin/CSRF 검사는 서버 구현을 대체하지 않는다(T1).
native 확인창, Android main-frame 판별·키 보관·실제 WebView navigation,
APK 서명·잠금·복귀는 M3 실기기 검증이 남는다.

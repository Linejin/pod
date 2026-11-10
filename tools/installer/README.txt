Pod - 에이전트 코딩 IDE (Windows)
=================================

Pod는 Claude Code와 Codex 같은 코딩 에이전트를 여러 개 띄워 두고, 작업(Task)별
worktree에서 돌리고, 변경을 리뷰하는 데스크톱 앱입니다. 라이선스: Apache-2.0
(LICENSE.txt). 함께 들어 있는 오픈 소스 구성 요소: THIRD-PARTY-NOTICES.txt.

들어 있는 파일
  pod-app.exe       데스크톱 앱 (시작 메뉴 "Pod")
  podd.exe          데몬. 앱이나 pod 명령이 필요할 때 자동으로 띄웁니다
  pod-holder.exe    터미널 하나당 하나. 데몬·앱을 껐다 켜도 터미널이 계속 돕니다
  pod.exe           명령줄 도구 (pod --help)
  pod-hook.exe      에이전트 상태 훅 (pod hooks install이 %LOCALAPPDATA%\Pod\bin으로 복사)
  conpty.dll, x64\OpenConsole.exe   Microsoft ConPTY (MIT, licenses\ConPTY-LICENSE.txt)

처음 쓸 때
  1. 시작 메뉴에서 Pod를 엽니다.
  2. 에이전트 상태(작업 중/입력 필요/완료) 표시를 쓰려면 한 번:
       pod hooks install
     ~/.claude/settings.json, ~/.codex/hooks.json을 백업한 뒤 Pod 훅만 더합니다.
     Pod 밖에서 실행한 에이전트에는 아무 영향이 없습니다. 확인: pod hooks status

데이터 위치
  %LOCALAPPDATA%\pod\state   터미널 기록·상태 (데몬이 씀)
  %LOCALAPPDATA%\pod\podd.log
  %APPDATA%\pod\pod-app      앱 설정·레이아웃
  %USERPROFILE%\pod\worktrees  Task worktree (제거해도 지우지 않음)

업데이트·제거
  새 설치 프로그램을 실행하면 됩니다. 실행 중인 터미널과 에이전트는 꺼지지 않습니다.
  제거는 "설정 > 앱"에서 Pod를 고릅니다. 실행 중인 터미널이 있으면 먼저 묻습니다.

포터블 zip
  압축을 푼 폴더에서 pod-app.exe를 바로 실행할 수 있습니다(설치·PATH·시작 메뉴 없음).
  데이터 위치는 설치판과 같습니다.

서명 안내
  이 빌드는 코드 서명이 없어 처음 실행할 때 Windows SmartScreen이
  "Windows의 PC 보호" 경고를 띄울 수 있습니다. "추가 정보 > 실행"을 누르세요.
  내려받은 파일은 SHA256SUMS.txt의 값과 비교해 확인할 수 있습니다.

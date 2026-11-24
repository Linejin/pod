# AI 사용 한도 모니터

상태 표시줄 오른쪽(상태 · 알림 앞)에 연결된 AI 서비스의 **남은 사용량**과 **초기화까지 남은 시간**을 항상 표시한다. 일곱 레이아웃 모두 같은 상태 표시줄을 쓰므로 어디서나 보인다. 항목을 누르거나 Tab으로 이동해 Enter/Space를 누르면 세부 정보가 열리고 Esc로 닫힌다.

구현: [usage.rs](../../crates/pod-app/src/usage.rs) (모니터·주기), [model.rs](../../crates/pod-app/src/usage/model.rs) (상태·문구), [codex.rs](../../crates/pod-app/src/usage/codex.rs), [claude.rs](../../crates/pod-app/src/usage/claude.rs), [view.rs](../../crates/pod-app/src/usage/view.rs), 수집기 [statusline.rs](../../crates/pod-hook/src/statusline.rs), 캐시 형식 [usage.rs](../../crates/pod-status/src/usage.rs).

## 무엇을 보여 주나

| 너비 | 상태 표시줄 항목 |
|---|---|
| 1400px 이상 | 서비스마다 마크 + `5h 76% · 2시간 13분` (가장 빠듯한 창의 이름·남은 비율·초기화까지 남은 시간) |
| 1100–1399px | 서비스마다 마크 + `76% · 2시간 13분` |
| 1100px 미만 (최소 800px) | 하나의 집계 항목: 모든 서비스에서 가장 빠듯한 살아 있는 창 하나의 서비스 마크 + `76% · 2h13m` (남은 비율 · 초기화까지 남은 시간) |

- 남은 비율 = 100 − 제공자가 보고한 사용 비율. 추정하지 않는다.
- 집계는 **합산이 아니다**. 5시간/주간, 모델별 버킷, 서비스, 프로필은 서로 다른 한도라 더하지 않고, 가장 적게 남은 창 하나를 이름과 함께 보여 준다.
- 좁은 항목의 카운트다운은 짧은 고정 형식이다: `3d4h`(일·시간), `2h13m`·`2h05m`(시간·분, 분은 항상 두 자리), `45m`, `<1m`. 분 단위 올림이라 시각이 지나기 전에 0이 되지 않는다. 그 밖의 상태는 꾸밈 없이 문구로 보인다:

  | 표시 | 뜻 |
  |---|---|
  | `소진 · 3d4h` | 가장 빠듯한 창이 소진됨. 보고된 초기화까지 남은 시간 |
  | `76% · 미보고` | 제공자가 이 창의 초기화 시각을 보고하지 않음(지어내지 않는다) |
  | `초기화 시각 지남` | 모든 창의 초기화 시각이 지났고 새 보고가 아직 없음. 이전 비율을 현재 값처럼 보이지 않으며 "초기화됨"이라고 하지 않는다 |
  | `한도 없음` | 숫자가 있는 서비스가 없음(설치·연결 안 됨 등. 세부 정보에 서비스별 상태) |

  항목 폭은 112px(가장 긴 문구 `초기화 시각 지남`도 기본 UI 글꼴에서 92px)이라 분이 바뀌거나, 초기화 시각이 지나 대표 창·서비스가 바뀌어도 상태 표시줄(항목·상태·알림)이 움직이지 않는다. 훨씬 넓은 UI 글꼴을 고르면 글자를 자르지 않고 항목이 넓어진다. 800px과 822px(125% 배율 포함)에서 라이트/다크/고대비로, 그리고 별도 실행 테스트로 모든 내장 스타일에서 잘림 없이 들어가는 것을 확인한다.
- 세부 정보: 연결된 모든 서비스(프로필)별 5시간·주간·지출 한도와 Codex의 추가 모델 버킷(예: `GPT-5.3-Codex-Spark · 5시간`) 각각의 남은 양과 초기화 예정, 상태, 마지막 동기화/보고 시각, 출처, 요금제, 새로고침, Claude 설정 복사(Git Bash / PowerShell)·기록 지우기, 그리고 Pod 터미널에서 감지됐지만 한도 연동이 없는 서비스(Gemini, Aider 등).
- 접근성: 항목은 하나의 버튼(탭 정지 하나)이며 접근 이름에 표시 내용 전체가 들어간다. 좁은 항목은 표시한 창을 먼저 온전한 문장으로 읽는다(예: "AI 사용 한도: 가장 빠듯한 한도 Codex 주간 39% 남음, 3일 4시간 후 초기화 예정. 전체: …", 초기화 시각 미보고·"초기화 시각 지남 · 새 보고 대기"도 그대로). 라이브 영역이 아니므로 분 단위 카운트다운이 읽히지 않는다. 색은 항상 문구(정상·얼마 안 남음·소진…)와 함께 쓴다.

### 상태

| 문구 | 뜻 |
|---|---|
| 정상 / 얼마 안 남음 | 최신 값. 20% 이하이면 경고 색 |
| 소진 | 남은 비율 0% 또는 제공자가 한도 도달을 보고 |
| 초기화 시각 지남 · 새 보고 대기 | 보고된 초기화 시각이 지났다. **초기화됐다고 말하지 않는다** — 다음 보고가 실제 값을 정한다. 창의 이전 값은 `이전 N% 남음`으로 흐리게 표시 |
| 오래된 데이터 | Codex 20분, Claude 60분 넘게 새 값이 없음(값은 흐리게 유지) |
| 읽기 실패 · 이전 값 | 마지막 읽기가 실패했고 같은 계정의 이전 값을 표시 |
| 연결 안 됨 / 설치 안 됨 | CLI가 없거나 로그인·수집기 연결이 없음 |
| 지원 안 됨 | 이 계정·인증 방식에는 한도 정보가 없음(Codex API 키, Bedrock 등) |
| 첫 보고 대기 / 한도 정보 없음 | Claude 수집기는 연결됐지만 아직 한도 보고가 없음 |

카운트다운은 앱이 로컬 시계로 계산하며 분 경계에서만 다시 그린다(초 단위 갱신 없음, 넓은 항목은 최소 너비, 좁은 항목은 너비 고정).

## Codex

공식 app-server 메서드 [`account/rateLimits/read`](https://learn.chatgpt.com/docs/app-server#6-rate-limits-chatgpt)를 쓴다. 설치된 Codex CLI(검증: codex-cli 0.172.0)와 이미 로그인된 사용자 프로필(`CODEX_HOME`)을 그대로 쓰며 따로 설정할 것은 없다.

- 한 번 읽기 = 짧게 실행되는 `codex app-server` 하나: `initialize` → `initialized` → `account/read {refreshToken:false}` → `account/rateLimits/read {excludeResetCreditDetails:true}`. 로그인 없음·API 키·기타 인증이면 한도 읽기를 건너뛰고, `account/read` 자체가 실패하면(실측: "workspace routing discovery failed") 한도 읽기를 계속한다. 스레드·턴(모델 요청), 로그인/로그아웃, 초기화 크레딧 사용, 이메일 발송 메서드는 부르지 않는다.
- 실행 파일: `POD_CODEX`(절대 경로 `.exe`만) → PATH의 절대 경로 폴더 순서대로 `codex.exe`, 또는 npm shim(`codex.cmd`)이 실행할 플랫폼 바이너리(`node_modules\@openai\codex\…\vendor\<target>\bin\codex.exe`). `.cmd`/`.ps1`·셸·node는 실행하지 않는다.
- 프로세스: 창 없이, kill-on-close Job Object 안에서, 작업 폴더는 Pod 소유의 빈 폴더(`%LOCALAPPDATA%\Pod\usage\codex-cwd`)라 저장소의 `.codex` 설정이 적용되지 않는다. 전체 25초 기한, 줄 256KiB·전체 4MiB 상한, 끝나면 프로세스 트리를 종료한다.
- 보관은 메모리에 한정한다. 비율·창 길이·초기화 시각·한도 이름·요금제·도달 여부와 계정 전환을 판별하기 위한 `accountId` 해시를 보관한다.
- 주기: 5분마다, 오류 시 10→20→40→60분 백오프. 새로고침은 즉시(10초 간격 제한), 한 번에 하나만 실행된다.
- 계정 전환·로그아웃: 다른 계정이 보이거나 로그아웃·API 키 로그인으로 바뀌면 이전 값을 즉시 버린다. 같은 계정의 읽기 실패만 이전 값을 "읽기 실패 · 이전 값"으로 남긴다.
- API 키 로그인은 "지원 안 됨"으로 정직하게 표시한다.

## Claude Code

Claude Code가 문서화한 [statusLine JSON의 `rate_limits`](https://code.claude.com/docs/en/statusline#rate-limit-usage) (`five_hour`, `seven_day`, `spend_limit`의 `used_percentage`·`resets_at`)를 쓴다. Pod는 값을 얻으려고 모델 요청을 보내지 않고, 대화 기록·터미널·자격 증명 파일을 읽지 않는다.

### 데이터가 있는 경우와 범위

[문서](https://code.claude.com/docs/en/statusline#available-data) 기준(2026-11-24 확인):

- `rate_limits`는 **claude.ai Pro, Max, Ultra 구독자**, 또는 지출 한도를 둔 **Claude apps gateway** 뒤에서만 나온다. API 키(Console 과금)·Bedrock·Vertex 사용에는 없다. 그 경우 Pod는 "한도 정보 없음"으로 표시한다.
- 그 경우에도 **세션의 첫 API 응답 이후**에만 나온다. 값은 **실행 중인 Claude Code 세션이 상태 줄을 갱신할 때만** 들어오므로, Claude Code를 쓰지 않는 동안에는 마지막 보고 값이 "N분 전 보고"와 함께 남고 60분 뒤 "오래된 데이터"가 된다.
- `five_hour`(5시간)·`seven_day`(주간)·`spend_limit`(게이트웨이 지출 한도, Claude Code v2.1.251 이상)는 각각 따로 빠질 수 있다. 게이트웨이의 달러 금액(`used_usd`·`limit_usd`)은 저장하지도 표시하지도 않고 비율과 기간만 쓴다.
- 범위: 값은 **계정 단위가 아니라 Claude 설정(프로필) 단위**로 모인다. 같은 계정을 여러 프로필에서 쓰면 프로필마다 따로 보이고(합산하지 않음), Pod가 아닌 다른 기기·claude.ai 웹 사용분은 그 기기의 Claude Code가 다음에 보고할 때 반영된다.

### 연결 (사용자가 직접)

Pod는 Claude 설정을 수정하지 않는다. 세부 정보의 **설정 복사 (Git Bash)** / **설정 복사 (PowerShell)** 가 붙여 넣을 값을 클립보드에 넣는다. Windows의 Claude Code는 Git Bash가 설치되어 있으면 Git Bash로, 없으면 PowerShell로 statusLine 명령을 실행하므로([Windows 설정](https://code.claude.com/docs/en/statusline#windows-configuration)) 해당하는 쪽을 고른다.

1. (권장) `pod hooks install`로 `pod-hook.exe`를 `%LOCALAPPDATA%\Pod\bin`에 복사해 두면 앱 업데이트 후에도 경로가 유지된다. 없으면 앱 옆의 `pod-hook.exe` 경로를 쓴다.
2. `~/.claude/settings.json`(또는 `CLAUDE_CONFIG_DIR/settings.json`)의 `statusLine`만 복사한 값으로 바꾼다. 다른 키는 그대로 둔다.

#### Git Bash

statusLine이 없을 때:

```json
{
  "statusLine": {
    "type": "command",
    "command": "C:/Users/<사용자>/AppData/Local/Pod/bin/pod-hook.exe statusline"
  }
}
```

이미 다른 statusLine 명령이 있으면 덮어쓰지 않고 앞에 붙인다(입력 JSON을 그대로 넘겨 기존 명령이 계속 동작):

```json
{
  "statusLine": {
    "type": "command",
    "command": "C:/Users/<사용자>/AppData/Local/Pod/bin/pod-hook.exe statusline --passthrough | ~/.claude/statusline.sh"
  }
}
```

- 경로는 슬래시(`/`)로 쓴다. Git Bash는 역슬래시를 지운다. 공백·괄호 등이 있는 경로는 큰따옴표로 감싼다.

#### PowerShell (Git Bash 없음)

`pod-hook.exe`는 창이 뜨지 않도록 GUI 서브시스템 프로그램이라 `cmd`로 감싸 실행한다(`cmd`는 항상 끝날 때까지 기다리고 표준 입출력을 넘긴다). `--%`는 그 뒤를 `|` 전까지 그대로 `cmd`에 넘겨, 공백·괄호가 있는 경로도 `cmd`의 따옴표 규칙(`""경로" 인수"`)대로 전달된다.

```json
{
  "statusLine": {
    "type": "command",
    "command": "cmd --% /d /s /c \"\"C:\\Users\\<사용자>\\AppData\\Local\\Pod\\bin\\pod-hook.exe\" statusline\""
  }
}
```

기존 statusLine 명령이 있으면 앞에 붙인다. Windows PowerShell 5.1은 명령 사이 파이프의 텍스트를 ASCII로 다시 인코딩해 `C:/작업` 같은 비ASCII 문자를 `?`로 바꾸므로(실측), 이 형태에서는 수집기가 `--ascii`로 같은 JSON을 비ASCII 문자만 `\uXXXX`로 이스케이프해 넘긴다(값·키 순서 동일, JSON 파서는 원래 문자열로 읽는다):

```json
{
  "statusLine": {
    "type": "command",
    "command": "cmd --% /d /s /c \"\"C:\\Users\\<사용자>\\AppData\\Local\\Pod\\bin\\pod-hook.exe\" statusline --passthrough --ascii\" | powershell -NoProfile -File C:/Users/<사용자>/.claude/statusline.ps1"
  }
}
```

- 기존 명령이 따옴표로 시작하는 경로면(`"C:\…\a.exe"`, `'C:\…\a.ps1'`) PowerShell에서는 문자열이 되므로 앞에 호출 연산자 `&`를 붙인다.
- 파이프 뒤에 올 수 없는 PowerShell 식(`$input | …`, `(…)`, `@…`, `{…}`, `[…]`)이나 여러 줄 명령, 경로에 `%`가 있는 경우는 자동 결합하지 않는다. 앱은 PowerShell 복사 버튼 대신 안내를 보여 주며, 직접 결합해야 한다.
- 두 형태는 서로 바꿔 쓸 수 없다(실측): PowerShell 형태를 Git Bash에서 실행하면 `cmd`가 명령을 받지 못하고 대화형으로 열리며, 따옴표로 감싼 Git Bash 형태(`"C:/…/pod-hook.exe" statusline`)는 PowerShell에서 구문 오류다.

#### 확인한 것과 남은 것

- 앱이 생성하는 다섯 형태(Git Bash 단독·기존 스크립트 결합, PowerShell 단독·`powershell -File` 결합·`& '…ps1'` 결합)를 공백·괄호가 있는 폴더의 실제 `pod-hook.exe`로, Git Bash(`bash -c`)와 Windows PowerShell 5.1(`powershell -NoProfile -NonInteractive -Command`)에서 격리된 표준 입력/출력 fixture로 실행해 확인했다: 수집기가 값을 저장하고, 단독이면 요약 줄을, 결합이면 기존 명령의 출력을 내며, 기존 명령이 받은 JSON이 원래 입력(한글 `cwd` 포함)과 같다(`smoke_claude_setup_in_git_bash_and_powershell`, 아래 확인 방법).
- 확인하지 않은 것: 실제 Claude Code 세션이 Git Bash·PowerShell을 띄우는 정확한 인수(위 fixture는 문서의 동작을 `bash -c`/`powershell -Command`로 재현한다), PowerShell 7(`pwsh`, 이 PC에 없음), 실제 Claude 모델 요청.
- 단독 사용 시 Claude 상태 줄에는 `5h 76% 남음 · 7d 59% 남음`처럼 짧게 표시된다(한도 보고 전에는 빈 줄).
- 연결 해제: `statusLine`에서 Pod 명령을 지운다. 수집기가 설정에 없고 15분 동안 보고가 없으면 이전 값은 숨기고 "연결 안 됨"으로 표시한다. **기록 지우기**는 Pod 캐시 파일만 지운다.

### 수집기와 캐시

- `pod-hook statusline [--passthrough [--ascii]]`: 창 없는 기존 `pod-hook.exe`의 별도 모드. 표준 입력을 700ms·1MiB 안에서 읽고, 기록하지 않으며, 자식 프로세스·네트워크 없이 끝난다. 읽을 수 없는 입력은 아무것도 쓰지 않는다.
- 저장: `%LOCALAPPDATA%\Pod\usage\claude-<프로필>.json` (`POD_USAGE_DIR`로 바꿀 수 있음). 프로필은 `default` 또는 `CLAUDE_CONFIG_DIR`의 해시(`cfg-<16진수>`)라 서로 다른 Claude 설정은 섞이지 않고 세부 정보에 프로필별로 따로 나온다.
- 내용은 비율, 초기화 시각, 지출 한도 기간(`daily`/`weekly`/`monthly`), 마지막 보고 시각, 프로필 식별자(`default` 또는 설정 경로의 해시)다.
- 임시 파일 + rename으로 원자적으로 쓰고, 앱은 크기(16KiB)·개수(8)·파일 이름을 검사해 읽는다.
- Claude Code는 초기화 시각이 지난 창을 입력에서 뺀다. 수집기는 그런 창의 이전 값을 남겨 앱이 "초기화 시각 지남 · 새 보고 대기"로 보여 준다.

## 그 밖의 서비스

Gemini, Aider 등 Pod가 알아보는 다른 에이전트는 문서화된 기계 판독용 한도 정보가 없으므로 터미널에서 감지되면 세부 정보에 "한도 연동 미지원"으로만 표시한다. Gemini의 대화형 `/stats`를 긁거나 세션 토큰 합계를 한도로 표시하지 않는다.

## 확인 방법

Windows 통합 소스 `3f961aa`에서 실제 앱의 7개 레이아웃과 3개 테마, 좁은 창에서의 카운트다운, 격리된 수집기·캐시 흐름을 시험했다. 시험에 쓴 보고값과 실제 계정 연동 여부는 [Windows 검증 결과](../27-feature-validation-results.md#2026-11-24-사용량-모니터-windows-검증)를 참고한다.

```powershell
cargo test -p pod-status usage
cargo test -p pod-hook --test statusline
cargo test -p pod-process lines
cargo test -p pod-app usage::
# 실제 로그인된 Codex에서 정규화된 값만 출력(네트워크 사용, 기본 실행 안 함):
cargo test -p pod-app usage::tests::live_codex_read_normalized_only -- --ignored --nocapture
# 설정 복사 명령을 Git Bash·PowerShell에서 fixture로 실행(Git Bash 필요, 기본 실행 안 함,
# Claude 실행·모델 요청·사용자 설정 접근 없음):
cargo build -p pod-hook
cargo test -p pod-app usage::tests::smoke_claude_setup_in_git_bash_and_powershell -- --ignored --nocapture
```

시각 확인용 고정 화면: `pod-app --ui-fixture --fixture-scene usage-demo` (예시 값, 실제 값 아님), `usage-empty` (아무것도 연결 안 됨). fixture 실행은 프로세스를 띄우지 않으며, 다른 fixture 화면에는 항목이 나오지 않는다(기존 픽셀 기준 이미지 유지). 실제 앱에서는 항상 표시된다.

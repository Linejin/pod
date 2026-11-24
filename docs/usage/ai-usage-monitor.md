# AI 사용 한도 모니터

상태 표시줄 오른쪽(상태 · 알림 앞)에 연결된 AI 서비스의 **남은 사용량**과 **초기화까지 남은 시간**을 항상 표시한다. 일곱 레이아웃 모두 같은 상태 표시줄을 쓰므로 어디서나 보인다. 항목을 누르거나 Tab으로 이동해 Enter/Space를 누르면 세부 정보가 열리고 Esc로 닫힌다.

구현: [usage.rs](../../crates/pod-app/src/usage.rs) (모니터·주기), [model.rs](../../crates/pod-app/src/usage/model.rs) (상태·문구), [codex.rs](../../crates/pod-app/src/usage/codex.rs), [claude.rs](../../crates/pod-app/src/usage/claude.rs), [view.rs](../../crates/pod-app/src/usage/view.rs), 수집기 [statusline.rs](../../crates/pod-hook/src/statusline.rs), 캐시 형식 [usage.rs](../../crates/pod-status/src/usage.rs).

## 무엇을 보여 주나

| 너비 | 상태 표시줄 항목 |
|---|---|
| 1400px 이상 | 서비스마다 마크 + `5h 76% · 2시간 13분` (가장 빠듯한 창의 이름·남은 비율·초기화까지 남은 시간) |
| 1100–1399px | 서비스마다 마크 + `76% · 2시간 13분` |
| 1100px 미만 (최소 800px) | 하나의 집계 항목: 가장 빠듯한 창 하나의 서비스 마크 + `76%` |

- 남은 비율 = 100 − 제공자가 보고한 사용 비율. 추정하지 않는다.
- 집계는 **합산이 아니다**. 5시간/주간, 모델별 버킷, 서비스, 프로필은 서로 다른 한도라 더하지 않고, 가장 적게 남은 창 하나를 이름과 함께 보여 준다.
- 세부 정보: 서비스(프로필)별 5시간·주간·지출 한도와 Codex의 추가 모델 버킷(예: `GPT-5.3-Codex-Spark · 5시간`), 상태, 마지막 동기화/보고 시각, 출처, 요금제, 새로고침, Claude 설정 복사·기록 지우기, 그리고 Pod 터미널에서 감지됐지만 한도 연동이 없는 서비스(Gemini, Aider 등).
- 접근성: 항목은 하나의 버튼(탭 정지 하나)이며 접근 이름에 표시 내용 전체가 들어간다. 라이브 영역이 아니므로 분 단위 카운트다운이 읽히지 않는다. 색은 항상 문구(정상·얼마 안 남음·소진…)와 함께 쓴다.

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

카운트다운은 앱이 로컬 시계로 계산하며 분 경계에서만 다시 그린다(초 단위 갱신 없음, 항목 최소 너비 고정).

## Codex

공식 app-server 메서드 [`account/rateLimits/read`](https://learn.chatgpt.com/docs/app-server#6-rate-limits-chatgpt)를 쓴다. 설치된 Codex CLI(검증: codex-cli 0.172.0)와 이미 로그인된 사용자 프로필(`CODEX_HOME`)을 그대로 쓰며 따로 설정할 것은 없다.

- 한 번 읽기 = 짧게 실행되는 `codex app-server` 하나: `initialize` → `initialized` → `account/read {refreshToken:false}` → `account/rateLimits/read {excludeResetCreditDetails:true}`. 로그인 없음·API 키·기타 인증이면 한도 읽기를 건너뛰고, `account/read` 자체가 실패하면(실측: "workspace routing discovery failed") 한도 읽기를 계속한다. 스레드·턴(모델 요청), 로그인/로그아웃, 초기화 크레딧 사용, 이메일 발송 메서드는 부르지 않는다.
- 실행 파일: `POD_CODEX`(절대 경로 `.exe`만) → PATH의 절대 경로 폴더 순서대로 `codex.exe`, 또는 npm shim(`codex.cmd`)이 실행할 플랫폼 바이너리(`node_modules\@openai\codex\…\vendor\<target>\bin\codex.exe`). `.cmd`/`.ps1`·셸·node는 실행하지 않는다.
- 프로세스: 창 없이, kill-on-close Job Object 안에서, 작업 폴더는 Pod 소유의 빈 폴더(`%LOCALAPPDATA%\Pod\usage\codex-cwd`)라 저장소의 `.codex` 설정이 적용되지 않는다. 전체 25초 기한, 줄 256KiB·전체 4MiB 상한, 끝나면 프로세스 트리를 종료한다.
- 보관: 메모리에만, 비율·창 길이·초기화 시각·한도 이름·요금제·도달 여부만. 토큰, 이메일, 계정/사용자 ID 원문, 크레딧 잔액은 읽거나 저장하지 않는다(계정 전환 판별용 해시만 메모리에).
- 주기: 5분마다, 오류 시 10→20→40→60분 백오프. 새로고침은 즉시(10초 간격 제한), 한 번에 하나만 실행된다.
- 계정 전환·로그아웃: 다른 계정이 보이거나 로그아웃·API 키 로그인으로 바뀌면 이전 값을 즉시 버린다. 같은 계정의 읽기 실패만 이전 값을 "읽기 실패 · 이전 값"으로 남긴다.
- API 키 로그인은 "지원 안 됨"으로 정직하게 표시한다.

## Claude Code

Claude Code가 문서화한 [statusLine JSON의 `rate_limits`](https://code.claude.com/docs/en/statusline#rate-limit-usage) (`five_hour`, `seven_day`, `spend_limit`의 `used_percentage`·`resets_at`)를 쓴다. 이 값은 **실행 중인 Claude Code 세션이 응답을 받을 때만** 상태 줄 명령에 전달되며, Pro·Max 구독(또는 지출 한도 게이트웨이)에서만, 세션의 첫 API 응답 이후에 들어온다. Pod는 값을 얻으려고 모델 요청을 보내지 않고, 대화 기록·터미널·자격 증명 파일을 읽지 않는다.

### 연결 (사용자가 직접)

Pod는 Claude 설정을 수정하지 않는다. 세부 정보의 **설정 복사**가 붙여 넣을 값을 클립보드에 넣는다.

1. (권장) `pod hooks install`로 `pod-hook.exe`를 `%LOCALAPPDATA%\Pod\bin`에 복사해 두면 앱 업데이트 후에도 경로가 유지된다. 없으면 앱 옆의 `pod-hook.exe` 경로를 쓴다.
2. `~/.claude/settings.json`(또는 `CLAUDE_CONFIG_DIR/settings.json`)의 `statusLine`만 복사한 값으로 바꾼다. 다른 키는 그대로 둔다.

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

- 경로는 슬래시(`/`)로 쓴다. Windows의 Claude Code는 Git Bash로 명령을 실행하며 역슬래시가 사라진다([Windows 설정](https://code.claude.com/docs/en/statusline#windows-configuration)). 공백이 있는 경로는 따옴표로 감싸는데, Git Bash가 없는 PowerShell 환경에서는 공백 없는 경로(`%LOCALAPPDATA%\Pod\bin`)를 쓴다.
- 단독 사용 시 Claude 상태 줄에는 `5h 76% 남음 · 7d 59% 남음`처럼 짧게 표시된다(한도 보고 전에는 빈 줄).
- 연결 해제: `statusLine`에서 Pod 명령을 지운다. 수집기가 설정에 없고 15분 동안 보고가 없으면 이전 값은 숨기고 "연결 안 됨"으로 표시한다. **기록 지우기**는 Pod 캐시 파일만 지운다.

### 수집기와 캐시

- `pod-hook statusline [--passthrough]`: 창 없는 기존 `pod-hook.exe`의 별도 모드. 표준 입력을 700ms·1MiB 안에서 읽고, 기록하지 않으며, 자식 프로세스·네트워크 없이 끝난다. 읽을 수 없는 입력은 아무것도 쓰지 않는다.
- 저장: `%LOCALAPPDATA%\Pod\usage\claude-<프로필>.json` (`POD_USAGE_DIR`로 바꿀 수 있음). 프로필은 `default` 또는 `CLAUDE_CONFIG_DIR`의 해시(`cfg-<16진수>`)라 서로 다른 Claude 설정은 섞이지 않고 세부 정보에 프로필별로 따로 나온다.
- 내용: 비율, 초기화 시각, 지출 한도 기간(`daily`/`weekly`/`monthly`), 마지막 보고 시각, 프로필 폴더 이름뿐. 세션 ID, 프롬프트, cwd, 대화 기록 경로, 모델, 비용·달러 금액은 저장하지 않는다.
- 임시 파일 + rename으로 원자적으로 쓰고, 앱은 크기(16KiB)·개수(8)·파일 이름을 검사해 읽는다.
- Claude Code는 초기화 시각이 지난 창을 입력에서 뺀다. 수집기는 그런 창의 이전 값을 남겨 앱이 "초기화 시각 지남 · 새 보고 대기"로 보여 준다.

## 그 밖의 서비스

Gemini, Aider 등 Pod가 알아보는 다른 에이전트는 문서화된 기계 판독용 한도 정보가 없으므로 터미널에서 감지되면 세부 정보에 "한도 연동 미지원"으로만 표시한다. Gemini의 대화형 `/stats`를 긁거나 세션 토큰 합계를 한도로 표시하지 않는다.

## 확인 방법

```powershell
cargo test -p pod-status usage
cargo test -p pod-hook --test statusline
cargo test -p pod-process lines
cargo test -p pod-app usage::
# 실제 로그인된 Codex에서 정규화된 값만 출력(네트워크 사용, 기본 실행 안 함):
cargo test -p pod-app usage::tests::live_codex_read_normalized_only -- --ignored --nocapture
```

시각 확인용 고정 화면: `pod-app --ui-fixture --fixture-scene usage-demo` (예시 값, 실제 값 아님), `usage-empty` (아무것도 연결 안 됨). fixture 실행은 프로세스를 띄우지 않으며, 다른 fixture 화면에는 항목이 나오지 않는다(기존 픽셀 기준 이미지 유지). 실제 앱에서는 항상 표시된다.

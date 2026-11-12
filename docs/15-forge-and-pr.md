# P6 1부: GitHub 연동 — 작업 브랜치 푸시와 PR

작성일: 2026-11-09 · 브랜치 `SJin-Choi/p6-pr` · 코드: `crates/pod-forge`, `crates/podd/src/{pr_protocol.rs,server/pr.rs}`, `crates/pod-cli/src/pr.rs`, `crates/pod-app/src/{pr_panel.rs,pr_qa.rs}` · 상위 기획: [01 §6 P4(“PR까지 앱 밖으로 안 나감”)·P6](01-plan.md) · 기반: [08](08-tasks.md)(Task = worktree, 브랜치 `pod/<name>`), [11](11-review-and-orchestration-backend.md)(리뷰·그룹), [09](09-pod-app.md)(앱)

작업 브랜치를 원격에 푸시하고, PR을 만들고, 상태·체크·리뷰 결정을 따라가고, 병합까지 Pod 안에서 한다. **아직 사용할 GitHub 저장소가 정해지지 않았으므로** 이 단계의 모든 검증은 가짜 forge(fake)로 했고, GitHub에는 아무것도 쓰지 않았다(저장소 생성·푸시·PR·코멘트 없음). 실제 저장소를 연결하는 방법은 §7. 그 뒤 버리는 비공개 저장소로 실제 GitHub 검증을 했다(§9).

---

## 1. 결정

| 항목 | 결정 | 이유 |
|---|---|---|
| GitHub 접근 | **`gh` CLI의 JSON 출력**(`gh pr create/view/list/checks/merge/close`, `gh api`) + 푸시는 `git push` | 인증·호스트·GHE·프록시를 사용자가 이미 설정한 `gh auth`에 맡긴다. HTTP 클라이언트·토큰 저장 의존성이 늘지 않는다(`cargo deny` 그대로). 08의 “git은 CLI로”와 같은 방침. |
| 제공자 계층 | 새 크레이트 **`pod-forge`**: `Forge` 트레이트 + `GhForge`(gh) + `FakeForge`(테스트·데모·스크린샷) | podd·앱·CLI가 같은 타입을 쓴다. 나중에 GitLab 등은 트레이트 구현만 더하면 된다. |
| 저장소 지정 | 매 명령 `--repo owner/name`(GHE는 `host/owner/name`) | gh가 현재 폴더로 추측하지 않게. 원격은 `git remote -v`로 찾는다(로컬만, 네트워크 없음). URL의 자격 증명(`https://user:token@…`)은 `ForgeRemote.url`·오류 문구·`--json` 출력에 넣기 전에 `***`로 가린다(`remote::redact_url`). |
| 원격 선택 | `origin`이 GitHub면 그것, 아니면 GitHub를 가리키는 첫 원격. GitHub 호스트 = `github.com`, `*.ghe.com`, `GH_HOST`, `POD_GITHUB_HOSTS` | 포크(`upstream`/`origin`) 구성에서도 내 저장소로 푸시. |
| 원격이 없을 때 | **아무것도 만들지 않는다.** 상태는 `no_remote`, 만들기는 `FORGE_FAILED(kind=no_remote)` | 저장소 생성·원격 추가는 사용자 결정. |
| 프로토콜 | **v10** (한 번만 올림): `task.pr.status`·`task.pr.create`·`task.pr.merge`, `TaskInfo.pr`, 알림 `task.pr.changed`, 오류 `FORGE_FAILED -32021` | 모두 추가형. 옛 클라이언트는 모르는 알림을 버리고 `pr` 필드를 무시한다. |
| 갱신 | 요청 시 + **열린 PR이 있는 작업만** 느린 타이머(`POD_PR_POLL`, 기본 2분, `off`로 끔), 그것도 앱(라이프사이클 구독자)이 붙어 있을 때만 | GitHub 요청 수를 작게(작업당 2분에 gh 3회). 병합·닫힌 PR은 다시 읽지 않는다. |
| 앱의 GitHub 접근 | PR 탭이 보일 때만 GitHub에 묻는다. 그 밖에는 `cached`(마지막 `TaskInfo.pr`) | 리뷰 패널을 여는 것만으로 gh가 돌지 않게. |

## 2. `pod-forge`

```
Forge (trait, blocking)
  detect(dir) -> ForgeRemote{name, url, slug{host, owner, name}}
  push(dir, remote, branch)                 git push --porcelain --set-upstream <remote> HEAD:refs/heads/<branch>
                                            (+ git branch --set-upstream-to <remote>/<branch> <branch>)
  create(dir, remote, CreatePr{head, base, title, body, draft}) -> PrView
                                            gh pr create --repo R --head B --base B --title T --body-file - [--draft]
                                            (본문은 stdin; "already exists"면 그 PR을 돌려줌)
  find_for_branch(dir, remote, branch)      gh pr list --repo R --head B --state all --json … --limit 20 (열린 것 우선, 없으면 최신)
  view(dir, remote, n)                      gh pr view N --repo R --json number,url,title,state,isDraft,headRefName,baseRefName,
                                            reviewDecision,mergeStateStatus,mergeable,additions,deletions,changedFiles,author,mergedAt,closedAt
  checks(dir, remote, n) -> [PrCheck]       gh pr checks N --repo R --json bucket,completedAt,description,event,link,name,startedAt,state,workflow
                                            (종료 코드 8 = 진행 중인 체크가 있음, JSON은 완전함; "no checks reported" = 빈 목록)
  review_comments(dir, remote, n)           gh api repos/O/R/pulls/N/comments --paginate (페이지가 이어 붙은 JSON 배열들을 모두 읽음)
  merge(dir, remote, n, merge|squash|rebase) gh pr merge N --repo R --merge|--squash|--rebase   (원격 브랜치 삭제 안 함)
  close(dir, remote, n)                     gh pr close N --repo R
status::collect(forge, dir, branch, known_number) -> PrStatus{remote?, pr?, checks, summary, comments, error?}
```

- **프로세스 실행**(`runner.rs`): `CREATE_NO_WINDOW`, stdin은 문자열 또는 null(본문은 별도 스레드가 씀), `GH_PROMPT_DISABLED=1`, `GIT_TERMINAL_PROMPT=0`, `GCM_INTERACTIVE=never`, `GH_NO_UPDATE_NOTIFIER`, `NO_COLOR`, 상속된 `GIT_DIR` 등 제거. 시간 제한: 조회 30 s, 쓰기 60 s, 푸시 180 s — 넘으면 프로세스 **트리**(Job Object)를 죽이고 `timeout`. 시간 제한은 출력 파이프를 잡은 손자 프로세스까지 포함한 호출 전체에 걸린다. stdout은 16 MiB를 넘으면 트리를 죽이고 `failed`("output too large"), stderr는 마지막 64 KiB만 둔다(`pod_process::bounded`, security-02/03). 실행 파일은 `POD_GH`/`POD_GIT`으로 바꿀 수 있다.
- **오류 분류**(`ForgeErrorKind`): `no_remote`, `unsupported_host`, `tool_missing`, `not_logged_in`(gh 종료 코드 4, "gh auth login", git "could not read Username"), `no_permission`(403, "Permission to … denied", "does not have the correct permissions"), `rate_limited`("rate limit", secondary rate limit), `not_found`, `already_exists`, `rejected`(푸시가 non-fast-forward로 거절됨: 다른 곳에서 같은 브랜치에 푸시함 — §9에서 추가), `network`, `timeout`, `failed`. 원격에 없는 base로 만들면 GitHub의 GraphQL 오류 묶음 대신 `not_found`: `base branch "x" does not exist on o/r; push it first or choose another base`. 문구는 이 PC의 gh 2.100과 git으로 **읽기 전용 명령**을 실제로 실행해 확인했다(단위 테스트 `error::tests`).
- **가짜 forge**(`fake.rs`): `FakeState`(원격 덮어쓰기/`no_remote`, PR 목록·본문·체크·코멘트, 푸시 기록, 연산별 실패 주입 `fail`, 호출 기록 `calls`)를 메모리 또는 JSON 파일에 둔다. 파일이면 **호출마다 다시 읽어** 테스트가 데몬 밖에서 체크·리뷰를 바꿀 수 있다. GitHub 규칙 일부를 흉내 낸다: 푸시 안 된 브랜치로는 PR을 만들 수 없음, 같은 브랜치의 열린 PR은 재사용, 초안·병합된·닫힌 PR은 병합 거절.
- **녹화 픽스처**(`fixtures/`): 공개 저장소 cli/cli에 읽기 전용 `gh` 명령을 실행한 실제 출력(#14964 열림·리뷰 필요, #14927 초안·`build (windows-latest)` 실패, #14976 병합·승인, 리뷰 코멘트 페이지). 진행 중 상태는 통과 목록에서 integration-tests 3개를 `IN_PROGRESS`/`pending`으로 바꿔 만든다(gh가 미완료 시각을 `0001-01-01T00:00:00Z`로 내보내는 것도 처리).

## 3. podd (프로토콜 v10)

| 메서드 | params | result |
|---|---|---|
| `task.pr.status` | `{task, cached?}` | `TaskPrInfo` |
| `task.pr.create` | `{task, title, body?, draft?, base?, push? (기본 true)}` | `TaskPrInfo` |
| `task.pr.merge` | `{task, method: merge\|squash\|rebase}` | `TaskPrInfo` |

- `TaskPrInfo = {task, branch, default_base?, ahead, unpushed, dirty, cached, refreshed_ms, …PrStatus}`(`remote?`, `pr?: PrView`, `checks`, `summary{total,passed,failed,pending,skipped}`, `comments`, `error?: {kind, message}`). `default_base`는 작업의 base에서 `refs/heads/`·`<remote>/`를 뗀 이름(커밋 id나 `HEAD`면 없음 → 만들 때 `base` 필요).
- `TaskInfo.pr?: TaskPr = {number, url, title, state, draft, review_decision?, checks: ChecksSummary, merge_state?, updated_ms}` — 레지스트리(`registry.json`)에 저장, 데몬 재시작 뒤에도 남는다. 사실이 바뀌면(시각 제외) `task.pr.changed {seq, id, pr?}`가 라이프사이클 구독자에게 간다.
- **만들기 순서**: 제목 확인 → worktree의 `ahead == 0`이면 `INVALID_STATE`("commit the changes first", `data = {ahead, dirty}`), GitHub에 묻기 전 → 원격 감지(없으면 `FORGE_FAILED kind=no_remote`, 아무것도 안 함) → base 결정 → (push) → `gh pr create` → 체크·코멘트 → 저장·알림. 커밋 안 한 변경은 PR에 들어가지 않는다(UI가 알림).
- **병합**: 열린 PR이 아니면 `INVALID_STATE`, 초안은 GitHub(가짜)의 거절 문구가 `FORGE_FAILED`로 온다. 로컬 worktree와 브랜치는 그대로 둔다(정리는 기존 `task.remove`).
- 생성·병합·GitHub 조회는 데몬 안에서 **하나씩**(`pr_ops` 뮤텍스): 늦게 끝난 조회가 새 상태를 덮지 않는다. 조회가 실패하면(속도 제한 등) 저장된 `TaskInfo.pr`는 그대로 두고 `error`만 돌려준다.
- `FORGE_FAILED` 메시지는 종류별 영어 문구로 시작한다(예: `not logged in to GitHub (run \`gh auth login\`): …`), `data`에 `{kind, message}`.
- forge 선택: `POD_FORGE=fake:<state.json>`이면 가짜, 그 밖에는 gh.

## 4. CLI

```
pod pr status <task> [--cached] [--json]
pod pr create <task> [--title T] [--body B | --body-file F] [--draft] [--base BRANCH] [--no-push]
                                         # 제목 기본값: 작업 프롬프트 첫 줄(72자), 없으면 작업 이름
pod orch worker pr <attempt> [--title T] [--body B] [--draft] [--base BRANCH] [--no-push] [--force]   # 끝난 워커(docs/19 §16.2): 제목=항목, 본문=done 보고
pod pr merge <task> --method merge|squash|rebase --yes   # --yes가 없으면 무엇을 병합할지만 보여 주고 종료 코드 2
pod pr open <task> [--print]             # 기본 브라우저로 PR 열기(https만), --print는 URL 출력
```

## 5. 앱 (리뷰 패널의 PR 탭)

리뷰 패널 오른쪽 칸 위에 **코멘트 | PR** 전환이 생겼다(PR이 있으면 옆에 `#번호`). PR 탭(`pr_panel.rs`)의 상태:

| 상태 | 화면 |
|---|---|
| 원격 없음 | “원격 저장소가 연결되지 않았습니다” + Pod는 저장소·원격을 만들지 않는다는 설명 + 연결 방법(`git remote add origin …`, `gh auth login`) + 다시 확인. GitHub가 아닌 원격은 “GitHub 저장소가 아닌 원격입니다”. |
| 로그인·권한·한도·네트워크 오류 | 종류별 한국어 제목과 안내 + gh 원문 |
| PR 없음 | 브랜치 사실(`pod/x → main · 커밋 2개 · 푸시 안 됨 2 · 커밋 안 한 변경 1`), **브랜치 푸시 및 PR 만들기**(앞선 커밋이 없으면 꺼짐 + 이유). 폼: 제목(프롬프트 첫 줄), 기준 브랜치, 본문(프롬프트 + `## 변경 요약`: 파일 수·±·파일별 ± 최대 20개 + `## 리뷰`: Pod 리뷰 코멘트 수·해결 수 + 출처 줄), 초안 체크박스, 취소 / 푸시하고 PR 만들기 |
| PR 있음 | 상태 배지(● 열림 / ◌ 초안 / ✓ 병합됨 / ✕ 닫힘 — 모양+색), `#번호 제목`, `head → base`(병합 시각), 리뷰 결정(✓ 승인됨 / ✕ 변경 요청됨 / ○ 리뷰 필요 / 필요 없음), 브라우저에서 열기, 병합 방식(스쿼시·병합 커밋·리베이스, 기본 스쿼시) + 병합…(확인 대화상자: 되돌릴 수 없음, 실패한 체크 경고), 체크 요약(✕/◷/✓ + 통과·실패·진행 중·건너뜀)과 목록(실패 → 진행 중 → 통과 순, 건너뜀은 숨김 토글, 행마다 “열기” 버튼 = 체크 링크, 접근성 이름 “체크 <이름> (<상태>) 열기”), GitHub 리뷰 코멘트 수 |

- 그룹 패널의 최종 선택 배지 옆 **PR 만들기** → 승자 작업의 리뷰 패널 PR 탭 + 만들기 폼(`Workspace::open_pr`).
- 갱신: PR 탭을 열 때·새로 고침·`task.pr.changed`(→ 작업 목록 → `PrSection::set_task`). 열린 PR의 체크 변화는 podd의 느린 타이머가 알린다.
- 키보드: 폼은 제목에 포커스, Tab 순서 제목 → 기준 → 본문 → 초안 → 취소 → 만들기(Enter로 제출, 테스트로 확인). 병합 확인 대화상자도 Tab+Enter, Esc 취소.
- 문구는 `pr_panel::s`에 모았다(공용 `strings.rs`를 건드리지 않아 다른 작업과 충돌 없음).

## 6. 검증 (2026-11-09, GitHub 쓰기 없음)

- 단위: `pod-forge` 25개(오류 분류, URL·`remote -v` 파싱, 실제 git으로 원격 감지, 프로세스 stdin/시간 초과/없는 도구, 녹화 픽스처 파싱, 녹화 러너로 gh 인자·stdin 본문·이미 있는 PR·체크 종료 코드 8·푸시 오류, 가짜 forge 흐름·초안·실패 주입·파일 공유), podd `pr::tests`·`pr_protocol::tests`, pod-cli `pr_commands_parse`·`summary_lines`, pod-app `pr_panel::tests`(제목·본문 미리 채우기, 문구, 픽스처 장면).
- podd 통합 `crates/podd/tests/pr.rs`(실제 podd·git, `POD_FORGE=fake:<file>`):
  1. 원격 없는 저장소: 상태 `no_remote`·`cached`, 커밋 없으면 `INVALID_STATE`, 커밋 뒤에도 `FORGE_FAILED kind=no_remote`, 가짜 forge 호출은 `detect`뿐, 저장소에 원격이 생기지 않음.
  2. `origin = https://github.com/pod-fixtures/demo.git`(설정만): 원격 감지, ahead/unpushed/dirty, 만들기(푸시가 먼저, 본문 그대로, 체크 진행 중), `task.pr.changed`, 같은 브랜치 재생성은 같은 PR, 파일로 체크 실패 → 1 s 타이머가 알림, 통과+승인 → 알림, 스쿼시 병합 → 병합됨·알림, 두 번째 병합 `INVALID_STATE`, 병합 뒤 폴링 없음, 데몬 재시작 뒤 `TaskInfo.pr` 유지.
  3. 실패 주입: 푸시 `not_logged_in` → PR 없음, `--no-push` 초안 만들기, 초안 병합 거절, 조회 속도 제한 → `error`만 있고 저장된 PR 유지.
  4. `pod pr create/status --json/open --print/merge`(--yes 없으면 종료 코드 2, 병합 안 됨).
- 앱 헤드리스(`pr_qa.rs`, native_qa 하네스 + 가짜 forge): 원격 없음 안내 카드(만들기 버튼 없음, PR 탭이 보일 때만 GitHub 조회), **키보드만으로** 폼 열기·미리 채움 확인·Tab 5번·Enter로 만들기(푸시 → 생성 순서), 실패 체크 PR의 병합: Esc 취소 시 병합 안 됨, Tab 2번+Enter로 병합, 기본 방식 스쿼시.
- 스크린샷(`--ui-fixture --fixture-scene pr-*`, 가짜 forge 데이터, 서브 모니터 150%, 1900×1150 물리 픽셀, 파일 목록 숨김): `crates/pod-app/qa/pr/{dark,light}-pr-{no-remote,ready,form,pending,failed,passed,merged}.png`(+UIA 트리, manifest). 기존 시각 회귀 기준(1440×960)과는 크기가 달라 비교 대상이 아니다.

## 7. 실제 저장소 연결하기 (나중에, 사용자가 저장소를 정한 뒤)

1. GitHub CLI 로그인 확인: `gh auth status` → `Logged in to github.com account …`, 토큰 범위에 `repo`. 아니면 `gh auth login`(HTTPS, 브라우저). git이 같은 자격 증명을 쓰도록 `gh auth setup-git`.
2. GitHub에 저장소를 만든다(사용자 결정: 공개/비공개, 이름). 웹에서 만들거나 `gh repo create <owner>/<name> --private --source C:\ai\ide --remote origin` — **Pod는 이 단계를 하지 않는다.**
3. 원격 추가(위에서 `--remote`를 쓰지 않았다면): 저장소 루트에서 `git remote add origin https://github.com/<owner>/<name>.git`, 그리고 기본 브랜치를 한 번 푸시: `git push -u origin master`(또는 main). PR의 base가 원격에 있어야 한다.
4. 이미 만든 작업의 worktree는 같은 저장소의 worktree라 원격 설정을 공유한다. 확인: `pod pr status <task>` → `remote origin https://github.com/<owner>/<name>`, `pr none for pod/<name>`.
5. 시험: 작은 작업에서 커밋 → `pod pr create <task> --draft` → `pod pr status <task>`(체크) → GitHub에서 확인 → `pod pr merge <task> --method squash --yes`는 원할 때만. 앱에서는 리뷰 패널 → PR 탭.
6. 작업의 base가 커밋 id나 `HEAD`면 `default_base`가 없으므로 `--base main`(앱은 기준 브랜치 칸)을 준다. 작업 base가 원격에 없는 로컬 브랜치(예: 다른 작업의 `pod/…`)면 그 브랜치를 먼저 푸시하거나 base를 바꾼다.
7. GitHub Enterprise: `gh auth login --hostname <host>` 후 `POD_GITHUB_HOSTS=<host>`(podd 환경)로 호스트를 GitHub로 인정한다.
8. 폴링 주기: `POD_PR_POLL=5m` 등(podd 환경), `off`로 끔.

실제 저장소가 있어야 확인할 수 있는 것: 실제 `gh pr create` 출력·권한 오류 문구(브랜치 보호, 필수 리뷰), 병합 차단(`mergeStateStatus` BLOCKED에서의 gh 문구), 대용량 체크 목록과 상태 컨텍스트(StatusContext) 표시, `git push`의 자격 증명 흐름(GCM 창이 뜨지 않는지), 속도 제한 실제 문구, GHE.

## 8. 한계와 다음 단계

1. PR 본문·제목 수정, 초안 → 검토 준비, 리뷰어 지정, 라벨은 없다(`gh pr edit`/`gh pr ready`로 쉽게 추가 가능).
2. GitHub 리뷰 코멘트는 개수만 보여 준다. Pod 리뷰(라인 코멘트 → 에이전트, docs/11)로 가져와 에이전트에 보내는 것은 다음 단계.
3. 병합 후 로컬 정리(작업 삭제, 원격 브랜치 삭제)는 자동으로 하지 않는다.
4. 체크 로그 보기·재실행 없음(링크로 GitHub 열기만).
5. 앱의 오류 종류는 podd 메시지 첫 문구로 판별한다(`SourceError`에 `data`가 없음).
6. 원격 감지는 `origin` 우선 규칙만 쓴다(브랜치별 upstream·`pushRemote`는 보지 않음).
7. (§9에서 발견) PR이 생긴 뒤 새 커밋을 푸시하는 버튼이 앱에 없다. 지금은 `pod pr create <task>`를 다시 실행하면 푸시하고 같은 PR을 돌려준다. 강제 푸시는 하지 않는다(리베이스한 브랜치는 거절 → `rejected`).
8. (§9에서 발견) 원격에 없는 base로 만들면 GitHub가 거절하기 전에 head 브랜치는 이미 푸시되어 있다.
9. (§9에서 발견) 시간 초과로 죽인 `gh`/`git`의 자식 프로세스(예: `git-remote-https`)는 남을 수 있다(직접 자식만 죽임). 작업 개체(Job Object)로 묶는 것은 다음 단계.

## 9. 실제 저장소 검증 (2026-11-09)

사용자 승인(2026-11-09)으로 버리는 **비공개** 저장소 `SJin-Choi/pod-pr-e2e-20261109-1754`를 만들어 release 빌드(`pod`·`podd`·`pod-app`, master 51c9169 + 이 브랜치)로 검증했다. 격리: `POD_SOCKET=pod-e2e-pr-1754`, `POD_WORKTREES_ROOT`·`POD_APP_CONFIG_DIR`는 `C:\ai\pod-e2e\pr-1754` 아래, `POD_PR_POLL=20s`, 번들 ConPTY. 드라이버: `tools/e2e/pr-session.ps1`. 증거: `docs/e2e/pr/`(`cli.log` = 모든 `pod` 호출과 출력, `app-*.png` = 서브 모니터 150% DPI 인식 캡처, `t-fail-status.json`).

**환경 제약**
- gh 토큰 범위가 `gist, read:org, repo`라 **`workflow` 범위가 없어 GitHub Actions 워크플로 파일을 푸시할 수 없었다**(`refusing to allow an OAuth App to create or update workflow … without workflow scope`). 범위를 늘리지 않고, 체크는 드라이버가 올리는 **커밋 상태(StatusContext)** `quick`(일정 시간 뒤 통과)·`gate`(`gate.txt`가 `pass`가 아니면 실패)로 대신했다. `gh pr checks`에는 Actions 체크와 같은 모양(bucket/state)으로 나오므로 Pod 쪽 경로는 같다. Actions 체크 실행(`CheckRun`, 워크플로 이름)은 이번에 확인하지 못했다.
- 비공개 저장소의 브랜치 보호는 무료 요금제에서 불가: `Upgrade to GitHub Pro or make this repository public to enable this feature. (HTTP 403)`. 그래서 “저장소 규칙에 따른 병합 거절”은 GitHub가 하지 않고, Pod가 경고한다.
- 작업은 모두 shell 에이전트(`pod task new … --agent shell` + `pod send`로 편집·커밋). Claude/Codex 프롬프트 없음.

**결과**

| # | 단계 | 결과 | 증거 |
|---|---|---|---|
| 1 | 저장소 등록, shell 작업 3개(`t-pass`·`t-draft`·`t-fail`)에서 편집·커밋, `pod pr status` → 원격 감지 `origin https://github.com/SJin-Choi/…`, `pr none`, `1 ahead, 1 unpushed` | PASS | cli.log 09:05:13–09:05:28 |
| 2a | `pod pr create`(준비됨, `t-pass`): 푸시 → PR #1, 9 s. 체크 없음 → 진행 중 2 → 1 통과·1 진행 중 → `2 passed (passed)`, `mergeable UNSTABLE → CLEAN` | PASS | cli.log 09:05:35–09:06:36 |
| 2b | `pod pr create --draft`(`t-draft`) → #2 `open (draft)` | PASS | cli.log 09:06:45 |
| 2c | 실패 작업(`t-fail`, `gate.txt=fail`) → #3 `1 passed, 1 failed (failed)`, `FAIL gate` | PASS | cli.log 09:07:20, t-fail-status.json |
| 2d | `pod pr merge --method squash`(--yes 없음) → 종료 코드 2, 병합 안 함. **고치기 전**: 실패한 체크를 말하지 않음 → 고친 뒤 `checks: 1 passed, 1 failed (failed)` + `warning: 1 check failed` | PASS (수정 후) | cli.log 09:07:33 / 09:27:35 |
| 2e | 초안 병합 `--yes` → GitHub 거절 `GraphQL: Pull Request is still a draft`, 종료 코드 2 | PASS | cli.log 09:07:34 |
| 2f | `t-pass` 스쿼시 병합 → `#1 merged`, origin/main에 `Add sub() (#1)`. 두 번째 병합 → `pull request #1 is merged, not open`. 병합 뒤 `mergeable UNKNOWN`이 남던 것 수정 | PASS | cli.log 09:07:36–09:07:43, 09:27:37 |
| 2g | `pod pr open --print` → PR URL(브라우저는 열지 않음) | PASS | cli.log 09:07:44 |
| 3a | 앱 PR 탭(`--review fx/t-app`): 원격·`pod/t-app → main · 커밋 1개 · 푸시 안 됨 1` + 만들기 버튼 | PASS | app-02 |
| 3b | 만들기 폼 미리 채움: 제목(작업 이름), 기준 `main`, 본문 `## 변경 요약`(+2 −0), 초안 체크박스 → 푸시하고 만들기 → #4 | PASS | app-03, app-04 |
| 3c | **느린 갱신**(20 s, 손으로 새로 고침 없음): 진행 중 2 → 통과 2 | PASS | app-05, app-06, app-07 |
| 3d | 병합 확인 대화상자(되돌릴 수 없음) → 병합 → **실제 충돌**: `t-app`이 #1보다 먼저 갈라져 `calc.py` 끝에 둘 다 추가. GitHub `not mergeable: the merge commit cannot be cleanly created`가 토스트와 패널에 나옴. 고치기 전에는 카드에 충돌 표시가 없었음 → `mergeable: CONFLICTING`/`DIRTY`면 카드와 대화상자에 충돌 안내 | PASS (수정 후) | app-08, app-09-merge-conflict-error, app-10-conflict-hint |
| 3e | worktree에서 origin/main 병합·해결 → `pod pr create`로 푸시(같은 PR 반환) → 패널이 저절로 충돌 안내를 지움 → 앱에서 스쿼시 병합 → `✓ 병합됨 … 병합 2026-11-09 09:36:11 UTC`, origin/main `Add mul() (#4)` | PASS | app-11, app-12, app-13 |
| 3f | 실패 체크 PR(#3): `✕ 체크 2개 · 통과 1 · 실패 1`, 병합 대화상자에 실패 체크 경고, Esc로 취소(병합 안 됨). 초안 PR(#2): `◌ 초안` + 초안 병합 불가 안내 | PASS | app-14, app-15, app-16 |
| 3g | 그룹 승자 “PR 만들기”: 그룹(`fx/duel`, argv 에이전트 2개, 승자 선택)은 만들었으나 앱에서 그룹 패널로 가는 경로(트리·명령 검색)에 그룹 항목이 없어 실제 앱에서는 건너뜀. 헤드리스 `pr_qa`가 `Workspace::open_pr`를 덮는다 | SKIP | cli.log 09:37:20–09:38:03 |
| 4a | 푸시 거절(다른 클론이 `pod/t-draft`에 먼저 푸시): **고치기 전** `the GitHub command failed: error: failed to push some refs …` → 고친 뒤 kind `rejected`, `the remote branch has commits the task does not have (push rejected; pull or rebase in the task first): …` (앱: “원격 브랜치가 바뀌어 푸시가 거절되었습니다”) | PASS (수정 후) | cli.log 09:08:19 / 09:27:29 |
| 4b | 원격에 없는 base(`--base no-such-branch`, 작업 base = 로컬 전용 브랜치): **고치기 전** `Head sha can't be blank, Base sha can't be blank, No commits between …, Base ref must be a branch` → 고친 뒤 `not found on GitHub: base branch "local-only" does not exist on SJin-Choi/…; push it first or choose another base` | PASS (수정 후) | cli.log 09:08:34–09:08:53 / 09:27:31 |
| 4c | gh 시간 초과(`POD_GH`=45 s 늦게 시작하는 래퍼, 데몬 재시작): 조회 30 s에 `timeout`. **고치기 전**: 저장된 PR #3이 있는데 `pr none for pod/t-fail`(앱이면 “PR 만들기”를 보였을 것) → 고친 뒤 마지막으로 아는 #3·체크 요약 + `note timeout: gh pr view did not finish within 30 s` + `(last known state: GitHub could not be read)`. 만들기는 `gh pr create`가 끝난 뒤 `gh pr view`에서 시간 초과(76 s) | PASS (수정 후) | cli.log 09:09:19–09:09:50 / 09:28:18 |
| 5 | 자격 증명: podd(창 없는 프로세스, `GCM_INTERACTIVE=never`)의 `git push`가 기존 자격 증명으로 성공, 로그인 창이나 프롬프트 없음 | PASS | cli.log 09:05:35 |

**고친 버그**(모두 가짜 forge 회귀 테스트 포함): ① 조회 실패 시 저장된 PR 대신 “없음”을 돌려줌, 체크만 실패하면 저장된 체크 요약을 0으로 덮음(podd `pr_refresh`, `tests/pr.rs::forge_failures_keep_their_kind_and_change_nothing`). ② 푸시 거절이 일반 실패로 분류(`ForgeErrorKind::Rejected`, `error::tests`, 앱 `checks_and_error_texts`). ③ 원격에 없는 base의 GraphQL 오류 묶음(`gh::tests::create_with_a_base_missing_on_the_remote`). ④ 시간 초과 문구에 하위 명령 이름 없음(`runner::tests`). ⑤ `pod pr merge` 미리 보기에 체크·경고 없음, 병합된 PR에 `mergeable` 줄(`tests/pr.rs::pod_pr_cli`, `pr::tests::summary_lines`). ⑥ 앱 카드·병합 대화상자에 충돌 안내 없음(`pr_panel::tests::conflicts_from_github_merge_state`).

**고치지 않은 것(범위 밖, 보고)**: ~~리뷰 diff가 `core.autocrlf=true` 저장소에서 작업 폴더의 CRLF 파일을 base(LF)와 그대로 비교해 바뀐 파일의 모든 줄을 바뀐 것으로 보여 준다(app-02의 `\ LF`/`\ CRLF`, 파일 +5 −3 vs PR 본문 +2 −0).~~ → crlf-diff 브랜치에서 고침(05 §데이터 흐름 1, `git diff`와 같은 줄 수). ~~`pod server stop` 직후의 명령이 아직 닫히는 중인 데몬에 붙어 `daemon closed the connection`(cli.log 09:09:09).~~ → stop-race 브랜치에서 고침: stop이 데몬 종료까지 기다리고, 멈추는 데몬은 새 연결을 받지 않으며 남은 요청에는 `SERVER_STOPPING`(docs/03 §2 수명, `tests/stop_race.rs`).

**GitHub 저장소의 끝 상태**: PR #1·#4 병합, #2(초안)·#3(실패 체크) 열린 채로 둠. `gh repo delete SJin-Choi/pod-pr-e2e-20261109-1754 --yes` → `HTTP 403: Must have admin rights to Repository … needs the "delete_repo" scope` → **보관(archive)함**(`isArchived: true`, 비공개 유지). 삭제는 사용자가 `gh auth refresh -h github.com -s delete_repo` 뒤 `gh repo delete SJin-Choi/pod-pr-e2e-20261109-1754 --yes`로 한다. 로컬 픽스처·worktree·격리 데몬 상태는 지웠다.

**최종 검사**(이 브랜치, master 51c9169+p6-ext 병합 뒤): `cargo test --workspace` 449 통과·0 실패·3 무시. `cargo clippy -p pod-forge -p podd -p pod-cli -p pod-app --all-targets` 경고 0. `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`.


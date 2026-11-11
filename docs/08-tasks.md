# P3 백엔드: Task = Worktree + Terminal(s)

작성일: 2026-11-08 · 코드: `crates/podd/src/server/tasks.rs`, `crates/podd/src/git.rs`, `crates/pod-cli` · 상위 기획: [01 §1, §4.2 (3), §6 P3](01-plan.md) · 데몬 기반: [03](03-podd-p0.md)

podd에 **저장소 레지스트리**와 **Task 모델**을 더했다. Task 하나는 git worktree 하나(브랜치 `pod/<name>`)와 그 안에서 일하는 터미널(첫 번째는 에이전트 터미널)이다. 터미널은 P0의 holder 구조를 그대로 쓰므로 데몬을 다시 켜도 Task와 에이전트가 함께 살아 있다. UI는 아직 없고, 데몬 API와 `pod` CLI만 제공한다.
다른 제품의 코드를 쓰지 않은 클린룸 구현이다.

---

## 1. 결정

| 항목 | 결정 | 이유 |
|---|---|---|
| 저장 형식 | **JSON 파일 하나** `<state_dir>\tasks\registry.json` | 데이터가 작다(저장소 수십 개, Task 수백 개). 의존성(rusqlite + C 빌드)이 늘지 않는다. 사람이 읽고 고칠 수 있다. holder 기록(`<id>.json`)과 같은 디렉터리 체계라 데몬 재시작 규칙이 같다. 쓰기는 `registry.json.tmp`에 쓴 뒤 `rename`(Windows에서 원자적 교체)한다. 파일이 깨지면 `registry.corrupt-<ms>.json`으로 옮기고 빈 상태로 시작한다(덮어쓰지 않음). 이벤트 로그·검색이 필요해지면(01 §4.2 "SQLite 작업/세션/이벤트 로그") 그때 SQLite로 옮긴다. |
| git 접근 | **`git` CLI** (libgit2 아님) | 사용자·에이전트가 셸에서 보는 동작과 같다(설정·훅·`core.longpaths` 등). 출력은 `--porcelain`/`-z`만 파싱한다. |
| Task id | `<repo>/<name>` | 사람이 읽을 수 있고, worktree 경로(`<root>\<repo>\<name>`)·브랜치(`pod/<name>`)와 1:1이다. 이름만으로도 찾는다(유일할 때). 에이전트 터미널의 **이름**도 Task id라 `pod attach demo/feat-1`이 된다. |
| 기본 에이전트 | `shell` (`powershell -NoLogo`) | 실수로 에이전트 세션(사용량)을 쓰지 않게 한다. UI가 기본값을 바꿀 수 있다. |

## 2. 데이터 모델

```
registry.json
{ "version": 1,
  "repos": [ {name, path, added_ms} ],
  "tasks": [ {id, repo, name, branch, base, base_commit, worktree, agent, prompt?,
              created_ms, parent_task?, parent_terminal?, terminals: ["t3", ...]} ] }
```

- **저장소**: `repo.add`는 git work tree의 **루트**만 받는다. 하위 디렉터리, bare 저장소, linked worktree는 `NOT_A_REPO`로 거절한다(`git rev-parse --path-format=absolute --is-bare-repository --git-dir --git-common-dir --show-toplevel`). 이름은 기본으로 디렉터리 이름이고 `[A-Za-z0-9._-]`만 허용한다(경로·id에 쓰이므로). 이름이 안 맞으면 `name`을 받는다.
- **Task**: `base`는 받은 그대로(또는 기본값) 저장하고, 생성 시점의 커밋 `base_commit`도 저장한다. ahead/behind와 diff는 `base`가 여전히 풀리면 그 ref를, 지워졌으면 `base_commit`을 기준으로 한다.
- **터미널 연결**: Task는 터미널 id만 저장한다. 데몬이 다시 켜지면 holder 재발견(03 §2)으로 터미널이 돌아오고 id로 다시 묶인다. `terminal.rm`으로 지운 터미널은 Task에서도 빠진다. 데몬은 시작할 때 `next_id`를 "Task가 참조하는 가장 큰 id + 1" 이상으로 올려 id 재사용을 막는다.
- **환경 변수**: Task의 터미널에는 P0의 `POD_TERMINAL_ID`·`POD_SOCKET`에 더해 `POD_TASK_ID=<repo>/<name>`이 들어간다.

## 3. API (프로토콜 v3, v4에서 상태 메서드 추가: docs/07 §8)

v2(03 §4)에 다음을 더했다. `terminal.run`에는 `task` 파라미터가 생겼다. 전체 스키마는 `pod server schema`.

| 메서드 | params | result |
|---|---|---|
| `repo.add` | `{path, name?}` | `RepoInfo = {name, path, added_ms, default_base?, tasks}` |
| `repo.list` | `{}` | `{repos: RepoInfo[]}` |
| `repo.remove` | `{repo}` (이름 또는 경로) | `{name}`. Task가 남아 있으면 `REPO_IN_USE`. 디스크는 건드리지 않는다 |
| `task.create` | `{repo, name, base?, agent?, prompt?, parent_terminal?, cols?, rows?}` | `TaskInfo` |
| `task.list` | `{repo?}` | `{tasks: TaskInfo[]}` |
| `task.show` | `{task}` | `TaskInfo` |
| `task.diff` | `{task}` | `{task, base, merge_base, files: DiffFile[], added, deleted}` |
| `task.remove` | `{task, force?, delete_branch?}` | `{id, terminals, worktree_removed, branch_deleted}` |
| `terminal.run` | 기존 + `task?` | `TerminalInfo`. `task`가 있으면 cwd 기본값 = worktree, `POD_TASK_ID` 설정, Task의 터미널 목록에 추가 |

- `agent`: `"claude" | "codex" | "shell" | {"argv": [...]}`. claude/codex는 `prompt`를 CLI의 첫 프롬프트 인자로 넘긴다(`claude "<prompt>"`, `codex "<prompt>"`). shell과 argv에는 prompt를 받지 않는다(`INVALID_PARAMS`).
- `TaskInfo = {id, repo, name, branch, base, base_commit, worktree, agent, prompt?, created_ms, parent_task?, parent_terminal?, terminals: [{id, info?: TerminalInfo}], worktree_exists, git?: TaskGit, git_error?}`
- `TaskGit = {branch?, head?, ahead, behind, dirty, unpushed, last_commit?: {sha, summary, author, time_ms}}`. `task.list`/`task.show` 때마다 계산한다.
  - ahead/behind: `git rev-list --left-right --count <base>...HEAD`
  - dirty: `git status --porcelain=v1 -z --untracked-files=all`의 항목 수(rename의 원본 경로는 세지 않음)
  - unpushed: `git rev-list --count HEAD --not <base> --remotes` = base에도 어떤 원격 추적 브랜치에도 없는 커밋 수(지우면 잃는 커밋)
- `DiffFile = {path, old_path?, status, added?, deleted?, binary}`. `status`는 `A M D R C T`와 추적 안 된 파일 `?`. 리뷰 UI용으로 **worktree 전체**(커밋된 것 + 커밋 안 된 것 + 추적 안 된 것)를 `merge-base(base, HEAD)`와 비교한다. base 쪽에만 생긴 변경은 들어가지 않는다. 구현: `git diff --no-ext-diff --no-textconv -M -z --numstat <mb> --`와 `--name-status`를 경로로 합치고, `git ls-files -z --others --exclude-standard`의 새 파일은 줄 수를 직접 센다(NUL이 있으면 binary, 8 MB 초과면 개수 없음). 바이너리는 `added/deleted = null`.

새 오류 코드:

| 코드 | 이름 | 의미 |
|---|---|---|
| -32006 | `NOT_A_REPO` | 저장소 루트가 아님(하위 디렉터리·bare·linked worktree) |
| -32007 | `REPO_NOT_FOUND` | 등록되지 않은 저장소 |
| -32008 | `REPO_EXISTS` | 같은 경로(대소문자 무시) 또는 같은 이름이 이미 있음 |
| -32009 | `REPO_IN_USE` | Task가 남은 저장소를 지우려 함 |
| -32010 | `TASK_NOT_FOUND` | 없는 Task, 또는 이름이 여러 저장소에 있어 모호함 |
| -32011 | `TASK_EXISTS` | 같은 Task, 브랜치 `pod/<name>`, 또는 worktree 디렉터리가 이미 있음 |
| -32012 | `TASK_DIRTY` | 커밋 안 된 변경이나 unpushed 커밋이 있음. `data = {dirty, unpushed}` |
| -32013 | `GIT_FAILED` | git 명령 실패. 메시지에 git의 stderr |

## 4. 동작

### 4.1 `task.create`
Task 생성·삭제는 데몬 안에서 하나씩 처리한다(`tokio::Mutex`). git 명령은 `spawn_blocking`에서 돈다.

1. 저장소 확인, 이름 검사(`[A-Za-z0-9._-]`, 1–64자, `.`/`-`로 시작 금지, `..`·`.lock` 금지, `CON`/`NUL`/`COM1` 같은 Windows 장치 이름 금지), `git check-ref-format --branch pod/<name>`.
2. 충돌 거절(`TASK_EXISTS`): 같은 Task id, 이미 있는 브랜치 `pod/<name>`(pod 밖에서 만든 것 포함), 이미 있는 worktree 디렉터리.
3. base: 지정이 없으면 기본값 = `origin/HEAD`가 가리키는 브랜치(로컬에 있으면 로컬 이름, 없으면 `origin/<branch>`) → 체크아웃된 브랜치(커밋이 있을 때) → `HEAD`. 커밋으로 풀리지 않으면 `INVALID_PARAMS`.
4. `git worktree add -b pod/<name> <worktrees_root>\<repo>\<name> <base_commit>`. 커밋 id로 만들어 그사이 base가 움직여도 기록과 같다.
5. 에이전트 터미널을 P0의 `terminal.run`으로 시작: cwd = worktree, 이름 = Task id, env `POD_TASK_ID`. 실패하면 worktree와 브랜치를 되돌린다(롤백).
6. 호출자가 Pod 터미널 안이면(`pod`가 자기 환경의 `POD_TERMINAL_ID`를 `parent_terminal`로 보냄, `POD_SOCKET`이 같은 데몬일 때만) 그 터미널과 그 터미널이 속한 Task를 부모로 기록한다. 데몬이 모르는 터미널 id는 무시한다.

**prompt와 `.cmd` shim**: npm으로 설치한 `claude`는 `claude.cmd`라서 `cmd.exe /d /s /c`를 거친다(03 §2.1). cmd는 따옴표 안에서도 `%VAR%`를 확장하고, 프롬프트 안의 `"`는 따옴표 상태를 뒤집어 `& |`가 명령으로 해석될 수 있다(BatBadBut류 문제). 그래서 에이전트가 `.cmd`/`.bat`로 풀릴 때 `" % CR LF`가 든 프롬프트는 `INVALID_PARAMS`로 거절한다. `& | < > ^`는 따옴표 안이라 안전하다. `codex`는 이 PC에서 `codex.exe`(네이티브)로 먼저 풀리므로 제한이 없다.

### 4.2 `task.remove`
1. worktree가 있고 `force`가 아니면 dirty와 unpushed를 계산한다. 하나라도 있으면 `TASK_DIRTY`로 거절하고 아무것도 바꾸지 않는다. (브랜치를 남기는 삭제라도 같은 규칙이다. 요청 명세를 그대로 따랐다.)
2. Task의 터미널을 모두 kill(프로세스 트리 전체, Job Object) 후 release한다. worktree를 쥐고 있는 프로세스가 없어야 디렉터리를 지울 수 있다.
3. 저장소의 `git worktree list --porcelain -z`에 그 경로가 있으면 `git worktree remove [--force] <path>`. 막 죽은 프로세스의 핸들이 늦게 풀릴 수 있어 최대 10초 재시도한다. `force`인데 git이 등록만 지우고 파일을 남기면 직접 지운다. 등록되지 않은 디렉터리는 지우지 않는다(사용자 파일일 수 있음, `worktree_removed=false`).
4. `git worktree prune`, `delete_branch`면 `git branch -D pod/<name>`, 레지스트리에서 삭제.

### 4.3 git 실행 규칙 (`git.rs`)
- 모든 호출: `git -C <dir> -c core.longpaths=true -c core.quotepath=false ...`, `GIT_TERMINAL_PROMPT=0`, `GIT_OPTIONAL_LOCKS=0`, `LC_ALL=C`, 상속된 `GIT_DIR`/`GIT_WORK_TREE`/`GIT_INDEX_FILE` 등 제거, stdin null, `CREATE_NO_WINDOW`(데몬은 콘솔 없이 분리 실행되므로, 콘솔 앱인 git이 새 창을 띄우지 않게).
- 경로는 인자로 그대로 넘긴다(공백 안전). 출력 경로(`C:/x/y`)는 `\`로 바꾸고, 비교는 대소문자·구분자·끝 `\` 무시. `canonicalize`가 붙이는 `\\?\`는 떼어낸다(git이 받지 않음). 긴 경로는 `core.longpaths`와 Rust std의 자동 `\\?\` 처리에 맡긴다.
- `-`로 시작하는 ref는 거절하고 `rev-parse`/`branch -D`에는 `--end-of-options`를 쓴다(옵션 주입 방지).
- `POD_GIT`으로 git 실행 파일을 바꿀 수 있다.

### 4.4 설정
- worktree 루트: `podd --worktrees-root DIR` > 환경 변수 `POD_WORKTREES_ROOT` > `%USERPROFILE%\pod\worktrees`. Task 기록은 절대 경로를 저장하므로 루트를 바꿔도 기존 Task는 그대로다. `pod`가 데몬을 자동 시작할 때는 `pod`의 환경을 물려주므로 `POD_WORKTREES_ROOT`가 전달된다.

## 5. CLI

```
pod repo add <path> [--name N]           pod repo ls           pod repo rm <name|path>
pod task new <repo> <name> [--base REF] [--agent claude|codex|shell] [--prompt TEXT] [--cols C --rows R] [-- ARGV...]
pod task ls [--repo R]                   pod task show <task>  pod task diff <task>
pod task rm <task> [--force] [--delete-branch]
pod run --task <task> -- <argv>          (Task에 터미널 추가, cwd = worktree)
```

모두 `--json`을 받는다(결과 구조체를 그대로 출력). `<task>`는 `<repo>/<name>` 또는 유일한 이름. 텍스트 출력 예(release 빌드 실제 실행, 데모 파이프 `--pipe podd-demo-tasks`, `POD_WORKTREES_ROOT`=temp):

```
> pod repo add "%TEMP%\pod-demo-tasks\my app" --name demo
demo C:\Users\User\AppData\Local\Temp\pod-demo-tasks\my app (default base main)
> pod task new demo feat-1
demo/feat-1  t1  pod/feat-1  C:\Users\User\AppData\Local\Temp\pod-demo-tasks\wt\demo\feat-1
> pod send demo/feat-1 "Set-Content hello.txt 'a','b'; git add hello.txt; git commit -qm 'add hello'; Set-Content scratch.txt 'x'; echo demo-ok"
> pod task ls
TASK         TERMINALS   BRANCH      AHEAD  BEHIND  DIRTY  LAST COMMIT
demo/feat-1  t1:running  pod/feat-1  1      0       1      6883022d07 add hello
> pod task show feat-1
task       demo/feat-1
agent      shell
branch     pod/feat-1
base       main (created from 5fa727c87d)
worktree   C:\Users\User\AppData\Local\Temp\pod-demo-tasks\wt\demo\feat-1
terminals  t1:running
status     1 ahead, 0 behind, 1 dirty, 1 unpushed
last       6883022d07 add hello (Pod Demo)
> pod task diff feat-1
ST  ADDED  DELETED  PATH
A   +2     -0       hello.txt
?   +1     -0       scratch.txt
2 file(s), +3 -0 against main (5fa727c87d)
> pod task rm feat-1
pod: task demo/feat-1 has 1 uncommitted change(s) and 1 unpushed commit(s); use force     (종료 코드 2)
> pod task rm feat-1 --force --delete-branch
demo/feat-1 removed (terminals: t1; worktree removed; branch deleted)
```

## 6. 검증

- 단위 테스트: 이름 규칙, 에이전트 argv·prompt 규칙, 레지스트리 저장/읽기·이름/경로 조회·모호한 이름·손상 파일 격리, `worktree list --porcelain -z`·`--numstat -z`(rename·binary)·`--name-status -z`·`status -z`(rename 원본)·`rev-list --left-right`·`log` 파서, 경로 비교·`\\?\` 제거, `AgentSpec` 직렬화, CLI 파싱.
- 통합 테스트 `crates/podd/tests/tasks.rs` (실제 podd·pod-holder·git, 저장소와 worktree 루트는 모두 시스템 temp 아래, 경로에 공백 포함):
  1. `repo.add`: 하위 디렉터리·저장소 아닌 곳 → `NOT_A_REPO`, 공백 있는 기본 이름 → `INVALID_PARAMS`, 이름 지정 → 성공(`default_base = main`), 대소문자만 다른 같은 경로 → `REPO_EXISTS`.
  2. `task.create` agent shell → worktree·브랜치·터미널(이름 = Task id) 확인. 같은 이름·잘못된 이름·없는 저장소·없는 base·shell에 prompt·pod 밖에서 만든 `pod/taken` 브랜치 → 각각 오류, 실패한 생성은 worktree를 남기지 않음.
  3. 터미널 안에서 `POD_TASK_ID`와 cwd 확인 → 파일 추가·수정 후 `git commit` → 추적 안 된 파일 하나 → 저장소 쪽 main에 커밋 하나. `task.show`: ahead 1, behind 1, dirty 1, unpushed 1, 마지막 커밋 "add f". `task.diff`: `f.txt A +3 -0`, `README.md M +0 -1`, `notes.txt ? +2`, base 쪽 `other.txt`는 없음, 합계 +5 -1.
  4. 자식 Task(`base = pod/feat-1`, argv 에이전트, `parent_terminal` = 첫 Task의 터미널) → `parent_task = demo/feat-1`, base_commit = 부모 HEAD. `terminal.run {task}`로 터미널 추가 → cwd = worktree, `POD_TASK_ID` 설정, Task 터미널 2개. Task 있는 저장소 `repo.remove` → `REPO_IN_USE`.
  5. **데몬 재시작**: stop(터미널 유지) → 에이전트 프로세스 생존 → 새 데몬에서 Task 2개, 같은 pid의 터미널이 running, send/wait 동작, 새 터미널 id가 Task의 id와 겹치지 않음.
  6. **삭제 규칙**: dirty+unpushed라 `TASK_DIRTY`(`data = {dirty:1, unpushed:1}`), worktree 그대로 → `force + delete_branch`로 성공: 터미널 kill·삭제, 에이전트 프로세스 종료, worktree 디렉터리·브랜치 없음. 깨끗한 자식 Task는 force 없이 삭제(base 브랜치가 지워져 `base_commit` 기준), 브랜치는 남음, `git worktree list`에 메인만 남음. 마지막으로 `repo.remove` 성공.
  7. 레지스트리가 데몬 재시작 뒤에도 남음, 없는 Task → `TASK_NOT_FOUND`, 경로로 `repo.remove`.
- 수동 확인(release): §5의 흐름을 `pod`로 실행. 이어서 `pod task new demo cl --agent claude`(프롬프트 없음)를 한 번 띄워 `pod read demo/cl`로 Claude Code의 시작 화면(새 worktree에 대한 "Is this a project you trust?" 폴더 신뢰 확인)이 worktree 경로와 함께 뜨는 것을 확인하고, 아무 입력 없이 `task rm --force --delete-branch`로 지웠다. codex는 띄우지 않았다.

## 7. 한계와 다음 단계

1. **unpushed 규칙**: 명세대로 브랜치를 남기는 삭제(`delete_branch=false`)도 unpushed 커밋이 있으면 거절한다. 브랜치가 남으면 커밋을 잃지 않으므로, 나중에 "브랜치를 지울 때만 unpushed 검사"로 완화할 수 있다. (§8: `delete_branch`는 이제 병합이 증명된 브랜치만 지운다.)
2. **`.cmd` 에이전트의 prompt 제한**(§4.1): `" % 줄바꿈`이 든 `prompt`는 여전히 받지 않는다. 긴·여러 줄 글은 §8의 **브리프**(`brief`)로 넘긴다.
3. 원격 저장소 작업(fetch/push/PR)은 없다(P4). ahead/behind는 로컬 ref 기준이다.
4. Task 상태(에이전트 idle/working/blocked)는 아직 없다. pod-status 엔진이 붙으면 `TaskInfo`에 터미널 상태를 더한다. 변경 알림(`events.subscribe`)도 아직 없어 UI는 `task.list`를 폴링해야 한다.
5. `task.list`는 Task마다 git을 5번 실행한다(Task 수십 개까지는 문제없음). 많아지면 캐시 + 파일 감시로 바꾼다.
6. fan-out(같은 프롬프트를 N개 Task로)과 자식 Task 트리 조회(`task.list --children`)는 P5에서 parent 기록 위에 올린다.
7. SSH/WSL 저장소는 지원하지 않는다(P6).

## 8. 워크트리 세션 (docs/19 S1–S3)

코드: `server/sessions.rs`(이름·base·브리프·setup·세션 id), `server/repo_config.rs`(`.pod/config.toml`·승인), `server/task_archive.rs`(보관·정리·재개·삭제 규칙), 타입 `session_protocol.rs`, CLI `pod-cli/src/session.rs`, 앱 `pod-app/src/auto_open.rs`. 프로토콜 v14, 모두 추가형. 테스트: `tests/sessions.rs`(S1·S2·S3), 단위 테스트, 앱 `auto_open::tests`.

**`task.create` 추가 필드**

| 필드 | 동작 |
|---|---|
| `name` 생략(`""`) | 단어 목록에서 자동 제안(`-2`, `-3`…). 살아 있는 Task, **지운 Task 이름**(`tasks/retired.json`), 있는 `pod/<name>` 브랜치·worktree 폴더는 건너뛴다. `task.suggest_name {repo}`도 같다. 직접 준 이름은 은퇴 이름이어도 받는다 |
| `base` | 같은 이름이 여러 ref 공간에 있으면 한정한다: `/`가 있으면 `refs/remotes/` 먼저, 아니면 `refs/heads/` 먼저(태그가 브랜치를 가리지 않음). 모호하지 않으면 그대로 |
| `branch` | `pod/<name>` 대신. 이미 있는 브랜치면 그대로 체크아웃(`branch_existed`, Pod는 절대 지우지 않음) |
| `brief` | 길이·문자 제한 없음. `<state>\briefs\<repo>\<name>.md`에 머리말(`brief_version 1`)과 함께 쓰고 `POD_BRIEF`로 알린다. 에이전트에는 안전한 한 줄 `Read the task brief in the file named by POD_BRIEF and carry it out.`(따옴표·`%`·줄바꿈 없음)만 간다. `brief_line`으로 바꿀 수 있고 `{brief_path}` 치환, 같은 문자 검사. `prompt`와 함께 줄 수 없다 |
| 전달 방식 | claude/codex: CLI 첫 인자(`argv`). argv 에이전트, 또는 `wait` 모드 setup이 있을 때: 터미널에 입력+Enter(`typed`, 상태 idle 또는 출력 1.5초 조용할 때, 제출 확인 경로). shell: 입력하지 않음(`env`, 문장이 명령으로 실행되므로) |
| `setup` / `setup_mode` | `run|skip|inherit`(기본 inherit), `parallel|wait`(기본 저장소 설정) |
| `open_hint` | 기본: Pod 터미널 밖에서 만들면 `{focus:true, reason:user}`, 안(`parent_terminal`)이면 `{focus:false, reason:worker}`. `task.changed created`에 실린다 |
| `env` | Task의 모든 터미널(에이전트·setup·탭·재개)에 더하는 환경 변수 |

`TaskInfo` 추가: `lifecycle`(active/archived), `agent_terminal`, `setup {state: running|ok|failed|skipped|untrusted, exit_code, terminal, source, hash}`, `brief {path, first_line, delivery, state: pending|sent|failed, after_setup}`, `agent_session {agent, id, captured_ms, source}`, `branch_existed`. `task.changed`에 `updated` 변경 종류(위 필드가 바뀜)와 `open_hint`.

**`.pod/config.toml`** (스키마는 `RepoConfigFile` 주석, 모르는 키는 오류): `[setup] script, mode`, `[[tabs]] title, command`, `[archive] script`, `[worker] agent, env`. 명령은 Windows PowerShell(`-NoProfile -ExecutionPolicy Bypass -EncodedCommand`, 따옴표 처리 없음)로 worktree에서 실행하고, 종료 코드는 `exit N` > 마지막 네이티브 종료 코드 > 마지막 문장의 `$?` 순이다.
- 새 worktree의 파일(= base 내용)을 읽는다. 명령(setup·tabs·archive)의 SHA-256이 저장소의 `approved_hash`와 같을 때만 실행한다. 승인 전에는 setup을 건너뛰고 `setup: untrusted` + `hash`, 탭도 열지 않는다. 내용이 바뀌면 해시가 달라져 다시 묻는다. 승인은 `repo.set {trust: {hash, approve}}` — 해시가 지금 파일과 다르면 거절.
- 개인 설정 `repo.set {personal: {setup?, tabs?, archive?}}`가 절별로 파일보다 우선하며 승인이 필요 없다. `repo.set {worktrees_root}`로 저장소별 worktree 루트. `repo.config {repo, task?}`가 파일·해시·승인·실제 명령 목록을 보여 준다.
- setup은 Task의 터미널 `<id>:setup`에서 돌고, 끝나면 상태·종료 코드를 기록한다. setup이 시작조차 못 해도 Task 생성은 실패하지 않는다. 탭은 `<id>:<title>` 터미널(`-NoExit`). `wait` 모드에서는 탭과 typed 브리프를 setup이 끝난 뒤(성공·실패 모두) 시작한다. 데몬이 재시작되면 진행 중인 setup 감시와 대기 중인 브리프 입력을 다시 잡는다.

**보관·재개·정리**

| 메서드 | 동작 |
|---|---|
| `task.archive {task, force?, ignore_script_failure?}` | 에이전트가 working/blocked면 `TASK_BUSY`(-32070, force로 무시). archive 스크립트를 **먼저** 실행(10분, `POD_ARCHIVE_TIMEOUT_SECS`). 스크립트와 그 자식 전체가 한 Job Object 트리에 들어가고, 시간 제한은 출력 파이프를 잡은 남은 자식까지 포함한 호출 전체에 걸리며(끝나면 트리를 죽임), 출력은 읽는 동안 마지막 4 KiB만 남긴다(`pod_process::bounded`, orch-03). 실패하거나 승인되지 않았으면 `ARCHIVE_FAILED`(-32071, `data {exit_code, output}`), 아무것도 바꾸지 않음. 그다음 터미널을 모두 닫고 `lifecycle: archived`. worktree·브랜치·세션 id는 남음 |
| `task.unarchive {task, start_agent?}` | active로 돌리고 에이전트를 다시 시작(세션이 있으면 재개) |
| `task.resume {task, session?, agent?}` | 에이전트 터미널이 살아 있으면 `TERMINAL_RUNNING`. 아니면 죽은 터미널을 지우고 새 에이전트 터미널(이름 = Task id, 첫 번째): 세션 id가 있으면 에이전트 자체 명령 `claude --resume <id>` / `codex [--no-daemon -c …] resume <id>`(둘 다 각 CLI의 `--help`로 확인), 없으면 새 에이전트. 세션 id는 `[A-Za-z0-9._:-]`만 |
| `task.cleanup_candidates {repo?, inactive_days?=7}` | **protected**(working/blocked 에이전트, 잠긴·중첩 worktree, setup 진행 중, git 상태 모름, 최근 활동), **review**(dirty, unpushed, base 없음), **ready**(깨끗 + PR 병합 / 보관됨 / N일 비활성) |

- **세션 id 기록**: 에이전트 터미널의 훅 메시지가 받아들여지면 페이로드의 `session_id`를 `agent_session {source: hook}`으로 저장한다(Claude·Codex 훅 모두). 18의 세션 찾기는 아직 병합되지 않아, 찾은 id는 `task.resume {session}`으로 넘기면 된다(`source: user`로 기록). 재개 명령은 `sessions::resume_argv` 하나에 있다.
- **`task.remove` 추가 규칙**: 잠긴(`git worktree lock`) worktree와 안에 다른 worktree가 있는 경로는 force여도 `WORKTREE_LOCKED`(-32072)로 거절. `delete_branch`는 **지금의** 브랜치 끝이 base에 포함되거나, 병합된 PR의 head 커밋(`TaskPr.head_oid`, gh `headRefOid`)과 같을 때만 지우고, 아니면 남기고 `branch_kept`에 이유를 준다(force여도 같음). PR 병합 뒤 커밋을 더한 브랜치, head 커밋을 모르는 옛 기록은 남는다(orch-04). 지운 Task 이름은 은퇴 목록에 들어가고 브리프 파일은 지운다.
- **앱**: `task.changed created`의 `open_hint`로 에이전트 터미널 탭을 연다. `focus:true`면 포커스까지, worker면 탭만 만들고 키보드 포커스와 보이는 탭은 그대로. `POD_APP_AUTO_OPEN=0`으로 끈다.

CLI: `pod task new <repo> [name] [--branch B] [--brief T | --brief-file F|-] [--setup run|skip|inherit] [--wait-setup] [--no-focus]`, `pod task archive|unarchive|resume|cleanup|suggest-name`, `pod repo config|trust <repo> <hash> [--revoke]|set [--setup S] [--archive S] [--worktrees-root D] [--clear-personal]`. `pod task show`에 setup·brief·session 줄.

남은 것: 정리 화면 UI(S7), 보호 단계의 "고정"·"열린 편집기 변경"(데몬이 모름), 워커 배지, 끝난 워커 10분 자동 정리·`[archive] on_release`(S5), sparse-checkout(S8), `handoff = paste`.

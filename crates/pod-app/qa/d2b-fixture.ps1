# Fixture setup only. No real agent, benchmark, user repository or production daemon.
param([switch]$Create)
$ErrorActionPreference = 'Stop'
$D2Root = (Resolve-Path (Join-Path $PSScriptRoot '../../..')).Path
$D2Demo = Join-Path $D2Root 'target/d2b-demo-v3'
$D2Pod = Join-Path $D2Root 'target/release/pod.exe'
$env:POD_SOCKET = 'podd-d2b-polish-qa-v3'
$env:POD_DAEMON = Join-Path $D2Root 'target/release/podd.exe'
$env:POD_WORKTREES_ROOT = Join-Path $D2Demo 'worktrees'
if ($Create) {
    if (Test-Path (Join-Path $D2Demo 'repo/.git')) { throw 'Fixture already exists; refusing to replace it.' }
    New-Item -ItemType Directory -Force (Join-Path $D2Demo 'repo') | Out-Null
    $repo = Join-Path $D2Demo 'repo'
    [IO.File]::WriteAllText((Join-Path $repo 'hello.rs'), "pub fn greeting() -> &'static str {`n    `"hello`"`n}`n")
    git -C $repo init -q -b main
    git -C $repo -c user.name=Fixture -c user.email=fixture@example.invalid add .
    git -C $repo -c user.name=Fixture -c user.email=fixture@example.invalid commit -q -m fixture
    $agent = Join-Path $D2Demo 'agent.ps1'
    [IO.File]::WriteAllText($agent, @'
$e = [char]27; $bel = [char]7
[Console]::Write("$e]7501;state=blocked:kind=permission:app=fixture$bel")
Write-Output 'Fixture only: permission requested; no real agent.'
Start-Sleep -Seconds 14400
'@)
    & $D2Pod repo add $repo --name sample
    & $D2Pod task new sample auth -- powershell.exe -NoProfile -File $agent
    $task = (& $D2Pod task show sample/auth --json | ConvertFrom-Json)
    [IO.File]::WriteAllText((Join-Path $task.worktree 'hello.rs'), "pub fn greeting() -> &'static str {`n    `"hello, Pod`"`n}`n")
    & $D2Pod review add sample/auth hello.rs 2 --body '검증용 의견: 인사 문구를 확인해 주세요.'
    & $D2Pod group new sample compare --prompt "Write-Output 'fixture only'" --agent shell --agent shell -- cmd /c echo pass
    & $D2Pod group check sample/compare
    & $D2Pod gate request '검증용 승인 요청입니다. 실제 작업은 실행하지 않습니다.' --title '샘플 승인 요청' --task sample/auth
    & $D2Pod run --name exit-seven -- cmd.exe /c exit 7
}

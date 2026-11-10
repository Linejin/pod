# Isolated real daemon/ConPTY fixture, for keyboard-only integration checks.
Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
$env:RUSTC_WRAPPER = 'C:\Users\User\AppData\Local\Microsoft\WinGet\Packages\Mozilla.sccache_Microsoft.Winget.Source_8wekyb3d8bbwe\sccache-v0.19.0-x86_64-pc-windows-msvc\sccache.exe'
$env:CARGO_BUILD_JOBS = '4'
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '../../../..')).Path
$fixture = Join-Path $root 'target/d3-native'
$repo = Join-Path $fixture 'keyboard-repo'
if (Test-Path $repo) { throw 'Use the existing fixture; refusing to overwrite it' }
New-Item -ItemType Directory -Force $repo | Out-Null
[IO.File]::WriteAllText((Join-Path $repo 'hello.txt'), "Fixture only`n한글 입력 확인`n")
git -C $repo init -q -b main
git -C $repo -c user.name=Fixture -c user.email=fixture@example.invalid add .
git -C $repo -c user.name=Fixture -c user.email=fixture@example.invalid commit -q -m fixture
$env:POD_APP_CONFIG_DIR = Join-Path $fixture 'config'
$env:POD_WORKTREES_ROOT = Join-Path $fixture 'worktrees'
$env:POD_DAEMON = Join-Path $root 'target/release/podd.exe'
$env:POD_SOCKET = 'podd-d3-native-qa'
New-Item -ItemType Directory -Force $env:POD_APP_CONFIG_DIR | Out-Null
[IO.File]::WriteAllText((Join-Path $env:POD_APP_CONFIG_DIR 'settings.json'), '{"theme":"dark","reduced_motion":true,"font":"Malgun Gothic"}')
& C:\ai\ide\tools\cpu-gate.ps1 -MaxPct 70 -MaxCargo 1
$app = Start-Process (Join-Path $root 'target/release/pod-app.exe') -ArgumentList '--display secondary --pipe podd-d3-native-qa --no-sound' -PassThru
$app.Id | Set-Content (Join-Path $fixture 'app.pid')
[pscustomobject]@{AppPid=$app.Id; Repository=$repo; Config=$env:POD_APP_CONFIG_DIR; Socket=$env:POD_SOCKET} | ConvertTo-Json

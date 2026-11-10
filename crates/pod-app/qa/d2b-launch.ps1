param(
    [ValidateSet('before','after')][string]$Phase = 'after',
    [ValidateSet('dark','light')][string]$Theme = 'dark',
    [string]$Task = 'sample/auth',
    [string]$CapturePass = 'integrated'
)
. (Join-Path $PSScriptRoot 'd2b-fixture.ps1')
$env:POD_APP_CONFIG_DIR = Join-Path $D2Demo "config-$Phase-$Theme-$CapturePass"
New-Item -ItemType Directory -Force $env:POD_APP_CONFIG_DIR | Out-Null
if (Test-Path (Join-Path $env:POD_APP_CONFIG_DIR 'layout.json')) {
    throw 'Use a fresh fixture config for a repeatable capture; existing layout was not removed.'
}
[IO.File]::WriteAllText((Join-Path $env:POD_APP_CONFIG_DIR 'settings.json'), ('{"theme":"'+$Theme+'","reduced_motion":true}'))
& C:\ai\ide\tools\cpu-gate.ps1 -MaxPct 70 -MaxCargo 1
$exe = Join-Path $D2Root "target/d2b-$Phase/pod-app.exe"
$app = Start-Process -FilePath $exe -ArgumentList "--display secondary --pipe podd-d2b-polish-qa-v3 --review $Task" -WindowStyle Normal -PassThru -RedirectStandardOutput (Join-Path $D2Demo "$Phase-$Theme.out") -RedirectStandardError (Join-Path $D2Demo "$Phase-$Theme.err")
$app.Id | Set-Content (Join-Path $D2Demo 'app.pid')
$app.Id

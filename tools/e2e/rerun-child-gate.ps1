# SPDX-License-Identifier: Apache-2.0
param([Parameter(Mandatory=$true)][string]$Pod)
Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
$env:RUSTC_WRAPPER = 'C:\Users\User\AppData\Local\Microsoft\WinGet\Packages\Mozilla.sccache_Microsoft.Winget.Source_8wekyb3d8bbwe\sccache-v0.19.0-x86_64-pc-windows-msvc\sccache.exe'
$env:CARGO_BUILD_JOBS = '4'
$root = 'C:\ai\pod-e2e\rerun-c9cf81'
if ($env:POD_SOCKET -notlike '*pod-e2e-rerun-c9cf81' -or !$env:POD_TERMINAL_ID) {
    throw 'Run only from the isolated rerun agent terminal.'
}
& $Pod run --name rerun-child -- powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root 'child.ps1')
if ($LASTEXITCODE -ne 0) { exit 1 }
& $Pod gate request 'Continue rerun after daemon restart?' --title 'Rerun reconnect' --wait --timeout 600
if ($LASTEXITCODE -ne 0) { exit 1 }
Set-Content gate-approved.txt 'approved'
Write-Output 'GATE_CONTINUED'

# SPDX-License-Identifier: Apache-2.0
# Copy to C:\ai\pod-e2e\child-gate.ps1 and ask a Pod Codex task to run it.
# A sandboxed Codex shell needs permission to access the private named pipe.
param([Parameter(Mandatory=$true)][string]$Pod)
Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
$env:RUSTC_WRAPPER = 'sccache'
$env:CARGO_BUILD_JOBS = '4'
if ($env:POD_SOCKET -notlike '*pod-e2e-5dfd20c69ade' -or !$env:POD_TERMINAL_ID) {
    throw 'Run only from the isolated Pod agent terminal.'
}
& $Pod run --name e2e-child -- powershell -NoProfile -ExecutionPolicy Bypass -File C:\ai\pod-e2e\child.ps1
& $Pod gate request 'Expiry fixture' --title 'E2E expiry' --expires 2 --wait --timeout 10
Write-Output "EXPIRY_EXIT=$LASTEXITCODE"
& $Pod gate request 'Continue after expiry?' --title 'E2E final approval' --expires 600 --wait --timeout 600
if ($LASTEXITCODE -eq 0) {
    Set-Content gate-approved.txt 'approved'
    Write-Output 'GATE_CONTINUED'
} else { exit 1 }

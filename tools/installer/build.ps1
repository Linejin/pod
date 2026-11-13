# Builds the Pod installer and portable zip from this checkout.
#
#   powershell -NoProfile -File tools/installer/build.ps1 [-Unsigned | -SigningConfig <json>] [-Release]
#   ... build.ps1 -PrintVersion   # print the build label and exit
#   ... build.ps1 -CheckPrerequisites [-SigningConfig <json>]
#
# Build label: without -Release it is <ver>-dev.<git sha>[-dirty], so every
# development installer has its own name and "Pod <label>" in Apps & features.
# -Release uses the plain <ver> and requires HEAD to carry the tag v<ver>
# (release steps: docs/14 §릴리스, CHANGELOG.md).
#
# Steps: version from [workspace.package] in Cargo.toml -> bundled ConPTY
# (tools/fetch-conpty.ps1 if vendor/conpty is missing) -> cargo deny check ->
# cargo build --release --locked (pod-app, podd, pod-cli, pod-hook) ->
# stage + THIRD-PARTY-NOTICES.txt (notices.ps1) -> optional payload signing ->
# Inno Setup internal signing (pod.iss) -> verification -> portable zip -> SHA-256.
#
# Output (git-ignored): dist\Pod-Setup-<label>-x64.exe, dist\Pod-<label>-x64-portable.zip,
# dist\SHA256SUMS.txt, dist\stage\Pod-<label>\ (the staged files).
# Needs: Rust (stable, msvc), cargo-deny, Inno Setup 6 (winget install JRSoftware.InnoSetup --scope user).
# Before each cargo run this waits on tools/cpu-gate.ps1 (shared machine; see docs/14).
param(
  [switch]$Release,
  [switch]$PrintVersion,
  [switch]$SkipBuild,
  [switch]$SkipDeny,
  [switch]$NoCpuGate,
  [switch]$CheckPrerequisites,
  [switch]$Unsigned,
  [string]$SigningConfig,
  [string]$Iscc = $env:ISCC
)
$ErrorActionPreference = "Stop"
# Resolve caller-relative configuration before changing the working directory.
if ($SigningConfig) { $SigningConfig = (Resolve-Path -LiteralPath $SigningConfig).Path }
$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
Set-Location $Root
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH" }

function Gate {
  if ($NoCpuGate) { return }
  $g = if ($env:POD_CPU_GATE) { $env:POD_CPU_GATE } else { Join-Path $Root "tools\cpu-gate.ps1" }
  if (Test-Path $g) { & $g -MaxPct 70 -MaxCargo 1 }
}
function Step([string]$s) { Write-Host "==> $s" -ForegroundColor Cyan }
. (Join-Path $PSScriptRoot 'sign.ps1')

# Diagnose signing inputs before downloading dependencies or compiling. A plain
# development/OSS application build stays available; unsigned release is explicit.
if (-not $PrintVersion) {
  if ($Unsigned -and $SigningConfig) { throw '-Unsigned and -SigningConfig cannot be combined.' }
  if ($Release -and -not $SigningConfig -and -not $Unsigned) { throw '-Release requires -SigningConfig, or explicit -Unsigned for an OSS application/release candidate. Unsigned packages are not SAC-compatible. See docs/install/signpath.md.' }
  $Signing = $null
  if ($SigningConfig) {
    $Signing = Read-PodSigningConfig $SigningConfig
    Assert-PodSigningReady $Signing
    if ($SkipBuild -or $SkipDeny) { throw 'Signed builds require fresh cargo build and cargo deny; omit -SkipBuild/-SkipDeny.' }
  } else { Write-Warning 'UNSIGNED build: not prepared for Smart App Control distribution. See docs/install/signpath.md for OSS signing preparation.' }
  $VerifyTool = if ($Signing) { $Signing.SignTool } else { Find-PodSignTool }

  if (-not $Iscc) {
    foreach ($k in 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1',
                   'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1',
                   'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1') {
      $loc = (Get-ItemProperty $k -ErrorAction SilentlyContinue).InstallLocation
      if ($loc -and (Test-Path (Join-Path $loc 'ISCC.exe'))) { $Iscc = Join-Path $loc 'ISCC.exe'; break }
    }
  }
  if (-not $Iscc -or -not (Test-Path -LiteralPath $Iscc -PathType Leaf)) {
    throw 'Inno Setup 6 not found: install Inno Setup 6.7.3 or set -Iscc to ISCC.exe.'
  }
  & rustc --version
  if ($LASTEXITCODE -ne 0) { throw 'rustc cannot run. If Windows reports error 4551, use an external Windows build host that permits the toolchain; do not disable Smart App Control or change application-control policy.' }
  if (-not $SkipDeny) {
    & cargo deny --version
    if ($LASTEXITCODE -ne 0) { throw 'cargo-deny is unavailable. On the external build host: cargo install cargo-deny --version 0.20.2 --locked' }
  }
  if ($CheckPrerequisites) { Step 'prerequisites passed (no build or signing performed)'; exit 0 }
}

# 1. version
$toml = Get-Content -Raw (Join-Path $Root "Cargo.toml")
$m = [regex]::Match($toml, '(?ms)^\[workspace\.package\][^\[]*?^version\s*=\s*"([^"]+)"')
if (-not $m.Success) { throw "no version in [workspace.package] of Cargo.toml" }
$Version = $m.Groups[1].Value
$sha = (& git rev-parse --short=8 HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or -not $sha) { throw "git rev-parse HEAD failed" }
if ($Release) {
  if (@(& git tag --points-at HEAD) -notcontains "v$Version") { throw "-Release needs HEAD tagged v$Version (git tag v$Version)" }
  if (& git status --porcelain --untracked-files=no) { throw "-Release needs a clean working tree" }
  $Label = $Version
} else {
  $Label = "$Version-dev.$sha"
  if (& git status --porcelain --untracked-files=no) { $Label += "-dirty" }
}
if ($PrintVersion) { Write-Output $Label; exit 0 }
Step "Pod $Label"
# A failed rebuild must not leave a success manifest for partly replaced files.
$Dist = Join-Path $Root 'dist'
$SumsPath = Join-Path $Dist 'SHA256SUMS.txt'
if (Test-Path -LiteralPath $SumsPath) { Remove-Item -LiteralPath $SumsPath -Force }

# 2. bundled ConPTY
$conpty = Join-Path $Root "vendor\conpty"
if (-not (Test-Path (Join-Path $conpty "conpty.dll")) -or -not (Test-Path (Join-Path $conpty "x64\OpenConsole.exe"))) {
  Step "fetch ConPTY"
  & powershell -NoProfile -File (Join-Path $Root "tools\fetch-conpty.ps1")
  if ($LASTEXITCODE -ne 0) { throw "fetch-conpty failed" }
}

# Verify upstream binaries even for the unsigned OSS application package.
foreach ($file in (Join-Path $conpty 'conpty.dll'), (Join-Path $conpty 'x64\OpenConsole.exe')) {
  $record = Test-PodSignature $file $VerifyTool
  if ($record.Publisher -notmatch '(^|,\s*)CN=Microsoft Corporation(,|$)') { throw "Expected original Microsoft signature: $file" }
}

# 3. license/advisory policy
if (-not $SkipDeny) {
  Step "cargo deny check"
  Gate
  & cargo deny --log-level error check
  if ($LASTEXITCODE -ne 0) { throw "cargo deny check failed" }
}

# 4. release build
Gate
$meta = (& cargo metadata --format-version 1 --no-deps --locked) -join "`n" | ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or -not $meta.target_directory) { throw 'cargo metadata failed' }
$Target = Join-Path $meta.target_directory "release"
if (-not $SkipBuild) {
  Step "cargo build --release"
  Gate
  & cargo build --release --locked -p pod-app -p podd -p pod-cli -p pod-hook
  if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
}

# 5. stage
$Stage = Join-Path $Dist "stage\Pod-$Label"
$Stage = [IO.Path]::GetFullPath($Stage)
$stageRoot = [IO.Path]::GetFullPath((Join-Path $Root "dist\stage")).TrimEnd('\') + '\'
if (-not $Stage.StartsWith($stageRoot, [StringComparison]::OrdinalIgnoreCase)) { throw "Unsafe stage path: $Stage" }
Step "stage -> $Stage"
if (Test-Path $Stage) { Remove-Item -Recurse -Force $Stage }
New-Item -ItemType Directory -Force (Join-Path $Stage "x64"), (Join-Path $Stage "licenses") | Out-Null
foreach ($exe in "pod-app.exe", "podd.exe", "pod-holder.exe", "pod.exe", "pod-hook.exe") {
  $src = Join-Path $Target $exe
  if (-not (Test-Path $src)) { throw "missing $src (build first)" }
  Copy-Item $src $Stage
}
Copy-Item (Join-Path $conpty "conpty.dll") $Stage
Copy-Item (Join-Path $conpty "x64\OpenConsole.exe") (Join-Path $Stage "x64")
Copy-Item (Join-Path $Root "LICENSE") (Join-Path $Stage "LICENSE.txt")
Copy-Item (Join-Path $PSScriptRoot "README.txt") (Join-Path $Stage "README.txt")
$signingNotice = if ($Signing) {
  "`r`nSigning: Authenticode signatures and timestamps verified during packaging. See SHA256SUMS.txt alongside the download."
} else { "`r`nSigning: UNSIGNED DEVELOPMENT BUILD. Not prepared for Smart App Control distribution." }
Add-Content -LiteralPath (Join-Path $Stage 'README.txt') -Encoding UTF8 -Value $signingNotice
Copy-Item (Join-Path $PSScriptRoot "licenses\ConPTY-LICENSE.txt") (Join-Path $Stage "licenses")
Copy-Item (Join-Path $Root "crates\pod-ui\THIRD-PARTY.md") (Join-Path $Stage "licenses\Lucide-LICENSE.txt")
Gate
& (Join-Path $PSScriptRoot "notices.ps1") -Out (Join-Path $Stage "THIRD-PARTY-NOTICES.txt") -Version $Version

# 6. Sign only staged Pod binaries. Preserve the pinned Microsoft components.
$isccArgs = @('/Qp', "/DAppVersion=$Label", "/DFileVersion=$Version", "/DStageDir=$Stage", "/DOutputDir=$Dist")
if ($Signing) {
  Step 'sign Pod payload and verify all bundled executable components'
  foreach ($exe in 'pod-app.exe', 'podd.exe', 'pod-holder.exe', 'pod.exe', 'pod-hook.exe') {
    Invoke-PodSign (Join-Path $Stage $exe) $Signing | Out-Null
  }
  $signatureRecords = @(Test-PodPayload $Stage $Signing.SignTool)
  $SignDir = Join-Path $Dist ("signing\$Label-" + [guid]::NewGuid().ToString('N'))
  New-Item -ItemType Directory -Path $SignDir -Force | Out-Null
  # Freeze resolved non-secret settings for Inno's child processes. Never pass
  # private keys or passwords on the command line or write them to this file.
  $ResolvedConfig = Join-Path $SignDir 'config.json'
  $Signing | ConvertTo-Json | Set-Content -LiteralPath $ResolvedConfig -Encoding UTF8
  $callback = Join-Path $PSScriptRoot 'sign.ps1'
  $ps = Join-Path $PSHOME 'powershell.exe'
  if (-not (Test-Path -LiteralPath $ps)) { throw 'Run build.ps1 in Windows PowerShell 5.1 (not pwsh); notices.ps1 also requires .NET Framework.' }
  $Evidence = Join-Path $SignDir 'verified-copies'
  New-Item -ItemType Directory -Path $Evidence -Force | Out-Null
  $command = New-PodInnoSignCommand $ps $callback $ResolvedConfig $Evidence
  $isccArgs += @('/DPodSigned', "/DSignedOutputDir=$SignDir", "/SPodReleaseSign=$command")
}

# 7. Inno signs Setup, uninstaller, and temporary setup self-copies on the fly.
Step "ISCC ($Iscc)"
$Setup = Join-Path $Dist "Pod-Setup-$Label-x64.exe"
if (Test-Path $Setup) { Remove-Item -Force $Setup }
& $Iscc @isccArgs (Join-Path $PSScriptRoot "pod.iss")
if ($LASTEXITCODE -ne 0) { throw "ISCC failed ($LASTEXITCODE)" }
if (-not (Test-Path -LiteralPath $Setup -PathType Leaf)) { throw "ISCC did not produce $Setup" }
if ($Signing) {
  $signatureRecords += Test-PodSignature $Setup $Signing.SignTool
  $copies = @(Get-ChildItem -LiteralPath $Evidence -Filter '*.signed.exe' -File)
  if (@($copies | Where-Object { $_.Name -like 'uninst*.signed.exe' }).Count -eq 0) { throw 'Inno signing callback did not retain a verified uninstaller copy.' }
  foreach ($copy in $copies) { $signatureRecords += Test-PodSignature $copy.FullName $Signing.SignTool }
  # Recheck the final stage just before creating the portable package.
  Test-PodPayload $Stage $Signing.SignTool | Out-Null
  $SignatureReport = Join-Path $Dist "Pod-$Label-signatures.json"
  $signatureRecords | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $SignatureReport -Encoding UTF8
}

# 8. portable zip (folder Pod-<ver>\ inside, already signed)
$Zip = Join-Path $Dist "Pod-$Label-x64-portable.zip"
Step "zip -> $Zip"
if (Test-Path $Zip) { Remove-Item -Force $Zip }
Compress-Archive -Path $Stage -DestinationPath $Zip -CompressionLevel Optimal

# 9. Checksums are generated only after signing and verification succeed.
$Artifacts = @($Setup, $Zip)
if ($Signing) { $Artifacts += $SignatureReport }
$sums = foreach ($f in $Artifacts) {
  $h = (Get-FileHash -Algorithm SHA256 $f).Hash.ToLower()
  "$h  $(Split-Path -Leaf $f)"
}
Set-Content -Encoding ascii $SumsPath $sums
Step "done"
foreach ($f in $Setup, $Zip) {
  $i = Get-Item $f
  "{0}  {1:N1} MB" -f $i.FullName, ($i.Length / 1MB)
}
$sums

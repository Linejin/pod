# Builds the Pod installer and portable zip from this checkout.
#
#   powershell -ExecutionPolicy Bypass -File tools/installer/build.ps1 [-Release] [-SkipBuild] [-SkipDeny] [-NoCpuGate]
#   ... build.ps1 -PrintVersion   # print the build label and exit
#
# Build label: without -Release it is <ver>-dev.<git sha>[-dirty], so every
# development installer has its own name and "Pod <label>" in Apps & features.
# -Release uses the plain <ver> and requires HEAD to carry the tag v<ver>
# (release steps: docs/14 §릴리스, CHANGELOG.md).
#
# Steps: version from [workspace.package] in Cargo.toml -> bundled ConPTY
# (tools/fetch-conpty.ps1 if vendor/conpty is missing) -> cargo deny check ->
# cargo build --release --locked (pod-app, podd, pod-cli, pod-hook) ->
# THIRD-PARTY-NOTICES.txt (notices.ps1) -> stage -> Inno Setup (pod.iss) ->
# portable zip -> SHA-256.
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
  [string]$Iscc = $env:ISCC
)
$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
Set-Location $Root
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH" }

function Gate {
  if ($NoCpuGate) { return }
  $g = if ($env:POD_CPU_GATE) { $env:POD_CPU_GATE } else { Join-Path $Root "tools\cpu-gate.ps1" }
  if (Test-Path $g) { & $g -MaxPct 70 -MaxCargo 1 }
}
function Step([string]$s) { Write-Host "==> $s" -ForegroundColor Cyan }

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

# 2. bundled ConPTY
$conpty = Join-Path $Root "vendor\conpty"
if (-not (Test-Path (Join-Path $conpty "conpty.dll")) -or -not (Test-Path (Join-Path $conpty "x64\OpenConsole.exe"))) {
  Step "fetch ConPTY"
  & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $Root "tools\fetch-conpty.ps1")
  if ($LASTEXITCODE -ne 0) { throw "fetch-conpty failed" }
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
$Target = Join-Path $meta.target_directory "release"
if (-not $SkipBuild) {
  Step "cargo build --release"
  Gate
  & cargo build --release --locked -p pod-app -p podd -p pod-cli -p pod-hook
  if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
}

# 5. stage
$Dist = Join-Path $Root "dist"
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
Copy-Item (Join-Path $PSScriptRoot "licenses\ConPTY-LICENSE.txt") (Join-Path $Stage "licenses")
Copy-Item (Join-Path $Root "crates\pod-ui\THIRD-PARTY.md") (Join-Path $Stage "licenses\Lucide-LICENSE.txt")
Gate
& (Join-Path $PSScriptRoot "notices.ps1") -Out (Join-Path $Stage "THIRD-PARTY-NOTICES.txt") -Version $Version

# 6. installer
if (-not $Iscc) {
  foreach ($k in "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1",
                 "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1",
                 "HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1") {
    $loc = (Get-ItemProperty $k -ErrorAction SilentlyContinue).InstallLocation
    if ($loc -and (Test-Path (Join-Path $loc "ISCC.exe"))) { $Iscc = Join-Path $loc "ISCC.exe"; break }
  }
}
if (-not $Iscc -or -not (Test-Path $Iscc)) {
  throw "Inno Setup 6 not found: winget install --id JRSoftware.InnoSetup -e --scope user (or set ISCC)"
}
Step "ISCC ($Iscc)"
$Setup = Join-Path $Dist "Pod-Setup-$Label-x64.exe"
if (Test-Path $Setup) { Remove-Item -Force $Setup }
& $Iscc /Qp "/DAppVersion=$Label" "/DFileVersion=$Version" "/DStageDir=$Stage" "/DOutputDir=$Dist" (Join-Path $PSScriptRoot "pod.iss")
if ($LASTEXITCODE -ne 0) { throw "ISCC failed ($LASTEXITCODE)" }

# 7. portable zip (folder Pod-<ver>\ inside)
$Zip = Join-Path $Dist "Pod-$Label-x64-portable.zip"
Step "zip -> $Zip"
if (Test-Path $Zip) { Remove-Item -Force $Zip }
Compress-Archive -Path $Stage -DestinationPath $Zip -CompressionLevel Optimal

# 8. checksums
$sums = foreach ($f in $Setup, $Zip) {
  $h = (Get-FileHash -Algorithm SHA256 $f).Hash.ToLower()
  "$h  $(Split-Path -Leaf $f)"
}
Set-Content -Encoding ascii (Join-Path $Dist "SHA256SUMS.txt") $sums
Step "done"
foreach ($f in $Setup, $Zip) {
  $i = Get-Item $f
  "{0}  {1:N1} MB" -f $i.FullName, ($i.Length / 1MB)
}
$sums

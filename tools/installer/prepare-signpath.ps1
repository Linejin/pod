# Prepares only project-owned PE files for a future SignPath request. No API use.
param(
  [Parameter(Mandatory = $true)][string]$Stage,
  [string]$OutDirectory,
  [switch]$RequireMetadata
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'sign.ps1')
$root = Split-Path (Split-Path $PSScriptRoot)
$Stage = (Resolve-Path -LiteralPath $Stage).Path
if (-not $OutDirectory) { $OutDirectory = Join-Path $root 'dist\signpath-input' }
$OutDirectory = [IO.Path]::GetFullPath($OutDirectory)
$distRoot = [IO.Path]::GetFullPath((Join-Path $root 'dist')).TrimEnd('\') + '\'
if (-not $OutDirectory.StartsWith($distRoot, [StringComparison]::OrdinalIgnoreCase)) { throw 'SignPath input output must be under this checkout/dist.' }
if ((Test-Path -LiteralPath $OutDirectory) -and (Get-ChildItem -LiteralPath $OutDirectory -Force)) { throw 'SignPath input output directory must be empty; do not mix builds.' }
$toml = Get-Content -LiteralPath (Join-Path $root 'Cargo.toml') -Raw
$version = [regex]::Match($toml, '(?ms)^\[workspace\.package\][^\[]*?^version\s*=\s*"([^"]+)"').Groups[1].Value
if (-not $version) { throw 'Workspace version missing.' }
$tool = Find-PodSignTool
$upstream = foreach ($name in 'conpty.dll','x64\OpenConsole.exe') {
  $record = Test-PodSignature (Join-Path $Stage $name) $tool
  if ($record.Publisher -notmatch '(^|,\s*)CN=Microsoft Corporation(,|$)') { throw "Expected Microsoft signature: $name" }
  $record
}
$ownNames = @('pod-app.exe','podd.exe','pod-holder.exe','pod.exe','pod-hook.exe')
$own = foreach ($name in $ownNames) {
  $file = Get-Item -LiteralPath (Join-Path $Stage $name)
  $info = $file.VersionInfo
  [pscustomobject]@{
    Name = $name; SHA256 = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    ProductName = $info.ProductName; ProductVersion = $info.ProductVersion
    MetadataReady = ($info.ProductName -eq 'Pod' -and $info.ProductVersion -eq $version)
  }
}
$metadataReady = @($own | Where-Object { -not $_.MetadataReady }).Count -eq 0
if (-not $metadataReady -and $RequireMetadata) { throw 'SignPath submission blocked: every Pod PE must have ProductName=Pod and matching workspace ProductVersion. See assets/windows/resources.rs.' }
New-Item -ItemType Directory -Path $OutDirectory -Force | Out-Null
foreach ($name in $ownNames) { Copy-Item -LiteralPath (Join-Path $Stage $name) -Destination $OutDirectory }
$commit = (& git -C $root rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0) { throw 'Cannot determine source commit.' }
[ordered]@{
  Status = 'UNSIGNED INPUT ONLY; no signing request or approval asserted; not SAC-compatible'
  SourceCommit = $commit; Version = $version; MetadataReady = $metadataReady
  Files = @($own); UpstreamVerifiedAndExcludedFromSigning = @($upstream)
  Provenance = 'Informational local manifest. Trusted build origin must be verified by the SignPath GitHub connector.'
} | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $OutDirectory 'readiness.json') -Encoding UTF8
if (-not $metadataReady) { Write-Warning 'PE metadata is not ready for the restrictive SignPath policy. Application samples can be inspected; submission must wait for metadata validation.' }
Write-Output "Prepared $OutDirectory (five project binaries only; no signing or submission)."

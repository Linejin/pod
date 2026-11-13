# Security-path tests; no compiler, signing identity, key creation or policy changes.
# Fetch vendor/conpty first. Runs in Windows PowerShell 5.1.
param([string]$Iscc = $env:ISCC)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'sign.ps1')
$root = Split-Path (Split-Path $PSScriptRoot)
$testRoot = Join-Path $root ('dist\signing-tests-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testRoot -Force | Out-Null
$script:passed = 0
function Assert([bool]$Condition, [string]$Name) {
  if (-not $Condition) { throw "FAIL: $Name" }
  $script:passed++
  Write-Host "PASS: $Name"
}
function Reject([scriptblock]$Body, [string]$Pattern, [string]$Name) {
  $message = ''
  try { & $Body | Out-Null } catch { $message = $_.Exception.Message }
  Assert ($message -match $Pattern) "$Name ($message)"
}
try {
  $tool = Find-PodSignTool
  Reject { Read-PodSigningConfig '' } 'SigningConfig is required' 'missing signing configuration'
  $config = Join-Path $testRoot 'config with spaces.json'
  @{ Provider = 'Unknown' } | ConvertTo-Json | Set-Content $config -Encoding UTF8
  Reject { Read-PodSigningConfig $config } 'Provider must be' 'unknown provider'
  @{ Provider = 'CertificateStore'; Password = 'test-value' } | ConvertTo-Json | Set-Content $config -Encoding UTF8
  Reject { Read-PodSigningConfig $config } 'Unknown signing setting: Password' 'secret fields rejected'
  @{ Provider = 'CertificateStore'; TimestampUrl = 'file:///invalid' } | ConvertTo-Json | Set-Content $config -Encoding UTF8
  Reject { Read-PodSigningConfig $config } 'TimestampUrl' 'non-HTTP timestamp endpoint rejected'
  $settings = @{ Provider = 'CertificateStore'; CertificateThumbprint = 'placeholder'; CertificateStoreLocation = 'CurrentUser' }
  Reject { Assert-PodSigningReady $settings } '40-digit' 'placeholder thumbprint rejected'
  $settings.CertificateThumbprint = '0000000000000000000000000000000000000000'
  Reject { Assert-PodSigningReady $settings } 'certificate/private key is unavailable' 'missing certificate diagnosed'
  Reject { Assert-PodSigningReady @{ Provider = 'ArtifactSigning' } } 'DlibPath.*MetadataPath' 'missing Artifact Signing inputs'
  $settings = @{ Provider = 'ArtifactSigning'; SignTool = $tool; DlibPath = 'fake-dlib.dll'; MetadataPath = 'metadata.json' }
  New-Item -ItemType File -Path (Join-Path $testRoot 'fake-dlib.dll') | Out-Null
  @{ Endpoint = 'https://<REGION>.codesigning.azure.net' } | ConvertTo-Json | Set-Content (Join-Path $testRoot 'metadata.json') -Encoding UTF8
  $settings | ConvertTo-Json | Set-Content $config -Encoding UTF8
  $resolved = Read-PodSigningConfig $config
  Assert ($resolved.DlibPath -eq (Join-Path $testRoot 'fake-dlib.dll')) 'configuration-relative paths resolved, including spaces'
  Assert ($resolved.TimestampUrl -eq 'http://timestamp.acs.microsoft.com') 'Artifact Signing timestamp default'
  Reject { Assert-PodSigningReady $resolved } 'real Endpoint' 'placeholder Artifact Signing metadata rejected'
  Reject { Test-PodPayload $testRoot $tool } 'Missing payload: pod-app.exe' 'incomplete payload rejected'
  $command = New-PodInnoSignCommand 'C:\Program Files\powershell.exe' 'C:\repo $q\sign.ps1' 'C:\config file.json' 'C:\verified copies'
  Assert ($command.Contains('$qC:\Program Files\powershell.exe$q') -and $command.Contains('$qC:\repo $$q\sign.ps1$q') -and $command.EndsWith('-Path $f')) 'Inno callback quotes spaces and escapes literal dollar sequences'

  # Test against the actual pinned, timestamped Microsoft component. These are
  # signature checks, not substitutes for compiling or signing Pod.
  $original = Join-Path $root 'vendor\conpty\conpty.dll'
  if (-not (Test-Path $original)) { throw 'Run tools/fetch-conpty.ps1 before this test.' }
  $record = Test-PodSignature $original $tool
  Assert ($record.Publisher -match 'CN=Microsoft Corporation') 'real Microsoft signature and timestamp verified'
  $tampered = Join-Path $testRoot 'tampered.dll'
  Copy-Item $original $tampered
  $stream = [IO.File]::Open($tampered, 'Open', 'ReadWrite')
  try {
    $stream.Position = 1024
    $value = $stream.ReadByte()
    $stream.Position = 1024
    $stream.WriteByte($value -bxor 1)
  } finally { $stream.Dispose() }
  Reject { Test-PodSignature $tampered $tool } 'verification failed' 'tampered binary rejected'
  $unsigned = Join-Path $testRoot 'unsigned.exe'
  [IO.File]::WriteAllText($unsigned, 'not an executable')
  Reject { Test-PodSignature $unsigned $tool } 'verification failed' 'unsigned file rejected'

  # Exercise the OSS handoff with signed upstream PE fixtures, not built Pod.
  # Output stays under the test directory and is removed in finally.
  $payloadFixture = Join-Path $testRoot 'payload-fixture'
  New-Item -ItemType Directory -Path (Join-Path $payloadFixture 'x64') -Force | Out-Null
  Copy-Item $original (Join-Path $payloadFixture 'conpty.dll')
  Copy-Item (Join-Path $root 'vendor\conpty\x64\OpenConsole.exe') (Join-Path $payloadFixture 'x64\OpenConsole.exe')
  foreach ($name in 'pod-app.exe','podd.exe','pod-holder.exe','pod.exe','pod-hook.exe') { Copy-Item $original (Join-Path $payloadFixture $name) }
  $prepared = Join-Path $testRoot 'prepared-input'
  function Invoke-PrepareFixture([string[]]$ExtraArguments) {
    $ErrorActionPreference = 'Continue'
    & powershell -NoProfile -File (Join-Path $PSScriptRoot 'prepare-signpath.ps1') -Stage $payloadFixture @ExtraArguments *> (Join-Path $testRoot 'prepare.log')
    return $LASTEXITCODE
  }
  $code = Invoke-PrepareFixture @('-OutDirectory', $prepared)
  Assert ($code -eq 0) 'unsigned OSS handoff requires no signing credentials'
  Assert (@(Get-ChildItem -LiteralPath $prepared -Filter '*.exe').Count -eq 5 -and -not (Test-Path (Join-Path $prepared 'conpty.dll')) -and -not (Test-Path (Join-Path $prepared 'OpenConsole.exe'))) 'handoff includes exactly five named project slots, excludes upstream'
  $readiness = Get-Content -LiteralPath (Join-Path $prepared 'readiness.json') -Raw | ConvertFrom-Json
  Assert (-not $readiness.MetadataReady -and $readiness.Status -like 'UNSIGNED INPUT ONLY*' -and $readiness.UpstreamVerifiedAndExcludedFromSigning.Count -eq 2) 'non-Pod fixture metadata is flagged and upstream verification recorded'
  $code = Invoke-PrepareFixture @('-OutDirectory', (Join-Path $testRoot 'strict-input'), '-RequireMetadata')
  Assert ($code -ne 0 -and -not (Test-Path (Join-Path $testRoot 'strict-input'))) 'actual submission gate rejects wrong product metadata before copying'
  $code = Invoke-PrepareFixture @('-OutDirectory', $prepared)
  Assert ($code -ne 0) 'handoff rejects mixed or stale output'

  # Exercise failures which cannot be created with a real signing identity here.
  # Preserve the actual helper and restore it after command construction tests.
  $verifyHelper = ${function:Test-PodSignature}
  function Test-PodSignature { param($File, $SignTool) [pscustomobject]@{ Path = $File } }
  function Test-NativeSignTool {
    $script:signArguments = @($args)
    $global:LASTEXITCODE = $script:fakeExit
  }
  $script:fakeExit = 0
  $settings = @{
    Provider = 'CertificateStore'; SignTool = 'Test-NativeSignTool'
    CertificateThumbprint = '1111111111111111111111111111111111111111'
    CertificateStoreLocation = 'LocalMachine'; TimestampUrl = 'https://timestamp.example.test'
  }
  Invoke-PodSign $unsigned $settings | Out-Null
  Assert (($script:signArguments -join '|') -eq ('sign|/v|/fd|SHA256|/tr|https://timestamp.example.test|/td|SHA256|/s|My|/sha1|' + $settings.CertificateThumbprint + '|/sm|' + $unsigned)) 'certificate store signing arguments and RFC 3161 SHA256 timestamp'
  $settings.Provider = 'ArtifactSigning'
  $settings.DlibPath = 'C:\tools with spaces\Azure.CodeSigning.Dlib.dll'
  $settings.MetadataPath = 'C:\config with spaces\metadata.json'
  Invoke-PodSign $unsigned $settings | Out-Null
  Assert (($script:signArguments -contains $settings.DlibPath) -and ($script:signArguments -contains $settings.MetadataPath) -and ($script:signArguments -contains '/dlib') -and ($script:signArguments -contains '/dmdf') -and ($script:signArguments -notcontains '/sha1')) 'Artifact Signing library and metadata remain individual arguments'
  $script:fakeExit = 2
  Reject { Invoke-PodSign $unsigned $settings } 'Signing failed \(2\)' 'signing/timestamp warning is fatal'
  Set-Item Function:\Test-PodSignature $verifyHelper

  # Simulate a tool that exits successfully but leaves invalid signature state.
  $script:fakeExit = 0
  function Get-AuthenticodeSignature { param($LiteralPath) $script:fakeSignature }
  $script:fakeSignature = [pscustomobject]@{
    Status = 'Valid'; SignatureType = 'Authenticode'
    SignerCertificate = [pscustomobject]@{ Subject = 'CN=Publisher'; Issuer = 'CN=CA' }
    TimeStamperCertificate = $null
  }
  Reject { Test-PodSignature $unsigned 'Test-NativeSignTool' } 'timestamped embedded signature required' 'missing timestamp rejected despite tool success'
  $script:fakeSignature.TimeStamperCertificate = [pscustomobject]@{ Subject = 'CN=TSA' }
  $script:fakeSignature.SignerCertificate.Issuer = 'CN=Publisher'
  Reject { Test-PodSignature $unsigned 'Test-NativeSignTool' } 'Self-signed publisher rejected' 'self-signed publisher rejected despite local trust'
  $script:fakeSignature.SignerCertificate.Issuer = 'CN=CA'
  $script:fakeSignature.SignatureType = 'Catalog'
  Reject { Test-PodSignature $unsigned 'Test-NativeSignTool' } 'Embedded Authenticode signature required' 'catalog-only trust rejected'

  if ($Iscc) {
    # Compile the actual installer script with output disabled: validate both
    # branches without producing a fake Pod installer or invoking a signer.
    $fixture = Join-Path $testRoot 'inno-fixture'
    New-Item -ItemType Directory -Path (Join-Path $fixture 'x64'),(Join-Path $fixture 'licenses') -Force | Out-Null
    foreach ($name in 'fixture.exe','fixture.dll','fixture.txt','x64\fixture.txt','licenses\fixture.txt') {
      Set-Content -LiteralPath (Join-Path $fixture $name) -Value 'compiler fixture only'
    }
    $compileArgs = @('/O-', '/Q', '/DAppVersion=0.1.0-dev.validation', '/DFileVersion=0.1.0', "/DStageDir=$fixture", "/DOutputDir=$testRoot")
    & $Iscc @compileArgs (Join-Path $PSScriptRoot 'pod.iss')
    Assert ($LASTEXITCODE -eq 0) 'Inno unsigned branch compiles with output disabled'
    $signedArgs = @('/DPodSigned', "/DSignedOutputDir=$testRoot", '/SPodReleaseSign=unused $f')
    & $Iscc @compileArgs @signedArgs (Join-Path $PSScriptRoot 'pod.iss')
    Assert ($LASTEXITCODE -eq 0) 'Inno signed branch compiles with output disabled'
    Assert (@(Get-ChildItem -LiteralPath $testRoot -Filter 'Pod-Setup-*.exe').Count -eq 0) 'compiler validation produced no installer'
  }
  Write-Host "$script:passed signing checks passed. No files signed."
} finally {
  # Only this invocation's known, absolute test directory may be removed.
  $expectedRoot = [IO.Path]::GetFullPath((Join-Path $root 'dist')).TrimEnd('\') + '\'
  $resolvedTestRoot = [IO.Path]::GetFullPath($testRoot)
  if (-not $resolvedTestRoot.StartsWith($expectedRoot, [StringComparison]::OrdinalIgnoreCase)) { throw "Unsafe test cleanup: $resolvedTestRoot" }
  Remove-Item -LiteralPath $resolvedTestRoot -Recurse -Force
}

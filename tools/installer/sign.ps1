# Authenticode helpers and the Inno Setup signing callback. No keys/passwords in JSON.
param(
  [ValidateSet('Preflight', 'Sign', 'Verify', 'Payload', 'Installed')][string]$Action = 'Preflight',
  [string]$ConfigPath,
  [string]$Path,
  [string]$EvidenceDirectory
)
$ErrorActionPreference = 'Stop'

function Find-PodSignTool {
  $sdk = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
  $tool = Get-ChildItem -LiteralPath $sdk -Directory -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -match '^10\.0\.\d+\.\d+$' } |
    Sort-Object { [version]$_.Name } -Descending |
    ForEach-Object { Join-Path $_.FullName 'x64\signtool.exe' } |
    Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
  if (-not $tool) { throw 'x64 Windows SDK SignTool not found. Install Windows SDK or set SignTool in signing JSON.' }
  return $tool
}

function Read-PodSigningConfig([string]$ConfigPath) {
  if (-not $ConfigPath) { throw 'SigningConfig is required: supply a publicly trusted CertificateStore or ArtifactSigning configuration (docs/install/signing.md).' }
  $full = (Resolve-Path -LiteralPath $ConfigPath).Path
  $raw = Get-Content -LiteralPath $full -Raw -Encoding UTF8 | ConvertFrom-Json
  $settings = @{
    Provider = ''; SignTool = ''; CertificateThumbprint = ''
    CertificateStoreLocation = 'CurrentUser'; TimestampUrl = ''
    DlibPath = ''; MetadataPath = ''
  }
  foreach ($property in $raw.PSObject.Properties) {
    if (-not $settings.ContainsKey($property.Name)) { throw "Unknown signing setting: $($property.Name). Do not put keys/passwords in this file." }
    $settings[$property.Name] = [string]$property.Value
  }
  if ($settings.Provider -notin @('CertificateStore', 'ArtifactSigning')) { throw 'Provider must be CertificateStore or ArtifactSigning.' }
  foreach ($key in 'SignTool', 'DlibPath', 'MetadataPath') {
    if ($settings[$key]) {
      $p = $settings[$key]
      if (-not [IO.Path]::IsPathRooted($p)) { $p = Join-Path (Split-Path $full) $p }
      $settings[$key] = (Resolve-Path -LiteralPath $p).Path
      if (-not (Test-Path -LiteralPath $settings[$key] -PathType Leaf)) { throw "$key must name a file." }
    }
  }
  if (-not $settings.SignTool) { $settings.SignTool = Find-PodSignTool }
  if ($settings.Provider -eq 'ArtifactSigning' -and -not $settings.TimestampUrl) {
    $settings.TimestampUrl = 'http://timestamp.acs.microsoft.com'
  }
  $uri = $null
  if (-not [uri]::TryCreate($settings.TimestampUrl, [UriKind]::Absolute, [ref]$uri) -or $uri.Scheme -notin @('http', 'https')) {
    throw 'TimestampUrl must be an RFC 3161 HTTP(S) endpoint from your signing provider.'
  }
  return $settings
}

function Assert-PodSigningReady($Settings) {
  if ($Settings.Provider -eq 'CertificateStore') {
    if ($Settings.CertificateThumbprint -notmatch '^[0-9A-Fa-f]{40}$') { throw 'CertificateThumbprint must be the exact 40-digit certificate thumbprint.' }
    if ($Settings.CertificateStoreLocation -notin @('CurrentUser', 'LocalMachine')) { throw 'CertificateStoreLocation must be CurrentUser or LocalMachine.' }
    $cert = Get-Item -LiteralPath "Cert:\$($Settings.CertificateStoreLocation)\My\$($Settings.CertificateThumbprint)" -ErrorAction SilentlyContinue
    if (-not $cert -or -not $cert.HasPrivateKey) { throw 'Signing certificate/private key is unavailable in the selected Personal (My) store. Configure the CA token/HSM provider on the build host.' }
    if ($cert.Subject -eq $cert.Issuer) { throw 'Self-signed certificates are not accepted for Pod distribution.' }
    if ($cert.NotBefore -gt (Get-Date) -or $cert.NotAfter -le (Get-Date)) { throw 'Signing certificate is not currently valid.' }
    if (@($cert.EnhancedKeyUsageList | Where-Object { [string]$_.ObjectId -eq '1.3.6.1.5.5.7.3.3' }).Count -eq 0) { throw 'Certificate lacks Code Signing EKU.' }
    $chain = New-Object System.Security.Cryptography.X509Certificates.X509Chain
    try {
      $chain.ChainPolicy.RevocationMode = 'Online'
      if (-not $chain.Build($cert)) { throw "Signing certificate chain/revocation validation failed: $($chain.ChainStatus.Status -join ', ')" }
    } finally { $chain.Dispose() }
  } else {
    if (-not $Settings.DlibPath -or -not $Settings.MetadataPath) { throw 'ArtifactSigning requires DlibPath (x64 Azure.CodeSigning.Dlib.dll) and MetadataPath.' }
    $metadata = Get-Content -LiteralPath $Settings.MetadataPath -Raw -Encoding UTF8 | ConvertFrom-Json
    foreach ($key in 'Endpoint', 'CodeSigningAccountName', 'CertificateProfileName') {
      if (-not $metadata.$key -or $metadata.$key -match '[<>]') { throw "Artifact Signing metadata requires a real $key value." }
    }
    $endpoint = $null
    if (-not [uri]::TryCreate($metadata.Endpoint, [UriKind]::Absolute, [ref]$endpoint) -or $endpoint.Scheme -ne 'https') { throw 'Artifact Signing Endpoint must be HTTPS.' }
    # The required runtime depends on the pinned client version. Inspect the
    # shipped runtimeconfig rather than assuming the version in web examples.
    $runtimeConfigPath = Join-Path (Split-Path $Settings.DlibPath) 'Azure.CodeSigning.Dlib.runtimeconfig.json'
    if (-not (Test-Path -LiteralPath $runtimeConfigPath -PathType Leaf)) { throw 'Artifact Signing runtimeconfig is missing. Preserve the complete client bin/x64 directory.' }
    $runtimeConfig = Get-Content -LiteralPath $runtimeConfigPath -Raw -Encoding UTF8 | ConvertFrom-Json
    $runtimeVersion = [version]$runtimeConfig.runtimeOptions.framework.version
    if (-not $runtimeVersion) { throw 'Cannot determine Artifact Signing runtime requirement.' }
    if (-not (Get-Command dotnet -ErrorAction SilentlyContinue)) { throw "Artifact Signing requires the x64 .NET $($runtimeVersion.Major) runtime." }
    $runtimes = & dotnet --list-runtimes
    if ($LASTEXITCODE -ne 0 -or -not ($runtimes -match "^Microsoft.NETCore.App $($runtimeVersion.Major)\.")) { throw "Artifact Signing requires the x64 .NET $($runtimeVersion.Major) runtime from $runtimeConfigPath." }
    Write-Host 'Artifact Signing inputs ready. Public Trust profile, account region, Certificate Profile Signer role and Azure authentication must be provisioned on the build host; authorization is checked by the signing service.'
  }
}

function Test-PodSignature([string]$File, [string]$SignTool) {
  if (-not (Test-Path -LiteralPath $File -PathType Leaf)) { throw "Missing signed file: $File" }
  # /pa uses Authenticode policy, /all checks every embedded signature, /tw warns
  # on missing timestamps. Treat warning (exit 2) as failure as well as errors.
  & $SignTool verify /pa /all /v /tw $File | Out-Host
  if ($LASTEXITCODE -ne 0) { throw "Authenticode verification failed ($LASTEXITCODE): $File" }
  $signature = Get-AuthenticodeSignature -LiteralPath $File
  if ($signature.Status -ne 'Valid' -or -not $signature.SignerCertificate -or -not $signature.TimeStamperCertificate) { throw "Valid timestamped embedded signature required: $File" }
  if ($signature.SignatureType -ne 'Authenticode') { throw "Embedded Authenticode signature required: $File" }
  if ($signature.SignerCertificate.Subject -eq $signature.SignerCertificate.Issuer) { throw "Self-signed publisher rejected: $File" }
  [pscustomobject]@{
    Path = [IO.Path]::GetFullPath($File)
    SHA256 = (Get-FileHash -LiteralPath $File -Algorithm SHA256).Hash.ToLowerInvariant()
    Publisher = $signature.SignerCertificate.Subject
    Thumbprint = $signature.SignerCertificate.Thumbprint
    TimestampAuthority = $signature.TimeStamperCertificate.Subject
  }
}

function Invoke-PodSign([string]$File, $Settings) {
  $arguments = @('sign', '/v', '/fd', 'SHA256', '/tr', $Settings.TimestampUrl, '/td', 'SHA256')
  if ($Settings.Provider -eq 'CertificateStore') {
    $arguments += @('/s', 'My', '/sha1', $Settings.CertificateThumbprint)
    if ($Settings.CertificateStoreLocation -eq 'LocalMachine') { $arguments += '/sm' }
  } else {
    $arguments += @('/dlib', $Settings.DlibPath, '/dmdf', $Settings.MetadataPath)
  }
  & $Settings.SignTool @arguments $File | Out-Host
  if ($LASTEXITCODE -ne 0) { throw "Signing failed ($LASTEXITCODE): $File" }
  Test-PodSignature $File $Settings.SignTool
}

function Test-PodPayload([string]$Directory, [string]$SignTool, [switch]$Installed) {
  foreach ($name in 'pod-app.exe', 'podd.exe', 'pod-holder.exe', 'pod.exe', 'pod-hook.exe', 'conpty.dll', 'x64\OpenConsole.exe') {
    if (-not (Test-Path -LiteralPath (Join-Path $Directory $name) -PathType Leaf)) { throw "Missing payload: $name" }
  }
  if ($Installed -and -not (Get-ChildItem -LiteralPath $Directory -Filter 'unins*.exe' -File)) { throw 'Installed uninstaller is missing.' }
  # Includes any future bundled executable components, not just today's list.
  foreach ($file in Get-ChildItem -LiteralPath $Directory -Recurse -File | Where-Object { $_.Extension -in @('.exe', '.dll') }) {
    $record = Test-PodSignature $file.FullName $SignTool
    if ($file.Name -in @('conpty.dll', 'OpenConsole.exe') -and $record.Publisher -notmatch '(^|,\s*)CN=Microsoft Corporation(,|$)') {
      throw "Expected original Microsoft signature on $($file.FullName)"
    }
    $record
  }
}

function New-PodInnoSignCommand([string]$PowerShell, [string]$Script, [string]$Config, [string]$Evidence) {
  # Inno expands $q to quotes and $f to its quoted target filename. Escape
  # literal dollars in paths so they cannot be interpreted as Inno sequences.
  return ('$q{0}$q -NoProfile -NonInteractive -File $q{1}$q -Action Sign -ConfigPath $q{2}$q -EvidenceDirectory $q{3}$q -Path $f' -f $PowerShell.Replace('$', '$$'), $Script.Replace('$', '$$'), $Config.Replace('$', '$$'), $Evidence.Replace('$', '$$'))
}

if ($MyInvocation.InvocationName -ne '.') {
  if ($Action -in @('Preflight', 'Sign')) {
    $settings = Read-PodSigningConfig $ConfigPath
    Assert-PodSigningReady $settings
    if ($Action -eq 'Sign') {
      if (-not $Path) { throw '-Path is required.' }
      $record = Invoke-PodSign $Path $settings
      if ($EvidenceDirectory) {
        # Inno 6.7.3 deletes uninst*.tmp after embedding the signed bytes.
        # Retain verified copies for the final packaging check/report.
        if (-not (Test-Path -LiteralPath $EvidenceDirectory -PathType Container)) { throw 'Signing evidence directory does not exist.' }
        $copy = Join-Path $EvidenceDirectory ((Split-Path -Leaf $Path) + '.signed.exe')
        Copy-Item -LiteralPath $Path -Destination $copy -Force
        if ((Get-FileHash -LiteralPath $copy -Algorithm SHA256).Hash.ToLowerInvariant() -ne $record.SHA256) { throw 'Signing evidence copy hash mismatch.' }
      }
      $record | ConvertTo-Json
    } else { Write-Output 'Signing preflight passed (no files signed).' }
  } else {
    if (-not $Path) { throw '-Path is required.' }
    $tool = if ($ConfigPath) { (Read-PodSigningConfig $ConfigPath).SignTool } else { Find-PodSignTool }
    if ($Action -eq 'Verify') { Test-PodSignature $Path $tool | ConvertTo-Json }
    else { @(Test-PodPayload $Path $tool -Installed:($Action -eq 'Installed')) | ConvertTo-Json }
  }
}

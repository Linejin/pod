# SPDX-License-Identifier: Apache-2.0
param([string]$Root = (Join-Path $PSScriptRoot '..'), [switch]$SelfTest)
$ErrorActionPreference = 'Stop'
function Test-Assets([string]$Dir) {
  $manifest = Get-Content -LiteralPath (Join-Path $Dir 'assets.json') -Raw -Encoding UTF8 | ConvertFrom-Json
  $expected = @('.gitattributes', 'app.js', 'contract.js', 'index.html', 'style.css')
  $entries = @(Get-ChildItem -LiteralPath $Dir -Recurse -Force)
  if (@($entries | Where-Object { $_.Attributes -band [IO.FileAttributes]::ReparsePoint }).Count) { throw 'Reparse asset denied' }
  $actual = @($entries | Where-Object { -not $_.PSIsContainer } | ForEach-Object { $_.FullName.Substring($Dir.TrimEnd('\', '/').Length + 1).Replace('\', '/') } | Sort-Object)
  if (@(Compare-Object (@($expected) + 'assets.json' | Sort-Object) $actual).Count) { throw 'Extra or missing bundled file' }
  $names = @($manifest.PSObject.Properties.Name | Sort-Object)
  if (@(Compare-Object $expected $names).Count) { throw 'Manifest file set differs' }
  foreach ($name in $expected) {
    $path = Join-Path $Dir $name
    $hash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($manifest.$name -cne $hash) { throw "SHA-256 mismatch: $name" }
    $s = [IO.File]::ReadAllText($path)
    if ($s -match '(?i)https?\s*:' -or $s -match '(?i)innerHTML|outerHTML|insertAdjacentHTML|document\.write|\beval\s*\(') { throw "Forbidden URL or HTML sink: $name" }
    if ($s -match '(?is)<style\b|<[^>]+\s(?:style|on\w+)\s*=|<script\b(?![^>]*\bsrc\s*=)[^>]*>') { throw "Inline content: $name" }
  }
}
if ($SelfTest) {
  $tmp = Join-Path ([IO.Path]::GetTempPath()) ('pod-assets-' + [Guid]::NewGuid().ToString('N'))
  [IO.Directory]::CreateDirectory($tmp) | Out-Null
  try {
    $source = Join-Path $Root 'mobile/web'
    Copy-Item -Path (Join-Path $source '*') -Destination $tmp
    Test-Assets $tmp
    $app = Join-Path $tmp 'app.js'; $original = [IO.File]::ReadAllBytes($app)
    [IO.File]::AppendAllText($app, ' ')
    try { Test-Assets $tmp; throw 'SelfTest missed tamper' } catch { if ($_.Exception.Message -notmatch 'SHA-256 mismatch') { throw } }
    [IO.File]::WriteAllBytes($app, $original)
    [IO.File]::WriteAllText((Join-Path $tmp 'extra.js'), '')
    try { Test-Assets $tmp; throw 'SelfTest missed extra' } catch { if ($_.Exception.Message -notmatch 'Extra or missing') { throw } }
    Remove-Item -LiteralPath (Join-Path $tmp 'extra.js')
    Remove-Item -LiteralPath $app
    try { Test-Assets $tmp; throw 'SelfTest missed missing' } catch { if ($_.Exception.Message -notmatch 'Extra or missing') { throw } }
    [IO.File]::WriteAllBytes($app, $original)
    foreach ($bad in @('https://invalid.example', '<script>alert(1)</script>', '<style>body{}</style>', '<p style="color:red">', 'x.innerHTML = y')) {
      [IO.File]::WriteAllText($app, $bad)
      $m = Get-Content -LiteralPath (Join-Path $tmp 'assets.json') -Raw | ConvertFrom-Json
      $m.'app.js' = (Get-FileHash -LiteralPath $app -Algorithm SHA256).Hash.ToLowerInvariant()
      [IO.File]::WriteAllText((Join-Path $tmp 'assets.json'), ($m | ConvertTo-Json))
      try { Test-Assets $tmp; throw 'SelfTest missed content' } catch { if ($_.Exception.Message -notmatch 'Forbidden|Inline content') { throw } }
    }
    Write-Output 'mobile assets SelfTest PASS (tamper, extra, missing, URL, inline, HTML sink)'
  } finally {
    $resolved = [IO.Path]::GetFullPath($tmp)
    if (-not $resolved.StartsWith([IO.Path]::GetTempPath()) -or [IO.Path]::GetFileName($resolved) -notlike 'pod-assets-*') { throw 'Unsafe cleanup path' }
    Remove-Item -LiteralPath $resolved -Recurse -Force
  }
} else {
  Test-Assets ([IO.Path]::GetFullPath((Join-Path $Root 'mobile/web')))
  Write-Output 'mobile assets PASS'
}

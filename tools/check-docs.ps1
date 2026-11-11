# Docs check (reads only, changes nothing):
# - every numbered doc (docs/NN-*.md) must be linked from the README document
#   table (quality-08);
# - a doc header that states "기준: 프로토콜 vN" must match PROTOCOL_VERSION in
#   crates/pod-protocol/src/protocol.rs (quality-09);
# - relative links in nested docs (docs/<folder>/**/*.md, e.g. the design set in
#   docs/20-refactor) point at a file or folder that exists. Nested docs need no
#   README entry. URLs (scheme:), #anchors and links inside code are skipped;
#   anchors themselves are not checked.
#   powershell -ExecutionPolicy Bypass -File tools/check-docs.ps1
#   powershell -ExecutionPolicy Bypass -File tools/check-docs.ps1 -SelfTest
param([string]$Root = (Join-Path $PSScriptRoot ".."), [switch]$SelfTest)
$ErrorActionPreference = "Stop"

# Link targets of Markdown inline links and images, outside code blocks/spans.
function Get-LinkTargets([string]$text) {
  $text = [regex]::Replace($text, '(?ms)^[ \t]*(```|~~~)[^\r\n]*\r?\n.*?^[ \t]*\1[^\r\n]*$', '')
  $text = [regex]::Replace($text, '`[^`\r\n]+`', '')
  return @([regex]::Matches($text, '\[[^\]]*\]\(\s*(<[^>]+>|[^)\s]+)(?:\s+"[^"]*")?\s*\)') | ForEach-Object { $_.Groups[1].Value })
}

function Test-Docs([string]$root) {
  $docs = Join-Path $root "docs"
  $readme = Get-Content (Join-Path $root "README.md") -Raw -Encoding UTF8
  $missing = @(Get-ChildItem $docs -Filter "*.md" | Where-Object { $_.Name -match '^\d\d-' } |
    Where-Object { $readme -notmatch [regex]::Escape("(docs/$($_.Name))") } | ForEach-Object { $_.Name })
  $fail = @($missing | ForEach-Object { "README.md does not link docs/$_" })
  $proto = [regex]::Match((Get-Content (Join-Path $root "crates\pod-protocol\src\protocol.rs") -Raw), 'PROTOCOL_VERSION: u32 = (\d+)').Groups[1].Value
  if (-not $proto) { $fail += "PROTOCOL_VERSION not found in crates/pod-protocol/src/protocol.rs" }
  Get-ChildItem $docs -Filter "*.md" | ForEach-Object {
    foreach ($m in [regex]::Matches((Get-Content $_.FullName -Raw -Encoding UTF8), '기준: 프로토콜 v(\d+)')) {
      if ($m.Groups[1].Value -ne $proto) { $fail += "docs/$($_.Name) says protocol v$($m.Groups[1].Value), code is v$proto" }
    }
  }
  $docsDir = (Get-Item $docs).FullName.TrimEnd('\')
  $rootDir = (Get-Item $root).FullName.TrimEnd('\')
  $links = 0
  $nested = Get-ChildItem $docs -Recurse -File -Filter "*.md" | Where-Object { $_.DirectoryName.TrimEnd('\') -ne $docsDir } | Sort-Object FullName
  foreach ($f in $nested) {
    $rel = $f.FullName.Substring($rootDir.Length + 1) -replace '\\', '/'
    foreach ($target in Get-LinkTargets (Get-Content $f.FullName -Raw -Encoding UTF8)) {
      $t = $target.Trim('<', '>')
      if ($t.StartsWith('#') -or $t -match '^[A-Za-z][A-Za-z0-9+.-]*:') { continue }
      $p = [uri]::UnescapeDataString(($t -split '[#?]')[0])
      if (-not $p) { continue }
      $links++
      $full = if ($p.StartsWith('/')) { Join-Path $rootDir $p.TrimStart('/') } else { Join-Path $f.DirectoryName $p }
      if (-not (Test-Path -LiteralPath $full)) { $fail += "$rel links $target, which does not exist" }
    }
  }
  return @{ Fail = $fail; Links = $links }
}

if ($SelfTest) {
  $tmp = Join-Path ([IO.Path]::GetTempPath()) ("pod-check-docs-" + [Guid]::NewGuid().ToString("N"))
  $fence = '```'
  $files = [ordered]@{
    "README.md" = "| [a](docs/01-a.md) | [c](docs/03-c.md) |"
    "crates/pod-protocol/src/protocol.rs" = "pub const PROTOCOL_VERSION: u32 = 7;"
    "docs/01-a.md" = "기준: 프로토콜 v7`n[top-level links are not checked](nope.md)"
    "docs/02-b.md" = "not in the README"
    "docs/03-c.md" = "기준: 프로토콜 v6"
    "docs/20-x/README.md" = @(
      "Nested, not in the README. [ok](b.md) [up](../01-a.md#top) [dir](sub/) [space](my%20doc.md)",
      "[angle](<my doc.md>) [root](/README.md) [img](pic.png `"title`") ![img](pic.png)",
      "[web](https://example.com/x) [mail](mailto:x@example.com) [anchor](#sec)",
      "Code is skipped: ``[inline](gone-inline.md)``",
      "$fence",
      "[fenced](gone-fence.md)",
      "$fence",
      "[broken](missing.md) and [broken2](../99-none.md#x)"
    ) -join "`n"
    "docs/20-x/b.md" = "b"
    "docs/20-x/my doc.md" = "space"
    "docs/20-x/pic.png" = "png"
    "docs/20-x/sub/deep.md" = "기준: 프로토콜 v1 (nested docs keep their own version notes)`n[back](../b.md) [bad](../../nope/)"
  }
  $expect = @(
    "README.md does not link docs/02-b.md",
    "docs/03-c.md says protocol v6, code is v7",
    "docs/20-x/README.md links missing.md, which does not exist",
    "docs/20-x/README.md links ../99-none.md#x, which does not exist",
    "docs/20-x/sub/deep.md links ../../nope/, which does not exist"
  )
  try {
    foreach ($k in $files.Keys) {
      $path = Join-Path $tmp $k
      [void](New-Item -ItemType Directory -Force (Split-Path $path))
      Set-Content -LiteralPath $path $files[$k] -Encoding UTF8
    }
    $r = Test-Docs $tmp
  } finally { Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue }
  $got = @($r.Fail | Sort-Object) -join " | "
  $want = @($expect | Sort-Object) -join " | "
  if ($got -cne $want -or $r.Links -ne 12) {
    Write-Output "SELFTEST FAIL: got [$got] ($($r.Links) links), want [$want] (12 links)"; exit 1
  }
  Write-Output "check-docs selftest ok"
  exit 0
}

$r = Test-Docs (Resolve-Path $Root).Path
if ($r.Fail.Count -gt 0) { $r.Fail | ForEach-Object { Write-Output "FAIL: $_" }; exit 1 }
Write-Output "docs index ok ($($r.Links) relative links in nested docs)"

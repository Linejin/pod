# Docs check (reads only, changes nothing):
# - every numbered doc (docs/NN-*.md) must be linked from the README document
#   table (quality-08);
# - a doc header that states "기준: 프로토콜 vN" must match PROTOCOL_VERSION in
#   crates/pod-protocol/src/protocol.rs (quality-09);
# - relative links in root Markdown and docs/**/*.md point at existing paths;
# - Markdown section links resolve to a heading or explicit HTML anchor.
#   External URLs, comments and fenced examples are skipped.
#   powershell -ExecutionPolicy Bypass -File tools/check-docs.ps1
#   powershell -ExecutionPolicy Bypass -File tools/check-docs.ps1 -SelfTest
param([string]$Root = (Join-Path $PSScriptRoot ".."), [switch]$SelfTest)
$ErrorActionPreference = "Stop"

function Remove-HiddenMarkdown([string]$text) {
  return [regex]::Replace($text, '(?ms)<!--.*?(?:-->|\z)|^ {0,3}(```|~~~)[^\r\n]*\r?\n.*?(?:^ {0,3}\1[ \t]*\r?$|\z)', '')
}

# Link targets of Markdown inline links and images, outside code blocks/spans.
function Get-LinkTargets([string]$text) {
  $text = Remove-HiddenMarkdown $text
  $text = [regex]::Replace($text, '`[^`\r\n]+`', '')
  return @([regex]::Matches($text, '\[[^\]]*\]\(\s*(<[^>]+>|[^)\s]+)(?:\s+"[^"]*")?\s*\)') | ForEach-Object { $_.Groups[1].Value })
}

function Get-SectionIds([string]$text) {
  $text = Remove-HiddenMarkdown $text
  $ids = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
  foreach ($heading in [regex]::Matches($text, '(?m)^ {0,3}#{1,6}[ \t]+([^\r\n]+)')) {
    $title = $heading.Groups[1].Value -replace '[ \t]+#+[ \t]*$', ''
    $title = $title -replace '!?\[([^\]]*)\]\([^)]*\)', '$1' -replace '<[^>]+>', ''
    $slug = $title.Trim().ToLowerInvariant() -replace '[^\p{L}\p{M}\p{N}_ \t-]', '' -replace '[ \t]', '-'
    $candidate = $slug
    $suffix = 1
    while (-not $ids.Add($candidate)) {
      $candidate = "$slug-$suffix"
      $suffix++
    }
  }
  foreach ($anchor in [regex]::Matches($text, '(?i)<a\s+[^>]*\b(?:name|id)=["'']([^"'']+)["'']')) {
    [void]$ids.Add($anchor.Groups[1].Value)
  }
  return ,$ids
}

function Test-Docs([string]$root) {
  $docs = Join-Path $root "docs"
  $readme = Get-Content -LiteralPath (Join-Path $root "README.md") -Raw -Encoding UTF8
  $missing = @(Get-ChildItem $docs -Filter "*.md" | Where-Object { $_.Name -match '^\d\d-' } |
    Where-Object { $readme -notmatch [regex]::Escape("(docs/$($_.Name))") } | ForEach-Object { $_.Name })
  $fail = @($missing | ForEach-Object { "README.md does not link docs/$_" })
  $proto = [regex]::Match((Get-Content -LiteralPath (Join-Path $root "crates\pod-protocol\src\protocol.rs") -Raw), 'PROTOCOL_VERSION: u32 = (\d+)').Groups[1].Value
  if (-not $proto) { $fail += "PROTOCOL_VERSION not found in crates/pod-protocol/src/protocol.rs" }
  Get-ChildItem -LiteralPath $docs -File -Filter "*.md" | ForEach-Object {
    foreach ($m in [regex]::Matches((Get-Content -LiteralPath $_.FullName -Raw -Encoding UTF8), '기준: 프로토콜 v(\d+)')) {
      if ($m.Groups[1].Value -ne $proto) { $fail += "docs/$($_.Name) says protocol v$($m.Groups[1].Value), code is v$proto" }
    }
  }
  $rootDir = (Get-Item -LiteralPath $root).FullName.TrimEnd([IO.Path]::DirectorySeparatorChar)
  $links = 0
  $sections = 0
  $sectionIds = @{}
  $markdown = @(Get-ChildItem -LiteralPath $root -File -Filter "*.md") +
              @(Get-ChildItem -LiteralPath $docs -Recurse -File -Filter "*.md")
  foreach ($f in ($markdown | Sort-Object FullName)) {
    $rel = $f.FullName.Substring($rootDir.Length + 1) -replace '\\', '/'
    foreach ($target in Get-LinkTargets (Get-Content -LiteralPath $f.FullName -Raw -Encoding UTF8)) {
      $t = $target.Trim('<', '>')
      if ($t.StartsWith('//') -or $t -match '^[A-Za-z][A-Za-z0-9+.-]*:') { continue }
      $parts = $t -split '#', 2
      $p = [uri]::UnescapeDataString(($parts[0] -split '\?', 2)[0])
      $full = $f.FullName
      if ($p) {
        $links++
        $full = if ($p.StartsWith('/')) { Join-Path $rootDir $p.TrimStart('/') } else { Join-Path $f.DirectoryName $p }
        if (-not (Test-Path -LiteralPath $full)) {
          $fail += "$rel links $target, which does not exist"
          continue
        }
      }
      if ($parts.Count -eq 2 -and $parts[1] -and [IO.Path]::GetExtension($full) -ieq '.md' -and
          (Test-Path -LiteralPath $full -PathType Leaf)) {
        $sections++
        $full = (Get-Item -LiteralPath $full).FullName
        if (-not $sectionIds.ContainsKey($full)) {
          $sectionIds[$full] = Get-SectionIds (Get-Content -LiteralPath $full -Raw -Encoding UTF8)
        }
        if (-not $sectionIds[$full].Contains([uri]::UnescapeDataString($parts[1]))) {
          $fail += "$rel links $target, which has no matching section"
        }
      }
    }
  }
  return @{ Fail = $fail; Links = $links; Sections = $sections }
}

if ($SelfTest) {
  $tmp = Join-Path ([IO.Path]::GetTempPath()) ("pod-check-docs-" + [Guid]::NewGuid().ToString("N"))
  $fence = '```'
  $files = [ordered]@{
    "README.md" = "| [a](docs/01-a.md) | [c](docs/03-c.md) | [missing](missing-root.md) |"
    "crates/pod-protocol/src/protocol.rs" = "pub const PROTOCOL_VERSION: u32 = 7;"
    "docs/01-a.md" = @(
      "기준: 프로토콜 v7", "# Top", "# Top", "## See [a link](https://example.com)",
      '<a name="custom-id"></a>',
      "[duplicate](#top-1) [inline link in heading](#see-a-link) [explicit anchor](#custom-id)",
      "[missing heading](#absent) [missing top-level link](nope.md)",
      "<!-- [ignored](comment.md) -->"
    ) -join "`n"
    "docs/02-b.md" = "not in the README"
    "docs/03-c.md" = "기준: 프로토콜 v6"
    "docs/20-x/README.md" = @(
      "Nested, not in the README. [ok](b.md) [up](../01-a.md#top) [dir](sub/) [space](my%20doc.md)",
      "[angle](<my doc.md>) [root](/README.md) [img](pic.png `"title`") ![img](pic.png)",
      "## Sec",
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
    "README.md links missing-root.md, which does not exist",
    "docs/01-a.md links #absent, which has no matching section",
    "docs/01-a.md links nope.md, which does not exist",
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
  if ($got -cne $want -or $r.Links -ne 16 -or $r.Sections -ne 6) {
    Write-Output "SELFTEST FAIL: got [$got] ($($r.Links) paths, $($r.Sections) sections), want [$want] (16 paths, 6 sections)"; exit 1
  }
  Write-Output "check-docs selftest ok"
  exit 0
}

$r = Test-Docs (Resolve-Path $Root).Path
if ($r.Fail.Count -gt 0) { $r.Fail | ForEach-Object { Write-Output "FAIL: $_" }; exit 1 }
Write-Output "docs index ok ($($r.Links) relative paths, $($r.Sections) Markdown sections)"

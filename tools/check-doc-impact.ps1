# Check the Docs field against the files in a pull request. The event body is data.
param(
  [string]$Root = (Join-Path $PSScriptRoot ".."),
  [string]$EventPath = $env:GITHUB_EVENT_PATH,
  [switch]$SelfTest
)
$ErrorActionPreference = "Stop"

$productPaths = '^(?:crates/[^/]+/(?:src/|(?:Cargo\.toml|build\.rs)$)|tools/installer/|(?:Cargo\.toml|Cargo\.lock|deny\.toml|\.github/workflows/windows-installer\.yml)$)'
$documentationPaths = '^(?:docs/.*\.md|docs/validation/.+/results\.json|(?:README|CHANGELOG|PRIVACY|SECURITY|CODE_SIGNING_POLICY)\.md|tools/installer/README\.txt)$'

function Get-ImpactProblem([string[]]$paths, [string]$body) {
  if (-not ($paths -cmatch $productPaths)) { return }

  $body = $body -replace '(?ms)<!--.*?(?:-->|\z)|^ {0,3}(```|~~~)[^\r\n]*\r?\n.*?(?:^ {0,3}\1[ \t]*\r?$|\z)', ''
  $fields = [regex]::Matches($body, '(?im)^ {0,3}(?:-[ \t]+)?Docs:[^\r\n]*')
  if ($fields.Count -ne 1) {
    return 'Include exactly one Docs field outside comments and code blocks. See CONTRIBUTING.md.'
  }
  $field = [regex]::Match($fields[0].Value, '(?i)Docs:[ \t]+(updated|none)[ \t]+--[ \t]+(\S.*)$')
  if (-not $field.Success) {
    return 'Use Docs: updated -- <changed paths> or Docs: none -- <reason>. See CONTRIBUTING.md.'
  }
  if ($field.Groups[1].Value -ieq 'updated') {
    $changedDocs = @($paths -cmatch $documentationPaths)
    foreach ($entry in ($field.Groups[2].Value -split ',')) {
      $path = $entry.Trim().Trim('`')
      if ($path -cnotin $changedDocs) {
        return 'List only changed documentation paths after Docs: updated --. Separate paths with commas.'
      }
    }
  }
}

if ($SelfTest) {
  $source = 'crates/pod-app/src/workspace.rs'
  $guide = 'docs/09-pod-app.md'
  $cases = @(
    @{ Name = 'docs only'; Paths = @($guide); Body = ''; Pass = $true },
    @{ Name = 'tests only'; Paths = @('crates/pod-cli/tests/contract.rs'); Body = ''; Pass = $true },
    @{ Name = 'missing field'; Paths = @($source, $guide); Body = ''; Pass = $false },
    @{ Name = 'template comment'; Paths = @($source, $guide); Body = '<!-- Docs: updated -- docs/09-pod-app.md -->'; Pass = $false },
    @{ Name = 'fenced example'; Paths = @($source); Body = "~~~text`nDocs: none -- Example only.`n~~~"; Pass = $false },
    @{ Name = 'comment text inside fence'; Paths = @($source); Body = "~~~text`n<!--`n~~~`nDocs: none -- Same output."; Pass = $true },
    @{ Name = 'duplicate field'; Paths = @($source); Body = "Docs: none -- One reason.`r`nDocs: none -- Another reason."; Pass = $false },
    @{ Name = 'changed docs'; Paths = @($source, $guide, 'CHANGELOG.md'); Body = '- Docs: updated -- `docs/09-pod-app.md`, CHANGELOG.md'; Pass = $true },
    @{ Name = 'unlisted doc change'; Paths = @($source, 'CHANGELOG.md'); Body = "Docs: updated -- $guide"; Pass = $false },
    @{ Name = 'mixed paths'; Paths = @($source, $guide); Body = "Docs: updated -- $guide, $source"; Pass = $false },
    @{ Name = 'refactor'; Paths = @($source); Body = "## Impact`r`nDocs: none -- Same UI, settings and output.`r`n"; Pass = $true },
    @{ Name = 'empty reason'; Paths = @('Cargo.lock'); Body = 'Docs: none --  '; Pass = $false },
    @{ Name = 'installer'; Paths = @('tools/installer/build.ps1', 'tools/installer/README.txt'); Body = 'Docs: updated -- tools/installer/README.txt'; Pass = $true }
  )
  foreach ($case in $cases) {
    $problem = Get-ImpactProblem $case.Paths $case.Body
    if ((-not $problem) -ne $case.Pass) { throw "Self-test '$($case.Name)' failed: $problem" }
  }
  Write-Output "check-doc-impact selftest ok ($($cases.Count) cases)"
  exit 0
}

if (-not $EventPath) { throw 'Set GITHUB_EVENT_PATH or supply -EventPath for a pull request event JSON file.' }
$pr = (Get-Content -LiteralPath $EventPath -Raw -Encoding UTF8 | ConvertFrom-Json).pull_request
$base = [string]$pr.base.sha
$head = [string]$pr.head.sha
if ($base -notmatch '^[0-9a-f]{40}$' -or $head -notmatch '^[0-9a-f]{40}$') {
  throw 'Pull request event is missing valid base and head commit IDs.'
}
$originalEncoding = [Console]::OutputEncoding
try {
  [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
  $paths = @(& git -C $Root -c core.quotepath=false diff --name-only --no-renames "$base...$head" --)
  if ($LASTEXITCODE -ne 0) { throw 'Unable to read pull request changes. Fetch the history for both commits.' }
} finally {
  [Console]::OutputEncoding = $originalEncoding
}
$problem = Get-ImpactProblem $paths ([string]$pr.body)
if ($problem) { Write-Output "FAIL: $problem"; exit 1 }
Write-Output 'Documentation impact ok'

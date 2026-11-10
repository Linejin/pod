# Workspace policy check (no cargo run needed):
# - every member except the spikes is a default member (quality-14), so plain
#   `cargo build/test/clippy` and CI cover every shipped crate;
# - every non-spike crate opts into the workspace lints (quality-10);
# - every crate inherits rust-version (quality-11).
#   powershell -ExecutionPolicy Bypass -File tools/check-workspace.ps1
$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$toml = Get-Content (Join-Path $root "Cargo.toml") -Raw
function List([string]$key) {
  $m = [regex]::Match($toml, "(?m)^$key\s*=\s*\[([^\]]*)\]")
  if (-not $m.Success) { return @() }
  return @([regex]::Matches($m.Groups[1].Value, '"([^"]+)"') | ForEach-Object { $_.Groups[1].Value })
}
$spikes = @("crates/pod-spike", "crates/diff-spike")
$members = List "members"
$default = List "default-members"
$fail = @()
if ($members.Count -eq 0) { $fail += "no [workspace] members found" }
if ($toml -notmatch '(?m)^rust-version\s*=\s*"\d+\.\d+') { $fail += "[workspace.package] has no rust-version" }
foreach ($m in $members) {
  $isSpike = $spikes -contains $m
  if ($isSpike -and $default -contains $m) { $fail += "$m is a spike but a default member" }
  if (-not $isSpike -and $default -notcontains $m) { $fail += "$m is missing from default-members" }
  $ct = Get-Content (Join-Path $root "$m\Cargo.toml") -Raw
  if ($ct -notmatch '(?m)^rust-version\.workspace\s*=\s*true') { $fail += "$m does not inherit rust-version" }
  if (-not $isSpike -and $ct -notmatch '(?m)^\[lints\]\s*\r?\n\s*workspace\s*=\s*true') { $fail += "$m does not opt into [lints] workspace = true" }
}
foreach ($d in $default) { if ($members -notcontains $d) { $fail += "default member $d is not a member" } }
if ($fail.Count -gt 0) { $fail | ForEach-Object { Write-Output "FAIL: $_" }; exit 1 }
Write-Output "workspace policy ok ($($members.Count) members, $($default.Count) default)"

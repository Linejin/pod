# Review panel demo data (docs/09 §7), on the isolated demo daemon of drive.ps1.
# Dot-source after drive.ps1:
#   . crates/pod-app/tools/drive.ps1; . crates/pod-app/tools/review-demo.ps1
#   New-ReviewDemo          # repo "revdemo" + task revdemo/feat (fake agent) + edits in its worktree
#   New-BenchTask           # task revdemo/bench: one 10k-line file with ~950 changed spots
#   Release-Agent           # fake agent: working -> idle (a queued review is then delivered)
#
# The task agent is podd's examples/fake-agent.exe (no real Claude/Codex turn):
#   osc:state=working  wait-file:<signal>  paste-loop:<log>
# so it is "working" until Release-Agent creates the signal file; then it
# reports idle and records every submitted prompt in $AgentLog.
$RevRepo = Join-Path $Demo "revdemo"
$AgentSignal = Join-Path $Demo "agent-go.txt"
$AgentLog = Join-Path $Demo "agent-prompts.log"
$FakeAgent = Join-Path $Rel "examples\fake-agent.exe"

# Input posted to the app window (does not move the user's cursor or need focus).
if (-not ("PM" -as [type])) {
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class PM {
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool ScreenToClient(IntPtr h, ref POINT p);
  [DllImport("user32.dll")] public static extern uint MapVirtualKey(uint code, uint type);
}
"@
}
function PLParam($pt) {
  $p = New-Object PM+POINT; $p.X = $pt[0]; $p.Y = $pt[1]; [void][PM]::ScreenToClient((Hwnd), [ref]$p)
  return [IntPtr](($p.Y -shl 16) -bor ($p.X -band 0xFFFF))
}
function PClick($pt, [int]$mods = 0) {
  $h = Hwnd; $l = PLParam $pt
  [void][PM]::PostMessage($h, 0x0200, [IntPtr]$mods, $l); Start-Sleep -Milliseconds 80          # WM_MOUSEMOVE
  [void][PM]::PostMessage($h, 0x0201, [IntPtr](1 -bor $mods), $l); Start-Sleep -Milliseconds 40 # WM_LBUTTONDOWN
  [void][PM]::PostMessage($h, 0x0202, [IntPtr]$mods, $l); Start-Sleep -Milliseconds 250        # WM_LBUTTONUP
}
function PMove($pt) { [void][PM]::PostMessage((Hwnd), 0x0200, [IntPtr]0, (PLParam $pt)); Start-Sleep -Milliseconds 150 }
function PType([string]$s) { $h = Hwnd; foreach ($c in $s.ToCharArray()) { [void][PM]::PostMessage($h, 0x0102, [IntPtr][int]$c, [IntPtr]1); Start-Sleep -Milliseconds 8 }; Start-Sleep -Milliseconds 200 }
function PKey([int]$vk) {
  $h = Hwnd; $scan = [PM]::MapVirtualKey($vk, 0)
  [void][PM]::PostMessage($h, 0x0100, [IntPtr]$vk, [IntPtr](1 -bor ($scan -shl 16))); Start-Sleep -Milliseconds 30
  [void][PM]::PostMessage($h, 0x0101, [IntPtr]$vk, [IntPtr](1 -bor ($scan -shl 16) -bor (3 -shl 30))); Start-Sleep -Milliseconds 200
}

function Git-In([string]$dir) { $ErrorActionPreference = "Continue"; & git -C $dir -c core.autocrlf=false @args 2>&1 | Out-Null }

function New-ReviewRepo {
  if (Test-Path $RevRepo) { Remove-Item -Recurse -Force $RevRepo }
  New-Item -ItemType Directory -Force (Join-Path $RevRepo "src") | Out-Null
  Set-Content -Encoding utf8 (Join-Path $RevRepo "README.md") "# revdemo`n`nA tiny crate for the review demo.`n"
  Set-Content -Encoding utf8 (Join-Path $RevRepo "Cargo.toml") "[package]`nname = `"revdemo`"`nversion = `"0.1.0`"`nedition = `"2024`"`n"
  $lib = @(
    "//! Parsing helpers.", "",
    "pub fn parse_port(s: &str) -> Option<u16> {",
    "    s.trim().parse().ok()",
    "}", "",
    "pub fn parse_host(s: &str) -> &str {",
    "    s.split(':').next().unwrap_or(s)",
    "}", "",
    "pub fn greeting(name: &str) -> String {",
    "    format!(`"hello {name}`")",
    "}"
  ) -join "`n"
  Set-Content -Encoding utf8 (Join-Path $RevRepo "src\lib.rs") $lib
  Set-Content -Encoding utf8 (Join-Path $RevRepo "src\legacy.rs") "// old code, to be removed`npub fn legacy() {}`n"
  $bench = Join-Path $RevRepo "src\bench.rs"
  # 10k lines, ~950 changed spots (pod-diff example; same generator as diff-spike --gen-bench).
  & (Join-Path $Rel "examples\gen-bench.exe") (Join-Path $Demo "bench") | Out-Null
  Copy-Item (Join-Path $Demo "bench\bench_old.rs") $bench -ErrorAction SilentlyContinue
  Git-In $RevRepo init -q -b main
  Git-In $RevRepo config user.email demo@example.com
  Git-In $RevRepo config user.name demo
  Git-In $RevRepo add .
  Git-In $RevRepo commit -q -m "base"
}

function New-ReviewDemo {
  New-ReviewRepo
  Remove-Item $AgentSignal, $AgentLog -ErrorAction SilentlyContinue
  Pod repo add $RevRepo --name revdemo | Out-Null
  Pod task new revdemo feat "--" $FakeAgent "osc:state=working" "print:fake agent working (waiting for $AgentSignal)" "wait-file:$AgentSignal" "paste-loop:$AgentLog" | Out-Null
  $wt = (Pod task show revdemo/feat --json | ConvertFrom-Json).worktree
  # "Agent" edits: modify, add, delete, and a docs change.
  $lib = @(
    "//! Parsing helpers.", "",
    "/// Parses a TCP port; rejects 0.",
    "pub fn parse_port(s: &str) -> Option<u16> {",
    "    let port: u16 = s.trim().parse().ok()?;",
    "    (port != 0).then_some(port)",
    "}", "",
    "pub fn parse_host(s: &str) -> &str {",
    "    s.rsplit_once(':').map(|(h, _)| h).unwrap_or(s)",
    "}", "",
    "pub fn greeting(name: &str) -> String {",
    "    format!(`"hello, {name}!`")",
    "}"
  ) -join "`n"
  Set-Content -Encoding utf8 (Join-Path $wt "src\lib.rs") $lib
  Set-Content -Encoding utf8 (Join-Path $wt "src\net.rs") "use crate::parse_port;`n`npub fn default_port() -> u16 {`n    parse_port(`"8080`").unwrap()`n}`n"
  Remove-Item (Join-Path $wt "src\legacy.rs")
  Add-Content -Encoding utf8 (Join-Path $wt "README.md") "`n## Ports`n`nPort 0 is rejected.`n"
  return $wt
}

function New-BenchTask {
  $wt = (Pod task show revdemo/feat --json | ConvertFrom-Json).worktree
  Pod task new revdemo bench | Out-Null
  $bwt = (Pod task show revdemo/bench --json | ConvertFrom-Json).worktree
  Copy-Item (Join-Path $Demo "bench\bench_new.rs") (Join-Path $bwt "src\bench.rs") -Force
  return $bwt
}

function Release-Agent { Set-Content $AgentSignal "go" }

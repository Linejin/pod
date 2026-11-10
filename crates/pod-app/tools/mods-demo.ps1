# Screenshots of the extensions feature (docs/16 §10), with no synthetic input.
#   . crates/pod-app/tools/mods-demo.ps1            (from the repo root, release build first)
#   Mods-Settings dark | Mods-Settings light | Mods-Action dark
# Isolated: its own daemon pipe (podd-mods-demo), app config, worktrees and
# mods folder under target/podapp-demo. Never touches %APPDATA%\Pod\mods.
. (Join-Path $PSScriptRoot "drive.ps1")
$env:POD_SOCKET = "podd-mods-demo"
$Mods = Join-Path $Demo "mods"
$env:POD_MODS_DIR = $Mods
$Fixtures = Join-Path $PSScriptRoot "mods-demo"

# serde_json rejects a BOM; Windows PowerShell's utf8 encoding writes one.
function Write-NoBom([string]$Path) { process { [IO.File]::WriteAllText($Path, $_, (New-Object Text.UTF8Encoding $false)) } }

function Reset-ModsDemo([string]$Theme) {
  Stop-PodApp
  Stop-ModsDaemon
  Start-Sleep -Milliseconds 500
  Remove-Item -Recurse -Force $Mods -ErrorAction SilentlyContinue
  New-Item -ItemType Directory -Force $Mods, $env:POD_APP_CONFIG_DIR | Out-Null
  Copy-Item -Recurse (Join-Path $Root "examples\mods\*") $Mods
  Copy-Item -Recurse (Join-Path $Fixtures "demo-notify") $Mods
  # Everything enabled with what it asks for (the user would do this in Settings).
  @{ mods = @{
      "aider-status" = @{ enabled = $true; granted = @("status") }
      "group-done-notify" = @{ enabled = $true; granted = @("notify", "commands") }
      "pod-layouts" = @{ enabled = $true; granted = @() }
      "demo-notify" = @{ enabled = $true; granted = @("notify") }
  } } | ConvertTo-Json -Depth 5 | Write-NoBom (Join-Path $Mods "pod-mods.json")
  Remove-Item (Join-Path $env:POD_APP_CONFIG_DIR "layout.json") -ErrorAction SilentlyContinue
  @{ theme = $Theme } | ConvertTo-Json | Write-NoBom (Join-Path $env:POD_APP_CONFIG_DIR "settings.json")
  # A fixture repository, so the app shows its normal workspace (no first-run card).
  $repo = Join-Path $Demo "mods-repo"
  if (-not (Test-Path (Join-Path $repo ".git"))) {
    New-Item -ItemType Directory -Force $repo | Out-Null
    Set-Content -Encoding ascii (Join-Path $repo "README.md") "fixture"
    git -C $repo init -q -b main 2>$null; git -C $repo add . ; git -C $repo -c user.name=fixture -c user.email=fixture@example.invalid commit -qm fixture
  }
  Pod repo add $repo | Out-Null
}

function Stop-ModsDaemon { try { Pod server stop --kill 2>$null | Out-Null } catch {} }

# The 확장 section of Settings.
function Mods-Settings([string]$Theme = "dark") {
  Reset-ModsDemo $Theme
  Pod run --name aider-demo '--' powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $Fixtures "aider.ps1") | Out-Null
  $null = Start-PodApp @("--display", "secondary", "--extensions") "mods-settings-$Theme.out"
  Start-Sleep -Seconds 3
  $f = Shot "mods-settings-$Theme"
  Stop-PodApp
  Stop-ModsDaemon
  return $f
}

# aider-status reading a fake Aider screen (source "mod") and demo-notify's toast.
function Mods-Action([string]$Theme = "dark") {
  Reset-ModsDemo $Theme
  Pod run --name aider-demo '--' powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $Fixtures "aider.ps1") | Out-Null
  $null = Start-PodApp @("--display", "secondary", "--open-all") "mods-action-$Theme.out"
  Start-Sleep -Seconds 3
  $idle = Shot "mods-action-idle-$Theme"
  Start-Sleep -Seconds 9
  $asked = Shot "mods-action-question-$Theme"
  $status = Pod status aider-demo
  Stop-PodApp
  Stop-ModsDaemon
  return @($idle, $asked, $status)
}

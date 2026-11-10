# Install / upgrade / uninstall test of the Pod installer in a throw-away prefix.
#
#   powershell -ExecutionPolicy Bypass -File tools/installer/test-install.ps1 [-Setup dist\Pod-Setup-<ver>-x64.exe]
#
# Isolation: the app goes to %TEMP%\pod-install-test\app (/DIR), and every Pod
# process started here uses its own daemon pipe (POD_SOCKET=podd-installer-test)
# and its own data dirs (LOCALAPPDATA/APPDATA/POD_WORKTREES_ROOT under the
# prefix; Pod and the installer's data handling read these variables). So the
# user's real daemon, terminals, data and agent hooks are never touched (the
# installer only *reads* hook status; installing hooks is never automatic).
# What is real and cleaned up again: the Start menu shortcut, the HKCU uninstall
# entry and (for one step) the HKCU PATH entry; the script checks they are back
# to the state before the test.
#
# Writes test-log.md and screenshots to -OutDir (default dist\test-<stamp>).
param(
  [string]$Setup = "",
  [string]$Prefix = (Join-Path $env:TEMP "pod-install-test"),
  [string]$OutDir = ""
)
$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if (-not $Setup) { $Setup = (Get-ChildItem (Join-Path $Root "dist") -Filter "Pod-Setup-*-x64.exe" | Sort-Object LastWriteTime | Select-Object -Last 1).FullName }
if (-not $OutDir) { $OutDir = Join-Path $Root ("dist\test-" + (Get-Date -Format "yyyyMMdd-HHmmss")) }
New-Item -ItemType Directory -Force $OutDir | Out-Null
# Absolute: the uninstaller re-launches itself from %TEMP%, so relative /LOG paths break.
$OutDir = (Resolve-Path $OutDir).Path
$Setup = (Resolve-Path $Setup).Path
$LogFile = Join-Path $OutDir "test-log.md"
$AppId = "{8C7541A9-1FDB-43FC-B2EB-44131622A0CF}_is1"
$UninstKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$AppId"
$StartLnk = Join-Path ([Environment]::GetFolderPath("Programs")) "Pod.lnk"
$DeskLnk = Join-Path ([Environment]::GetFolderPath("Desktop")) "Pod.lnk"
# AppUserModelID for Windows toasts (docs/16 §8.2).
$AumidKey = "HKCU:\Software\Classes\AppUserModelId\Pod.App"
$AumidBefore = Test-Path $AumidKey
# Recursive cleanup is restricted to a child of the OS temporary directory.
$Prefix = [IO.Path]::GetFullPath($Prefix)
$tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
if (-not $Prefix.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -or $Prefix.TrimEnd('\') -eq $tempRoot.TrimEnd('\')) { throw "Unsafe test prefix: $Prefix" }
$Dir = Join-Path $Prefix "app"

$CpuGate = if ($env:POD_CPU_GATE) { $env:POD_CPU_GATE } else { Join-Path $Root "tools\cpu-gate.ps1" }
$script:pass = 0; $script:fail = 0
function Log([string]$s) { Add-Content -Encoding utf8 $LogFile $s; Write-Host $s }
function Check([string]$name, [bool]$ok, [string]$detail = "") {
  if ($ok) { $script:pass++; $m = "- PASS $name" } else { $script:fail++; $m = "- **FAIL** $name" }
  if ($detail) { $m += " - $detail" }
  Log $m
}
function UserPath { (Get-Item "HKCU:\Environment").GetValue("Path", $null, "DoNotExpandEnvironmentNames") }
function AppProcs {
  @(Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith($Dir + "\", [StringComparison]::OrdinalIgnoreCase) })
}
function RunSetup([string[]]$extra, [string]$log) {
  $a = @("/DIR=`"$Dir`"", "/CURRENTUSER", "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/LOG=`"$log`"") + $extra
  & $CpuGate -MaxPct 70 -MaxCargo 1 | Out-Host
  $p = Start-Process -WindowStyle Hidden -FilePath $Setup -ArgumentList $a -PassThru -Wait
  return $p.ExitCode
}
function RunUninstall([string[]]$extra, [string]$log) {
  $u = Join-Path $Dir "unins000.exe"
  $a = @("/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/LOG=`"$log`"") + $extra
  & $CpuGate -MaxPct 70 -MaxCargo 1 | Out-Host
  $p = Start-Process -WindowStyle Hidden -FilePath $u -ArgumentList $a -PassThru -Wait
  # The uninstaller re-launches itself from %TEMP% and returns at once; wait for that copy.
  $deadline = (Get-Date).AddSeconds(90)
  while ((Get-Date) -lt $deadline) {
    $busy = @(Get-CimInstance Win32_Process -Filter "Name like '[_]unins%' or Name like '[_]iu%'")
    if ($busy.Count -eq 0) { break }
    Start-Sleep -Milliseconds 500
  }
  Start-Sleep -Seconds 1
  return $p.ExitCode
}
function Pod([string[]]$a) { & $CpuGate -MaxPct 70 -MaxCargo 1 | Out-Host; $o = & (Join-Path $Dir "pod.exe") @a 2>&1 | Out-String; return $o.Trim() }

Add-Type -AssemblyName System.Drawing, System.Windows.Forms
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class Cap {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr GetClassLongPtr(IntPtr h, int n);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
}
"@
[void][Cap]::SetProcessDpiAwarenessContext([IntPtr](-4))
function Shot([IntPtr]$h, [string]$file) {
  $r = New-Object Cap+RECT; [void][Cap]::GetWindowRect($h, [ref]$r)
  $bmp = New-Object System.Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $hdc = $g.GetHdc(); [void][Cap]::PrintWindow($h, $hdc, 2); $g.ReleaseHdc($hdc)
  $bmp.Save($file); $g.Dispose(); $bmp.Dispose()
}

# ---------------------------------------------------------------- setup
if (Test-Path $UninstKey) { throw "Pod is already installed for this user ($UninstKey); refusing to test over it" }
if (Test-Path $Prefix) { Remove-Item -Recurse -Force $Prefix }
New-Item -ItemType Directory -Force $Prefix, "$Prefix\localappdata", "$Prefix\appdata", "$Prefix\worktrees" | Out-Null
$env:POD_SOCKET = "podd-installer-test"
$env:LOCALAPPDATA = "$Prefix\localappdata"
$env:APPDATA = "$Prefix\appdata"
$env:POD_WORKTREES_ROOT = "$Prefix\worktrees"
$PathBefore = UserPath
$StartBefore = Test-Path $StartLnk
$DeskBefore = Test-Path $DeskLnk

$hash = (Get-FileHash -Algorithm SHA256 $Setup).Hash.ToLower()
Log "# Pod installer test $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')"
Log ""
Log "- installer: $Setup ($([math]::Round((Get-Item $Setup).Length / 1MB, 1)) MB, sha256 $hash)"
Log "- prefix: $Prefix (app: $Dir), pipe: $env:POD_SOCKET"
Log "- before: start menu Pod.lnk=$StartBefore, desktop Pod.lnk=$DeskBefore, uninstall key=$(Test-Path $UninstKey)"
Log ""

try {
# ---------------------------------------------------------------- 1. fresh install
Log "## 1. Fresh install (/VERYSILENT /TASKS=addtopath)"
$code = RunSetup @("/TASKS=addtopath") "$OutDir\1-install.log"
Check "setup exit code 0" ($code -eq 0) "exit $code"
foreach ($f in "pod-app.exe", "podd.exe", "pod-holder.exe", "pod.exe", "pod-hook.exe", "conpty.dll", "x64\OpenConsole.exe",
               "LICENSE.txt", "THIRD-PARTY-NOTICES.txt", "README.txt", "licenses\ConPTY-LICENSE.txt", "unins000.exe") {
  Check "file $f" (Test-Path (Join-Path $Dir $f))
}
Check "Start menu shortcut" (Test-Path $StartLnk) $StartLnk
if (Test-Path $StartLnk) {
  $lnk = (New-Object -ComObject WScript.Shell).CreateShortcut($StartLnk)
  Check "shortcut icon" ($lnk.IconLocation -eq ((Join-Path $Dir "pod-app.exe") + ",0")) $lnk.IconLocation
  Check "shortcut target" ($lnk.TargetPath -eq (Join-Path $Dir "pod-app.exe")) $lnk.TargetPath
}
Check "no desktop shortcut (task unchecked)" ((Test-Path $DeskLnk) -eq $DeskBefore)
$aumid = Get-ItemProperty $AumidKey -ErrorAction SilentlyContinue
Check "toast AppUserModelID registered" ($aumid.DisplayName -eq "Pod") "$AumidKey DisplayName=$($aumid.DisplayName)"
$k = Get-ItemProperty $UninstKey -ErrorAction SilentlyContinue
Check "HKCU uninstall entry" ($null -ne $k) "$($k.DisplayName) $($k.DisplayVersion), InstallLocation=$($k.InstallLocation)"
Check "PATH has app dir" ((UserPath) -split ';' -contains $Dir)
$ilog = Get-Content "$OutDir\1-install.log" -Raw
Check "hooks checked, not installed silently" (($ilog -match "pod hooks status -> exit 0") -and ($ilog -notmatch "hooks install")) ""
Check "hooks dir untouched (prefix)" (-not (Test-Path "$Prefix\localappdata\Pod\bin"))

$releaseVersion = [regex]::Match((Get-Content (Join-Path $Root "Cargo.toml") -Raw), '(?ms)^\[workspace\.package\][^\[]*?^version\s*=\s*"([^"]+)"').Groups[1].Value
foreach ($binary in "pod-app", "podd", "pod-holder", "pod", "pod-hook") {
  $vi = (Get-Item (Join-Path $Dir "$binary.exe")).VersionInfo
  Check "$binary version resource" ($vi.ProductName -eq "Pod" -and $vi.CompanyName -eq "SJin-Choi" -and $vi.FileVersion -eq $releaseVersion -and $vi.OriginalFilename -eq "$binary.exe" -and $vi.FileDescription -match 'Apache-2.0') "$($vi.FileVersion), $($vi.FileDescription)"
}
Check "uninstall icon" ($k.DisplayIcon -like '*pod-app.exe*') $k.DisplayIcon

# ---------------------------------------------------------------- 2. run installed binaries
Log ""
Log "## 2. Installed binaries"
$v = Pod @("--version"); Check "pod --version" ($v -match '^pod \d') $v
& $CpuGate -MaxPct 70 -MaxCargo 1
$v = & (Join-Path $Dir "podd.exe") --version 2>&1 | Out-String; Check "podd --version" ($v -match '^podd \d') $v.Trim()
$hs = Pod @("hooks", "status"); Log ('    pod hooks status: ' + ($hs -replace "`r?`n", ' | '))
Check "pod hooks status runs" ($LASTEXITCODE -eq 0)

& $CpuGate -MaxPct 70 -MaxCargo 1
$app = Start-Process -FilePath (Join-Path $Dir "pod-app.exe") -ArgumentList "--ui-fixture" -PassThru
$h = [IntPtr]::Zero
for ($i = 0; $i -lt 60 -and $h -eq [IntPtr]::Zero; $i++) { Start-Sleep -Milliseconds 500; $app.Refresh(); $h = $app.MainWindowHandle }
Check "pod-app --ui-fixture opens a window" ($h -ne [IntPtr]::Zero)
if ($h -ne [IntPtr]::Zero) {
  Check "runtime large window icon" ([Cap]::SendMessage($h, 0x7F, [IntPtr]1, [IntPtr]::Zero) -ne [IntPtr]::Zero)
  Check "runtime small window icon" ([Cap]::SendMessage($h, 0x7F, [IntPtr]0, [IntPtr]::Zero) -ne [IntPtr]::Zero)
  Check "runtime class icon" ([Cap]::GetClassLongPtr($h, -14) -ne [IntPtr]::Zero)
  Start-Sleep -Seconds 4
  $r = New-Object Cap+RECT; [void][Cap]::GetWindowRect($h, [ref]$r)
  $onPrimary = [System.Windows.Forms.Screen]::FromHandle($h).Primary
  Check "app window on the secondary monitor" (-not $onPrimary) "rect $($r.L),$($r.T)-$($r.R),$($r.B)"
  Shot $h "$OutDir\app-installed-fixture.png"
  Log "    screenshot: app-installed-fixture.png"
}
if (-not $app.HasExited) { [void]$app.CloseMainWindow(); if (-not $app.WaitForExit(5000)) { $app.Kill() } }

# A real terminal through the installed daemon + holder.
$o = Pod @("run", "--name", "keep", "--", "cmd.exe", "/k", "echo installer-test")
Check "pod run (installed podd + holder)" ($LASTEXITCODE -eq 0) $o
Start-Sleep -Seconds 2
$holder = @(AppProcs | Where-Object Name -eq "pod-holder.exe")
$podd = @(AppProcs | Where-Object Name -eq "podd.exe")
Check "holder and podd run from the install dir" (($holder.Count -eq 1) -and ($podd.Count -eq 1)) "holder pid $($holder.ProcessId), podd pid $($podd.ProcessId)"
$conpty = (Pod @("--json", "ls") | ConvertFrom-Json)
Log "    terminal conpty: $($conpty.terminals[0].conpty)"
Check "terminal uses the bundled ConPTY of the install" ("$($conpty.terminals[0].conpty)" -like "bundled:$Dir\conpty.dll") "$($conpty.terminals[0].conpty)"

# ---------------------------------------------------------------- 3. upgrade over itself
Log ""
Log "## 3. Upgrade over itself while podd + 1 terminal run"
$code = RunSetup @("/TASKS=addtopath") "$OutDir\3-upgrade.log"
Check "upgrade exit code 0" ($code -eq 0) "exit $code"
$ulog = Get-Content "$OutDir\3-upgrade.log" -Raw
Check "podd stopped gracefully (pod server stop)" ($ulog -match "pod server stop -> exit 0")
$holder2 = @(AppProcs | Where-Object Name -eq "pod-holder.exe")
Check "holder survived (same pid)" (($holder2.Count -eq 1) -and ($holder2[0].ProcessId -eq $holder[0].ProcessId)) "pid $($holder2.ProcessId)"
$aside = @(Get-ChildItem $Dir, "$Dir\x64" -Filter "*.old-*" -ErrorAction SilentlyContinue)
Check "in-use files moved aside" ($aside.Count -ge 1) (($aside | ForEach-Object Name) -join ", ")
$staged = Get-ChildItem (Join-Path (Split-Path $Setup) "stage") -Recurse -Filter "pod-holder.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
if ($staged) { Check "new pod-holder.exe in place" ((Get-FileHash "$Dir\pod-holder.exe").Hash -eq (Get-FileHash $staged.FullName).Hash) }
$ls = Pod @("ls")
Check "terminal still listed as running after upgrade (new podd re-attached)" ($ls -match 'keep' -and $ls -match 'running') ($ls -replace "`r?`n", ' | ')
$rd = Pod @("read", "keep")
Check "terminal output intact" ($rd -match 'installer-test')
$podd2 = @(AppProcs | Where-Object Name -eq "podd.exe")
Check "new podd started from the install dir" (($podd2.Count -eq 1) -and ($podd2[0].ProcessId -ne $podd[0].ProcessId)) "pid $($podd2.ProcessId)"

# ---------------------------------------------------------------- 4. uninstall refused while terminals run
Log ""
Log "## 4. Silent uninstall with a running terminal (no /STOPTERMINALS)"
$code = RunUninstall @() "$OutDir\4-uninstall-refused.log"
Check "uninstall refused (non-zero exit)" ($code -ne 0) "exit $code"
Check "install intact" ((Test-Path "$Dir\pod.exe") -and (Test-Path $UninstKey))
Check "terminal still running" (@(AppProcs | Where-Object Name -eq "pod-holder.exe").Count -eq 1)

# ---------------------------------------------------------------- 5. uninstall, keep data
Log ""
Log "## 5. Silent uninstall /STOPTERMINALS (keep user data)"
$code = RunUninstall @("/STOPTERMINALS") "$OutDir\5-uninstall.log"
Check "uninstall exit code 0" ($code -eq 0) "exit $code"
Check "no Pod processes left" ((AppProcs).Count -eq 0) ((AppProcs | ForEach-Object { "$($_.Name):$($_.ProcessId)" }) -join ", ")
Check "app dir removed" (-not (Test-Path $Dir)) $(if (Test-Path $Dir) { (Get-ChildItem -Recurse $Dir | ForEach-Object FullName) -join ", " })
Check "Start menu shortcut removed" ((Test-Path $StartLnk) -eq $StartBefore)
Check "uninstall entry removed" (-not (Test-Path $UninstKey))
Check "toast AppUserModelID removed" ((Test-Path $AumidKey) -eq $AumidBefore)
Check "PATH restored" ((UserPath) -eq $PathBefore)
Check "user data kept (state dir)" (Test-Path "$Prefix\localappdata\pod\state")

# ---------------------------------------------------------------- 6. reinstall (no PATH) + uninstall with purge
Log ""
Log "## 6. Reinstall without PATH task, then uninstall /PURGEDATA"
$code = RunSetup @("/TASKS=") "$OutDir\6-install.log"
Check "reinstall exit code 0" ($code -eq 0) "exit $code"
$pa = UserPath
Check "PATH unchanged when task is off" ($pa -eq $PathBefore) "len $($pa.Length) vs $($PathBefore.Length); has dir: $($pa -split ';' -contains $Dir)"
New-Item -ItemType Directory -Force "$Prefix\appdata\pod\pod-app" | Out-Null
Set-Content "$Prefix\appdata\pod\pod-app\settings.json" "{}"
$code = RunUninstall @("/PURGEDATA") "$OutDir\6-uninstall.log"
Check "uninstall exit code 0" ($code -eq 0) "exit $code"
Check "app dir removed" (-not (Test-Path $Dir))
Check "state + log + app config removed" (-not (Test-Path "$Prefix\localappdata\pod") -and -not (Test-Path "$Prefix\appdata\pod"))
Check "Start menu, uninstall entry, PATH back to before" (((Test-Path $StartLnk) -eq $StartBefore) -and -not (Test-Path $UninstKey) -and ((UserPath) -eq $PathBefore))
Check "desktop unchanged" ((Test-Path $DeskLnk) -eq $DeskBefore)

Log ""
Log "Result: $script:pass passed, $script:fail failed"
} finally {
# Safety net: never leave a test install behind, including on exceptions.
if (Test-Path $UninstKey) {
  Log "cleanup: uninstall entry still present, running the uninstaller with /STOPTERMINALS /PURGEDATA"
  if (Test-Path "$Dir\unins000.exe") { [void](RunUninstall @("/STOPTERMINALS", "/PURGEDATA") "$OutDir\cleanup-uninstall.log") }
}
AppProcs | ForEach-Object { Log "cleanup: killing $($_.Name) $($_.ProcessId)"; Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
if (Test-Path $UninstKey) { Log "cleanup: removing uninstall entry"; Remove-Item $UninstKey }
if (-not $AumidBefore -and (Test-Path $AumidKey)) { Log "cleanup: removing toast AppUserModelID"; Remove-Item $AumidKey -Recurse }
if (-not $StartBefore -and (Test-Path $StartLnk)) { Log "cleanup: removing Start menu shortcut"; Remove-Item $StartLnk }
if ((UserPath) -ne $PathBefore) {
  Log "cleanup: restoring PATH (was: $(UserPath))"
  Set-ItemProperty "HKCU:\Environment" -Name Path -Value $PathBefore -Type ExpandString
}
Remove-Item -Recurse -Force $Prefix -ErrorAction SilentlyContinue
}
if ($script:fail -gt 0) { exit 1 }

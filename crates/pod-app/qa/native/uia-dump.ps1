# UI Automation tree of the real release pod-app on read-only fixture scenes:
# what Narrator would read (Name, ControlType, keyboard focusability, shortcut
# properties). No input is sent, focus is not taken, no setting is changed.
# Each fixture window opens on the secondary monitor and is closed afterwards.
#
#   powershell -NoProfile -File crates/pod-app/qa/native/uia-dump.ps1
#
# Output: uia/<scene>.uia.txt (indented tree), uia/summary.json, uia/failures.txt;
# full JSON in target/native-qa/uia-json.
# Exit 1 when an interactive element has no accessible name, a UI label (not
# fixture data) has no Korean, or an expected element/role is missing.
param(
  [string[]]$Scenes = @('workspace','palette','settings','agents','inspect','open-repo','new-task','terminal','new-group','gates','notifications','review','shortcuts'),
  [string]$Output = (Join-Path $PSScriptRoot 'uia'),
  [int]$SettleSeconds = 8
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '../../../..')).Path
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
New-Item -ItemType Directory -Force $Output | Out-Null
# Full per-scene JSON (large) stays in target/; the readable trees are the evidence.
$jsonDir = Join-Path $root 'target/native-qa/uia-json'
New-Item -ItemType Directory -Force $jsonDir | Out-Null
$exe = Join-Path $root 'target/release/pod-app.exe'
if (-not (Test-Path $exe)) { throw "build first: $exe" }

$interactive = @('Button','Edit','TabItem','TreeItem','ListItem','MenuItem','CheckBox','RadioButton','ComboBox','Hyperlink','Slider','Spinner','SplitButton','Tree','List','Tab','Document')
# Fixture data and proper nouns may be non-Korean; UI labels may not.
$dataName = '^(Pod|Claude|Codex|PowerShell|powershell|sample|main|README\.md|docs/.*|src/.*|.*\.(rs|md|toml|json|py|txt))$|^(Task \d{3}|Worker \d+|sample/|fixture-|pod/task-)|^\d+$'
# The managed UIA client predates UIA_FullDescriptionPropertyId (30159), where
# AccessKit puts accessible descriptions (Pod's shortcut hints). Read it via COM.
Add-Type -TypeDefinition @'
using System; using System.Collections.Generic; using System.Runtime.InteropServices;
[ComImport, Guid("352ffba8-0973-437c-a61f-f64cafd81df9"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)] public interface IUiaCondition {}
[ComImport, Guid("14314595-b4bc-4055-95f2-58f2e42c9855"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
public interface IUiaArray { int Length { get; } IUiaElement GetElement(int i); }
[ComImport, Guid("d22108aa-8ac5-49a5-837b-37bbb3d7591e"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
public interface IUiaElement {
  void SetFocus(); void GetRuntimeId(); void FindFirst();
  IUiaArray FindAll(int scope, IUiaCondition condition);
  void FindFirstBuildCache(); void FindAllBuildCache(); void BuildUpdatedCache();
  [return: MarshalAs(UnmanagedType.Struct)] object GetCurrentPropertyValue(int id);
}
[ComImport, Guid("30cbe57d-d9d0-452a-ab13-7ac5ac4825ee"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
public interface IUia {
  void CompareElements(); void CompareRuntimeIds(); void GetRootElement();
  IUiaElement ElementFromHandle(IntPtr hwnd);
  void ElementFromPoint(); void GetFocusedElement(); void GetRootElementBuildCache(); void ElementFromHandleBuildCache();
  void ElementFromPointBuildCache(); void GetFocusedElementBuildCache(); void CreateTreeWalker(); void ControlViewWalker();
  void ContentViewWalker(); void RawViewWalker(); void RawViewCondition();
  IUiaCondition ControlViewCondition { get; }
}
public static class PodUia {
  // name|type -> FullDescription, for every control-view descendant.
  public static Dictionary<string,string> Descriptions(IntPtr hwnd) {
    var uia = (IUia)Activator.CreateInstance(Type.GetTypeFromCLSID(new Guid("ff48dba4-60ef-4201-aa87-54103eef594e")));
    var all = uia.ElementFromHandle(hwnd).FindAll(4, uia.ControlViewCondition);
    var map = new Dictionary<string,string>();
    for (int i = 0; i < all.Length; i++) {
      var e = all.GetElement(i);
      var d = e.GetCurrentPropertyValue(30159) as string;
      if (!String.IsNullOrEmpty(d)) map[(e.GetCurrentPropertyValue(30005) as string) + "|" + e.GetCurrentPropertyValue(30003)] = d;
    }
    return map;
  }
}
'@

function Walk($el, $depth, $rows) {
  $c = $el.Current
  $type = $c.ControlType.ProgrammaticName -replace '^ControlType\.', ''
  $desc = $script:descriptions["$($c.Name)|$($c.ControlType.Id)"]
  $row = [ordered]@{
    depth = $depth; type = $type; localized = $c.LocalizedControlType; name = $c.Name; id = $c.AutomationId
    focusable = $c.IsKeyboardFocusable; enabled = $c.IsEnabled; offscreen = $c.IsOffscreen
    accelerator = $c.AcceleratorKey; accesskey = $c.AccessKey; help = $c.HelpText; description = $desc
  }
  $rows.Add([pscustomobject]$row) | Out-Null
  $walker = [Windows.Automation.TreeWalker]::ControlViewWalker
  $child = $walker.GetFirstChild($el)
  while ($child) { Walk $child ($depth + 1) $rows; $child = $walker.GetNextSibling($child) }
}

$summary = @()
$failures = @()
$script:descriptions = @{}
$oldConfig = $env:POD_APP_CONFIG_DIR
try {
  foreach ($scene in $Scenes) {
    $config = Join-Path $root ('target/native-qa/' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force $config | Out-Null
    $env:POD_APP_CONFIG_DIR = $config
    [IO.File]::WriteAllText((Join-Path $config 'settings.json'), '{"theme":"dark","reduced_motion":true,"font":"Malgun Gothic"}')
    & C:\ai\ide\tools\cpu-gate.ps1 -MaxPct 70 -MaxCargo 1 | Out-Null
    $app = Start-Process $exe -ArgumentList "--ui-fixture --display secondary --no-sound --fixture-scene $scene" -PassThru
    try {
      $deadline = [DateTime]::UtcNow.AddSeconds(30)
      do { Start-Sleep -Milliseconds 200; $app.Refresh() } until ($app.MainWindowHandle -ne 0 -or [DateTime]::UtcNow -gt $deadline)
      if ($app.MainWindowHandle -eq 0) { throw "${scene}: no window" }
      Start-Sleep -Seconds $SettleSeconds
      # The first UIA query activates AccessKit; the tree arrives with a later frame.
      $win = [Windows.Automation.AutomationElement]::FromHandle($app.MainWindowHandle)
      $last = -1
      for ($try = 0; $try -lt 30; $try++) {
        $rows = New-Object System.Collections.ArrayList
        Walk $win 0 $rows
        if ($rows.Count -gt 5 -and $rows.Count -eq $last) { break }
        $last = $rows.Count
        Start-Sleep -Seconds 2
      }
      $script:descriptions = [PodUia]::Descriptions($app.MainWindowHandle)
      $rows = New-Object System.Collections.ArrayList
      Walk $win 0 $rows
      $rows | ConvertTo-Json -Depth 3 | Set-Content -Encoding UTF8 (Join-Path $jsonDir "$scene.json")
      $lines = foreach ($r in $rows) {
        $flags = @()
        if ($r.focusable) { $flags += 'focusable' }
        if (-not $r.enabled) { $flags += 'disabled' }
        if ($r.accelerator) { $flags += "accel=$($r.accelerator)" }
        if ($r.accesskey) { $flags += "access=$($r.accesskey)" }
        if ($r.description) { $flags += "desc=$($r.description)" }
        ("`t" * $r.depth) + "$($r.type) [$($r.localized)] `"$($r.name)`"" + $(if ($flags) { ' {' + ($flags -join '; ') + '}' } else { '' })
      }
      $lines | Set-Content -Encoding UTF8 (Join-Path $Output "$scene.uia.txt")

      $ui = @($rows | Where-Object { $interactive -contains $_.type -and -not $_.offscreen })
      $unnamed = @($ui | Where-Object { -not $_.name -or -not $_.name.Trim() })
      $notKorean = @($ui | Where-Object { $_.name -and $_.name -notmatch '[\uAC00-\uD7A3]' -and $_.name -notmatch $dataName })
      $focusable = @($ui | Where-Object { $_.focusable })
      $withShortcut = @($rows | Where-Object { $_.accelerator -or ($_.description -match 'Ctrl\+') })
      foreach ($u in $unnamed) { $failures += "${scene}: unnamed $($u.type) (id $($u.id))" }
      foreach ($n in $notKorean) { $failures += "${scene}: non-Korean UI label $($n.type) `"$($n.name)`"" }
      $summary += [pscustomobject]@{
        scene = $scene; elements = $rows.Count; interactive = $ui.Count; focusable = $focusable.Count
        unnamed = $unnamed.Count; non_korean = $notKorean.Count; shortcut_described = $withShortcut.Count
        types = (($ui | Group-Object type | ForEach-Object { "$($_.Name)=$($_.Count)" }) -join ', ')
      }
    } finally {
      if (-not $app.HasExited) { [void]$app.CloseMainWindow(); if (-not $app.WaitForExit(5000)) { Stop-Process -Id $app.Id } }
    }
  }
} finally { $env:POD_APP_CONFIG_DIR = $oldConfig }

# Expected names and roles (what Narrator announces for the main controls).
$expect = @(
  @{scene='workspace'; type='Button'; name='명령·터미널 검색'},
  @{scene='workspace'; type='Button'; name='설정'},
  @{scene='workspace'; type='Tree'; name='저장소와 작업 탐색'},
  @{scene='workspace'; type='Edit'; name='저장소, 작업, 터미널 검색'},
  @{scene='workspace'; type='TreeItem'; name='Task 000 - 한글 작업과 레이아웃 🔎'},
  @{scene='workspace'; type='List'; name='최근 알림'},
  @{scene='workspace'; type='Button'; name='상태'},
  @{scene='palette'; type='Edit'; name='작업, 터미널 검색 · > 명령만'},
  @{scene='palette'; type='ListItem'; name='*'},
  @{scene='settings'; type='TabItem'; name='설정'},
  @{scene='inspect'; type='TabItem'; name='상태'},
  @{scene='new-task'; type='Button'; name='만들기'},
  @{scene='new-task'; type='Button'; name='닫기'},
  @{scene='agents'; type='Tree'; name='에이전트 트리'}
)
foreach ($e in $expect) {
  if ($Scenes -notcontains $e.scene) { continue }
  $rows = Get-Content -Raw -Encoding UTF8 (Join-Path $jsonDir "$($e.scene).json") | ConvertFrom-Json
  $hit = @($rows | Where-Object { $_.type -eq $e.type -and ($e.name -eq '*' -or $_.name -eq $e.name) })
  if (-not $hit.Count) { $failures += "$($e.scene): expected $($e.type) `"$($e.name)`" not found" }
  else { $summary += [pscustomobject]@{ scene = $e.scene; expect = "$($e.type) `"$($e.name)`""; found = $hit.Count; focusable = [bool]($hit | Where-Object focusable) } }
}
$summary | ConvertTo-Json -Depth 3 | Set-Content -Encoding UTF8 (Join-Path $Output 'summary.json')
$failures | Set-Content -Encoding UTF8 (Join-Path $Output 'failures.txt')
$summary | Format-Table -AutoSize | Out-String -Width 220
"failures: $($failures.Count)"
$failures
if ($failures.Count) { exit 1 }

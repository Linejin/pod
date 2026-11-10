# UIA pattern states of the settings controls in a running pod-app (read only).
#   powershell -NoProfile -File crates/pod-app/qa/settings/uia-probe.ps1 -AppPid <pid>
# Prints: TabItem IsSelected, ComboBox ExpandCollapse + Value, ListItem IsSelected,
# Toggle ToggleState. Exit 1 if a category tab, select or switch lacks its pattern.
param([Parameter(Mandatory)][int]$AppPid)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$A = [Windows.Automation.AutomationElement]
$cond = New-Object Windows.Automation.PropertyCondition($A::ProcessIdProperty, $AppPid)
$win = $A::RootElement.FindFirst([Windows.Automation.TreeScope]::Children, $cond)
if (-not $win) { throw "no window for pid $AppPid" }
$fail = 0
foreach ($e in $win.FindAll([Windows.Automation.TreeScope]::Descendants, [Windows.Automation.Condition]::TrueCondition)) {
  $type = $e.Current.ControlType.ProgrammaticName -replace '^ControlType\.', ''
  $name = $e.Current.Name
  $p = $null
  switch ($type) {
    'TabItem' {
      if ($e.TryGetCurrentPattern([Windows.Automation.SelectionItemPattern]::Pattern, [ref]$p)) { "TabItem  $name  IsSelected=$($p.Current.IsSelected)" } else { "FAIL TabItem ${name}: no SelectionItem"; $fail++ }
    }
    'ComboBox' {
      $v = $null
      if ($e.TryGetCurrentPattern([Windows.Automation.ExpandCollapsePattern]::Pattern, [ref]$p)) { $s = $p.Current.ExpandCollapseState } else { $s = 'none'; $fail++ }
      if ($e.TryGetCurrentPattern([Windows.Automation.ValuePattern]::Pattern, [ref]$v)) { $val = $v.Current.Value } else { $val = '' }
      "ComboBox $name  ExpandCollapse=$s  Value=$val"
    }
    'ListItem' {
      if ($e.TryGetCurrentPattern([Windows.Automation.SelectionItemPattern]::Pattern, [ref]$p)) { "ListItem $name  IsSelected=$($p.Current.IsSelected)" }
    }
    default {
      if ($e.TryGetCurrentPattern([Windows.Automation.TogglePattern]::Pattern, [ref]$p)) { "$type $name  Toggle=$($p.Current.ToggleState)" }
    }
  }
}
if ($fail) { "FAIL: $fail"; exit 1 }

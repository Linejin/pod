# Native Windows regression for the settings button's titlebar hit target.
# The caller launches an isolated app fixture and foregrounds it first.
# Run through the shared QA executor when used in the validation workstream.
param([Parameter(Mandatory)][int]$AppPid, [Parameter(Mandatory)][string]$ExpectedBinary, [Parameter(Mandatory)][string]$Output)
$ErrorActionPreference='Stop'
Add-Type -AssemblyName UIAutomationClient,UIAutomationTypes
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class PodSettingsMouse {
 [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X,Y; }
 [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr c);
 [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
 [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT p);
 [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr h,uint flags);
 [DllImport("user32.dll")] public static extern bool SetCursorPos(int x,int y);
 [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
 [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h,uint msg,IntPtr w,IntPtr l);
 [DllImport("user32.dll")] public static extern void mouse_event(uint flags,uint x,uint y,uint data,UIntPtr extra);
 public static long Probe(IntPtr h,int x,int y) {
  if(GetForegroundWindow()!=h || GetAncestor(WindowFromPoint(new POINT {X=x,Y=y}),2)!=h) throw new Exception("BLOCKED: fixture is not foreground or its target is covered");
  SetCursorPos(x,y); System.Threading.Thread.Sleep(150);
  POINT actual; GetCursorPos(out actual);
  if(actual.X!=x || actual.Y!=y) throw new Exception("BLOCKED: target point is outside the usable desktop");
  if(GetForegroundWindow()!=h) throw new Exception("BLOCKED: fixture lost foreground");
  uint point=(((uint)y & 65535u)<<16)|((uint)x & 65535u);
  return SendMessage(h,0x84,IntPtr.Zero,new IntPtr((long)point)).ToInt64();
 }
 public static void Click(IntPtr h) {
  if(GetForegroundWindow()!=h) throw new Exception("BLOCKED: fixture lost foreground");
  mouse_event(2,0,0,0,UIntPtr.Zero); mouse_event(4,0,0,0,UIntPtr.Zero);
 }
}
"@
[void][PodSettingsMouse]::SetProcessDpiAwarenessContext([IntPtr](-4))
$app=Get-Process -Id $AppPid
if($app.Path -ne [IO.Path]::GetFullPath($ExpectedBinary)){throw 'Refusing input: fixture executable does not match ExpectedBinary'}
$record=[ordered]@{pid=$AppPid;binary=$app.Path;sha256=(Get-FileHash $app.Path).Hash;expected='HTCLIENT (1), then real mouse click opens settings TabItem';status='FAIL'}
try {
 $root=[Windows.Automation.AutomationElement]::FromHandle($app.MainWindowHandle)
 $buttonCondition=New-Object Windows.Automation.AndCondition ((New-Object Windows.Automation.PropertyCondition ([Windows.Automation.AutomationElement]::NameProperty),'설정'),(New-Object Windows.Automation.PropertyCondition ([Windows.Automation.AutomationElement]::ControlTypeProperty),([Windows.Automation.ControlType]::Button)))
 $button=$root.FindFirst([Windows.Automation.TreeScope]::Descendants,$buttonCondition)
 for($attempt=0;-not $button -and $attempt -lt 6;$attempt++){Start-Sleep -Milliseconds 500;$button=$root.FindFirst([Windows.Automation.TreeScope]::Descendants,$buttonCondition)}
 if(-not $button){throw 'BLOCKED: fixture settings button not available in UIA'}
 $bounds=$button.Current.BoundingRectangle
 $record.point=@([int]($bounds.X+$bounds.Width/2),[int]($bounds.Y+$bounds.Height/2))
 $record.hit_test=[PodSettingsMouse]::Probe($app.MainWindowHandle,$record.point[0],$record.point[1])
 if($record.hit_test -ne 1){throw "Settings button is not a client hit target: WM_NCHITTEST=$($record.hit_test)"}
 [PodSettingsMouse]::Click($app.MainWindowHandle)
 Start-Sleep -Seconds 3
 $tabCondition=New-Object Windows.Automation.AndCondition ((New-Object Windows.Automation.PropertyCondition ([Windows.Automation.AutomationElement]::NameProperty),'설정'),(New-Object Windows.Automation.PropertyCondition ([Windows.Automation.AutomationElement]::ControlTypeProperty),([Windows.Automation.ControlType]::TabItem)))
 $record.tab_found=$null -ne $root.FindFirst([Windows.Automation.TreeScope]::Descendants,$tabCondition)
 if(-not $record.tab_found){throw 'Native mouse click did not open the settings tab'}
 $record.status='PASS';$record.actual='HTCLIENT; real mouse opened settings tab'
} catch {$record.actual=$_.Exception.Message;if($record.actual.StartsWith('BLOCKED:')){$record.status='BLOCKED'};throw} finally {
 $record | ConvertTo-Json -Depth 5 | Set-Content -Encoding utf8 $Output
}
$record | ConvertTo-Json -Depth 5

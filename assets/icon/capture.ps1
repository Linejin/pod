# Native QA on the secondary monitor. Only the fixture and its file-properties
# dialog are opened; no real daemon or user workspace is needed.
param([string]$Exe = "", [string]$Out = "", [switch]$PropertiesOnly)
$ErrorActionPreference = 'Stop'
$root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
if (-not $Exe) { $Exe = Join-Path $root 'target\release\pod-app.exe' }
if (-not $Out) { $Out = Join-Path $PSScriptRoot 'qa' }
$Exe = (Resolve-Path $Exe).Path
Add-Type -AssemblyName System.Windows.Forms,System.Drawing,UIAutomationClient,UIAutomationTypes
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class IconQA {
 [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L,T,R,B; }
 [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X,Y; }
 [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr c);
 [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr a,int x,int y,int w,int t,uint f);
 [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h,out RECT r);
 [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h,out RECT r);
 [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h,ref POINT p);
 [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h,IntPtr dc,uint f);
 [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
 [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
 [DllImport("user32.dll")] public static extern IntPtr MonitorFromWindow(IntPtr h,uint f);
 [DllImport("shcore.dll")] public static extern int GetDpiForMonitor(IntPtr m,int t,out uint x,out uint y);
 [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowEx(IntPtr parent,IntPtr after,string cls,string name);
 [StructLayout(LayoutKind.Sequential,CharSet=CharSet.Unicode)] public struct SHELLEXECUTEINFO {
  public int cbSize; public uint fMask; public IntPtr hwnd;
  public string lpVerb,lpFile,lpParameters,lpDirectory; public int nShow;
  public IntPtr hInstApp,lpIDList; public string lpClass; public IntPtr hkeyClass;
  public uint dwHotKey; public IntPtr hIcon,hProcess;
 }
 [DllImport("shell32.dll",CharSet=CharSet.Unicode,SetLastError=true)] public static extern bool ShellExecuteEx(ref SHELLEXECUTEINFO info);
}
"@
[void][IconQA]::SetProcessDpiAwarenessContext([IntPtr](-4))
$sec = [System.Windows.Forms.Screen]::AllScreens | Where-Object { -not $_.Primary } | Select-Object -First 1
if (-not $sec) { throw 'Secondary monitor required' }
function CaptureWindow([IntPtr]$h,[string]$name) {
 $r = New-Object IconQA+RECT; [void][IconQA]::GetWindowRect($h,[ref]$r)
 $b = New-Object System.Drawing.Bitmap ($r.R-$r.L),($r.B-$r.T)
 $g = [System.Drawing.Graphics]::FromImage($b); $dc=$g.GetHdc()
 [void][IconQA]::PrintWindow($h,$dc,2);$g.ReleaseHdc($dc);$g.Dispose()
 $mx=[uint32]0;$my=[uint32]0
 [void][IconQA]::GetDpiForMonitor([IconQA]::MonitorFromWindow($h,2),0,[ref]$mx,[ref]$my)
 $scale=[double]$mx/[double][IconQA]::GetDpiForWindow($h)
 if($name -like 'explorer-properties*'){
  # On a disconnected desktop DWM can omit tabs/buttons after a page switch.
  # Keep the actual DWM frame and ask the native controls to paint their client.
  $client=New-Object System.Drawing.Bitmap $b.Width,$b.Height
  $cg=[System.Drawing.Graphics]::FromImage($client);$cdc=$cg.GetHdc()
  [void][IconQA]::PrintWindow($h,$cdc,0);$cg.ReleaseHdc($cdc);$cg.Dispose()
  $cr=New-Object IconQA+RECT;$pt=New-Object IconQA+POINT
  [void][IconQA]::GetClientRect($h,[ref]$cr);[void][IconQA]::ClientToScreen($h,[ref]$pt)
  $src=New-Object System.Drawing.Rectangle ([int](($pt.X-$r.L)/$scale)),([int](($pt.Y-$r.T)/$scale)),($cr.R-$cr.L),($cr.B-$cr.T)
  $cg=[System.Drawing.Graphics]::FromImage($b);$cg.DrawImage($client,$src,$src,[System.Drawing.GraphicsUnit]::Pixel);$cg.Dispose();$client.Dispose()
 }
 if($scale -gt 1.01){
  $crop=$b.Clone((New-Object System.Drawing.Rectangle 0,0,([int]($b.Width/$scale)),([int]($b.Height/$scale))),$b.PixelFormat)
  $crop.Save((Join-Path $Out $name));$crop.Dispose()
 } else {$b.Save((Join-Path $Out $name))}
 $b.Dispose()
}
$gate = if ($env:POD_CPU_GATE) {$env:POD_CPU_GATE} else {Join-Path $root 'tools\cpu-gate.ps1'}
if (-not $PropertiesOnly) {
$capturePrefix = [IO.Path]::GetFullPath((Join-Path ([IO.Path]::GetTempPath()) ("pod-icon-capture-" + [guid]::NewGuid())))
$tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
if (-not $capturePrefix.StartsWith($tempRoot,[StringComparison]::OrdinalIgnoreCase)) {throw 'Unsafe capture prefix'}
New-Item -ItemType Directory -Force "$capturePrefix\local", "$capturePrefix\roaming" | Out-Null
$env:LOCALAPPDATA = "$capturePrefix\local"; $env:APPDATA = "$capturePrefix\roaming"
$env:POD_SOCKET = 'podd-icon-fixture'
& $gate -MaxPct 70 -MaxCargo 1
$app=Start-Process $Exe -ArgumentList '--ui-fixture','--display','secondary' -PassThru
try {
 for($i=0;$i -lt 60;$i++){ $app.Refresh();if($app.MainWindowHandle -ne 0){break};Start-Sleep -Milliseconds 250 }
 $h=$app.MainWindowHandle;if($h -eq 0){throw 'No app window'}
 [void][IconQA]::SetForegroundWindow($h);Start-Sleep -Seconds 3
 if([System.Windows.Forms.Screen]::FromHandle($h).Primary){throw 'App is on primary monitor'}
 CaptureWindow $h 'app-window.png'
 $bnd=$sec.Bounds
 $tray=[IconQA]::FindWindowEx([IntPtr]::Zero,[IntPtr]::Zero,'Shell_SecondaryTrayWnd',$null)
 if($tray -ne [IntPtr]::Zero){CaptureWindow $tray 'taskbar-native.png'}
 else {
  $tray=[IconQA]::FindWindowEx([IntPtr]::Zero,[IntPtr]::Zero,'Shell_TrayWnd',$null)
  if($tray -ne [IntPtr]::Zero){CaptureWindow $tray 'taskbar-primary-native.png'}
 }
 "App HWND=$h DPI=$([IconQA]::GetDpiForWindow($h)); monitor=$($sec.DeviceName) bounds=$bnd" | Set-Content (Join-Path $Out 'capture-info.txt')
} finally {
 if(-not $app.HasExited){[void]$app.CloseMainWindow();if(-not $app.WaitForExit(5000)){$app.Kill();$app.WaitForExit()}}
 Remove-Item -LiteralPath $capturePrefix -Recurse -Force
}
}
# Ask Explorer for the actual Properties dialog, then select its Details tab.
& $gate -MaxPct 70 -MaxCargo 1
$info=New-Object IconQA+SHELLEXECUTEINFO
$info.cbSize=[Runtime.InteropServices.Marshal]::SizeOf($info)
$info.fMask=0x0000000C; $info.lpVerb='properties'; $info.lpFile=$Exe; $info.nShow=1
if(-not [IconQA]::ShellExecuteEx([ref]$info)){throw ('ShellExecuteEx properties: '+[Runtime.InteropServices.Marshal]::GetLastWin32Error())}
$A=[System.Windows.Automation.AutomationElement];$TS=[System.Windows.Automation.TreeScope]
$w=$null
for($i=0;$i -lt 40 -and -not $w;$i++){
 $w=$A::RootElement.FindAll($TS::Children,[System.Windows.Automation.Condition]::TrueCondition) | Where-Object {$_.Current.Name -like '*pod-app*' -and $_.Current.ClassName -eq '#32770'} | Select-Object -First 1
 if(-not $w){Start-Sleep -Milliseconds 250}
}
if(-not $w){throw 'Explorer properties dialog did not open'}
$h=[IntPtr]$w.Current.NativeWindowHandle
try {
 [void][IconQA]::SetWindowPos($h,[IntPtr]::Zero,($sec.WorkingArea.X+100),($sec.WorkingArea.Y+80),0,0,0x0001)
 Start-Sleep -Seconds 2
 $w=$A::FromHandle($h)
 CaptureWindow $h 'explorer-properties-general.png'
 $w.FindAll($TS::Descendants,[System.Windows.Automation.Condition]::TrueCondition) | ForEach-Object {"$($_.Current.ControlType.ProgrammaticName): $($_.Current.Name)"} | Set-Content -Encoding utf8 (Join-Path $Out 'properties-controls.txt')
 $tabs=$w.FindAll($TS::Descendants,[System.Windows.Automation.Condition]::TrueCondition) | Where-Object {$_.Current.ControlType -eq [System.Windows.Automation.ControlType]::TabItem}
 # Prefer semantic Details selection when UIA exposes the native tabs.
 $detail=$tabs | Where-Object {$_.Current.Name -match 'Details|\uC790\uC138\uD788'} | Select-Object -First 1
 if($detail){
  $pattern=$detail.GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern);$pattern.Select()
 } else {
  # The native sheet's UIA provider exposes generic panes in this session.
  # Observed tabs: General, Compatibility, Digital Signatures, Security,
  # Details, Previous Versions. PSM_SETCURSEL selects the observed fifth page.
  [void][IconQA]::PostMessage($h,0x0465,[IntPtr]4,[IntPtr]::Zero)
 }
 [void][IconQA]::SetForegroundWindow($h);Start-Sleep -Seconds 2
 CaptureWindow $h 'explorer-properties-details.png'
 $w.FindAll($TS::Descendants,[System.Windows.Automation.Condition]::TrueCondition) | ForEach-Object {$_.Current.Name} | Set-Content -Encoding utf8 (Join-Path $Out 'properties-text.txt')
} finally {[void][IconQA]::PostMessage($h,0x0010,[IntPtr]::Zero,[IntPtr]::Zero)}

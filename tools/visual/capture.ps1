param(
 [ValidateSet('capture','compare')][string]$Mode = 'capture',
 [string]$Output = 'crates/pod-app/qa/d3/current',
 [string]$Baseline = 'tools/visual/baseline',
 [int]$Tolerance = 8,
 [int]$Width = 1440, [int]$Height = 960,
 [int]$SettleSeconds = 2,
 [string[]]$Themes = @('dark','light'),
 [switch]$HighContrast,
 [switch]$HelpersOnly,
 [string]$Font = 'Malgun Gothic',
 [string]$TerminalFont = 'Cascadia Mono',
 # Style theme id (docs/10 §D5); empty = the default style. Non-empty styles get a `-<style>` file suffix.
 [string]$Style = '',
 # Layout mode id (docs/10 §D5.4, e.g. kanban); empty = the style's default. Adds a `-<layout>` file suffix.
 [string]$Layout = '',
 # UIA dump attempts per scene until the required nodes of the scene in this layout appear (uia-required.ps1).
 [int]$UiaAttempts = 6,
 [string[]]$Scenes = @('workspace','palette','settings','agents','inspect','open-repo','new-task','terminal','new-group','gates','notifications','review','shortcuts','editor','orch-board','orch-inbox','orch-cleanup','orch-settings')
)
Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
$env:RUSTC_WRAPPER = 'C:\Users\User\AppData\Local\Microsoft\WinGet\Packages\Mozilla.sccache_Microsoft.Winget.Source_8wekyb3d8bbwe\sccache-v0.19.0-x86_64-pc-windows-msvc\sccache.exe'
$env:CARGO_BUILD_JOBS = '4'
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'uia-required.ps1')
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
Set-Location $root
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type -ReferencedAssemblies System.Drawing @'
using System; using System.Drawing; using System.Drawing.Imaging; using System.Runtime.InteropServices;
public static class PodCapture {
 [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L,T,R,B; }
 [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
 [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h,out RECT r);
 [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
 [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h,IntPtr dc,uint flags);
 [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h,int cmd);
 [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h,IntPtr after,int x,int y,int w,int height,uint flags);
 [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X,Y; }
 [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h,out RECT r);
 [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h,ref POINT p);
 [DllImport("user32.dll")] public static extern bool IsZoomed(IntPtr h);
 // Client area in screen coordinates.
 public static RECT ClientOnScreen(IntPtr h) {
  RECT c; GetClientRect(h,out c); var p=new POINT(); ClientToScreen(h,ref p);
  var r=new RECT(); r.L=p.X; r.T=p.Y; r.R=p.X+c.R; r.B=p.Y+c.B; return r;
 }
 public static long Compare(string expected,string actual,string diff,int tolerance) {
  using(var a=new Bitmap(expected)) using(var b=new Bitmap(actual)) {
   if(a.Size!=b.Size) throw new Exception("Image dimensions differ: "+expected);
   using(var d=new Bitmap(a.Width,a.Height,PixelFormat.Format32bppArgb)) {
    var rect=new Rectangle(0,0,a.Width,a.Height);
    var ad=a.LockBits(rect,ImageLockMode.ReadOnly,PixelFormat.Format32bppArgb);
    var bd=b.LockBits(rect,ImageLockMode.ReadOnly,PixelFormat.Format32bppArgb);
    var dd=d.LockBits(rect,ImageLockMode.WriteOnly,PixelFormat.Format32bppArgb);
    long changed=0;
    try {
     int length=ad.Stride*a.Height; var aa=new byte[length]; var bb=new byte[length]; var pixels=new byte[length];
     Marshal.Copy(ad.Scan0,aa,0,length); Marshal.Copy(bd.Scan0,bb,0,length);
     for(int i=0;i<length;i+=4) {
      bool bad=Math.Abs(aa[i]-bb[i])>tolerance||Math.Abs(aa[i+1]-bb[i+1])>tolerance||Math.Abs(aa[i+2]-bb[i+2])>tolerance;
      if(bad) changed++; pixels[i]=(byte)(bad?255:bb[i]/4); pixels[i+1]=(byte)(bad?0:bb[i+1]/4); pixels[i+2]=(byte)(bad?255:bb[i+2]/4); pixels[i+3]=255;
     }
     Marshal.Copy(pixels,0,dd.Scan0,length);
    } finally {a.UnlockBits(ad);b.UnlockBits(bd);d.UnlockBits(dd);}
    d.Save(diff,ImageFormat.Png); return changed;
   }
  }
 }
}
'@
[void][PodCapture]::SetProcessDpiAwarenessContext([IntPtr](-4))

# Restores the fixture window to $Width x $Height at the monitor's working
# area +24 px, waits $SettleSeconds and returns its Rect and Client (screen).
# The app opens maximized. Restoring before that maximize lands leaves the
# window flagged maximized at the capture size, and the app then insets its
# frame top (~11 px blank strip, content lower; about 1 capture in 50). So:
# wait for the maximize (3 s at most), and place again until the window is
# not zoomed and its client area starts at the window top (own title bar).
function Set-PodCaptureWindow([IntPtr]$h, $monitor, [int]$Width, [int]$Height, [int]$SettleSeconds, [string]$scene) {
 $zoomBy = [DateTime]::UtcNow.AddSeconds(3)
 while (-not [PodCapture]::IsZoomed($h) -and [DateTime]::UtcNow -lt $zoomBy) { Start-Sleep -Milliseconds 50 }
 $r = New-Object PodCapture+RECT
 $client = New-Object PodCapture+RECT
 for ($place = 1; $place -le 3; $place++) {
  [void][PodCapture]::ShowWindow($h,4)
  [void][PodCapture]::SetWindowPos($h,[IntPtr]::Zero,$monitor.WorkingArea.X+24,$monitor.WorkingArea.Y+24,$Width,$Height,0x14)
  Start-Sleep -Seconds $SettleSeconds
  [void][PodCapture]::GetWindowRect($h,[ref]$r)
  $client = [PodCapture]::ClientOnScreen($h)
  $zoomed = [PodCapture]::IsZoomed($h)
  if (-not $zoomed -and $client.T -eq $r.T) { return @{ Rect = $r; Client = $client } }
  Write-Host "Window of '$scene' not restored (zoomed $zoomed, client top $($client.T), window top $($r.T)); placing again"
 }
 throw "Window of '$scene' never restored (client top $($client.T), window top $($r.T))"
}
if ($HelpersOnly) { return }
New-Item -ItemType Directory -Force $Output | Out-Null
$results = @()
if ($Mode -eq 'compare') {
 $baselineFiles = @(Get-ChildItem $Baseline -Filter '*.png')
 if (-not $baselineFiles.Count) { throw 'No baseline images' }
 foreach ($file in $baselineFiles) {
  $actual = Join-Path $Output $file.Name
  if (-not (Test-Path $actual)) { throw "Missing capture: $actual" }
  $changed = [PodCapture]::Compare($file.FullName, (Resolve-Path $actual).Path, (Join-Path (Resolve-Path $Output).Path ($file.BaseName+'.diff.png')), $Tolerance)
  $results += [pscustomobject]@{Image=$file.Name; ChangedPixels=$changed; Tolerance=$Tolerance; Pass=($changed -eq 0)}
 }
 $results | ConvertTo-Json | Set-Content -Encoding UTF8 (Join-Path $Output compare.json)
 $results | Format-Table
 if ($results.Where({-not $_.Pass}).Count) { exit 1 }
 exit 0
}
$monitor = [Windows.Forms.Screen]::AllScreens | Where-Object {-not $_.Primary} | Select-Object -First 1
if (-not $monitor) { throw 'Secondary monitor required' }
if ($Width -gt $monitor.WorkingArea.Width -or $Height -gt $monitor.WorkingArea.Height) { throw 'Capture size exceeds secondary monitor' }
# Unknown layout ids fail here, before any window opens.
foreach ($scene in $Scenes) { [void](Get-PodUiaRequired $scene $Layout) }
$exe = Join-Path $root 'target/release/pod-app.exe'
$oldConfig = $env:POD_APP_CONFIG_DIR
try {
 foreach ($theme in $Themes) {
  foreach ($scene in $Scenes) {
   $config = Join-Path $root ('target/visual/'+[Guid]::NewGuid().ToString('N'))
   New-Item -ItemType Directory -Force $config | Out-Null
   $env:POD_APP_CONFIG_DIR = $config
   $settings = @{theme=$theme;reduced_motion=$true;font=$Font;terminal_font=$TerminalFont;high_contrast=[bool]$HighContrast}
   if ($Style) { $settings.style = $Style }
   if ($Layout) { $settings.layout = $Layout }
   [IO.File]::WriteAllText((Join-Path $config 'settings.json'), ($settings | ConvertTo-Json -Compress))
   & C:\ai\ide\tools\cpu-gate.ps1 -MaxPct 70 -MaxCargo 1
   $app = Start-Process $exe -ArgumentList "--ui-fixture --display secondary --no-sound --fixture-scene $scene" -PassThru
   try {
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    do { Start-Sleep -Milliseconds 200; $app.Refresh() } until ($app.MainWindowHandle -ne 0 -or [DateTime]::UtcNow -gt $deadline)
    $h = $app.MainWindowHandle
    if ($h -eq 0) { throw 'App did not open a window' }
    $placed = Set-PodCaptureWindow $h $monitor $Width $Height $SettleSeconds $scene
    $r = $placed.Rect
    $client = $placed.Client
    $name = if ($Style) { "$theme-$scene-$Style" } else { "$theme-$scene" }
    if ($Layout) { $name = "$name-$Layout" }
    $bitmap = New-Object Drawing.Bitmap ($r.R-$r.L),($r.B-$r.T)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    $dc = $graphics.GetHdc()
    try { $ok = [PodCapture]::PrintWindow($h,$dc,2) } finally { $graphics.ReleaseHdc($dc); $graphics.Dispose() }
    if (-not $ok) { $bitmap.Dispose(); throw 'PrintWindow failed' }
    $bitmap.Save((Join-Path (Resolve-Path $Output).Path "$name.png")); $bitmap.Dispose()
    $appId = $app.Id
    $treeText = Get-PodUiaTree $scene {
     $tree = orca computer get-app-state --app "pid:$appId" --no-screenshot --json | ConvertFrom-Json
     if (-not $tree.ok) { throw 'Accessibility snapshot failed' }
     $tree.result.snapshot.treeText
    } -Attempts $UiaAttempts -Layout $Layout
    $treeText | Set-Content -Encoding UTF8 (Join-Path $Output "$name.tree.txt")
    $results += [pscustomobject]@{Image="$name.png"; DPI=[PodCapture]::GetDpiForWindow($h); Rect=@($r.L,$r.T,$r.R,$r.B); Client=@($client.L,$client.T,$client.R,$client.B); Scene=$scene; Theme=$theme; Style=$Style; Layout=$Layout; Fixture=$true; Font=$Font; TerminalFont=$TerminalFont; HighContrast=[bool]$HighContrast}
   } finally {
    if (-not $app.HasExited) { [void]$app.CloseMainWindow(); if (-not $app.WaitForExit(5000)) { Stop-Process -Id $app.Id } }
   }
  }
 }
} finally { $env:POD_APP_CONFIG_DIR = $oldConfig }
$results | ConvertTo-Json -Depth 4 | Set-Content -Encoding UTF8 (Join-Path $Output manifest.json)
$results | Format-Table

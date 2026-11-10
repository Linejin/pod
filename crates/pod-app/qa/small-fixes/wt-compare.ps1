# Item 1 reference: the same Korean/emoji lines in Windows Terminal (Cascadia
# Mono, Malgun Gothic fallback), captured DPI-aware on the secondary monitor.
#   & crates\pod-app\qa\small-fixes\wt-compare.ps1
# Not tracked (public repo: no third-party UI screenshots): written under target.
param([string]$Out = (Join-Path $PSScriptRoot '..\..\..\..\target\wt-reference.png'))
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System; using System.Text; using System.Runtime.InteropServices;
public static class WtCap {
 [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L,T,R,B; }
 public delegate bool EnumProc(IntPtr h, IntPtr l);
 [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr l);
 [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
 [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
 [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
 [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
 [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint f);
 [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
 public static IntPtr Find(string title) {
  IntPtr found = IntPtr.Zero;
  EnumWindows((h, l) => { var sb = new StringBuilder(256); GetWindowText(h, sb, 256);
   if (IsWindowVisible(h) && sb.ToString().Contains(title)) { found = h; return false; } return true; }, IntPtr.Zero);
  return found;
 }
}
'@
[void][WtCap]::SetProcessDpiAwarenessContext([IntPtr](-4))
$sample = Join-Path $env:TEMP 'pod-wide-sample.txt'
# Same lines as the pod-app `editor-terminal-hover` fixture, plus emoji.
$text = "src\lib.rs(3,1): note: MSVC 형식 경로`r`nmissing/file.rs:9 (없는 파일은 링크가 아님)`r`n문서: https://doc.rust-lang.org/error_codes/E0425.html`r`n한글 가나다 ABC 漢字 かな 😀👍 end`r`n"
[IO.File]::WriteAllText($sample, $text, [Text.Encoding]::Unicode)  # UTF-16 LE + BOM: cmd type prints it as is
$title = 'PodWideCompare'
Start-Process wt.exe -ArgumentList @('-w','new','--pos','2620,300','--size','80,8','--title',$title,'--suppressApplicationTitle','cmd','/k',"cls & type `"$sample`"")
$h = [IntPtr]::Zero
for ($i = 0; $i -lt 50 -and $h -eq [IntPtr]::Zero; $i++) { Start-Sleep -Milliseconds 200; $h = [WtCap]::Find($title) }
if ($h -eq [IntPtr]::Zero) { throw 'Windows Terminal window not found' }
Start-Sleep -Seconds 2
$r = New-Object WtCap+RECT; [void][WtCap]::GetWindowRect($h, [ref]$r)
$bmp = New-Object Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
$g = [Drawing.Graphics]::FromImage($bmp); $dc = $g.GetHdc(); [void][WtCap]::PrintWindow($h, $dc, 2); $g.ReleaseHdc($dc)
$bmp.Save($Out); $g.Dispose(); $bmp.Dispose()
"rect $($r.L),$($r.T) $($r.R - $r.L)x$($r.B - $r.T) -> $Out"
[void][WtCap]::PostMessage($h, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)  # WM_CLOSE
Remove-Item $sample

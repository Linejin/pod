param([string]$Exe = "$PSScriptRoot/../../../target/release/examples/gallery.exe", [switch]$Primary, [switch]$Dialog, [switch]$SelfCheck)
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class GalleryCapture {
 [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L,T,R,B; }
 [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
 [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h,out RECT r);
 [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
 [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
 [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h,IntPtr dc,uint flags);
}
"@
[void][GalleryCapture]::SetProcessDpiAwarenessContext([IntPtr](-4))
foreach ($theme in @('dark','light')) {
 $galleryArgs=@('--duration','30'); if($theme -eq 'light'){$galleryArgs+='--light'};if($Primary){$galleryArgs+='--primary'};if($Dialog){$galleryArgs+='--dialog'};if($SelfCheck){$galleryArgs+='--self-check'}
 if(Test-Path 'C:/ai/ide/tools/cpu-gate.ps1'){ & C:/ai/ide/tools/cpu-gate.ps1 -MaxPct 70 -MaxCargo 1 }
$p=Start-Process -FilePath $Exe -ArgumentList $galleryArgs -PassThru -RedirectStandardError "$PSScriptRoot/startup-error.log"
 $deadline=(Get-Date).AddSeconds(60)
 do {Start-Sleep -Milliseconds 500;$p.Refresh();$h=$p.MainWindowHandle} while($h -eq [IntPtr]::Zero -and -not $p.HasExited -and (Get-Date) -lt $deadline)
 Start-Sleep -Seconds 3
 if($h -eq [IntPtr]::Zero){throw 'Gallery has no window'}
 $r=New-Object GalleryCapture+RECT;[void][GalleryCapture]::GetWindowRect($h,[ref]$r)
 $dpi=[GalleryCapture]::GetDpiForWindow($h);$name="$theme-$dpi";if($Primary){$name+='-primary'}else{$name+='-secondary'};if($Dialog){$name+='-dialog'};if($SelfCheck){$name+='-focus';Copy-Item -LiteralPath "$PSScriptRoot/live-check.txt" -Destination "$PSScriptRoot/$name-check.txt" -Force}
 $bmp=New-Object System.Drawing.Bitmap ($r.R-$r.L),($r.B-$r.T);$g=[System.Drawing.Graphics]::FromImage($bmp);$dc=$g.GetHdc();$ok=[GalleryCapture]::PrintWindow($h,$dc,2);$g.ReleaseHdc($dc);$g.Dispose();if(-not $ok){throw 'PrintWindow failed'}
 $bmp.Save("$PSScriptRoot/$name.png")
 $gray=New-Object System.Drawing.Bitmap $bmp.Width,$bmp.Height;$gg=[System.Drawing.Graphics]::FromImage($gray)
 $matrix=New-Object System.Drawing.Imaging.ColorMatrix
 $matrix.Matrix00=0.2126;$matrix.Matrix01=0.2126;$matrix.Matrix02=0.2126
 $matrix.Matrix10=0.7152;$matrix.Matrix11=0.7152;$matrix.Matrix12=0.7152
 $matrix.Matrix20=0.0722;$matrix.Matrix21=0.0722;$matrix.Matrix22=0.0722
 $attributes=New-Object System.Drawing.Imaging.ImageAttributes;$attributes.SetColorMatrix($matrix)
 $gg.DrawImage($bmp,(New-Object System.Drawing.Rectangle 0,0,$bmp.Width,$bmp.Height),0,0,$bmp.Width,$bmp.Height,[System.Drawing.GraphicsUnit]::Pixel,$attributes)
 $gray.Save("$PSScriptRoot/$name-grayscale.png");$gg.Dispose();$gray.Dispose();$attributes.Dispose();$bmp.Dispose()
 "$name dpi=$dpi rect=$($r.L),$($r.T),$($r.R),$($r.B) PrintWindow=$ok" | Tee-Object -FilePath "$PSScriptRoot/capture.log" -Append
 if(-not $Dialog -and -not $SelfCheck) {
  $x=[int]($r.L+($r.R-$r.L)*0.4);$y=[int]($r.T+($r.B-$r.T)*0.7)
  [void][GalleryCapture]::PostMessage($h,0x20A,[IntPtr](-840*65536),[IntPtr](($y -shl 16) -bor ($x -band 65535)))
  Start-Sleep -Milliseconds 700
  $bottom=New-Object System.Drawing.Bitmap ($r.R-$r.L),($r.B-$r.T);$bg=[System.Drawing.Graphics]::FromImage($bottom);$dc=$bg.GetHdc();[void][GalleryCapture]::PrintWindow($h,$dc,2);$bg.ReleaseHdc($dc);$bg.Dispose();$bottom.Save("$PSScriptRoot/$name-controls.png");$bottom.Dispose()
 }
 if(-not $p.HasExited){Stop-Process -Id $p.Id}
}

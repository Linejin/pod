param([int]$AppPid,[int]$Key=0,[string]$Text='',[double]$X=-1,[double]$Y=-1)
# Target-window input only; never changes the user's foreground window or pointer.
Add-Type @"
using System;using System.Runtime.InteropServices;
public static class D2Input {
 [StructLayout(LayoutKind.Sequential)]public struct POINT{public int X,Y;}
 [StructLayout(LayoutKind.Sequential)]public struct RECT{public int L,T,R,B;}
 [DllImport("user32.dll")]public static extern uint GetDpiForWindow(IntPtr h);
 [DllImport("user32.dll")]public static extern bool GetWindowRect(IntPtr h,out RECT r);
 [DllImport("user32.dll")]public static extern bool ScreenToClient(IntPtr h,ref POINT p);
 [DllImport("user32.dll")]public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
 [DllImport("user32.dll")]public static extern bool PostMessage(IntPtr h,uint msg,IntPtr w,IntPtr l);
 [DllImport("user32.dll")]public static extern uint MapVirtualKey(uint key,uint map);
}
"@
[void][D2Input]::SetProcessDpiAwarenessContext([IntPtr](-4))
$h=(Get-Process -Id $AppPid -ErrorAction Stop).MainWindowHandle
if($h -eq [IntPtr]::Zero){throw 'No fixture window'}
if($Text){foreach($c in $Text.ToCharArray()){[void][D2Input]::PostMessage($h,0x102,[IntPtr][int]$c,[IntPtr]1)}}
if($Key){$scan=[D2Input]::MapVirtualKey($Key,0);$flags=1 -bor ($scan -shl 16);if($Key -ge 0x21 -and $Key -le 0x28){$flags=$flags -bor (1 -shl 24)};[void][D2Input]::PostMessage($h,0x100,[IntPtr]$Key,[IntPtr]$flags);[void][D2Input]::PostMessage($h,0x101,[IntPtr]$Key,[IntPtr]($flags -bor (3 -shl 30)))}
Start-Sleep -Milliseconds 250

if($X -ge 0 -and $Y -ge 0){
 $r=New-Object D2Input+RECT;[void][D2Input]::GetWindowRect($h,[ref]$r);$scale=[D2Input]::GetDpiForWindow($h)/96.0
 $pt=New-Object D2Input+POINT;$pt.X=[int]($r.L+$X*$scale);$pt.Y=[int]($r.T+$Y*$scale);[void][D2Input]::ScreenToClient($h,[ref]$pt)
 $lp=[IntPtr](($pt.Y -shl 16) -bor ($pt.X -band 65535))
 [void][D2Input]::PostMessage($h,0x200,[IntPtr]0,$lp);Start-Sleep -Milliseconds 100
 [void][D2Input]::PostMessage($h,0x201,[IntPtr]1,$lp);[void][D2Input]::PostMessage($h,0x202,[IntPtr]0,$lp);Start-Sleep -Milliseconds 500
}

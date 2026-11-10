# Read-only probe: is an interactive input desktop available, and how long has the
# user been idle? Appends one line to desktop-probe.log. Never sends input and never
# changes focus or system settings.
param([string]$Log = (Join-Path $PSScriptRoot 'desktop-probe.log'))

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class PodDesk {
    [DllImport("user32.dll", SetLastError=true)] public static extern IntPtr OpenInputDesktop(uint flags, bool inherit, uint access);
    [DllImport("user32.dll")] public static extern bool CloseDesktop(IntPtr h);
    [StructLayout(LayoutKind.Sequential)] public struct LASTINPUTINFO { public uint cbSize; public uint dwTime; }
    [DllImport("user32.dll")] public static extern bool GetLastInputInfo(ref LASTINPUTINFO lii);
    [DllImport("kernel32.dll")] public static extern uint GetTickCount();
    public static string Probe() {
        IntPtr h = OpenInputDesktop(0, false, 0x0001 /* DESKTOP_READOBJECTS */);
        int err = h == IntPtr.Zero ? Marshal.GetLastWin32Error() : 0;
        if (h != IntPtr.Zero) CloseDesktop(h);
        LASTINPUTINFO l = new LASTINPUTINFO(); l.cbSize = (uint)Marshal.SizeOf(l);
        GetLastInputInfo(ref l);
        uint idle = unchecked(GetTickCount() - l.dwTime) / 1000;
        return String.Format("input_desktop={0} err={1} idle_s={2}", h != IntPtr.Zero ? "ok" : "denied", err, idle);
    }
}
'@
$logon = [bool](Get-Process LogonUI -ErrorAction SilentlyContinue)
$line = "{0} {1} logonui={2}" -f (Get-Date -Format 's'), [PodDesk]::Probe(), $logon
Add-Content -Path $Log -Value $line -Encoding utf8
$line

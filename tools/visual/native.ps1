# Native keyboard input, restricted to a verified target HWND. No PostMessage.
param([Parameter(Mandatory)][int]$AppPid, [int[]]$Keys = @(), [string]$Text)
Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
$env:RUSTC_WRAPPER = 'C:\Users\User\AppData\Local\Microsoft\WinGet\Packages\Mozilla.sccache_Microsoft.Winget.Source_8wekyb3d8bbwe\sccache-v0.19.0-x86_64-pc-windows-msvc\sccache.exe'
$env:CARGO_BUILD_JOBS = '4'
$ErrorActionPreference = 'Stop'
if (-not ('PodNative' -as [type])) {
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class PodNative {
 [StructLayout(LayoutKind.Sequential)] struct INPUT { public uint type; public UNION u; }
 [StructLayout(LayoutKind.Explicit)] struct UNION { [FieldOffset(0)] public KEY k; [FieldOffset(0)] public MOUSE m; }
 [StructLayout(LayoutKind.Sequential)] struct KEY { public ushort vk, scan; public uint flags,time; public UIntPtr extra; }
 [StructLayout(LayoutKind.Sequential)] struct MOUSE { public int x,y; public uint data,flags,time; public UIntPtr extra; }
 [StructLayout(LayoutKind.Sequential)] struct MSG { public IntPtr h; public uint message; public UIntPtr w; public IntPtr l; public uint time; public int x,y; public uint extra; }
 [DllImport("user32.dll")] static extern bool PeekMessage(out MSG msg,IntPtr h,uint min,uint max,uint remove);
 [DllImport("user32.dll")] static extern uint SendInput(uint count, INPUT[] input, int size);
 [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
 [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr h);
 [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, IntPtr pid);
 [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
 [DllImport("user32.dll",SetLastError=true)] static extern bool AttachThreadInput(uint a,uint b,bool attach);
 [DllImport("user32.dll")] static extern bool BringWindowToTop(IntPtr h);
 [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr h,int cmd);
 [DllImport("user32.dll")] static extern bool IsIconic(IntPtr h);
 public static bool Focus(IntPtr h) {
   MSG msg; PeekMessage(out msg,IntPtr.Zero,0,0,0); // Ensure this helper thread has an input queue.
   uint current=GetCurrentThreadId(), foreground=GetWindowThreadProcessId(GetForegroundWindow(),IntPtr.Zero), target=GetWindowThreadProcessId(h,IntPtr.Zero);
   bool a=false,b=false;
   try {
     if(current!=foreground) a=AttachThreadInput(current,foreground,true);
     if(current!=target) b=AttachThreadInput(current,target,true);
     Console.WriteLine("focus current={0} foregroundThread={1} targetThread={2} attachedForeground={3} attachedTarget={4} error={5} foregroundHwnd={6}",current,foreground,target,a,b,Marshal.GetLastWin32Error(),GetForegroundWindow());
     if(IsIconic(h)) ShowWindow(h,9); BringWindowToTop(h); SetForegroundWindow(h);
   } finally { if(b) AttachThreadInput(current,target,false); if(a) AttachThreadInput(current,foreground,false); }
   return GetForegroundWindow()==h;
 }
 static INPUT Key(ushort vk,ushort scan,uint flags) { return new INPUT { type=1, u=new UNION { k=new KEY { vk=vk,scan=scan,flags=flags } } }; }
 public static void Chord(IntPtr h,int[] keys) {
   if(GetForegroundWindow()!=h) throw new Exception("Target lost foreground; input refused");
   var input=new INPUT[keys.Length*2];
   for(int i=0;i<keys.Length;i++) {input[i]=Key((ushort)keys[i],0,0);input[input.Length-1-i]=Key((ushort)keys[i],0,2);}
   if(SendInput((uint)input.Length,input,Marshal.SizeOf(typeof(INPUT)))!=input.Length) throw new Exception("SendInput failed");
 }
 public static void Text(IntPtr h,string value) {
   foreach(char c in value) { if(GetForegroundWindow()!=h) throw new Exception("Target lost foreground"); var input=new[]{Key(0,c,4),Key(0,c,6)}; if(SendInput(2,input,Marshal.SizeOf(typeof(INPUT)))!=2) throw new Exception("SendInput text failed"); }
 }
}
'@
}
$handle = (Get-Process -Id $AppPid).MainWindowHandle
if ($handle -eq [IntPtr]::Zero -or -not [PodNative]::Focus($handle)) { throw 'Target cannot take native keyboard focus' }
if ($Keys.Count) { [PodNative]::Chord($handle, $Keys) }
if ($Text) { [PodNative]::Text($handle, $Text) }
[pscustomobject]@{ Target=$handle.ToInt64(); Foreground=[PodNative]::GetForegroundWindow().ToInt64(); Keys=$Keys; TextLength=$Text.Length; Delivery='SendInput' } | ConvertTo-Json -Compress

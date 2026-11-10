"""Verify every embedded optical PNG byte-for-byte without executing the binaries."""
from pathlib import Path
import ctypes as c
from ctypes import wintypes as w
import struct
root=Path(__file__).resolve().parent
k=c.WinDLL('kernel32',use_last_error=True)
k.LoadLibraryExW.argtypes=[w.LPCWSTR,w.HANDLE,w.DWORD];k.LoadLibraryExW.restype=w.HMODULE
k.FindResourceW.argtypes=[w.HMODULE,c.c_void_p,c.c_void_p];k.FindResourceW.restype=c.c_void_p
k.LoadResource.argtypes=[w.HMODULE,c.c_void_p];k.LoadResource.restype=c.c_void_p
k.LockResource.argtypes=[c.c_void_p];k.LockResource.restype=c.c_void_p
k.SizeofResource.argtypes=[w.HMODULE,c.c_void_p];k.SizeofResource.restype=w.DWORD
k.FreeLibrary.argtypes=[w.HMODULE]
def data(h,name,kind):
 r=k.FindResourceW(h,name,kind); assert r, (name,kind,c.get_last_error())
 return c.string_at(k.LockResource(k.LoadResource(h,r)),k.SizeofResource(h,r))
for binary in ['pod-app','podd','pod-holder','pod','pod-hook']:
 h=k.LoadLibraryExW(str(root.parent.parent/'target/release'/f'{binary}.exe'),None,2)
 assert h, c.get_last_error()
 try:
  g=data(h,1,14); assert struct.unpack_from('<HHH',g)==(0,1,9)
  sizes=[]
  for i in range(9):
   width,height,_,_,planes,bits,length,ident=struct.unpack_from('<BBBBHHIH',g,6+i*14)
   n=width or 256; assert n==(height or 256) and planes==1 and bits==32
   png=data(h,ident,3); assert len(png)==length
   assert png==(root/f'pod-{n}.png').read_bytes(),(binary,n)
   sizes.append(n)
  assert sizes==[16,20,24,32,40,48,64,128,256],sizes
  assert b'P\x00o\x00d\x00' in data(h,1,16)
  print(f'PASS {binary}: nine exact optical PNGs, VERSIONINFO present')
 finally:k.FreeLibrary(h)

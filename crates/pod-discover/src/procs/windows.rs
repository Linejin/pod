//! The Windows process snapshot behind [`snapshot`](super::snapshot):
//! Toolhelp for every process, then start time, command line and cwd
//! from the processes that may be opened.

use super::{ProcInfo, is_candidate_name};
use windows_sys::Wdk::System::Threading::{
    NtQueryInformationProcess, ProcessBasicInformation, ProcessCommandLineInformation,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE, UNICODE_STRING,
};
use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, IsWow64Process, OpenProcess, PROCESS_BASIC_INFORMATION,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
};

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: we own the handle.
        unsafe { CloseHandle(self.0) };
    }
}

fn wide(s: &[u16]) -> String {
    let end = s.iter().position(|c| *c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..end])
}

pub fn snapshot() -> Vec<ProcInfo> {
    let mut out = Vec::new();
    // SAFETY: plain Win32 calls; the entry is sized as the API requires.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE || snap.is_null() {
            return out;
        }
        let snap = Handle(snap);
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap.0, &mut e) == 0 {
            return out;
        }
        loop {
            out.push(ProcInfo {
                pid: e.th32ProcessID,
                ppid: e.th32ParentProcessID,
                name: wide(&e.szExeFile),
                ..Default::default()
            });
            if Process32NextW(snap.0, &mut e) == 0 {
                break;
            }
        }
    }
    for p in out.iter_mut() {
        if p.pid == 0 {
            continue;
        }
        let details = is_candidate_name(&p.name);
        let access = PROCESS_QUERY_LIMITED_INFORMATION | if details { PROCESS_VM_READ } else { 0 };
        // SAFETY: OpenProcess has no memory-safety preconditions.
        let h = unsafe { OpenProcess(access, 0, p.pid) };
        let h = if h.is_null() && details {
            // SAFETY: as above.
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, p.pid) }
        } else {
            h
        };
        if h.is_null() {
            continue;
        }
        let h = Handle(h);
        p.start_ms = start_ms(h.0);
        if details {
            p.cmdline = command_line(h.0);
            p.cwd = cwd(h.0);
        }
    }
    out
}

fn start_ms(h: HANDLE) -> u64 {
    let z = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut c, mut x, mut k, mut u) = (z, z, z, z);
    // SAFETY: four valid out pointers.
    if unsafe { GetProcessTimes(h, &mut c, &mut x, &mut k, &mut u) } == 0 {
        return 0;
    }
    let t = ((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64;
    t.saturating_sub(116_444_736_000_000_000) / 10_000
}

fn command_line(h: HANDLE) -> Option<String> {
    let mut len = 0u32;
    let mut buf = vec![0u64; 512];
    for _ in 0..2 {
        let bytes = (buf.len() * 8) as u32;
        // SAFETY: `buf` is `bytes` long and 8-aligned (UNICODE_STRING header + text).
        let st = unsafe {
            NtQueryInformationProcess(
                h,
                ProcessCommandLineInformation,
                buf.as_mut_ptr().cast(),
                bytes,
                &mut len,
            )
        };
        if st >= 0 {
            // SAFETY: on success the buffer starts with a UNICODE_STRING whose
            // Buffer points inside `buf`.
            let us = unsafe { &*(buf.as_ptr() as *const UNICODE_STRING) };
            if us.Buffer.is_null() || us.Length == 0 {
                return None;
            }
            let base = buf.as_ptr() as usize;
            let start = us.Buffer as usize;
            let end = start + us.Length as usize;
            if start < base || end > base + buf.len() * 8 {
                return None;
            }
            // SAFETY: bounds checked above.
            let text = unsafe { std::slice::from_raw_parts(us.Buffer, us.Length as usize / 2) };
            return Some(String::from_utf16_lossy(text));
        }
        if len as usize <= buf.len() * 8 || len > 1 << 20 {
            return None;
        }
        buf = vec![0u64; (len as usize).div_ceil(8)];
    }
    None
}

fn read<T: Copy>(h: HANDLE, addr: usize) -> Option<T> {
    let mut v = std::mem::MaybeUninit::<T>::uninit();
    let mut got = 0usize;
    // SAFETY: reads at most size_of::<T>() bytes into `v`.
    let ok = unsafe {
        ReadProcessMemory(
            h,
            addr as *const _,
            v.as_mut_ptr().cast(),
            std::mem::size_of::<T>(),
            &mut got,
        )
    };
    // SAFETY: fully written when the call succeeded with the full size; T is plain data.
    (ok != 0 && got == std::mem::size_of::<T>()).then(|| unsafe { v.assume_init() })
}

/// `PEB.ProcessParameters.CurrentDirectory.DosPath` of a 64-bit process.
#[cfg(target_pointer_width = "64")]
fn cwd(h: HANDLE) -> Option<String> {
    let mut wow = 0;
    // SAFETY: valid out pointer.
    if unsafe { IsWow64Process(h, &mut wow) } == 0 || wow != 0 {
        return None;
    }
    // SAFETY: zeroed is a valid PROCESS_BASIC_INFORMATION (pointers/ints).
    let mut pbi: PROCESS_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    let mut len = 0u32;
    // SAFETY: `pbi` is the size passed.
    let st = unsafe {
        NtQueryInformationProcess(
            h,
            ProcessBasicInformation,
            (&mut pbi as *mut PROCESS_BASIC_INFORMATION).cast(),
            std::mem::size_of::<PROCESS_BASIC_INFORMATION>() as u32,
            &mut len,
        )
    };
    if st < 0 || pbi.PebBaseAddress.is_null() {
        return None;
    }
    // x64 layout: PEB+0x20 = ProcessParameters; +0x38 = CurrentDirectory.DosPath.
    let params: usize = read(h, pbi.PebBaseAddress as usize + 0x20)?;
    let us: UNICODE_STRING = read(h, params.checked_add(0x38)?)?;
    let n = us.Length as usize / 2;
    if n == 0 || n > 32_768 || us.Buffer.is_null() {
        return None;
    }
    let mut text = vec![0u16; n];
    let mut got = 0usize;
    // SAFETY: `text` holds n u16s.
    let ok = unsafe {
        ReadProcessMemory(
            h,
            us.Buffer as *const _,
            text.as_mut_ptr().cast(),
            n * 2,
            &mut got,
        )
    };
    if ok == 0 || got != n * 2 {
        return None;
    }
    let s = String::from_utf16_lossy(&text);
    Some(if s.len() > 3 {
        s.trim_end_matches('\\').to_string()
    } else {
        s
    })
}

#[cfg(not(target_pointer_width = "64"))]
fn cwd(_h: HANDLE) -> Option<String> {
    None
}

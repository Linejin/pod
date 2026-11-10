//! Windows security and environment helpers for the daemon and holders.

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::io;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_DUPLICATE,
    TOKEN_IMPERSONATE, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_TERMINATE, TerminateProcess,
};

struct Token(HANDLE);

impl Drop for Token {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

fn process_token() -> io::Result<Token> {
    let mut h: HANDLE = std::ptr::null_mut();
    let ok = unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_IMPERSONATE,
            &mut h,
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(Token(h))
    }
}

/// Terminate a process by pid (exit code 1). Used for holders the daemon
/// cannot talk to; closing a holder closes its Job Object (kill on close).
pub fn terminate_process(pid: u32) -> io::Result<()> {
    if pid == 0 || pid == std::process::id() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "refusing pid"));
    }
    let h = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
    if h.is_null() {
        return Err(io::Error::last_os_error());
    }
    let ok = unsafe { TerminateProcess(h, 1) };
    let err = io::Error::last_os_error();
    unsafe { CloseHandle(h) };
    if ok == 0 { Err(err) } else { Ok(()) }
}

/// String SID of the user running this process (e.g. `S-1-5-21-...`).
pub fn current_user_sid() -> io::Result<String> {
    let token = process_token()?;
    unsafe {
        let mut len = 0u32;
        GetTokenInformation(token.0, TokenUser, std::ptr::null_mut(), 0, &mut len);
        let mut buf = vec![0u8; len as usize];
        if GetTokenInformation(
            token.0,
            TokenUser,
            buf.as_mut_ptr() as *mut c_void,
            len,
            &mut len,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s: *mut u16 = std::ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut s) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut n = 0;
        while *s.add(n) != 0 {
            n += 1;
        }
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(s, n));
        LocalFree(s as *mut c_void);
        Ok(sid)
    }
}

/// `SECURITY_ATTRIBUTES` whose DACL grants access to the current user only
/// (protected, so nothing is inherited; no Everyone/Anonymous read access).
pub struct UserOnly {
    sd: PSECURITY_DESCRIPTOR,
    sa: SECURITY_ATTRIBUTES,
}

// SAFETY: the descriptor is immutable after creation and freed once on drop.
unsafe impl Send for UserOnly {}
unsafe impl Sync for UserOnly {}

impl UserOnly {
    pub fn new() -> io::Result<Self> {
        let sddl = format!("D:P(A;;GA;;;{})", current_user_sid()?);
        let w: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                w.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd,
            bInheritHandle: 0,
        };
        Ok(Self { sd, sa })
    }

    /// Pointer for `ServerOptions::create_with_security_attributes_raw`.
    pub fn as_ptr(&self) -> *mut c_void {
        &self.sa as *const SECURITY_ATTRIBUTES as *mut c_void
    }
}

impl Drop for UserOnly {
    fn drop(&mut self) {
        unsafe { LocalFree(self.sd) };
    }
}

/// The user's environment as a fresh logon would see it (system + user
/// variables from the registry), *not* inherited from whatever launched us.
pub fn user_environment() -> io::Result<BTreeMap<String, String>> {
    let token = process_token()?;
    let mut block: *mut c_void = std::ptr::null_mut();
    if unsafe { CreateEnvironmentBlock(&mut block, token.0, 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut env = BTreeMap::new();
    unsafe {
        let mut p = block as *const u16;
        loop {
            let mut n = 0;
            while *p.add(n) != 0 {
                n += 1;
            }
            if n == 0 {
                break;
            }
            let entry = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
            // Skip the hidden "=C:=C:\..." per-drive entries (key starts with '=').
            if !entry.starts_with('=')
                && let Some((k, v)) = entry.split_once('=')
            {
                env.insert(k.to_string(), v.to_string());
            }
            p = p.add(n + 1);
        }
        DestroyEnvironmentBlock(block);
    }
    Ok(env)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sid_and_user_only_descriptor() {
        let sid = current_user_sid().unwrap();
        assert!(sid.starts_with("S-1-"), "{sid}");
        let _ = UserOnly::new().unwrap();
    }

    #[test]
    fn registry_environment_has_basics() {
        let env = user_environment().unwrap();
        assert!(crate::env::get_var(&env, "PATH").is_some());
        assert!(crate::env::get_var(&env, "SystemRoot").is_some());
        assert!(crate::env::get_var(&env, "CLAUDECODE").is_none());
    }
}

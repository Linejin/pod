//! Private generation files. Security is installed at creation, never after
//! exposing a file. Handles refuse reparse points and pin the directory.
use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::{
    CloseHandle, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACL, DACL_SECURITY_INFORMATION, EqualSid, GetAce,
    GetSecurityDescriptorControl, GetTokenInformation, OWNER_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CREATE_NEW, CreateDirectoryW, CreateFileW, FILE_ALL_ACCESS, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW, OPEN_EXISTING, READ_CONTROL,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "generation owner, ACL or reparse point",
    )
}
fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

struct Local(*mut c_void);
impl Drop for Local {
    fn drop(&mut self) {
        // SAFETY: allocation returned by an API documented to use LocalFree.
        unsafe {
            LocalFree(self.0);
        }
    }
}

struct Security {
    token_user: Vec<usize>,
    descriptor: Local,
}
impl Security {
    fn new() -> io::Result<Self> {
        let mut token = ptr::null_mut();
        // SAFETY: current process pseudo-handle and valid output pointer.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut size = 0;
        // SAFETY: size query. Token is closed on all paths below.
        unsafe {
            GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut size);
        }
        let mut token_user = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
        // SAFETY: aligned allocation of at least size bytes.
        let ok = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                token_user.as_mut_ptr().cast(),
                size,
                &mut size,
            )
        };
        let error = io::Error::last_os_error();
        // SAFETY: owned token handle.
        unsafe {
            CloseHandle(token);
        }
        if ok == 0 {
            return Err(error);
        }
        // SAFETY: GetTokenInformation returned TOKEN_USER in aligned storage.
        let sid = unsafe { (*token_user.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let mut text = ptr::null_mut();
        // SAFETY: live token SID and output string pointer.
        if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let text_mem = Local(text.cast());
        let mut len = 0;
        // SAFETY: API returns a NUL-terminated wide string.
        while unsafe { *text.add(len) } != 0 {
            len += 1;
        }
        // SAFETY: length measured inside the allocated string.
        let sid_text = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, len) });
        drop(text_mem);
        let sddl = super::wide(&format!("O:{sid_text}D:P(A;;FA;;;{sid_text})"));
        let mut descriptor = ptr::null_mut();
        // SAFETY: valid SDDL, local allocation returned and freed by Local.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            token_user,
            descriptor: Local(descriptor),
        })
    }
    fn attrs(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.descriptor.0,
            bInheritHandle: 0,
        }
    }
    fn check(&self, file: &File, directory: bool) -> io::Result<()> {
        let meta = file.metadata()?;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || meta.is_dir() != directory
            || (!directory && !meta.is_file())
        {
            return Err(invalid());
        }
        let mut owner: PSID = ptr::null_mut();
        let mut acl: *mut ACL = ptr::null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        // SAFETY: live file handle, all requested outputs are valid locals.
        let status = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                ptr::null_mut(),
                &mut acl,
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        let _descriptor = Local(descriptor);
        let mut control = 0;
        let mut revision = 0;
        // SAFETY: descriptor and SID pointers live through _descriptor/token_user.
        let sid = unsafe { (*self.token_user.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        if owner.is_null() || acl.is_null() {
            return Err(invalid());
        }
        // SAFETY: the returned descriptor/ACL/SIDs are valid for these APIs.
        unsafe {
            if GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) == 0
                || control & SE_DACL_PROTECTED == 0
                || EqualSid(owner, sid) == 0
                || (*acl).AceCount != 1
            {
                return Err(invalid());
            }
            let mut ace = ptr::null_mut();
            if GetAce(acl, 0, &mut ace) == 0 {
                return Err(invalid());
            }
            let ace = &*ace.cast::<ACCESS_ALLOWED_ACE>();
            // ACCESS_ALLOWED_ACE_TYPE = 0 (winnt.h). No inheritance/extra ACE.
            if ace.Header.AceType != 0
                || ace.Header.AceFlags != 0
                || ace.Mask != FILE_ALL_ACCESS
                || EqualSid(ptr::from_ref(&ace.SidStart).cast_mut().cast(), sid) == 0
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
}

fn open(path: &Path, create: bool, directory: bool, security: &Security) -> io::Result<File> {
    let name = wide(path);
    let access = READ_CONTROL | GENERIC_READ | if create { GENERIC_WRITE } else { 0 };
    // SAFETY: input pointers live for the call. Reparse points are opened as
    // links and then refused; no delete sharing pins files/directories.
    let share = FILE_SHARE_READ | if directory { FILE_SHARE_WRITE } else { 0 };
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            access,
            share,
            &security.attrs(),
            if create { CREATE_NEW } else { OPEN_EXISTING },
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        let e = io::Error::last_os_error();
        return Err(io::Error::new(
            e.kind(),
            format!("open generation object: {e}"),
        ));
    }
    // SAFETY: newly owned handle, transferred exactly once.
    let file = unsafe { File::from_raw_handle(handle) };
    security.check(&file, directory)?;
    Ok(file)
}

pub(crate) fn directory(path: &Path, create: bool) -> io::Result<File> {
    if path.as_os_str().encode_wide().any(|c| c == 0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "NUL in generation path",
        ));
    }
    // Refuse reparse points in existing ancestors as well as the leaf.
    for part in path.ancestors().filter(|p| !p.as_os_str().is_empty()) {
        match std::fs::symlink_metadata(part) {
            Ok(m) if m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 => {
                return Err(invalid());
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    let security = Security::new()?;
    if create && !path.try_exists()? {
        // SAFETY: security attributes contain a protected user-only DACL.
        if unsafe { CreateDirectoryW(wide(path).as_ptr(), &security.attrs()) } == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    open(path, false, true, &security)
}
pub(crate) fn read(path: &Path) -> io::Result<File> {
    open(path, false, false, &Security::new()?)
}
pub(crate) fn create(path: &Path) -> io::Result<File> {
    open(path, true, false, &Security::new()?)
}
pub(crate) fn replace(temp: &Path, target: &Path, _directory: &File) -> io::Result<()> {
    // SAFETY: same-directory paths, write-through atomic replacement.
    if unsafe {
        MoveFileExW(
            wide(temp).as_ptr(),
            wide(target).as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        let e = io::Error::last_os_error();
        return Err(io::Error::new(e.kind(), format!("replace generation: {e}")));
    }
    Ok(())
}

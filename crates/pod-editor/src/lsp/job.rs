//! Process-tree ownership for language servers.
//!
//! On Windows each server is assigned to its own Job Object with
//! `KILL_ON_JOB_CLOSE`: when the job handle closes (normal shutdown, crash
//! of Pod, or `terminate`) every process the server started — rustup proxy,
//! the real rust-analyzer, `cargo check`, proc-macro servers, node — dies
//! with it. Elsewhere this falls back to killing the direct child.

use std::process::{Child, Command};

/// Spawns `cmd` inside a new process tree. On Windows the process starts
/// suspended and only runs once it is in the job, so nothing it starts can
/// escape the job (a rustup proxy starting the real server right away).
/// `flags` are extra creation flags (e.g. `CREATE_NO_WINDOW`).
pub fn spawn_in_tree(cmd: &mut Command, flags: u32) -> std::io::Result<(Child, ProcessTree)> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(flags | windows_sys::Win32::System::Threading::CREATE_SUSPENDED);
        let mut child = cmd.spawn()?;
        let tree = ProcessTree::adopt(&child).and_then(|t| resume_threads(child.id()).map(|()| t));
        match tree {
            Ok(t) => Ok((child, t)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(e)
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = flags;
        let child = cmd.spawn()?;
        let tree = ProcessTree::adopt(&child)?;
        Ok((child, tree))
    }
}

/// Resumes the (single, suspended) main thread of process `pid`.
#[cfg(windows)]
fn resume_threads(pid: u32) -> std::io::Result<()> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};
    // SAFETY: plain Win32 calls on handles opened and closed here; the
    // THREADENTRY32 is zeroed with its size set as the API requires.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snap == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        let mut te: THREADENTRY32 = std::mem::zeroed();
        te.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut resumed = 0;
        let mut ok = Thread32First(snap, &mut te);
        while ok != 0 {
            if te.th32OwnerProcessID == pid {
                let t = OpenThread(THREAD_SUSPEND_RESUME, 0, te.th32ThreadID);
                if !t.is_null() {
                    if ResumeThread(t) != u32::MAX {
                        resumed += 1;
                    }
                    CloseHandle(t);
                }
            }
            ok = Thread32Next(snap, &mut te);
        }
        CloseHandle(snap);
        if resumed == 0 {
            return Err(std::io::Error::other("could not resume the new process"));
        }
        Ok(())
    }
}

#[cfg(windows)]
pub struct ProcessTree {
    job: windows_sys::Win32::Foundation::HANDLE,
}

// SAFETY: a job handle is a kernel object handle usable from any thread.
#[cfg(windows)]
unsafe impl Send for ProcessTree {}
#[cfg(windows)]
unsafe impl Sync for ProcessTree {}

#[cfg(windows)]
impl ProcessTree {
    /// Creates a kill-on-close job and puts `child` in it. Processes the
    /// child starts afterwards inherit the job.
    pub fn adopt(child: &Child) -> std::io::Result<Self> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::*;
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            let tree = ProcessTree { job };
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            if AssignProcessToJobObject(job, child.as_raw_handle() as _) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(tree)
        }
    }

    /// Kills every process in the tree now.
    pub fn terminate(&self) {
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job, 1);
        }
    }

    /// Number of live processes in the job (diagnostics/tests).
    pub fn active_processes(&self) -> u32 {
        use windows_sys::Win32::System::JobObjects::*;
        unsafe {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
            let ok = QueryInformationJobObject(
                self.job,
                JobObjectBasicAccountingInformation,
                &mut info as *mut _ as *mut core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                std::ptr::null_mut(),
            );
            if ok == 0 { 0 } else { info.ActiveProcesses }
        }
    }
}

#[cfg(windows)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.job);
        }
    }
}

#[cfg(not(windows))]
pub struct ProcessTree {
    pid: u32,
}

#[cfg(not(windows))]
impl ProcessTree {
    pub fn adopt(child: &Child) -> std::io::Result<Self> {
        Ok(ProcessTree { pid: child.id() })
    }
    pub fn terminate(&self) {
        let _ = std::process::Command::new("kill")
            .arg("-9")
            .arg(self.pid.to_string())
            .status();
    }
    pub fn active_processes(&self) -> u32 {
        0
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    /// A process started suspended runs once in the job, and what it
    /// starts right away is in the job too.
    #[test]
    fn spawn_in_tree_runs_inside_the_job() {
        let mut cmd = Command::new("cmd.exe");
        cmd.args(["/C", "ping -n 30 127.0.0.1 >NUL"])
            .stdout(Stdio::null());
        let (mut child, tree) = spawn_in_tree(&mut cmd, 0x0800_0000).unwrap();
        let t = std::time::Instant::now();
        while tree.active_processes() < 2 {
            assert!(
                t.elapsed().as_secs() < 5,
                "the process runs and its child is in the job"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        drop(tree);
        let t = std::time::Instant::now();
        while child.try_wait().unwrap().is_none() {
            assert!(t.elapsed().as_secs() < 5, "process tree not killed");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    #[test]
    fn closing_the_job_kills_grandchildren() {
        // cmd -> ping (grandchild). Dropping the job must kill both.
        let child = Command::new("cmd.exe")
            .args(["/C", "ping -n 30 127.0.0.1 >NUL"])
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let mut child = child;
        let tree = ProcessTree::adopt(&child).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert!(tree.active_processes() >= 2, "cmd + ping in the job");
        drop(tree);
        let t = std::time::Instant::now();
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            assert!(t.elapsed().as_secs() < 5, "process tree not killed");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

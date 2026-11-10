//! Measurements for the worker resource gate (docs/19 §5.1), behind a trait
//! so tests inject their own values.
//!
//! - [`WinMeter`]: system CPU averaged over a window of `GetSystemTimes`
//!   samples (the supervisor samples every tick), available physical memory
//!   (`GlobalMemoryStatusEx`), free space of the worktrees drive
//!   (`GetDiskFreeSpaceExW`, for the caller).
//! - [`FileMeter`]: reads `{cpu_pct?, free_mem?, free_disk?}` from a JSON
//!   file on every sample (`POD_ORCH_METER_FILE`; tests and diagnosis). A
//!   missing or unreadable file means "unknown", which never holds a worker.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use pod_orch::Sample;

/// Window of the CPU average.
pub const CPU_WINDOW: Duration = Duration::from_secs(60);

pub trait Meter: Send + Sync {
    /// CPU and memory now (CPU averaged over the meter's window).
    fn sample(&self) -> Sample;
    /// Free bytes available to this user on the drive of `path`.
    fn free_disk(&self, path: &Path) -> Option<u64>;
}

/// The meter of this daemon: `POD_ORCH_METER_FILE` if set, else Windows.
pub fn from_env() -> Box<dyn Meter> {
    match std::env::var_os("POD_ORCH_METER_FILE").filter(|v| !v.is_empty()) {
        Some(p) => Box::new(FileMeter(PathBuf::from(p))),
        None => Box::new(WinMeter::new(CPU_WINDOW)),
    }
}

pub struct FileMeter(pub PathBuf);

impl FileMeter {
    fn read(&self) -> serde_json::Value {
        std::fs::read(&self.0)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
}

impl Meter for FileMeter {
    fn sample(&self) -> Sample {
        let v = self.read();
        Sample {
            cpu_pct: v["cpu_pct"].as_f64(),
            free_mem: v["free_mem"].as_u64(),
        }
    }

    fn free_disk(&self, _path: &Path) -> Option<u64> {
        self.read()["free_disk"].as_u64()
    }
}

/// (when, idle ticks, total ticks)
type CpuTimes = (Instant, u64, u64);

pub struct WinMeter {
    window: Duration,
    samples: Mutex<VecDeque<CpuTimes>>,
}

impl WinMeter {
    pub fn new(window: Duration) -> Self {
        WinMeter {
            window,
            samples: Mutex::new(VecDeque::new()),
        }
    }

    /// Busy share between the oldest and newest sample in the window.
    fn cpu(&self) -> Option<f64> {
        let (idle, kernel, user) = system_times()?;
        let now = Instant::now();
        let mut s = self.samples.lock().unwrap();
        // Kernel time includes idle time.
        s.push_back((now, idle, kernel + user));
        // Keep one sample at or before the window start.
        while s.len() > 2
            && s.get(1)
                .is_some_and(|x| now.duration_since(x.0) >= self.window)
        {
            s.pop_front();
        }
        cpu_between(*s.front()?, *s.back()?)
    }
}

/// Busy percent between two `GetSystemTimes` samples.
fn cpu_between(old: CpuTimes, new: CpuTimes) -> Option<f64> {
    let total = new.2.checked_sub(old.2)?;
    let idle = new.1.checked_sub(old.1)?;
    if total == 0 {
        return None;
    }
    Some(100.0 * (total.saturating_sub(idle)) as f64 / total as f64)
}

impl Meter for WinMeter {
    fn sample(&self) -> Sample {
        Sample {
            cpu_pct: self.cpu(),
            free_mem: available_memory(),
        }
    }

    fn free_disk(&self, path: &Path) -> Option<u64> {
        free_disk_space(path)
    }
}

fn filetime(f: windows_sys::Win32::Foundation::FILETIME) -> u64 {
    (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime)
}

/// (idle, kernel, user) in 100 ns ticks since boot, all processors.
fn system_times() -> Option<(u64, u64, u64)> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetSystemTimes;
    let z = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut idle, mut kernel, mut user) = (z, z, z);
    // SAFETY: three valid out pointers.
    let ok = unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) };
    (ok != 0).then(|| (filetime(idle), filetime(kernel), filetime(user)))
}

fn available_memory() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: plain-data struct; zeroed is a valid value.
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: valid pointer with dwLength set.
    let ok = unsafe { GlobalMemoryStatusEx(&mut m) };
    (ok != 0).then_some(m.ullAvailPhys)
}

/// Free bytes for this user on the drive of `path` (its nearest existing
/// ancestor: the worktrees root may not exist yet).
pub fn free_disk_space(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let dir = path.ancestors().find(|p| p.is_dir())?;
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut avail = 0u64;
    // SAFETY: NUL-terminated path; out pointers valid or null.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut avail,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(avail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_share_between_samples() {
        let t = Instant::now();
        assert_eq!(cpu_between((t, 0, 0), (t, 50, 200)), Some(75.0));
        assert_eq!(
            cpu_between((t, 10, 100), (t, 10, 100)),
            None,
            "no time passed"
        );
        assert_eq!(
            cpu_between((t, 10, 100), (t, 5, 90)),
            None,
            "counter went back"
        );
    }

    #[test]
    fn windows_meter_measures_something() {
        let m = WinMeter::new(Duration::from_secs(60));
        let _ = m.sample();
        std::thread::sleep(Duration::from_millis(50));
        let s = m.sample();
        assert!(
            s.cpu_pct.is_some_and(|c| (0.0..=100.0).contains(&c)),
            "{s:?}"
        );
        assert!(s.free_mem.is_some_and(|b| b > 0), "{s:?}");
        let tmp = std::env::temp_dir()
            .join("pod-meter-missing")
            .join("deeper");
        assert!(
            m.free_disk(&tmp).is_some_and(|b| b > 0),
            "nearest existing ancestor"
        );
    }

    #[test]
    fn file_meter_reads_injected_values() {
        let p = std::env::temp_dir().join(format!("pod-meter-{}.json", std::process::id()));
        let m = FileMeter(p.clone());
        assert_eq!(m.sample(), Sample::default(), "missing file: unknown");
        std::fs::write(&p, r#"{"cpu_pct": 91.5, "free_mem": 1024, "free_disk": 7}"#).unwrap();
        assert_eq!(
            m.sample(),
            Sample {
                cpu_pct: Some(91.5),
                free_mem: Some(1024)
            }
        );
        assert_eq!(m.free_disk(Path::new("C:\\")), Some(7));
        let _ = std::fs::remove_file(&p);
    }
}

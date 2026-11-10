//! Native shell icons come from the same group-icon resource as Explorer.
use gpui_kit::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};
use windows_sys::Win32::{
    System::LibraryLoader::GetModuleHandleW,
    UI::HiDpi::GetDpiForWindow,
    UI::WindowsAndMessaging::{
        GCLP_HICON, GCLP_HICONSM, ICON_BIG, ICON_SMALL, IMAGE_ICON, LoadImageW, SendMessageW,
        SetClassLongPtrW, WM_SETICON,
    },
};

// Windows destroys these cached HICONs on process exit. Keep them alive while
// any class/window refers to them. LR_SHARED cannot distinguish requested sizes.
static ICONS: OnceLock<Mutex<HashMap<i32, usize>>> = OnceLock::new();

pub fn install(window: &Window) {
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    let hwnd = handle.hwnd.get() as *mut core::ffi::c_void;
    let mut icons = ICONS.get_or_init(Default::default).lock().unwrap();
    // The borrowed HWND belongs to this live GPUI window. Resource 1 is the
    // executable's ICO group; each cached handle belongs to this process.
    unsafe {
        let dpi = GetDpiForWindow(hwnd).max(96);
        let module = GetModuleHandleW(std::ptr::null());
        for (size, kind, class_slot) in [(32, ICON_BIG, GCLP_HICON), (16, ICON_SMALL, GCLP_HICONSM)]
        {
            let size = (size * dpi / 96) as i32;
            let icon = *icons.entry(size).or_insert_with(|| {
                LoadImageW(
                    module,
                    std::ptr::without_provenance(1),
                    IMAGE_ICON,
                    size,
                    size,
                    0,
                ) as usize
            });
            if icon != 0 {
                SetClassLongPtrW(hwnd, class_slot, icon as isize);
                SendMessageW(hwnd, WM_SETICON, kind as usize, icon as isize);
            }
        }
    }
}

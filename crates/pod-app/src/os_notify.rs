//! Windows toast notifications for mod notices (docs/16 §8.2).
//!
//! A mod's `pod.notify` always raises the in-app toast. When the Pod window
//! is not in front (inactive or minimized) and the "OS 알림" setting is on,
//! it also posts a Windows toast; clicking it brings Pod forward and shows
//! the notice's terminal or task.
//!
//! Toasts need a registered AppUserModelID: the installer writes
//! `HKCU\Software\Classes\AppUserModelId\Pod.App` (and puts [`AUMID`] on
//! its shortcut). A portable build has none. The app never registers one
//! itself: without the key, or when the user turned Pod's notifications off
//! in Windows, or when anything fails, it logs once and keeps to the in-app
//! toast.

use std::cell::Cell;
use std::collections::VecDeque;

/// Pod's AppUserModelID (also set on the installer's shortcut, tools/installer/pod.iss).
pub const AUMID: &str = "Pod.App";

/// `POD_TOAST_AUMID` overrides the id (the QA script registers a temporary one).
pub fn aumid() -> String {
    std::env::var("POD_TOAST_AUMID")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| AUMID.to_string())
}

/// Should a notice also go to the OS? Only when the user is not looking at
/// Pod: the window is inactive or minimized. The in-app toast is raised either way.
pub fn should_post(setting_on: bool, window_active: bool, minimized: bool) -> bool {
    setting_on && (!window_active || minimized)
}

/// Can a toast be shown? `registered`: the AUMID key exists. `setting`:
/// `ToastNotifier.Setting` (0 = enabled); Windows only answers it after the
/// id showed its first toast, so `None` (not readable yet) still means yes.
pub fn availability(registered: bool, setting: Option<i32>) -> Result<(), String> {
    if !registered {
        return Err("AppUserModelID가 등록되지 않음 (설치하지 않은 빌드)".into());
    }
    match setting {
        None | Some(0) => Ok(()),
        Some(1) => Err("Windows 설정에서 Pod 알림이 꺼짐".into()),
        Some(2) => Err("Windows 알림이 꺼짐".into()),
        Some(3) => Err("그룹 정책이 알림을 막음".into()),
        Some(n) => Err(format!("알림을 쓸 수 없음 (설정 {n})")),
    }
}

/// Is `Software\Classes\AppUserModelId\<id>` present (HKCU or HKLM)?
#[cfg(windows)]
fn registered(id: &str) -> bool {
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegOpenKeyExW,
    };
    let path: Vec<u16> = format!(r"Software\Classes\AppUserModelId\{id}")
        .encode_utf16()
        .chain([0])
        .collect();
    [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE]
        .into_iter()
        .any(|root| {
            let mut key: HKEY = std::ptr::null_mut();
            // SAFETY: `path` is NUL-terminated and outlives the call; `key` is closed below.
            let ok = unsafe { RegOpenKeyExW(root, path.as_ptr(), 0, KEY_READ, &mut key) } == 0;
            if ok {
                // SAFETY: `key` was opened above.
                unsafe { RegCloseKey(key) };
            }
            ok
        })
}

/// What a click on the toast should show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Target {
    pub task: Option<String>,
    pub terminal: Option<String>,
}

/// `&`, `<`, `>`, `"`, `'` escaped; control characters other than tab and
/// newline dropped (XML 1.0 forbids them).
pub fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' => out.push(c),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}

/// The toast document: title, body and the mod as attribution line.
pub fn toast_xml(title: &str, body: &str, attribution: &str) -> String {
    let mut x = String::from(r#"<toast><visual><binding template="ToastGeneric">"#);
    x.push_str(&format!("<text>{}</text>", xml_escape(title)));
    if !body.trim().is_empty() {
        x.push_str(&format!("<text>{}</text>", xml_escape(body)));
    }
    if !attribution.trim().is_empty() {
        x.push_str(&format!(
            r#"<text placement="attribution">{}</text>"#,
            xml_escape(attribution)
        ));
    }
    x.push_str("</binding></visual></toast>");
    x
}

/// Lazily created notifier; once toasts fail, the app stays on in-app toasts
/// for the session (logged once).
pub struct OsToasts {
    #[cfg(windows)]
    notifier: Option<windows::UI::Notifications::ToastNotifier>,
    unavailable: Cell<bool>,
    /// Shown toasts are kept alive so their click handler still runs.
    #[cfg(windows)]
    shown: VecDeque<windows::UI::Notifications::ToastNotification>,
    #[cfg(not(windows))]
    shown: VecDeque<()>,
    clicks: futures::channel::mpsc::UnboundedSender<Target>,
}

impl OsToasts {
    pub fn new(clicks: futures::channel::mpsc::UnboundedSender<Target>) -> Self {
        OsToasts {
            #[cfg(windows)]
            notifier: None,
            unavailable: Cell::new(false),
            shown: VecDeque::new(),
            clicks,
        }
    }

    /// Post a toast. Errors are logged once; afterwards it is a no-op.
    pub fn post(&mut self, title: &str, body: &str, attribution: &str, target: Target) {
        if self.unavailable.get() {
            return;
        }
        let id = aumid();
        if let Err(e) = self.try_post(&id, title, body, attribution, target) {
            self.unavailable.set(true);
            println!("pod-app: OS 알림을 쓸 수 없어 앱 안 알림만 띄웁니다 ({id}): {e}");
        }
    }

    #[cfg(windows)]
    fn try_post(
        &mut self,
        id: &str,
        title: &str,
        body: &str,
        attribution: &str,
        target: Target,
    ) -> Result<(), String> {
        use windows::Data::Xml::Dom::XmlDocument;
        use windows::Foundation::TypedEventHandler;
        use windows::UI::Notifications::{
            NotificationSetting, ToastNotification, ToastNotificationManager,
        };
        use windows::core::{HSTRING, IInspectable};

        if self.notifier.is_none() {
            availability(registered(id), None)?;
            println!("pod-app: 모드 알림을 Windows 알림으로도 띄웁니다 ({id})");
            let n = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(id))
                .map_err(|e| format!("notifier: {e}"))?;
            self.notifier = Some(n);
        }
        let notifier = self.notifier.as_ref().expect("set above");
        // The user can turn Pod's toasts off in Windows at any time.
        availability(
            true,
            notifier.Setting().ok().map(|s: NotificationSetting| s.0),
        )?;
        let doc = XmlDocument::new().map_err(|e| e.to_string())?;
        doc.LoadXml(&HSTRING::from(toast_xml(title, body, attribution)))
            .map_err(|e| format!("xml: {e}"))?;
        let toast = ToastNotification::CreateToastNotification(&doc).map_err(|e| e.to_string())?;
        let clicks = self.clicks.clone();
        toast
            .Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(
                move |_, _| {
                    let _ = clicks.unbounded_send(target.clone());
                    Ok(())
                },
            ))
            .map_err(|e| e.to_string())?;
        notifier.Show(&toast).map_err(|e| format!("show: {e}"))?;
        if self.shown.len() >= 20 {
            self.shown.pop_front();
        }
        self.shown.push_back(toast);
        Ok(())
    }

    #[cfg(not(windows))]
    fn try_post(&mut self, _: &str, _: &str, _: &str, _: &str, _: Target) -> Result<(), String> {
        let _ = &self.clicks;
        Err("not supported on this platform".into())
    }
}

/// Is the window minimized? (`IsIconic` on its HWND.)
pub fn minimized(window: &gpui_kit::gpui::Window) -> bool {
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return false;
        };
        let RawWindowHandle::Win32(h) = handle.as_raw() else {
            return false;
        };
        // SAFETY: the HWND belongs to this live window; IsIconic only reads it.
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsIconic(h.hwnd.get() as _) != 0 }
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posts_only_when_the_user_is_not_looking_at_pod() {
        // (setting, active, minimized) -> post
        assert!(
            !should_post(true, true, false),
            "focused, visible: in-app toast only"
        );
        assert!(should_post(true, false, false), "another app in front");
        assert!(should_post(true, false, true), "minimized");
        assert!(
            should_post(true, true, true),
            "minimized but still the active window"
        );
        for (a, m) in [(true, false), (false, false), (false, true), (true, true)] {
            assert!(!should_post(false, a, m), "setting off: never");
        }
    }

    #[test]
    fn toast_xml_escapes_and_keeps_korean() {
        let x = toast_xml(
            "에이전트가 <답>을 기다립니다",
            "\"repo/a\" & 'b'\u{7}",
            "그룹 완료 알림",
        );
        assert_eq!(
            x,
            "<toast><visual><binding template=\"ToastGeneric\"><text>에이전트가 &lt;답&gt;을 기다립니다</text>\
             <text>&quot;repo/a&quot; &amp; &apos;b&apos;</text><text placement=\"attribution\">그룹 완료 알림</text></binding></visual></toast>"
        );
        assert!(
            !toast_xml("제목", "  ", "").contains("<text></text>"),
            "empty body and attribution are left out"
        );
    }

    #[test]
    fn availability_needs_a_registered_id_and_windows_consent() {
        assert!(
            availability(false, None).is_err(),
            "portable build: no AUMID key"
        );
        assert!(availability(false, Some(0)).is_err());
        assert_eq!(
            availability(true, None),
            Ok(()),
            "registered, no toast shown yet: Setting not readable"
        );
        assert_eq!(availability(true, Some(0)), Ok(()));
        for off in 1..=4 {
            let e = availability(true, Some(off)).unwrap_err();
            assert!(
                e.chars().any(|c| ('가'..='힣').contains(&c)),
                "Korean reason: {e}"
            );
        }
    }

    /// The real check against an id nobody registered: falls back without
    /// touching WinRT (so no toast can appear) and stays quiet afterwards.
    #[cfg(windows)]
    #[test]
    fn unregistered_id_falls_back_silently() {
        const ID: &str = "Pod.Test.Unregistered.7f3c";
        assert!(!registered(ID));
        let (tx, _rx) = futures::channel::mpsc::unbounded();
        let mut t = OsToasts::new(tx);
        let e = t
            .try_post(ID, "제목", "본문", "모드", Target::default())
            .unwrap_err();
        assert!(e.contains("AppUserModelID"), "{e}");
        assert!(
            t.notifier.is_none() && t.shown.is_empty(),
            "no WinRT notifier, nothing shown"
        );
        t.unavailable.set(true);
        t.post("제목", "본문", "모드", Target::default()); // stays quiet once unavailable
        assert!(t.shown.is_empty());
    }
}

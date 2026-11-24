use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use windows::{
    Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
    core::w,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW,
    TranslateMessage, WS_OVERLAPPED,
};

use super::{Browser, CoTaskMemPWSTR, Event, HWND, PWSTR, RECT, start_on_parent};

struct Host(HWND);

impl Host {
    fn new() -> Self {
        // SAFETY: the test owns this STA; STATIC is a system class and all
        // strings and null optional arguments meet CreateWindowExW's contract.
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
            let hwnd = CreateWindowExW(
                0,
                w!("STATIC").0,
                w!("Pod browser QA").0,
                WS_OVERLAPPED,
                0,
                0,
                640,
                480,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            if hwnd.is_null() {
                CoUninitialize();
                panic!(
                    "CreateWindowExW failed: {}",
                    std::io::Error::last_os_error()
                );
            }
            Self(HWND(hwnd))
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        // SAFETY: the browser was dropped before this test-owned window/STA.
        unsafe {
            DestroyWindow(self.0.0);
            CoUninitialize();
        }
    }
}

fn wait_for(rx: &mut UnboundedReceiver<Event>, matches: impl Fn(&Event) -> bool) -> Event {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        // SAFETY: pump only this test thread's queue, with initialized storage
        // for each message, so WebView2 can deliver its completion callbacks.
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        while let Ok(event) = rx.try_recv() {
            if matches(&event) {
                return event;
            }
            match event {
                Event::StartupFailed(error) => panic!("WebView2 startup failed: {error}"),
                Event::NavigationFailed => panic!("WebView2 loopback navigation failed"),
                Event::BrowserFailed => panic!("WebView2 process failed"),
                _ => {}
            }
        }
        assert!(Instant::now() < deadline, "WebView2 callback timed out");
        thread::sleep(Duration::from_millis(10));
    }
}

fn title(browser: &Browser) -> String {
    let mut title = PWSTR::null();
    // SAFETY: the test owns the configured controller on this STA.
    unsafe { browser.view().DocumentTitle(&mut title).unwrap() };
    CoTaskMemPWSTR::from(title).to_string()
}

#[test]
#[ignore = "requires the installed Windows WebView2 runtime"]
fn webview2_navigates_loopback_pages_and_reloads() {
    let host = Host::new();
    let profile = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let done = AtomicBool::new(false);
    thread::scope(|scope| {
        let serve = scope.spawn(|| {
            while !done.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut input = BufReader::new(&mut stream);
                let mut request = String::new();
                if !matches!(input.read_line(&mut request), Ok(n) if n > 0)
                    || !input.lines().take(64).any(|line| line.is_ok_and(|line| line.is_empty()))
                {
                    continue;
                }
                let title = if request.starts_with("GET /two ") { "Page Two" } else { "Page One" };
                let body = format!("<!doctype html><title>{title}</title><h1>{title}</h1>");
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len(),
                );
            }
        });
        let result = std::panic::catch_unwind(|| {
            let (tx, mut rx) = unbounded();
            start_on_parent(host.0, profile.path(), &tx).unwrap();
            let Event::Ready(mut browser) =
                wait_for(&mut rx, |event| matches!(event, Event::Ready(_)))
            else {
                unreachable!();
            };
            let rect = RECT {
                left: 12,
                top: 20,
                right: 620,
                bottom: 440,
            };
            browser.set_viewport(Some(rect)).unwrap();
            for (path, expected) in [("one", "Page One"), ("two", "Page Two")] {
                browser
                    .navigate(&format!("http://{address}/{path}"))
                    .unwrap();
                wait_for(&mut rx, |event| matches!(event, Event::NavigationSucceeded));
                assert_eq!(title(&browser), expected);
            }
            browser.back().unwrap();
            wait_for(&mut rx, |event| matches!(event, Event::NavigationSucceeded));
            assert_eq!(title(&browser), "Page One");
            browser.forward().unwrap();
            wait_for(&mut rx, |event| matches!(event, Event::NavigationSucceeded));
            assert_eq!(title(&browser), "Page Two");
            browser.reload().unwrap();
            wait_for(&mut rx, |event| matches!(event, Event::NavigationSucceeded));
            assert_eq!(title(&browser), "Page Two");
            browser.set_viewport(None).unwrap();
        });
        done.store(true, Ordering::Relaxed);
        serve.join().unwrap();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    });
}

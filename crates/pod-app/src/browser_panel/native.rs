//! WebView2 COM calls stay on GPUI's Windows UI thread. Callbacks enqueue
//! messages, so they never re-enter an in-progress GPUI update.

use std::{os::windows::ffi::OsStrExt, path::Path};

use anyhow::{Context as _, Result};
use futures::channel::mpsc::UnboundedSender;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use webview2_com::{Microsoft::Web::WebView2::Win32::*, *};
use windows::{
    Win32::{
        Foundation::{HWND, RECT},
        UI::Input::KeyboardAndMouse::{GetKeyState, SetFocus, VK_CONTROL},
    },
    core::{Interface, PCWSTR, PWSTR},
};

use super::http_address;

pub(super) enum Event {
    Ready(Browser),
    StartupFailed(String),
    Address(String),
    History { back: bool, forward: bool },
    NavigationFailed,
    NavigationSucceeded,
    ReturnFocus,
    BrowserFailed,
}

pub(super) struct Browser {
    parent: HWND,
    controller: ICoreWebView2Controller,
    webview: Option<ICoreWebView2>,
    viewport: Option<RECT>,
}

pub(super) fn start(
    window: &gpui_kit::Window,
    profile: &Path,
    events: &UnboundedSender<Event>,
) -> Result<()> {
    let handle = HasWindowHandle::window_handle(window)
        .map_err(|error| anyhow::anyhow!("Windows 창 핸들이 없습니다. ({error})"))?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        anyhow::bail!("Windows 창에서 브라우저를 열 수 있습니다.");
    };
    start_on_parent(HWND(handle.hwnd.get() as *mut _), profile, events)
}

fn start_on_parent(parent: HWND, profile: &Path, events: &UnboundedSender<Event>) -> Result<()> {
    let profile: Vec<u16> = profile.as_os_str().encode_wide().chain([0]).collect();
    let events = events.clone();
    let handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
        move |result, environment| {
            if !events.is_closed() {
                let start = result.map_err(anyhow::Error::from).and_then(|()| {
                    let environment = environment.context("WebView2 환경을 받지 못했습니다.")?;
                    start_controller(parent, &environment, &events)
                });
                if let Err(error) = start {
                    let _ = events.unbounded_send(Event::StartupFailed(error.to_string()));
                }
            }
            Ok(())
        },
    ));
    // SAFETY: GPUI initialized STA on this thread. The UTF-16 string and
    // interface are valid during the call. WebView2 retains the callback.
    unsafe {
        CreateCoreWebView2EnvironmentWithOptions(
            PCWSTR::null(),
            PCWSTR(profile.as_ptr()),
            None::<&ICoreWebView2EnvironmentOptions>,
            &handler,
        )?
    };
    Ok(())
}

fn start_controller(
    parent: HWND,
    environment: &ICoreWebView2Environment,
    events: &UnboundedSender<Event>,
) -> Result<()> {
    let events = events.clone();
    let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
        move |result, controller| {
            let result = result.map_err(anyhow::Error::from).and_then(|()| {
                let controller = controller.context("WebView2 컨트롤러를 받지 못했습니다.")?;
                let mut browser = Browser {
                    parent,
                    controller,
                    webview: None,
                    viewport: None,
                };
                if events.is_closed() {
                    return Ok(None);
                }
                browser.configure(&events)?;
                Ok(Some(browser))
            });
            match result {
                Ok(Some(browser)) => {
                    let _ = events.unbounded_send(Event::Ready(browser));
                }
                Err(error) => {
                    let _ = events.unbounded_send(Event::StartupFailed(error.to_string()));
                }
                Ok(None) => {}
            }
            Ok(())
        },
    ));
    // SAFETY: the browser panel is still listening on the live UI thread, and
    // the HWND belongs to the GPUI window that opened it.
    unsafe { environment.CreateCoreWebView2Controller(parent, &handler)? };
    Ok(())
}

impl Browser {
    fn configure(&mut self, events: &UnboundedSender<Event>) -> Result<()> {
        let mut token = 0;
        // SAFETY: all interfaces are obtained from this live controller on
        // its owning STA. The controller owns these event registrations.
        unsafe {
            self.controller.SetIsVisible(false)?;
            let webview = self.controller.CoreWebView2()?;
            let settings = webview.Settings()?;
            settings.SetAreHostObjectsAllowed(false)?;
            settings.SetIsWebMessageEnabled(false)?;
            settings.SetAreDefaultScriptDialogsEnabled(false)?;

            webview.add_NavigationStarting(
                &NavigationStartingEventHandler::create(Box::new(|_, args| {
                    if let Some(args) = args {
                        let mut uri = PWSTR::null();
                        args.Uri(&mut uri)?;
                        let uri = CoTaskMemPWSTR::from(uri);
                        if http_address(&uri.to_string()).is_none() {
                            args.SetCancel(true)?;
                        }
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            webview.add_NewWindowRequested(
                &NewWindowRequestedEventHandler::create(Box::new(|_, args| {
                    if let Some(args) = args {
                        args.SetHandled(true)?;
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            webview.cast::<ICoreWebView2_4>()?.add_DownloadStarting(
                &DownloadStartingEventHandler::create(Box::new(|_, args| {
                    if let Some(args) = args {
                        args.SetCancel(true)?;
                        args.SetHandled(true)?;
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            webview
                .cast::<ICoreWebView2_18>()?
                .add_LaunchingExternalUriScheme(
                    &LaunchingExternalUriSchemeEventHandler::create(Box::new(|_, args| {
                        if let Some(args) = args {
                            args.SetCancel(true)?;
                        }
                        Ok(())
                    })),
                    &mut token,
                )?;
            let tx = events.clone();
            webview.add_SourceChanged(
                &SourceChangedEventHandler::create(Box::new(move |webview, _| {
                    if let Some(webview) = webview {
                        let mut uri = PWSTR::null();
                        webview.Source(&mut uri)?;
                        let uri = CoTaskMemPWSTR::from(uri);
                        let _ = tx.unbounded_send(Event::Address(uri.to_string()));
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            let tx = events.clone();
            webview.add_HistoryChanged(
                &HistoryChangedEventHandler::create(Box::new(move |webview, _| {
                    if let Some(webview) = webview {
                        let (mut back, mut forward) = Default::default();
                        webview.CanGoBack(&mut back)?;
                        webview.CanGoForward(&mut forward)?;
                        let _ = tx.unbounded_send(Event::History {
                            back: back.as_bool(),
                            forward: forward.as_bool(),
                        });
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            let tx = events.clone();
            webview.add_NavigationCompleted(
                &NavigationCompletedEventHandler::create(Box::new(move |_, args| {
                    if let Some(args) = args {
                        let mut success = Default::default();
                        args.IsSuccess(&mut success)?;
                        if success.as_bool() {
                            let _ = tx.unbounded_send(Event::NavigationSucceeded);
                        } else {
                            let mut status = Default::default();
                            args.WebErrorStatus(&mut status)?;
                            if status != COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED {
                                let _ = tx.unbounded_send(Event::NavigationFailed);
                            }
                        }
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            let tx = events.clone();
            webview.add_ProcessFailed(
                &ProcessFailedEventHandler::create(Box::new(move |_, args| {
                    if let Some(args) = args {
                        let mut kind = Default::default();
                        args.ProcessFailedKind(&mut kind)?;
                        if matches!(
                            kind,
                            COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED
                                | COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED
                        ) {
                            let _ = tx.unbounded_send(Event::BrowserFailed);
                        }
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            let tx = events.clone();
            self.controller.add_MoveFocusRequested(
                &MoveFocusRequestedEventHandler::create(Box::new(move |_, args| {
                    if let Some(args) = args {
                        args.SetHandled(true)?;
                        let _ = tx.unbounded_send(Event::ReturnFocus);
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            let tx = events.clone();
            self.controller.add_AcceleratorKeyPressed(
                &AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
                    if let Some(args) = args {
                        let mut key = 0;
                        args.VirtualKey(&mut key)?;
                        if key == u32::from(b'L') && GetKeyState(i32::from(VK_CONTROL.0)) < 0 {
                            let mut kind = Default::default();
                            args.KeyEventKind(&mut kind)?;
                            args.SetHandled(true)?;
                            if kind == COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN {
                                let _ = tx.unbounded_send(Event::ReturnFocus);
                            }
                        }
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            self.webview = Some(webview);
        }
        Ok(())
    }

    fn view(&self) -> &ICoreWebView2 {
        self.webview.as_ref().expect("configured WebView2")
    }

    pub(super) fn navigate(&self, address: &str) -> windows::core::Result<()> {
        let address: Vec<u16> = address.encode_utf16().chain([0]).collect();
        // SAFETY: the normalized address is NUL-terminated and valid for this call.
        unsafe { self.view().Navigate(PCWSTR(address.as_ptr())) }
    }

    pub(super) fn back(&self) -> windows::core::Result<()> {
        // SAFETY: the owning controller stays alive on the UI thread.
        unsafe { self.view().GoBack() }
    }

    pub(super) fn forward(&self) -> windows::core::Result<()> {
        // SAFETY: the owning controller stays alive on the UI thread.
        unsafe { self.view().GoForward() }
    }

    pub(super) fn reload(&self) -> windows::core::Result<()> {
        // SAFETY: the owning controller stays alive on the UI thread.
        unsafe { self.view().Reload() }
    }

    pub(super) fn focus_page(&self) -> windows::core::Result<()> {
        // SAFETY: this is the UI thread that owns the controller.
        unsafe {
            self.controller
                .MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC)
        }
    }

    pub(super) fn focus_parent(&self) {
        // SAFETY: the panel and its live parent run on this UI thread.
        let _ = unsafe { SetFocus(Some(self.parent)) };
    }

    pub(super) fn parent_moved(&self) {
        // SAFETY: the controller was created on this UI thread.
        let _ = unsafe { self.controller.NotifyParentWindowPositionChanged() };
    }

    pub(super) fn set_viewport(&mut self, bounds: Option<RECT>) -> windows::core::Result<()> {
        if bounds == self.viewport {
            return Ok(());
        }
        // SAFETY: the controller is live on this UI thread. Bounds are
        // expressed in physical pixels in the parent window's client area.
        unsafe {
            if let Some(bounds) = bounds {
                self.controller.SetBounds(bounds)?;
            }
            if bounds.is_some() != self.viewport.is_some() {
                self.controller.SetIsVisible(bounds.is_some())?;
            }
        }
        self.viewport = bounds;
        Ok(())
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        // SAFETY: all Browser values are owned and dropped on the GPUI thread.
        let _ = unsafe { self.controller.Close() };
    }
}

#[cfg(test)]
#[path = "native/tests.rs"]
mod tests;

//! Core merge orchestration — the eight-step sequence from the design doc, §5.
//!
//! Called from the IDispatch sink on the STA message-loop thread. Synchronous; failures
//! degrade gracefully (the user's window is preserved).

use std::thread::sleep;
use std::time::{Duration, Instant};

use windows::core::{Interface, Result as WinResult, BSTR, VARIANT};
use windows::Win32::Foundation::{E_FAIL, HWND};
use windows::Win32::System::Com::IDispatch;
use windows::Win32::UI::Shell::{IShellWindows, IWebBrowser2};

use crate::log;
use crate::win_util;

const WAIT_NEW_TAB_TIMEOUT_MS: u64 = 2_000;
const WAIT_NEW_TAB_POLL_MS: u64 = 25;

/// Entry point called by the IDispatch sink on each new Explorer window.
pub fn on_new_window(shell_windows: &IShellWindows, new_window: IDispatch) {
    if let Err(e) = try_merge(shell_windows, &new_window) {
        log::write(&format!("merge failed: {:?}", e));
    }
}

fn try_merge(shell_windows: &IShellWindows, new_window: &IDispatch) -> WinResult<()> {
    // The IDispatch we're handed represents the newly-registered shell window. Cast to
    // IWebBrowser2 to access its HWND and target URL.
    let new_wb: IWebBrowser2 = new_window.cast()?;

    let new_hwnd_raw = unsafe { new_wb.HWND()?.0 };
    let new_hwnd = HWND(new_hwnd_raw as *mut std::ffi::c_void);

    if !win_util::is_explorer(new_hwnd) {
        // Could be IE-derived, Control Panel, etc. Leave it alone.
        return Ok(());
    }

    let host = match win_util::select_host(new_hwnd) {
        Some(h) => h,
        None => return Ok(()), // no host: new window lives on its own
    };

    let tabs_before: Vec<HWND> = win_util::find_tab_handles(host);

    win_util::request_new_tab(host)?;

    let new_tab = match wait_for_new_tab(host, &tabs_before) {
        Some(h) => h,
        None => {
            return Err(windows::core::Error::new(
                E_FAIL,
                "timeout waiting for new tab",
            ));
        }
    };

    // Read target URL from the original window — must do this before Quit().
    let location_bstr: BSTR = unsafe { new_wb.LocationURL()? };

    let new_tab_wb = match find_wb_for_tab(shell_windows, new_tab) {
        Some(wb) => wb,
        None => {
            return Err(windows::core::Error::new(
                E_FAIL,
                "could not locate new tab IWebBrowser2",
            ));
        }
    };

    // Navigate the freshly-created tab to the URL. Navigate2 takes one VARIANT for URL plus
    // four optional VARIANTs (flags/target/postdata/headers); we only set the URL.
    unsafe {
        let url_var = VARIANT::from(location_bstr);
        let empty = VARIANT::new();
        new_tab_wb.Navigate2(&url_var, &empty, &empty, &empty, &empty)?;
    }

    // Dispose of the original spawned window. Quit() is the COM-clean route.
    unsafe {
        let _ = new_wb.Quit();
    }

    win_util::bring_to_foreground(host);
    Ok(())
}

fn wait_for_new_tab(host: HWND, before: &[HWND]) -> Option<HWND> {
    let deadline = Instant::now() + Duration::from_millis(WAIT_NEW_TAB_TIMEOUT_MS);
    while Instant::now() < deadline {
        let now = win_util::find_tab_handles(host);
        for t in &now {
            if !before.iter().any(|b| b.0 == t.0) {
                return Some(*t);
            }
        }
        sleep(Duration::from_millis(WAIT_NEW_TAB_POLL_MS));
    }
    None
}

/// Walk the IShellWindows collection and find the IWebBrowser2 whose HWND matches `tab_hwnd`.
/// Returns None if no match (race: tab vanished, or Explorer is mid-update).
fn find_wb_for_tab(shell_windows: &IShellWindows, tab_hwnd: HWND) -> Option<IWebBrowser2> {
    let count = unsafe { shell_windows.Count().ok()? };
    for i in 0..count {
        let idx_var = VARIANT::from(i);
        let disp = match unsafe { shell_windows.Item(&idx_var) } {
            Ok(d) => d,
            Err(_) => continue,
        };
        let wb: IWebBrowser2 = match disp.cast() {
            Ok(w) => w,
            Err(_) => continue,
        };
        let h_raw = match unsafe { wb.HWND() } {
            Ok(h) => h.0,
            Err(_) => continue,
        };
        let hwnd = HWND(h_raw as *mut std::ffi::c_void);
        if hwnd.0 == tab_hwnd.0 {
            return Some(wb);
        }
    }
    None
}

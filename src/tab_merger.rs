//! Core merge orchestration.
//!
//! Strategy: instead of the WM_COMMAND-then-poll dance, ask the host's own IWebBrowser2
//! to navigate with the `navOpenInNewTab` flag (BrowserNavConstants = 0x800). Explorer
//! routes the navigation into a fresh tab in that host in a single COM round-trip, so
//! no waiting on IShellWindows to register the new tab.
//!
//! Called from the WinEvent hook on the STA message-loop thread. Synchronous; failures
//! degrade gracefully (the user's window is preserved).

use std::thread::sleep;
use std::time::{Duration, Instant};

use windows::core::{Interface, Result as WinResult, BSTR, VARIANT};
use windows::Win32::Foundation::{E_FAIL, HWND};
use windows::Win32::UI::Shell::{IShellWindows, IWebBrowser2};

use crate::log;
use crate::win_util;

/// `BrowserNavConstants::navOpenInNewTab` — tells IWebBrowser2 to route the navigation
/// into a new tab in the same top-level window rather than reusing the current one.
const NAV_OPEN_IN_NEW_TAB: i32 = 0x800;

/// Max wait for the new top-level's IWebBrowser2 to appear in IShellWindows (so we can
/// read its `LocationURL`). The new top is freshly registered, so this almost always
/// resolves within a single poll.
const WAIT_WB_TIMEOUT_MS: u64 = 5_000;
const WAIT_WB_POLL_MS: u64 = 5;

/// Called on `EVENT_OBJECT_CREATE`. Fires before the window's first paint, so cloaking
/// here actually prevents the visible flash.
pub fn on_window_created(hwnd: HWND) {
    if let Some(class) = win_util::get_window_class(hwnd) {
        if class == win_util::CABINET_WCLASS {
            win_util::cloak(hwnd);
        }
    }
}

/// Called on `EVENT_OBJECT_SHOW`.
pub fn on_window_shown(shell_windows: &IShellWindows, hwnd: HWND) {
    let class = match win_util::get_window_class(hwnd) {
        Some(c) => c,
        None => return,
    };
    if class != win_util::CABINET_WCLASS {
        return; // very frequent path
    }

    // Multi-tab → existing window being re-shown (un-minimised, focus change, etc.).
    // Defensive uncloak in case some path cloaked it.
    if win_util::find_tab_handles(hwnd).len() > 1 {
        win_util::uncloak(hwnd);
        return;
    }

    // First Explorer window of the session — no host to merge into.
    let host = match win_util::select_host(hwnd) {
        Some(h) => h,
        None => {
            win_util::uncloak(hwnd);
            return;
        }
    };

    if let Err(e) = try_merge(shell_windows, hwnd, host) {
        log::write(&format!("merge failed for {:?}: {:?}", hwnd.0, e));
        win_util::uncloak(hwnd);
    }
}

fn try_merge(shell_windows: &IShellWindows, new_top: HWND, host: HWND) -> WinResult<()> {
    // Need the new top-level's IWebBrowser2 to read its URL.
    let new_wb = wait_for_wb_for_top_level(shell_windows, new_top).ok_or_else(|| {
        windows::core::Error::new(E_FAIL, "IWebBrowser2 for new top-level not found")
    })?;

    let location_bstr: BSTR = unsafe { new_wb.LocationURL()? };

    // Any of host's existing IWebBrowser2 entries will do — `navOpenInNewTab` makes
    // Explorer create a fresh tab regardless of which tab's WB we call Navigate2 on.
    let host_wb = find_first_wb_for_top(shell_windows, host).ok_or_else(|| {
        windows::core::Error::new(E_FAIL, "no host WB found")
    })?;

    unsafe {
        let url_var = VARIANT::from(location_bstr);
        let flags_var = VARIANT::from(NAV_OPEN_IN_NEW_TAB);
        let empty = VARIANT::default();
        host_wb.Navigate2(
            &url_var as *const VARIANT,
            Some(&flags_var as *const VARIANT),
            Some(&empty as *const VARIANT),
            Some(&empty as *const VARIANT),
            Some(&empty as *const VARIANT),
        )?;
    }

    // Close the original spawned window (still cloaked at this point).
    unsafe {
        let _ = new_wb.Quit();
    }

    win_util::bring_to_foreground(host);
    Ok(())
}

fn wait_for_wb_for_top_level(shell_windows: &IShellWindows, top: HWND) -> Option<IWebBrowser2> {
    let deadline = Instant::now() + Duration::from_millis(WAIT_WB_TIMEOUT_MS);
    while Instant::now() < deadline {
        if let Some(wb) = find_first_wb_for_top(shell_windows, top) {
            return Some(wb);
        }
        sleep(Duration::from_millis(WAIT_WB_POLL_MS));
    }
    None
}

fn find_first_wb_for_top(shell_windows: &IShellWindows, top: HWND) -> Option<IWebBrowser2> {
    let count = unsafe { shell_windows.Count().ok()? };
    (0..count).find_map(|i| wb_if_top_matches(shell_windows, i, top))
}

fn wb_if_top_matches(
    shell_windows: &IShellWindows,
    index: i32,
    target_top: HWND,
) -> Option<IWebBrowser2> {
    let idx_var = VARIANT::from(index);
    let disp = unsafe { shell_windows.Item(&idx_var) }.ok()?;
    let wb: IWebBrowser2 = disp.cast().ok()?;
    let h_raw = unsafe { wb.HWND() }.ok()?.0;
    let hwnd = HWND(h_raw as *mut std::ffi::c_void);
    let top = win_util::top_level_window(hwnd);
    if top.0 == target_top.0 {
        Some(wb)
    } else {
        None
    }
}

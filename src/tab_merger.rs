//! Core merge orchestration — the eight-step sequence from the design doc, §5.
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

const WAIT_NEW_TAB_TIMEOUT_MS: u64 = 2_000;
const WAIT_NEW_TAB_POLL_MS: u64 = 25;

const WAIT_WB_TIMEOUT_MS: u64 = 1_500;
const WAIT_WB_POLL_MS: u64 = 25;

/// Entry point called by the WinEvent hook on each `EVENT_OBJECT_SHOW`.
///
/// `hwnd` is whatever window just got shown — could be anything system-wide. We filter
/// inside.
pub fn on_window_shown(shell_windows: &IShellWindows, hwnd: HWND) {
    let class = match win_util::get_window_class(hwnd) {
        Some(c) => c,
        None => return, // window probably already gone
    };

    if class != win_util::CABINET_WCLASS {
        return; // not a File Explorer top-level — silently ignore (frequent path)
    }

    if let Err(e) = try_merge(shell_windows, hwnd) {
        log::write(&format!("merge failed for {:?}: {:?}", hwnd.0, e));
    }
}

fn try_merge(shell_windows: &IShellWindows, new_top: HWND) -> WinResult<()> {
    let tab_count = win_util::find_tab_handles(new_top).len();

    log::write(&format!(
        "event: top={:?} tabs={}",
        new_top.0, tab_count
    ));

    // If the top-level window already has more than one tab, this is an EXISTING window
    // that's just being re-shown (un-minimised or re-focused) OR our own merge just added
    // a tab to it. Either way, leave it alone.
    if tab_count > 1 {
        log::write("skip: multi-tab existing window");
        return Ok(());
    }

    let host = match win_util::select_host(new_top) {
        Some(h) => h,
        None => {
            log::write("skip: no host candidate");
            return Ok(());
        }
    };

    log::write(&format!("merging {:?} -> {:?}", new_top.0, host.0));

    // Find the IWebBrowser2 corresponding to the new top-level window. The shell may not
    // have registered it in IShellWindows yet, so poll briefly.
    let new_wb = match wait_for_wb_for_top_level(shell_windows, new_top) {
        Some(wb) => wb,
        None => {
            return Err(windows::core::Error::new(
                E_FAIL,
                "IWebBrowser2 for new top-level not found in time",
            ));
        }
    };

    // Snapshot current host tabs so we can detect the freshly added one.
    let tabs_before: Vec<HWND> = win_util::find_tab_handles(host);

    win_util::request_new_tab(host)?;

    let new_tab = match wait_for_new_tab(host, &tabs_before) {
        Some(h) => h,
        None => {
            return Err(windows::core::Error::new(
                E_FAIL,
                "timeout waiting for new tab to appear in host",
            ));
        }
    };

    // Read target URL from the original window before we destroy it.
    let location_bstr: BSTR = unsafe { new_wb.LocationURL()? };
    log::write(&format!("location = {:?}", location_bstr.to_string()));

    let new_tab_wb = match wait_for_wb_for_tab(shell_windows, new_tab) {
        Some(wb) => wb,
        None => {
            return Err(windows::core::Error::new(
                E_FAIL,
                "could not locate new tab IWebBrowser2",
            ));
        }
    };

    unsafe {
        let url_var = VARIANT::from(location_bstr);
        new_tab_wb.Navigate2(
            &url_var as *const VARIANT,
            None,
            None,
            None,
            None,
        )?;
    }

    // Dispose of the originally-spawned top-level. Quit() closes the browser tab; for a
    // single-tab top-level this also closes the top-level itself.
    unsafe {
        let _ = new_wb.Quit();
    }

    win_util::bring_to_foreground(host);
    log::write("merge complete");
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

fn wait_for_wb_for_top_level(shell_windows: &IShellWindows, top: HWND) -> Option<IWebBrowser2> {
    let deadline = Instant::now() + Duration::from_millis(WAIT_WB_TIMEOUT_MS);
    while Instant::now() < deadline {
        if let Some(wb) = find_wb_matching(shell_windows, |wb_top| wb_top.0 == top.0) {
            return Some(wb);
        }
        sleep(Duration::from_millis(WAIT_WB_POLL_MS));
    }
    None
}

fn wait_for_wb_for_tab(shell_windows: &IShellWindows, tab: HWND) -> Option<IWebBrowser2> {
    let deadline = Instant::now() + Duration::from_millis(WAIT_WB_TIMEOUT_MS);
    while Instant::now() < deadline {
        // Each IWebBrowser2 represents a tab; the HWND returned by its HWND() is the
        // ShellTabWindowClass child. So we match the tab HWND directly here, not its
        // top-level ancestor.
        if let Some(wb) = find_wb_matching_tab(shell_windows, tab) {
            return Some(wb);
        }
        sleep(Duration::from_millis(WAIT_WB_POLL_MS));
    }
    None
}

fn find_wb_matching(
    shell_windows: &IShellWindows,
    predicate: impl Fn(HWND) -> bool,
) -> Option<IWebBrowser2> {
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
        let tab_hwnd = HWND(h_raw as *mut std::ffi::c_void);
        let top = win_util::top_level_window(tab_hwnd);
        if predicate(top) {
            return Some(wb);
        }
    }
    None
}

fn find_wb_matching_tab(shell_windows: &IShellWindows, tab: HWND) -> Option<IWebBrowser2> {
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
        let tab_hwnd = HWND(h_raw as *mut std::ffi::c_void);
        if tab_hwnd.0 == tab.0 {
            return Some(wb);
        }
    }
    None
}

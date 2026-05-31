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

/// How long to wait for IShellWindows to register the new tab. Win11 has been observed
/// taking several seconds for this — much longer than the visual appearance of the tab.
const WAIT_WB_TIMEOUT_MS: u64 = 5_000;
const WAIT_WB_POLL_MS: u64 = 50;

/// Entry point called by the WinEvent hook on each `EVENT_OBJECT_SHOW`.
pub fn on_window_shown(shell_windows: &IShellWindows, hwnd: HWND) {
    let class = match win_util::get_window_class(hwnd) {
        Some(c) => c,
        None => return,
    };

    if class != win_util::CABINET_WCLASS {
        return; // not File Explorer — the frequent path; silently ignore
    }

    if let Err(e) = try_merge(shell_windows, hwnd) {
        log::write(&format!("merge failed for {:?}: {:?}", hwnd.0, e));
    }
}

fn try_merge(shell_windows: &IShellWindows, new_top: HWND) -> WinResult<()> {
    let tab_count = win_util::find_tab_handles(new_top).len();
    log::write(&format!("event: top={:?} tabs={}", new_top.0, tab_count));

    // Existing window (un-minimised or our own newly-added tab landing) → skip.
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

    // Need the new top-level's IWebBrowser2 to read its URL. Poll briefly.
    let new_wb = match wait_for_wb_for_top_level(shell_windows, new_top) {
        Some(wb) => wb,
        None => {
            return Err(windows::core::Error::new(
                E_FAIL,
                "IWebBrowser2 for new top-level not found",
            ));
        }
    };

    // Snapshot: how many IShellWindows entries currently map to host? After we trigger
    // a new tab, this count grows by one, and the newly-registered entry is the
    // most-recently-added (highest index) entry that matches host.
    let host_entry_count_before = count_entries_for_top(shell_windows, host);
    let tabs_before: Vec<HWND> = win_util::find_tab_handles(host);
    log::write(&format!(
        "before WM_COMMAND: host has {} IShellWindows entries, {} ShellTabWindowClass children",
        host_entry_count_before,
        tabs_before.len()
    ));

    win_util::request_new_tab(host)?;

    // The tab HWND appears quickly; the IShellWindows registration is much slower.
    let new_tab = match wait_for_new_tab(host, &tabs_before) {
        Some(h) => h,
        None => {
            return Err(windows::core::Error::new(
                E_FAIL,
                "timeout waiting for new tab to appear",
            ));
        }
    };
    log::write(&format!("new tab hwnd={:?}", new_tab.0));

    // Read target URL from the original new window before destroying it.
    let location_bstr: BSTR = unsafe { new_wb.LocationURL()? };
    log::write(&format!("location = {:?}", location_bstr.to_string()));

    // Wait for the new IShellWindows entry to materialise, then take the newest one
    // matching host (reverse iteration). This is critical: navigating an older entry
    // would land the URL in the wrong tab.
    let nav_wb = match wait_for_new_host_entry(shell_windows, host, host_entry_count_before) {
        Some(wb) => wb,
        None => {
            log_shell_windows_snapshot(shell_windows, "wait_for_new_host_entry timeout");
            return Err(windows::core::Error::new(
                E_FAIL,
                "timeout waiting for new host IShellWindows entry",
            ));
        }
    };
    log::write("got newest host WB, navigating");

    // Optional VARIANT params: COM requires non-null pointers (to VT_EMPTY), not NULL.
    // Passing `None` (raw NULL) errors with RPC_X_NULL_REF_POINTER (0x800706F4).
    unsafe {
        let url_var = VARIANT::from(location_bstr);
        let empty = VARIANT::default(); // VT_EMPTY
        nav_wb.Navigate2(
            &url_var as *const VARIANT,
            Some(&empty as *const VARIANT),
            Some(&empty as *const VARIANT),
            Some(&empty as *const VARIANT),
            Some(&empty as *const VARIANT),
        )?;
    }
    log::write("Navigate2 succeeded");

    // Quit the original spawned window (its single tab — closes the top-level).
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
        if let Some(wb) = find_first_wb_for_top(shell_windows, top) {
            return Some(wb);
        }
        sleep(Duration::from_millis(WAIT_WB_POLL_MS));
    }
    None
}

/// Wait until IShellWindows has registered a NEW entry for `host` (i.e., the host's entry
/// count exceeds `base_count`), then return the newest such entry (highest index).
fn wait_for_new_host_entry(
    shell_windows: &IShellWindows,
    host: HWND,
    base_count: usize,
) -> Option<IWebBrowser2> {
    let deadline = Instant::now() + Duration::from_millis(WAIT_WB_TIMEOUT_MS);
    while Instant::now() < deadline {
        let current = count_entries_for_top(shell_windows, host);
        if current > base_count {
            return find_last_wb_for_top(shell_windows, host);
        }
        sleep(Duration::from_millis(WAIT_WB_POLL_MS));
    }
    None
}

fn count_entries_for_top(shell_windows: &IShellWindows, top: HWND) -> usize {
    let count = match unsafe { shell_windows.Count() } {
        Ok(c) => c,
        Err(_) => return 0,
    };
    let mut hits = 0usize;
    for i in 0..count {
        if entry_top(shell_windows, i).map(|t| t.0 == top.0).unwrap_or(false) {
            hits += 1;
        }
    }
    hits
}

fn find_first_wb_for_top(shell_windows: &IShellWindows, top: HWND) -> Option<IWebBrowser2> {
    let count = unsafe { shell_windows.Count().ok()? };
    for i in 0..count {
        if let Some(wb) = wb_if_top_matches(shell_windows, i, top) {
            return Some(wb);
        }
    }
    None
}

fn find_last_wb_for_top(shell_windows: &IShellWindows, top: HWND) -> Option<IWebBrowser2> {
    let count = unsafe { shell_windows.Count().ok()? };
    for i in (0..count).rev() {
        if let Some(wb) = wb_if_top_matches(shell_windows, i, top) {
            return Some(wb);
        }
    }
    None
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

fn entry_top(shell_windows: &IShellWindows, index: i32) -> Option<HWND> {
    let idx_var = VARIANT::from(index);
    let disp = unsafe { shell_windows.Item(&idx_var) }.ok()?;
    let wb: IWebBrowser2 = disp.cast().ok()?;
    let h_raw = unsafe { wb.HWND() }.ok()?.0;
    let hwnd = HWND(h_raw as *mut std::ffi::c_void);
    Some(win_util::top_level_window(hwnd))
}

/// Dump IShellWindows contents to the log. Useful when something went wrong.
fn log_shell_windows_snapshot(shell_windows: &IShellWindows, label: &str) {
    let count = match unsafe { shell_windows.Count() } {
        Ok(c) => c,
        Err(e) => {
            log::write(&format!("snapshot[{}]: Count err: {:?}", label, e));
            return;
        }
    };
    log::write(&format!("snapshot[{}]: count={}", label, count));
    for i in 0..count {
        let desc = match unsafe { shell_windows.Item(&VARIANT::from(i)) } {
            Ok(disp) => match disp.cast::<IWebBrowser2>() {
                Ok(wb) => match unsafe { wb.HWND() } {
                    Ok(h) => {
                        let hwnd = HWND(h.0 as *mut std::ffi::c_void);
                        let top = win_util::top_level_window(hwnd);
                        let cls = win_util::get_window_class(hwnd).unwrap_or_default();
                        format!("hwnd={:?}({}) top={:?}", hwnd.0, cls, top.0)
                    }
                    Err(e) => format!("(HWND err {:?})", e),
                },
                Err(_) => "(not IWebBrowser2)".into(),
            },
            Err(e) => format!("(Item err {:?})", e),
        };
        log::write(&format!("  [{}] {}", i, desc));
    }
}

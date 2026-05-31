//! Core merge orchestration — the eight-step sequence from the design doc, §5.
//!
//! Called from the WinEvent hook on the STA message-loop thread. Synchronous; failures
//! degrade gracefully (the user's window is preserved).
//!
//! No logging on the happy path — the spec mandates a silent log for routine operation.
//! Only errors and the IShellWindows snapshot (on lookup failure) are written.

use std::thread::sleep;
use std::time::{Duration, Instant};

use windows::core::{Interface, Result as WinResult, BSTR, VARIANT};
use windows::Win32::Foundation::{E_FAIL, HWND};
use windows::Win32::UI::Shell::{IShellWindows, IWebBrowser2};

use crate::log;
use crate::win_util;

const WAIT_NEW_TAB_TIMEOUT_MS: u64 = 2_000;
const WAIT_NEW_TAB_POLL_MS: u64 = 5;

/// How long to wait for IShellWindows to register the new tab. Win11 has been observed
/// taking up to a few seconds.
const WAIT_WB_TIMEOUT_MS: u64 = 5_000;
const WAIT_WB_POLL_MS: u64 = 5;

/// Called on `EVENT_OBJECT_CREATE`. Fires before the window's first paint, so cloaking
/// here actually prevents the visible flash. Any cloak applied is "speculative" — if
/// the subsequent SHOW handler decides not to merge, it must uncloak.
pub fn on_window_created(hwnd: HWND) {
    if let Some(class) = win_util::get_window_class(hwnd) {
        if class == win_util::CABINET_WCLASS {
            win_util::cloak(hwnd);
        }
    }
}

/// Called on `EVENT_OBJECT_SHOW`. The window is fully initialised by now and is either
/// a fresh window (already cloaked by [`on_window_created`]) or an existing one being
/// re-shown (un-minimised, etc., NOT cloaked).
pub fn on_window_shown(shell_windows: &IShellWindows, hwnd: HWND) {
    let class = match win_util::get_window_class(hwnd) {
        Some(c) => c,
        None => return,
    };
    if class != win_util::CABINET_WCLASS {
        return; // very frequent path
    }

    // Multi-tab → existing window being re-shown. Defensive uncloak in case some other
    // path cloaked it (no-op otherwise; DwmSetWindowAttribute(CLOAK, 0) is idempotent).
    if win_util::find_tab_handles(hwnd).len() > 1 {
        win_util::uncloak(hwnd);
        return;
    }

    // First Explorer window of the session — no host to merge into. Uncloak so user
    // sees it.
    let host = match win_util::select_host(hwnd) {
        Some(h) => h,
        None => {
            win_util::uncloak(hwnd);
            return;
        }
    };

    // Real merge. Window was cloaked at CREATE; on success Quit() destroys it (still
    // invisible); on failure we uncloak to hand it back to the user.
    if let Err(e) = try_merge(shell_windows, hwnd, host) {
        log::write(&format!("merge failed for {:?}: {:?}", hwnd.0, e));
        win_util::uncloak(hwnd);
    }
}

fn try_merge(shell_windows: &IShellWindows, new_top: HWND, host: HWND) -> WinResult<()> {
    // Need the new top-level's IWebBrowser2 to read its URL.
    let new_wb = match wait_for_wb_for_top_level(shell_windows, new_top) {
        Some(wb) => wb,
        None => {
            return Err(windows::core::Error::new(
                E_FAIL,
                "IWebBrowser2 for new top-level not found",
            ));
        }
    };

    // Snapshot host's IShellWindows entry count BEFORE we trigger a new tab; we'll wait
    // for it to grow by one and then take the newest matching entry.
    let host_entry_count_before = count_entries_for_top(shell_windows, host);
    let tabs_before: Vec<HWND> = win_util::find_tab_handles(host);

    win_util::request_new_tab(host)?;

    // Confirm the tab HWND appears (fast — just window-tree mutation).
    if wait_for_new_tab(host, &tabs_before).is_none() {
        return Err(windows::core::Error::new(
            E_FAIL,
            "timeout waiting for new tab to appear",
        ));
    }

    // Read URL before we destroy the original window.
    let location_bstr: BSTR = unsafe { new_wb.LocationURL()? };

    // Wait for the new IShellWindows entry. In Win11 each tab IS its own entry, but they
    // all report the same top-level HWND, so we can't distinguish by HWND — we rely on
    // ordering: new entries get appended, so the newest matching entry (reverse iteration)
    // is our freshly-created tab.
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

    // Optional VARIANT params: COM here needs non-null pointers to VT_EMPTY VARIANTs,
    // not real NULL. Passing None errors with RPC_X_NULL_REF_POINTER (0x800706F4).
    unsafe {
        let url_var = VARIANT::from(location_bstr);
        let empty = VARIANT::default();
        nav_wb.Navigate2(
            &url_var as *const VARIANT,
            Some(&empty as *const VARIANT),
            Some(&empty as *const VARIANT),
            Some(&empty as *const VARIANT),
            Some(&empty as *const VARIANT),
        )?;
    }

    // Quit the original spawned window (its single tab — closes the top-level).
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

/// Wait until host's IShellWindows entry count exceeds `base_count`, then return the
/// most-recently-added matching entry.
///
/// Hot loop: probe `Count()` (1 COM call) every poll; only run the O(N) host-match
/// iteration when the total entry count has actually changed. Without this, polling at
/// 5 ms × ~5 entries × ~3 COM calls per entry = sustained noticeable CPU during the wait.
fn wait_for_new_host_entry(
    shell_windows: &IShellWindows,
    host: HWND,
    base_count: usize,
) -> Option<IWebBrowser2> {
    let mut last_total = unsafe { shell_windows.Count() }.unwrap_or(-1);
    let deadline = Instant::now() + Duration::from_millis(WAIT_WB_TIMEOUT_MS);
    while Instant::now() < deadline {
        let current_total = unsafe { shell_windows.Count() }.unwrap_or(-1);
        if current_total != last_total {
            last_total = current_total;
            if count_entries_for_top(shell_windows, host) > base_count {
                return find_last_wb_for_top(shell_windows, host);
            }
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
    (0..count)
        .filter(|i| entry_top(shell_windows, *i).map(|t| t.0 == top.0).unwrap_or(false))
        .count()
}

fn find_first_wb_for_top(shell_windows: &IShellWindows, top: HWND) -> Option<IWebBrowser2> {
    let count = unsafe { shell_windows.Count().ok()? };
    (0..count).find_map(|i| wb_if_top_matches(shell_windows, i, top))
}

fn find_last_wb_for_top(shell_windows: &IShellWindows, top: HWND) -> Option<IWebBrowser2> {
    let count = unsafe { shell_windows.Count().ok()? };
    (0..count).rev().find_map(|i| wb_if_top_matches(shell_windows, i, top))
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
    if top.0 == target_top.0 { Some(wb) } else { None }
}

fn entry_top(shell_windows: &IShellWindows, index: i32) -> Option<HWND> {
    let idx_var = VARIANT::from(index);
    let disp = unsafe { shell_windows.Item(&idx_var) }.ok()?;
    let wb: IWebBrowser2 = disp.cast().ok()?;
    let h_raw = unsafe { wb.HWND() }.ok()?.0;
    let hwnd = HWND(h_raw as *mut std::ffi::c_void);
    Some(win_util::top_level_window(hwnd))
}

/// Dumps IShellWindows contents to the log. Only called on failure paths.
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

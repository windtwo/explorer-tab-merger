//! Core merge orchestration — the eight-step sequence from the design doc, §5.
//!
//! Called from the WinEvent hook on the STA message-loop thread. Synchronous; failures
//! degrade gracefully (the user's window is preserved).
//!
//! No logging on the happy path — the spec mandates a silent log for routine operation.
//! Only errors and the IShellWindows snapshot (on lookup failure) are written.

use std::thread::sleep;
use std::time::{Duration, Instant};

use windows::core::{Interface, Result as WinResult, BSTR, GUID, VARIANT};
use windows::Win32::Foundation::{E_FAIL, HWND};
use windows::Win32::System::Com::{CoTaskMemFree, IServiceProvider};
use windows::Win32::UI::Shell::{
    IFolderView, IPersistFolder2, IShellBrowser, IShellView, IShellWindows, IWebBrowser2,
    SHGetNameFromIDList, SIGDN_DESKTOPABSOLUTEPARSING,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MsgWaitForMultipleObjectsEx, PeekMessageW, TranslateMessage,
    MSG, MWMO_INPUTAVAILABLE, PM_REMOVE, QS_ALLINPUT,
};

/// `SID_STopLevelBrowser` — service id used to fetch the top-level `IShellBrowser`
/// from a window's `IServiceProvider`. Value from `shlguid.h`.
const SID_S_TOP_LEVEL_BROWSER: GUID = GUID::from_u128(0x4C96BE40_915C_11CF_99D3_00AA004AE837);

use crate::cloak;
use crate::log;
use crate::shell_events;
use crate::win_util;

const WAIT_NEW_TAB_TIMEOUT_MS: u64 = 2_000;
const WAIT_NEW_TAB_POLL_MS: u64 = 5;

/// How long to wait for IShellWindows to register the new tab. Win11 sometimes takes
/// 5-7 s for this under load (empirically), so we go to 10 s. Poll cadence is 25 ms
/// because we do a full O(N) iteration every poll now (see wait_for_new_host_entry).
const WAIT_WB_TIMEOUT_MS: u64 = 10_000;
const WAIT_WB_POLL_MS: u64 = 25;

/// How long to pump messages waiting for `NavigateComplete2` after issuing `Navigate2`.
/// If this fires we know navigation truly landed; if it doesn't, Explorer never made
/// the trip and the new tab is stuck at "This PC" / default-tab content.
const NAV_COMPLETE_TIMEOUT_MS: u64 = 3_000;

/// Called on `EVENT_OBJECT_CREATE`. Fires before the window's first paint, so cloaking
/// here actually prevents the visible flash during a merge.
///
/// We only cloak if there's at least one OTHER Explorer window already open — i.e., a
/// potential merge host. If this is the only Explorer window, there's nothing to merge
/// into, the window will live as a standalone, and cloaking it would just create the
/// "stuck invisible after uncloak" failure mode we see for WeChat-spawned windows.
pub fn on_window_created(hwnd: HWND) {
    let class = match win_util::get_window_class(hwnd) {
        Some(c) => c,
        None => return,
    };
    if class != win_util::CABINET_WCLASS {
        return;
    }
    // find_all_explorer_windows() includes `hwnd` itself, so we want strictly >1.
    if win_util::find_all_explorer_windows().len() > 1 {
        cloak::cloak(hwnd);
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

    // Multi-tab → existing window being re-shown. Defensive uncloak in case some path
    // cloaked it (idempotent for windows we don't track).
    if win_util::find_tab_handles(hwnd).len() > 1 {
        cloak::uncloak(hwnd);
        return;
    }

    // First Explorer window of the session — no host to merge into.
    let host = match win_util::select_host(hwnd) {
        Some(h) => h,
        None => {
            cloak::uncloak(hwnd);
            return;
        }
    };

    // Real merge. Window was cloaked at CREATE; on success Quit() destroys it (still
    // invisible) and we `forget` it in the tracker; on failure we uncloak to hand it
    // back to the user.
    match try_merge(shell_windows, hwnd, host) {
        Ok(()) => cloak::forget(hwnd),
        Err(e) => {
            log::write(&format!("merge failed for {:?}: {:?}", hwnd.0, e));
            cloak::uncloak(hwnd);
        }
    }
}

fn try_merge(shell_windows: &IShellWindows, new_top: HWND, host: HWND) -> WinResult<()> {
    // Refresh cloak timestamp on entry so the sweep doesn't fire while we're working.
    cloak::touch(new_top);

    // Need the new top-level's IWebBrowser2 to read its URL.
    let new_wb = match wait_for_wb_for_top_level(shell_windows, new_top, new_top) {
        Some(wb) => wb,
        None => {
            return Err(windows::core::Error::new(
                E_FAIL,
                "[wait_wb_top] IWebBrowser2 for new top-level not found",
            ));
        }
    };

    // Snapshot host's IShellWindows entry count BEFORE we trigger a new tab; we'll wait
    // for it to grow by one and then take the newest matching entry.
    let host_entry_count_before = count_entries_for_top(shell_windows, host);
    let tabs_before: Vec<HWND> = win_util::find_tab_handles(host);

    win_util::request_new_tab(host).map_err(|e| ctx(e, "request_new_tab"))?;

    // Confirm the tab HWND appears (fast — just window-tree mutation).
    if wait_for_new_tab(host, &tabs_before, new_top).is_none() {
        return Err(windows::core::Error::new(
            E_FAIL,
            "[wait_new_tab] timeout waiting for new ShellTabWindowClass child",
        ));
    }

    // Read the target location before we destroy the original window. For filesystem
    // folders LocationURL is a `file://` URL. For virtual shell folders (Recycle Bin,
    // Control Panel, This PC, Network, ...) LocationURL is EMPTY — fall back to the
    // PIDL-derived shell parsing path (e.g. "::{645FF040-...}"), which Navigate2 also
    // accepts.
    let location: String = {
        let url = unsafe { new_wb.LocationURL() }
            .map(|b| b.to_string())
            .unwrap_or_default();
        if !url.is_empty() {
            url
        } else {
            match parsing_path_via_pidl(&new_wb) {
                Some(p) => {
                    log::write(&format!("virtual folder fallback path = {:?}", p));
                    p
                }
                None => {
                    return Err(windows::core::Error::new(
                        E_FAIL,
                        "[location] empty LocationURL and PIDL fallback failed",
                    ));
                }
            }
        }
    };

    // Wait for the new IShellWindows entry. In Win11 each tab IS its own entry, but they
    // all report the same top-level HWND, so we can't distinguish by HWND — we rely on
    // ordering: new entries get appended, so the newest matching entry (reverse iteration)
    // is our freshly-created tab. Pass `new_top` so the loop can refresh the cloak
    // timestamp each poll; without that, a 3-5 s wait here would let the sweep uncloak
    // the new window prematurely (creating the "appears, then merges" disjointed UX).
    let nav_wb = match wait_for_new_host_entry(shell_windows, host, host_entry_count_before, new_top) {
        Some(wb) => wb,
        None => {
            log_shell_windows_snapshot(shell_windows, "wait_for_new_host_entry timeout");
            return Err(windows::core::Error::new(
                E_FAIL,
                "[wait_new_entry] timeout waiting for new IShellWindows entry",
            ));
        }
    };

    // Subscribe to NavigateComplete2 on the new tab's WB *before* issuing Navigate2,
    // so we don't miss the event if it fires synchronously.
    let watch =
        shell_events::watch_navigate_complete(&nav_wb).map_err(|e| ctx(e, "watch_navigate"))?;

    // Optional VARIANT params: COM here needs non-null pointers to VT_EMPTY VARIANTs,
    // not real NULL. Passing None errors with RPC_X_NULL_REF_POINTER (0x800706F4).
    unsafe {
        let url_var = VARIANT::from(BSTR::from(location.as_str()));
        let empty = VARIANT::default();
        nav_wb
            .Navigate2(
                &url_var as *const VARIANT,
                Some(&empty as *const VARIANT),
                Some(&empty as *const VARIANT),
                Some(&empty as *const VARIANT),
                Some(&empty as *const VARIANT),
            )
            .map_err(|e| ctx(e, "Navigate2"))?;
    }

    // Wait for NavigateComplete2. Without pumping messages, the COM event sink would
    // never run. We use MsgWaitForMultipleObjectsEx + PeekMessage to drain the queue.
    let completed = pump_until_navigated(&watch.completed, NAV_COMPLETE_TIMEOUT_MS, new_top);
    if !completed {
        log::write(&format!(
            "Navigate2 did NOT complete in {} ms (new tab may be left at default page) for {:?}",
            NAV_COMPLETE_TIMEOUT_MS, new_top.0
        ));
    }
    drop(watch); // Unadvise

    // Quit the original spawned window (its single tab — closes the top-level).
    unsafe {
        let _ = new_wb.Quit();
    }

    win_util::bring_to_foreground(host);
    Ok(())
}

/// Wrap a windows::core::Error with the name of the step that failed, so the merge-
/// failed log line tells us which COM call returned the generic E_FAIL / RPC error.
fn ctx(e: windows::core::Error, step: &'static str) -> windows::core::Error {
    let msg = format!("[{}] {}", step, e.message());
    windows::core::Error::new(e.code(), msg)
}

/// Resolve the shell parsing path of the folder a window is currently showing.
///
/// Used as a fallback when `IWebBrowser2::LocationURL` is empty, which happens for
/// virtual shell folders (Recycle Bin, Control Panel, This PC, Network, ...). The chain
/// is the standard one for getting a window's current PIDL:
///   IWebBrowser2 → IServiceProvider → IShellBrowser → IShellView → IFolderView
///   → IPersistFolder2 → GetCurFolder (PIDL) → SHGetNameFromIDList(DESKTOPABSOLUTEPARSING)
///
/// The parsing name (e.g. `::{645FF040-5081-101B-9F08-00AA002F954E}` for the Recycle
/// Bin, or a normal `C:\...` path for filesystem folders) is accepted by `Navigate2`.
///
/// Returns `None` if any link in the chain fails. Frees both the PIDL and the returned
/// string buffer (both are CoTaskMem-allocated).
fn parsing_path_via_pidl(wb: &IWebBrowser2) -> Option<String> {
    unsafe {
        let sp: IServiceProvider = wb.cast().ok()?;
        let browser: IShellBrowser = sp.QueryService(&SID_S_TOP_LEVEL_BROWSER).ok()?;
        let view: IShellView = browser.QueryActiveShellView().ok()?;
        let folder_view: IFolderView = view.cast().ok()?;
        let persist: IPersistFolder2 = folder_view.GetFolder().ok()?;

        let pidl = persist.GetCurFolder().ok()?;
        let name_result = SHGetNameFromIDList(pidl, SIGDN_DESKTOPABSOLUTEPARSING);
        // Free the PIDL regardless of whether name resolution succeeded.
        CoTaskMemFree(Some(pidl.0 as *const core::ffi::c_void));

        let pwstr = name_result.ok()?;
        let s = pwstr.to_string().ok();
        CoTaskMemFree(Some(pwstr.0 as *const core::ffi::c_void));

        match s {
            Some(ref text) if !text.is_empty() => s,
            _ => None,
        }
    }
}

/// Pump messages on this STA thread until `flag` becomes true or the timeout expires.
/// Returns whether `flag` was observed true.
///
/// `keepalive_hwnd` is touched each iteration so the cloak safety net doesn't fire
/// while we're waiting for navigation to land.
fn pump_until_navigated(
    flag: &std::rc::Rc<std::cell::Cell<bool>>,
    timeout_ms: u64,
    keepalive_hwnd: HWND,
) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while !flag.get() {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        cloak::touch(keepalive_hwnd);
        let remaining_ms = (deadline - now).as_millis().min(50) as u32;
        unsafe {
            // Wait until a message arrives or up to remaining_ms.
            // windows-rs 0.58 signature: phandles: Option<&[HANDLE]>, timeout, wake mask, flags.
            let _ = MsgWaitForMultipleObjectsEx(
                None,
                remaining_ms,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            );
            // Drain whatever's queued.
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, HWND(std::ptr::null_mut()), 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
    flag.get()
}

fn wait_for_new_tab(host: HWND, before: &[HWND], keepalive_hwnd: HWND) -> Option<HWND> {
    let deadline = Instant::now() + Duration::from_millis(WAIT_NEW_TAB_TIMEOUT_MS);
    while Instant::now() < deadline {
        let now = win_util::find_tab_handles(host);
        for t in &now {
            if !before.iter().any(|b| b.0 == t.0) {
                return Some(*t);
            }
        }
        cloak::touch(keepalive_hwnd);
        sleep(Duration::from_millis(WAIT_NEW_TAB_POLL_MS));
    }
    None
}

fn wait_for_wb_for_top_level(
    shell_windows: &IShellWindows,
    top: HWND,
    keepalive_hwnd: HWND,
) -> Option<IWebBrowser2> {
    let deadline = Instant::now() + Duration::from_millis(WAIT_WB_TIMEOUT_MS);
    while Instant::now() < deadline {
        if let Some(wb) = find_first_wb_for_top(shell_windows, top) {
            return Some(wb);
        }
        cloak::touch(keepalive_hwnd);
        sleep(Duration::from_millis(WAIT_WB_POLL_MS));
    }
    None
}

/// Wait until host's IShellWindows entry count exceeds `base_count`, then return the
/// most-recently-added matching entry.
///
/// We do a full host-match scan on every poll. A previous version optimised this by
/// only re-scanning when `Count()` changed — but real logs showed it missing the
/// new entry (likely COM-proxy stale-cache effects, or the increment landing in a
/// window between our polls without us seeing a different value). The full scan is
/// the safe baseline; the 25 ms cadence keeps CPU minimal.
///
/// `keepalive_hwnd` is touched each iteration so cloak::sweep_stale doesn't fire
/// while we're actively polling.
///
/// A final post-deadline check covers entries that registered exactly at the timeout
/// boundary.
fn wait_for_new_host_entry(
    shell_windows: &IShellWindows,
    host: HWND,
    base_count: usize,
    keepalive_hwnd: HWND,
) -> Option<IWebBrowser2> {
    let deadline = Instant::now() + Duration::from_millis(WAIT_WB_TIMEOUT_MS);
    while Instant::now() < deadline {
        if count_entries_for_top(shell_windows, host) > base_count {
            return find_last_wb_for_top(shell_windows, host);
        }
        cloak::touch(keepalive_hwnd);
        sleep(Duration::from_millis(WAIT_WB_POLL_MS));
    }
    if count_entries_for_top(shell_windows, host) > base_count {
        return find_last_wb_for_top(shell_windows, host);
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

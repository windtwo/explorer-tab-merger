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
    // The IDispatch we're handed represents the newly-registered shell window. In
    // Windows 11 this is the per-tab IShellBrowser, NOT the top-level window — so
    // IWebBrowser2::HWND returns the ShellTabWindowClass child HWND. We walk up to the
    // CabinetWClass top-level via GetAncestor(GA_ROOT) to decide what to do.
    let new_wb: IWebBrowser2 = new_window.cast()?;

    let tab_hwnd_raw = unsafe { new_wb.HWND()?.0 };
    let tab_hwnd = HWND(tab_hwnd_raw as *mut std::ffi::c_void);
    let top = win_util::top_level_window(tab_hwnd);
    let class = win_util::get_window_class(top).unwrap_or_default();
    let tab_count_in_top = win_util::find_tab_handles(top).len();

    log::write(&format!(
        "event: tab_hwnd={:?} top={:?} class={:?} tabs_in_top={}",
        tab_hwnd.0, top.0, class, tab_count_in_top
    ));

    if class != win_util::CABINET_WCLASS {
        // Not File Explorer (could be IE-derived, Control Panel, etc.).
        return Ok(());
    }

    // If the top-level window already has more than one tab, this event was triggered by
    // a tab being added to an EXISTING window — most likely by our own WM_COMMAND from a
    // previous merge call. Skipping prevents infinite loops.
    if tab_count_in_top > 1 {
        log::write("skip: top-level already has multiple tabs (likely our own merge)");
        return Ok(());
    }

    let host = match win_util::select_host(top) {
        Some(h) => h,
        None => {
            log::write("skip: no host candidate, letting new window live");
            return Ok(());
        }
    };

    log::write(&format!("merging into host={:?}", host.0));

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
    // four optional VARIANTs (flags/target/postdata/headers); we only set the URL — the rest
    // are passed as None which Explorer treats as defaults.
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

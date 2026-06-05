//! Thin Win32 helpers used by the merger: Explorer-window enumeration, the "new tab"
//! WM_COMMAND, cloak via WS_EX_LAYERED, host selection, and foreground activation.
//!
//! All public functions are safe wrappers; unsafe blocks are confined to the actual
//! Win32 call site.

use windows::core::{Error, HSTRING, PCWSTR};
use windows::Win32::Foundation::{BOOL, COLORREF, HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, GetAncestor, GetClassNameW, GetWindowLongPtrW, PostMessageW,
    SetForegroundWindow, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos,
    ShowWindow, GA_ROOT, GWL_EXSTYLE, LWA_ALPHA, SWP_FRAMECHANGED, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_SHOWNORMAL, WM_COMMAND, WS_EX_LAYERED,
};

/// File Explorer's top-level window class.
pub const CABINET_WCLASS: &str = "CabinetWClass";

/// The class used by Explorer for each tab's hosting window. Children of CABINET_WCLASS.
pub const SHELL_TAB_WCLASS: &str = "ShellTabWindowClass";

/// Undocumented Explorer command IDs (community-known, used by the original
/// w4po/ExplorerTabUtility):
/// - 0xA21B: "New Tab"
/// - 0xA021: close active tab
/// - 0xA221 + 1-based index: activate tab N (unused here)
pub const CMD_NEW_TAB: u32 = 0xA21B;

/// Enumerate every top-level window whose class is `CabinetWClass`.
pub fn find_all_explorer_windows() -> Vec<HWND> {
    let mut out: Vec<HWND> = Vec::new();
    unsafe {
        // EnumWindows ignores its return; pass our Vec via LPARAM.
        let lparam = LPARAM(&mut out as *mut Vec<HWND> as isize);
        let _ = EnumWindows(Some(enum_cabinet_proc), lparam);
    }
    out
}

unsafe extern "system" fn enum_cabinet_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    if let Some(class) = get_window_class(hwnd) {
        if class == CABINET_WCLASS {
            let vec = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
            vec.push(hwnd);
        }
    }
    BOOL(1) // keep enumerating
}

pub fn get_window_class(hwnd: HWND) -> Option<String> {
    let mut buf = [0u16; 256];
    let len = unsafe { GetClassNameW(hwnd, &mut buf) };
    if len <= 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

/// Walk up to the top-level (GA_ROOT) ancestor. Returns the input unchanged if it has
/// no ancestor.
pub fn top_level_window(hwnd: HWND) -> HWND {
    let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
    if root.0.is_null() {
        hwnd
    } else {
        root
    }
}

/// Return every `ShellTabWindowClass` child of the given host, in enumeration order.
pub fn find_tab_handles(host: HWND) -> Vec<HWND> {
    let mut out = Vec::new();
    let class_w = HSTRING::from(SHELL_TAB_WCLASS);
    let mut prev = HWND(std::ptr::null_mut());
    loop {
        let next =
            unsafe { FindWindowExW(host, prev, PCWSTR(class_w.as_ptr()), PCWSTR::null()) };
        let next_hwnd = match next {
            Ok(h) if !h.0.is_null() => h,
            _ => break,
        };
        out.push(next_hwnd);
        prev = next_hwnd;
    }
    out
}

/// The first `ShellTabWindowClass` child, or `HWND(null)` if the host has none.
pub fn first_tab_handle(host: HWND) -> HWND {
    let class_w = HSTRING::from(SHELL_TAB_WCLASS);
    unsafe {
        FindWindowExW(
            host,
            HWND(std::ptr::null_mut()),
            PCWSTR(class_w.as_ptr()),
            PCWSTR::null(),
        )
        .unwrap_or(HWND(std::ptr::null_mut()))
    }
}

/// Post the Explorer "new tab" command. Returns Err if the host has no tab child to target.
pub fn request_new_tab(host: HWND) -> Result<(), Error> {
    let tab = first_tab_handle(host);
    if tab.0.is_null() {
        return Err(Error::from_win32());
    }
    unsafe {
        PostMessageW(tab, WM_COMMAND, WPARAM(CMD_NEW_TAB as usize), LPARAM(0))?;
    }
    Ok(())
}

/// Hide the window by making it a layered window with alpha=0 (fully transparent).
/// This is the same mechanism the original w4po project uses (see Helper.HideWindow,
/// non-keepTheme branch).
///
/// Why not DWM cloak? Cloak only tells the compositor "don't composite next frame", but
/// Win11 paints the window-open animation BEFORE the compositor stage — so cloak misses
/// the first visible frame. WS_EX_LAYERED + alpha=0 is enforced at the GDI level for
/// every painted pixel, including animation frames, so the window is invisible from the
/// moment the style is applied.
pub fn cloak(hwnd: HWND) {
    unsafe {
        let exstyle = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let layered_bit = WS_EX_LAYERED.0 as isize;
        if exstyle & layered_bit == 0 {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, exstyle | layered_bit);
        }
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_ALPHA);
    }
}

/// Reverse of [`cloak`]. Three steps, mirroring what the original w4po project does on
/// uncloak:
/// 1. Restore alpha to 255 (fully opaque).
/// 2. **Remove the `WS_EX_LAYERED` style bit.** Without this, some Win11 Explorer
///    windows (notably those spawned by external apps — WeChat, "Show in folder", etc.)
///    stay invisible even after alpha=255 because the layered-window rendering path
///    keeps a stale invisible composition. Removing the style returns the window to
///    normal rendering and forces a fresh paint.
/// 3. `SetWindowPos(SWP_FRAMECHANGED)` to tell the compositor to re-evaluate the
///    frame — a belt-and-braces against any leftover invisible state from step 2.
pub fn uncloak(hwnd: HWND) {
    unsafe {
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);

        let exstyle = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let layered_bit = WS_EX_LAYERED.0 as isize;
        if exstyle & layered_bit != 0 {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, exstyle & !layered_bit);
        }

        let _ = SetWindowPos(
            hwnd,
            HWND(std::ptr::null_mut()),
            0,
            0,
            0,
            0,
            SWP_NOSIZE | SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );

        // Belt-and-braces: explicitly show the window. No-op if already visible; covers
        // the rare case where WS_VISIBLE got cleared or the window was minimised.
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
    }
}

pub fn bring_to_foreground(hwnd: HWND) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        let _ = SetForegroundWindow(hwnd);
    }
}

/// A host is rejected once it has this many tabs. Empirical observation: when a Win11
/// Explorer top-level holds ~17 tabs, the WM_COMMAND new-tab command starts being
/// silently dropped — and pushing further appears to destabilise Explorer (we've seen
/// crashes of explorer.exe under that condition). Capping below the limit forces a
/// new top-level to be created naturally, which the next merge will then host into.
const MAX_HOST_TABS: usize = 15;

/// Pick the best host among existing Explorer windows: highest tab count, excluding
/// `other_than`, and rejecting any host already at or above [`MAX_HOST_TABS`].
/// Returns `None` if no eligible Explorer window exists.
pub fn select_host(other_than: HWND) -> Option<HWND> {
    let mut candidates: Vec<(HWND, usize)> = find_all_explorer_windows()
        .into_iter()
        .filter(|h| h.0 != other_than.0)
        .map(|h| {
            let n = find_tab_handles(h).len();
            (h, n)
        })
        .filter(|(_, n)| *n < MAX_HOST_TABS)
        .collect();
    candidates.sort_by(|a, b| b.1.cmp(&a.1));
    candidates.into_iter().next().map(|(h, _)| h)
}

//! Thin Win32 helpers used by the merger: window enumeration, the Explorer "new tab"
//! WM_COMMAND, and foreground/close.
//!
//! All public functions in this module are safe wrappers; unsafe blocks are confined to the
//! actual Win32 call site.

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{BOOL, COLORREF, HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, GetAncestor, GetClassNameW, GetWindowLongPtrW,
    SetForegroundWindow, SetLayeredWindowAttributes, SetWindowLongPtrW, ShowWindow,
    GA_ROOT, GWL_EXSTYLE, LWA_ALPHA, SW_SHOWNORMAL, WS_EX_LAYERED,
};

/// File Explorer's top-level window class.
pub const CABINET_WCLASS: &str = "CabinetWClass";

/// The class used by Explorer for each tab's hosting window. Children of CABINET_WCLASS.
pub const SHELL_TAB_WCLASS: &str = "ShellTabWindowClass";

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

/// Reverse of [`cloak`] — set alpha back to 255 (fully opaque). We leave WS_EX_LAYERED on
/// the window; removing it is unnecessary and a touch risky (race with paint).
pub fn uncloak(hwnd: HWND) {
    unsafe {
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
    }
}

pub fn bring_to_foreground(hwnd: HWND) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        let _ = SetForegroundWindow(hwnd);
    }
}

/// Pick the best host among existing Explorer windows: highest tab count, excluding `other_than`.
/// Returns `None` if no other Explorer window exists.
pub fn select_host(other_than: HWND) -> Option<HWND> {
    let mut candidates: Vec<(HWND, usize)> = find_all_explorer_windows()
        .into_iter()
        .filter(|h| h.0 != other_than.0)
        .map(|h| {
            let n = find_tab_handles(h).len();
            (h, n)
        })
        .collect();
    candidates.sort_by(|a, b| b.1.cmp(&a.1));
    candidates.into_iter().next().map(|(h, _)| h)
}

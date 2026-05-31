//! Thin Win32 helpers used by the merger: window enumeration, the Explorer "new tab"
//! WM_COMMAND, and foreground/close.
//!
//! All public functions in this module are safe wrappers; unsafe blocks are confined to the
//! actual Win32 call site.

use windows::core::{Error, HSTRING, PCWSTR};
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, GetAncestor, GetClassNameW, IsWindow, PostMessageW,
    SetForegroundWindow, ShowWindow, GA_ROOT, SW_SHOWNORMAL, WM_CLOSE, WM_COMMAND,
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

pub fn is_explorer(hwnd: HWND) -> bool {
    get_window_class(hwnd).as_deref() == Some(CABINET_WCLASS)
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

pub fn close_window(hwnd: HWND) {
    if !unsafe { IsWindow(hwnd).as_bool() } {
        return;
    }
    unsafe {
        let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
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

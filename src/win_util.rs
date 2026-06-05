//! Thin Win32 helpers used by the merger: Explorer-window enumeration, the "new tab"
//! WM_COMMAND, cloak via WS_EX_LAYERED, host selection, and foreground activation.
//!
//! All public functions are safe wrappers; unsafe blocks are confined to the actual
//! Win32 call site.

use windows::core::{Error, HSTRING, PCWSTR};
use windows::Win32::Foundation::{BOOL, HANDLE, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, GetAncestor, GetClassNameW, GetPropW, GetWindowRect,
    PostMessageW, RemovePropW, SetForegroundWindow, SetPropW, SetWindowPos, ShowWindow,
    GA_ROOT, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW,
    SW_SHOWNORMAL, WM_COMMAND,
};

const PROP_ORIG_X: &str = "ExplorerTabMerger_OrigX";
const PROP_ORIG_Y: &str = "ExplorerTabMerger_OrigY";
const PROP_CLOAKED: &str = "ExplorerTabMerger_Cloaked";

/// Off-screen coordinate to park cloaked windows at. Far enough that no real monitor
/// arrangement reaches it.
const OFFSCREEN: i32 = -32_000;

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

/// Hide a window by moving it far off-screen with `SWP_HIDEWINDOW`, after recording its
/// original position in window properties. This is w4po's "keep theme" hide method.
///
/// Why not `WS_EX_LAYERED` + alpha=0 (what we used through v1.0.1)? Restoring a layered
/// window to visible (uncloak) proved unreliable on Win11 for windows spawned by some
/// sources — external apps ("Show in folder" from WeChat etc.) and virtual folders
/// (This PC, Recycle Bin) opened while another Explorer window exists. They'd stay
/// invisible after uncloak (present in the taskbar, blank on click). Moving off-screen
/// and back is a pure coordinate operation that cannot leave the window stuck invisible.
pub fn cloak(hwnd: HWND) {
    let px = HSTRING::from(PROP_ORIG_X);
    let py = HSTRING::from(PROP_ORIG_Y);
    let pc = HSTRING::from(PROP_CLOAKED);
    unsafe {
        // Record the original position so uncloak can restore it.
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_ok() {
            let _ = SetPropW(hwnd, PCWSTR(px.as_ptr()), pack_handle(rect.left));
            let _ = SetPropW(hwnd, PCWSTR(py.as_ptr()), pack_handle(rect.top));
        }
        let _ = SetPropW(hwnd, PCWSTR(pc.as_ptr()), pack_handle(1));

        let _ = SetWindowPos(
            hwnd,
            HWND(std::ptr::null_mut()),
            OFFSCREEN,
            OFFSCREEN,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_HIDEWINDOW,
        );
    }
}

/// Reverse of [`cloak`]: move the window back to its recorded position and show it.
pub fn uncloak(hwnd: HWND) {
    let px = HSTRING::from(PROP_ORIG_X);
    let py = HSTRING::from(PROP_ORIG_Y);
    let pc = HSTRING::from(PROP_CLOAKED);
    unsafe {
        let x = unpack_handle(GetPropW(hwnd, PCWSTR(px.as_ptr())));
        let y = unpack_handle(GetPropW(hwnd, PCWSTR(py.as_ptr())));
        let _ = RemovePropW(hwnd, PCWSTR(px.as_ptr()));
        let _ = RemovePropW(hwnd, PCWSTR(py.as_ptr()));
        let _ = RemovePropW(hwnd, PCWSTR(pc.as_ptr()));

        // If we somehow lack a recorded on-screen position, fall back to a sane spot
        // rather than leaving the window off-screen.
        let (tx, ty) = if x <= OFFSCREEN || (x == 0 && y == 0) {
            (120, 120)
        } else {
            (x, y)
        };

        let _ = SetWindowPos(
            hwnd,
            HWND(std::ptr::null_mut()),
            tx,
            ty,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        // Belt-and-braces in case the window was also minimised.
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
    }
}

/// True if this window currently carries our cloak marker property (i.e., we hid it and
/// haven't restored it yet). Used by startup recovery to find orphans from a crash.
pub fn is_cloaked(hwnd: HWND) -> bool {
    let pc = HSTRING::from(PROP_CLOAKED);
    unsafe { !GetPropW(hwnd, PCWSTR(pc.as_ptr())).0.is_null() }
}

fn pack_handle(v: i32) -> HANDLE {
    HANDLE(v as isize as *mut core::ffi::c_void)
}

fn unpack_handle(h: HANDLE) -> i32 {
    h.0 as isize as i32
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

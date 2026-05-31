//! Global "new window shown" detector using `SetWinEventHook(EVENT_OBJECT_SHOW)`.
//!
//! We tried `IShellWindowsEvents::WindowRegistered` first; on Windows 11 it does not
//! reliably fire for newly-spawned top-level Explorer windows (only for the per-tab
//! IShellBrowser revoke/register sequence). Falling back to the WinEvent hook — same
//! mechanism the original w4po/ExplorerTabUtility uses — catches every window-show
//! across the system, and we filter for `CabinetWClass` in the callback.
//!
//! With `WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS`, the callback runs on whatever
//! thread is pumping messages for the calling thread (ours, since we own the message
//! loop). So no synchronisation is needed between the callback and the rest of the app.

use std::cell::RefCell;

use windows::core::{Error, Result as WinResult};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_OBJECT_SHOW, OBJID_WINDOW, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
};

type Callback = Box<dyn Fn(HWND)>;

thread_local! {
    static CALLBACK: RefCell<Option<Callback>> = RefCell::new(None);
}

pub fn subscribe(callback: impl Fn(HWND) + 'static) -> WinResult<Subscription> {
    CALLBACK.with(|c| *c.borrow_mut() = Some(Box::new(callback)));

    let hook = unsafe {
        SetWinEventHook(
            EVENT_OBJECT_SHOW,
            EVENT_OBJECT_SHOW,
            None,
            Some(win_event_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        )
    };

    if hook.0.is_null() {
        CALLBACK.with(|c| *c.borrow_mut() = None);
        return Err(Error::from_win32());
    }
    Ok(Subscription(hook))
}

unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    _id_child: i32,
    _id_event_thread: u32,
    _dwms_event_time: u32,
) {
    // We only care about top-level window show events.
    if event != EVENT_OBJECT_SHOW || id_object != OBJID_WINDOW.0 {
        return;
    }
    CALLBACK.with(|c| {
        if let Some(cb) = c.borrow().as_ref() {
            cb(hwnd);
        }
    });
}

pub struct Subscription(HWINEVENTHOOK);

impl Drop for Subscription {
    fn drop(&mut self) {
        unsafe {
            let _ = UnhookWinEvent(self.0);
        }
        CALLBACK.with(|c| *c.borrow_mut() = None);
    }
}

//! Global window-event hook covering EVENT_OBJECT_CREATE and EVENT_OBJECT_SHOW.
//!
//! Two events instead of one: CREATE fires *before* the window's first paint, which
//! lets us DWM-cloak a candidate Explorer window before the user can perceive it.
//! SHOW fires once the window is fully initialised — that's when we run the actual merge.
//!
//! With `WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS`, the callback runs on whatever
//! thread is pumping messages for the calling thread (ours), so no synchronisation is
//! needed between the callback and the rest of the app.

use std::cell::RefCell;

use windows::core::{Error, Result as WinResult};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_OBJECT_CREATE, EVENT_OBJECT_SHOW, OBJID_WINDOW, WINEVENT_OUTOFCONTEXT,
    WINEVENT_SKIPOWNPROCESS,
};

type Callback = Box<dyn Fn(u32, HWND)>;

thread_local! {
    static CALLBACK: RefCell<Option<Callback>> = RefCell::new(None);
}

/// Subscribe to OBJID_WINDOW CREATE+SHOW events system-wide. Callback receives the event
/// ID (`EVENT_OBJECT_CREATE` or `EVENT_OBJECT_SHOW`) and the HWND.
pub fn subscribe(callback: impl Fn(u32, HWND) + 'static) -> WinResult<Subscription> {
    CALLBACK.with(|c| *c.borrow_mut() = Some(Box::new(callback)));

    let hook = unsafe {
        SetWinEventHook(
            EVENT_OBJECT_CREATE,
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
    // Only care about top-level windows themselves, not children/objects on them.
    if id_object != OBJID_WINDOW.0 {
        return;
    }
    // SetWinEventHook(CREATE..SHOW) also delivers EVENT_OBJECT_DESTROY (0x8001); skip.
    if event != EVENT_OBJECT_CREATE && event != EVENT_OBJECT_SHOW {
        return;
    }
    CALLBACK.with(|c| {
        if let Some(cb) = c.borrow().as_ref() {
            cb(event, hwnd);
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

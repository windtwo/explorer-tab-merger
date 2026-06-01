//! Global window-event hook + per-WB NavigateComplete2 sink.
//!
//! 1. WinEvent hook (CREATE..SHOW): lets us cloak new Explorer windows before their
//!    first paint and trigger the merge once they're fully initialised.
//!
//! 2. NavigateComplete2 sink: implements `IDispatch` and subscribes to a specific
//!    `IWebBrowser2`'s `DWebBrowserEvents2::NavigateComplete2`. Used by the merger to
//!    know when a freshly-issued `Navigate2` has truly landed (rather than guessing
//!    with a sleep).
//!
//! With `WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS`, the WinEvent callback runs
//! on whatever thread is pumping messages for the calling thread (ours), so no
//! synchronisation is needed between the callback and the rest of the app.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use windows::core::{implement, Interface, Result as WinResult, Error, GUID, PCWSTR, VARIANT};
use windows::Win32::Foundation::{E_NOTIMPL, HWND};
use windows::Win32::System::Com::{
    IConnectionPoint, IConnectionPointContainer, IDispatch, IDispatch_Impl, ITypeInfo,
    DISPATCH_FLAGS, DISPPARAMS, EXCEPINFO,
};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::Shell::IWebBrowser2;
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

// ---------------------------------------------------------------------------
// NavigateComplete2 sink
// ---------------------------------------------------------------------------

const DIID_D_WEB_BROWSER_EVENTS_2: GUID =
    GUID::from_u128(0x34A715A0_6587_11D0_924A_0020AFC7AC4D);
/// `DISPID_NAVIGATECOMPLETE2` from `exdispid.h`.
const DISPID_NAVIGATE_COMPLETE_2: i32 = 252;

#[implement(IDispatch)]
struct NavigateCompleteSink {
    completed: Rc<Cell<bool>>,
}

impl IDispatch_Impl for NavigateCompleteSink_Impl {
    fn GetTypeInfoCount(&self) -> WinResult<u32> {
        Ok(0)
    }
    fn GetTypeInfo(&self, _itinfo: u32, _lcid: u32) -> WinResult<ITypeInfo> {
        Err(E_NOTIMPL.into())
    }
    fn GetIDsOfNames(
        &self,
        _riid: *const GUID,
        _rgsznames: *const PCWSTR,
        _cnames: u32,
        _lcid: u32,
        _rgdispid: *mut i32,
    ) -> WinResult<()> {
        Err(E_NOTIMPL.into())
    }
    fn Invoke(
        &self,
        dispidmember: i32,
        _riid: *const GUID,
        _lcid: u32,
        _wflags: DISPATCH_FLAGS,
        _pdispparams: *const DISPPARAMS,
        _pvarresult: *mut VARIANT,
        _pexcepinfo: *mut EXCEPINFO,
        _puargerr: *mut u32,
    ) -> WinResult<()> {
        if dispidmember == DISPID_NAVIGATE_COMPLETE_2 {
            self.completed.set(true);
        }
        Ok(())
    }
}

/// Subscribe to a specific `IWebBrowser2`'s `NavigateComplete2` event. Returns a
/// [`NavigateCompleteWatch`] whose `completed` flag flips `true` when navigation lands.
/// Drop the watch to unsubscribe.
pub fn watch_navigate_complete(wb: &IWebBrowser2) -> WinResult<NavigateCompleteWatch> {
    let cpc: IConnectionPointContainer = wb.cast()?;
    let cp = unsafe { cpc.FindConnectionPoint(&DIID_D_WEB_BROWSER_EVENTS_2)? };

    let completed = Rc::new(Cell::new(false));
    let sink: IDispatch = NavigateCompleteSink {
        completed: completed.clone(),
    }
    .into();

    let cookie = unsafe { cp.Advise(&sink)? };
    Ok(NavigateCompleteWatch {
        cp,
        cookie,
        completed,
    })
}

pub struct NavigateCompleteWatch {
    cp: IConnectionPoint,
    cookie: u32,
    pub completed: Rc<Cell<bool>>,
}

impl Drop for NavigateCompleteWatch {
    fn drop(&mut self) {
        unsafe {
            let _ = self.cp.Unadvise(self.cookie);
        }
    }
}

//! IDispatch sink for `DShellWindowsEvents::WindowRegistered`.
//!
//! We implement the bare minimum of `IDispatch` (only `Invoke` does real work; the rest
//! return E_NOTIMPL — fine because the connection-point machinery never asks for type info
//! on a pure event sink) and attach it via the standard ConnectionPoint protocol.
//!
//! Well-known IIDs/DISPIDs (from `shldisp.h`):
//! - `DIID_DShellWindowsEvents` = {FE4106E0-399A-11D0-A48C-00A0C90A8F39}
//! - `DISPID_WINDOWREGISTERED` = 0xC8 (200)
//! - `DISPID_WINDOWREVOKED` = 0xC9 (201)  // unused here

use std::rc::Rc;

use windows::core::{implement, Interface, Result as WinResult, GUID, PCWSTR, VARIANT};
use windows::Win32::Foundation::E_NOTIMPL;
use windows::Win32::System::Com::{
    IConnectionPoint, IConnectionPointContainer, IDispatch, IDispatch_Impl, ITypeInfo,
    DISPATCH_FLAGS, DISPPARAMS, EXCEPINFO,
};
use windows::Win32::UI::Shell::IShellWindows;

const DIID_DSHELL_WINDOWS_EVENTS: GUID =
    GUID::from_u128(0xFE4106E0_399A_11D0_A48C_00A0C90A8F39);
const DISPID_WINDOWREGISTERED: i32 = 0xC8;

type Callback = Rc<dyn Fn(IDispatch)>;

#[implement(IDispatch)]
struct WindowRegisteredSink {
    shell_windows: IShellWindows,
    callback: Callback,
}

impl IDispatch_Impl for WindowRegisteredSink_Impl {
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
        pdispparams: *const DISPPARAMS,
        _pvarresult: *mut VARIANT,
        _pexcepinfo: *mut EXCEPINFO,
        _puargerr: *mut u32,
    ) -> WinResult<()> {
        if dispidmember != DISPID_WINDOWREGISTERED {
            return Ok(());
        }

        let params = unsafe { &*pdispparams };
        if params.cArgs < 1 || params.rgvarg.is_null() {
            return Ok(());
        }

        // The cookie is a LONG packed as a VARIANT, arg index 0.
        let cookie_variant: &VARIANT = unsafe { &*params.rgvarg };
        let cookie: i32 = match i32::try_from(cookie_variant) {
            Ok(c) => c,
            Err(_) => return Ok(()),
        };

        // Look the new window up. Race: it may have been revoked already — that's fine.
        let cookie_var = VARIANT::from(cookie);
        if let Ok(dispatch) = unsafe { self.shell_windows.Item(&cookie_var) } {
            (self.callback)(dispatch);
        }
        Ok(())
    }
}

/// Advise the IShellWindows event source. The returned [`Subscription`] holds the connection;
/// drop it to unadvise.
pub fn subscribe(
    shell_windows: &IShellWindows,
    callback: impl Fn(IDispatch) + 'static,
) -> WinResult<Subscription> {
    let cpc: IConnectionPointContainer = shell_windows.cast()?;
    let cp: IConnectionPoint = unsafe { cpc.FindConnectionPoint(&DIID_DSHELL_WINDOWS_EVENTS)? };

    let sink: IDispatch = WindowRegisteredSink {
        shell_windows: shell_windows.clone(),
        callback: Rc::new(callback),
    }
    .into();

    let cookie = unsafe { cp.Advise(&sink)? };
    Ok(Subscription { cp, cookie })
}

pub struct Subscription {
    cp: IConnectionPoint,
    cookie: u32,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        unsafe {
            let _ = self.cp.Unadvise(self.cookie);
        }
    }
}

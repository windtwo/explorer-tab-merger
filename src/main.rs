#![windows_subsystem = "windows"] // no console window

//! Explorer Tab Merger — process entry.
//!
//! Single STA thread, single COM apartment, single GetMessage loop. All work happens
//! synchronously inside the IDispatch sink (`tab_merger::on_new_window`) or in the
//! watchdog timer tick.

use std::cell::RefCell;
use std::env;
use std::rc::Rc;

use windows::core::Result as WinResult;
use windows::Win32::Foundation::{HWND, RPC_E_DISCONNECTED};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_LOCAL_SERVER,
    COINIT_APARTMENTTHREADED, IDispatch,
};
use windows::Win32::UI::Shell::{IShellWindows, ShellWindows};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, KillTimer, SetTimer, TranslateMessage, MSG, WM_TIMER,
};

use explorer_tab_merger::{autostart, log, shell_events, single_instance, tab_merger};

const WATCHDOG_TIMER_ID: usize = 1;
const WATCHDOG_INTERVAL_MS: u32 = 10_000;

struct App {
    shell_windows: IShellWindows,
    subscription: Option<shell_events::Subscription>,
}

impl App {
    fn new() -> WinResult<Self> {
        let shell_windows: IShellWindows =
            unsafe { CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER)? };
        Ok(Self {
            shell_windows,
            subscription: None,
        })
    }

    fn subscribe(this: Rc<RefCell<Self>>) -> WinResult<()> {
        let shell_windows = this.borrow().shell_windows.clone();
        let weak = Rc::downgrade(&this);
        let sub = shell_events::subscribe(&shell_windows, move |dispatch: IDispatch| {
            if let Some(app) = weak.upgrade() {
                let sw = app.borrow().shell_windows.clone();
                tab_merger::on_new_window(&sw, dispatch);
            }
        })?;
        this.borrow_mut().subscription = Some(sub);
        Ok(())
    }

    fn is_alive(&self) -> bool {
        unsafe {
            match self.shell_windows.Count() {
                Ok(_) => true,
                Err(e) => e.code() != RPC_E_DISCONNECTED,
            }
        }
    }
}

fn main() {
    // Refuse to start a second instance.
    let _guard = match single_instance::acquire() {
        Some(g) => g,
        None => return,
    };

    unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if hr.is_err() {
            log::write(&format!("CoInitializeEx failed: {:?}", hr));
            return;
        }
    }

    // Idempotent autostart. Best-effort: if it fails, we keep running anyway.
    if let Ok(exe) = env::current_exe() {
        if let Err(e) = autostart::ensure_run(&exe) {
            log::write(&format!("autostart register failed: {:?}", e));
        }
    }

    let app = match App::new() {
        Ok(a) => Rc::new(RefCell::new(a)),
        Err(e) => {
            log::write(&format!("CoCreateInstance(ShellWindows) failed: {:?}", e));
            unsafe { CoUninitialize() };
            return;
        }
    };

    if let Err(e) = App::subscribe(app.clone()) {
        log::write(&format!("Advise failed: {:?}", e));
        unsafe { CoUninitialize() };
        return;
    }

    // Watchdog: WM_TIMER posted to this thread's queue every 10 s.
    unsafe {
        SetTimer(HWND(std::ptr::null_mut()), WATCHDOG_TIMER_ID, WATCHDOG_INTERVAL_MS, None);
    }

    run_message_loop(app.clone());

    unsafe {
        let _ = KillTimer(HWND(std::ptr::null_mut()), WATCHDOG_TIMER_ID);
    }

    // Drop subscription (calls Unadvise) before CoUninitialize.
    drop(app);

    unsafe { CoUninitialize() };
}

fn run_message_loop(app: Rc<RefCell<App>>) {
    unsafe {
        let mut msg = MSG::default();
        loop {
            let r = GetMessageW(&mut msg, HWND(std::ptr::null_mut()), 0, 0);
            if r.0 == 0 {
                break; // WM_QUIT
            }
            if r.0 == -1 {
                log::write("GetMessageW returned -1");
                break;
            }

            if msg.message == WM_TIMER && msg.wParam.0 == WATCHDOG_TIMER_ID {
                watchdog_tick(&app);
                continue;
            }

            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn watchdog_tick(app: &Rc<RefCell<App>>) {
    if app.borrow().is_alive() {
        return;
    }

    log::write("ShellWindows disconnected; attempting reconnect");

    // Drop the dead subscription before creating a new one.
    app.borrow_mut().subscription = None;

    let new_app = match App::new() {
        Ok(a) => a,
        Err(e) => {
            log::write(&format!("reconnect failed: {:?}", e));
            return;
        }
    };
    *app.borrow_mut() = new_app;
    if let Err(e) = App::subscribe(app.clone()) {
        log::write(&format!("re-subscribe failed: {:?}", e));
    }
}

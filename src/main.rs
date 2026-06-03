#![windows_subsystem = "windows"] // no console window

//! Explorer Tab Merger — process entry.
//!
//! Single STA thread, single COM apartment, single GetMessage loop. Detection of new
//! Explorer windows is via `SetWinEventHook(EVENT_OBJECT_SHOW)`; the callback runs on
//! this thread's message queue. The merge work happens synchronously inside that callback.

use std::cell::RefCell;
use std::env;
use std::rc::Rc;

use windows::core::Result as WinResult;
use windows::Win32::Foundation::{HWND, RPC_E_DISCONNECTED};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_LOCAL_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::{IShellWindows, ShellWindows};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, KillTimer, SetTimer, TranslateMessage, EVENT_OBJECT_CREATE,
    EVENT_OBJECT_SHOW, MSG, WM_TIMER,
};

use explorer_tab_merger::{
    autostart, cloak, conflicts, log, shell_events, single_instance, tab_merger,
};

const WATCHDOG_TIMER_ID: usize = 1;
/// Watchdog now ticks every 1 s (was 10 s) because cloak's STALE_THRESHOLD is 2 s —
/// we need to sweep often enough that no cloaked window outlives that threshold by
/// much. Tick body is cheap (a Count() COM probe + a hashmap scan).
const WATCHDOG_INTERVAL_MS: u32 = 1_000;

struct App {
    shell_windows: IShellWindows,
    /// Held to keep the WinEvent hook alive; dropped on shutdown to unhook.
    _hook: Option<shell_events::Subscription>,
}

impl App {
    fn new() -> WinResult<Self> {
        let shell_windows: IShellWindows =
            unsafe { CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER)? };
        Ok(Self {
            shell_windows,
            _hook: None,
        })
    }

    fn install_hook(this: Rc<RefCell<Self>>) -> WinResult<()> {
        let weak = Rc::downgrade(&this);
        let hook = shell_events::subscribe(move |event: u32, hwnd: HWND| {
            match event {
                EVENT_OBJECT_CREATE => tab_merger::on_window_created(hwnd),
                EVENT_OBJECT_SHOW => {
                    if let Some(app) = weak.upgrade() {
                        let sw = app.borrow().shell_windows.clone();
                        tab_merger::on_window_shown(&sw, hwnd);
                    }
                }
                _ => {}
            }
        })?;
        this.borrow_mut()._hook = Some(hook);
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

    if let Ok(exe) = env::current_exe() {
        if let Err(e) = autostart::ensure_run(&exe) {
            log::write(&format!("autostart register failed: {:?}", e));
        }
    }

    // Warn about other tools that compete for the same job.
    let detected = conflicts::detect_running();
    if !detected.is_empty() {
        log::write(&format!(
            "WARNING: other tools detected that may race for new Explorer windows: {:?}. \
             Recommended: close them or stop this merger.",
            detected
        ));
    }

    // Salvage anything a previous crashed instance left behind (cloaked Explorer windows
    // that are still on screen but invisible).
    cloak::recover_orphans();

    let app = match App::new() {
        Ok(a) => Rc::new(RefCell::new(a)),
        Err(e) => {
            log::write(&format!("CoCreateInstance(ShellWindows) failed: {:?}", e));
            unsafe { CoUninitialize() };
            return;
        }
    };

    if let Err(e) = App::install_hook(app.clone()) {
        log::write(&format!("SetWinEventHook failed: {:?}", e));
        unsafe { CoUninitialize() };
        return;
    }

    unsafe {
        SetTimer(HWND(std::ptr::null_mut()), WATCHDOG_TIMER_ID, WATCHDOG_INTERVAL_MS, None);
    }

    run_message_loop(app.clone());

    unsafe {
        let _ = KillTimer(HWND(std::ptr::null_mut()), WATCHDOG_TIMER_ID);
    }

    // Best-effort cleanup before exit: anything we left cloaked must be restored, or the
    // user gets a permanently invisible Explorer window.
    cloak::uncloak_all();

    drop(app);
    unsafe { CoUninitialize() };
}

fn run_message_loop(app: Rc<RefCell<App>>) {
    unsafe {
        let mut msg = MSG::default();
        loop {
            let r = GetMessageW(&mut msg, HWND(std::ptr::null_mut()), 0, 0);
            if r.0 == 0 {
                break;
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
    // Safety net: uncloak any window we cloaked but never finished processing. Cheap.
    cloak::sweep_stale();

    if app.borrow().is_alive() {
        return;
    }

    log::write("ShellWindows disconnected; reconnecting");

    let new_app = match App::new() {
        Ok(a) => a,
        Err(e) => {
            log::write(&format!("reconnect failed: {:?}", e));
            return;
        }
    };
    // The WinEvent hook is independent of the COM channel; preserve it across reconnect.
    let old_hook = app.borrow_mut()._hook.take();
    *app.borrow_mut() = new_app;
    app.borrow_mut()._hook = old_hook;
}

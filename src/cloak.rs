//! Cloak tracker: state-managed wrapper around `win_util::cloak`/`uncloak`.
//!
//! Why track: a window could be cloaked but the SHOW handler that would normally
//! uncloak/Quit it might never run (rare timing race, or the process gets killed
//! mid-merge). Without tracking we could leave Explorer windows permanently invisible.
//!
//! Safety nets exposed here:
//! - [`sweep_stale`] — called from the watchdog (every 1 s); uncloaks anything held
//!   longer than [`STALE_THRESHOLD`] (5 s). The merger's wait loops call [`touch`]
//!   each poll iteration, so a slow-but-progressing merge keeps its cloak.
//! - [`uncloak_all`] — called on graceful exit.
//! - [`recover_orphans`] — called at startup; finds any CabinetWClass top-level with
//!   `WS_EX_LAYERED` + alpha < 255 (left over from a crashed previous instance) and
//!   restores its opacity.
//!
//! All state is thread-local because we live on a single STA thread.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;

use crate::log;
use crate::win_util;

/// Windows cloaked longer than this without being released → safety-net uncloak.
///
/// The merger's wait loops call [`touch`] each poll iteration, which refreshes this
/// timestamp — so as long as the merge is actively polling, the safety net does not
/// fire. The 5 s threshold therefore only triggers if the merger has truly stopped
/// making progress (a silent hang we never observe via timeout returns), not for
/// normal slow merges.
const STALE_THRESHOLD: Duration = Duration::from_secs(5);

thread_local! {
    static CLOAKED: RefCell<HashMap<usize, Instant>> = RefCell::new(HashMap::new());
}

fn key(hwnd: HWND) -> usize {
    hwnd.0 as usize
}

fn hwnd_from_key(k: usize) -> HWND {
    HWND(k as *mut std::ffi::c_void)
}

/// Cloak the window and remember it.
pub fn cloak(hwnd: HWND) {
    win_util::cloak(hwnd);
    CLOAKED.with(|m| {
        m.borrow_mut().insert(key(hwnd), Instant::now());
    });
}

/// Uncloak the window and forget it.
pub fn uncloak(hwnd: HWND) {
    let was_tracked = CLOAKED.with(|m| m.borrow_mut().remove(&key(hwnd)).is_some());
    // Always restore opacity, even for windows we don't recognise — idempotent and safe.
    win_util::uncloak(hwnd);
    let _ = was_tracked;
}

/// Forget the window — it's been destroyed (e.g., by `IWebBrowser2::Quit`) so we no
/// longer need to track or uncloak it. Idempotent.
pub fn forget(hwnd: HWND) {
    CLOAKED.with(|m| {
        m.borrow_mut().remove(&key(hwnd));
    });
}

/// Refresh the cloak timestamp for an already-cloaked window. Called from inside the
/// merger's polling waits — as long as we're actively making progress, the safety-net
/// sweep should not fire. If we stop calling `touch` (because the merge errored out
/// or hung silently), the timestamp ages and sweep eventually uncloaks. No-op if the
/// window isn't being tracked.
pub fn touch(hwnd: HWND) {
    CLOAKED.with(|m| {
        if let Some(entry) = m.borrow_mut().get_mut(&key(hwnd)) {
            *entry = Instant::now();
        }
    });
}

/// Safety net: uncloak any window that's been held cloaked beyond [`STALE_THRESHOLD`].
pub fn sweep_stale() {
    let now = Instant::now();
    let stale: Vec<HWND> = CLOAKED.with(|m| {
        m.borrow()
            .iter()
            .filter(|(_, t)| now.duration_since(**t) > STALE_THRESHOLD)
            .map(|(k, _)| hwnd_from_key(*k))
            .collect()
    });
    for hwnd in stale {
        log::write(&format!("safety net: uncloaking stale {:?}", hwnd.0));
        uncloak(hwnd);
    }
}

/// Uncloak everything we're tracking and clear the map. Call on graceful exit.
pub fn uncloak_all() {
    let all: Vec<HWND> = CLOAKED.with(|m| {
        m.borrow()
            .keys()
            .map(|k| hwnd_from_key(*k))
            .collect()
    });
    for hwnd in all {
        win_util::uncloak(hwnd);
    }
    CLOAKED.with(|m| m.borrow_mut().clear());
}

/// Startup recovery: if a previous run of us crashed mid-merge, Explorer windows may
/// still be parked off-screen carrying our cloak marker property. Walk all CabinetWClass
/// top-levels and restore any that are still marked.
pub fn recover_orphans() {
    let mut restored = 0usize;
    for hwnd in win_util::find_all_explorer_windows() {
        if win_util::is_cloaked(hwnd) {
            win_util::uncloak(hwnd);
            restored += 1;
        }
    }
    if restored > 0 {
        log::write(&format!(
            "startup recovery: restored {} off-screen orphan window(s)",
            restored
        ));
    }
}

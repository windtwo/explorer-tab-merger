//! Explorer Tab Merger — library surface.
//!
//! Most of the implementation lives in these modules; `src/main.rs` is just the entry point.
//! Exposing a library also lets integration tests under `tests/` reach into pure-Rust modules
//! (`log`, `autostart`) for verification.

pub mod autostart;
pub mod log;
pub mod shell_events;
pub mod single_instance;
pub mod tab_merger;
pub mod win_util;

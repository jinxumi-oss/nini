//! EventBus stub.
use std::sync::{Mutex, OnceLock};

pub enum AppEvent {
    ThemeChanged(String),
}

pub struct EventBus;

impl EventBus {
    pub fn new() -> Self {
        Self
    }
    pub fn emit(&self, _event: AppEvent) {}
}

static BUS: OnceLock<Mutex<EventBus>> = OnceLock::new();

/// v0.9.1: poison-free, like [`crate::runtime::lock_state`].
///
/// A poisoned *global* is worse than a poisoned per-run state: it
/// outlives every session, so a single panic anywhere would make the
/// next run of nini panic on its first emitted event. `EventBus::emit`
/// is a no-op today, but the locking discipline should not depend on
/// that staying true.
pub fn global() -> std::sync::MutexGuard<'static, EventBus> {
    BUS.get_or_init(|| Mutex::new(EventBus))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

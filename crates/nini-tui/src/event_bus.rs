//! EventBus stub.
use std::sync::{Mutex, OnceLock};

pub enum AppEvent {
    ThemeChanged(String),
}

pub struct EventBus;

impl EventBus {
    pub fn new() -> Self { Self }
    pub fn emit(&self, _event: AppEvent) {}
}

static BUS: OnceLock<Mutex<EventBus>> = OnceLock::new();

pub fn global() -> std::sync::MutexGuard<'static, EventBus> {
    BUS.get_or_init(|| Mutex::new(EventBus)).lock().unwrap()
}

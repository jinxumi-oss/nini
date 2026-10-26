//! A cancellation signal that cannot lose a wakeup.
//!
//! # Why not `Notify`
//!
//! The TUI used `Arc<Notify>` for "abort this turn": Esc called
//! `notify_waiters()`, and the driver was supposed to wait on it. Two
//! things were wrong with that:
//!
//! 1. **`Notify::notify_waiters()` stores no permit.** If the notification
//!    fires before anybody is waiting, it is *gone* — the turn then runs to
//!    completion and the user's Esc does nothing at all.
//! 2. **Nobody was waiting.** The signal had no consumer in the product
//!    (the driver never read it), so "aborted" was a status string and
//!    nothing else: the model kept streaming after the user pressed Esc.
//!
//! # The shape
//!
//! A flag plus a notification:
//!
//! * `trigger()` sets the flag **first**, then wakes the waiters — so a
//!   waiter that arrives late still sees the flag and returns at once;
//! * `wait()` registers as a waiter **before** it checks the flag — so a
//!   trigger that lands between the check and the await is not lost
//!   either.
//!
//! Both orderings matter, and both are the reason this is a type instead
//! of a bare `Notify`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;

/// A one-shot cancellation signal.
#[derive(Debug, Clone, Default)]
pub struct AbortSignal {
    flag: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl AbortSignal {
    /// A fresh, untriggered signal.
    pub fn new() -> Self {
        Self::default()
    }

    /// Cancel. Idempotent.
    pub fn trigger(&self) {
        // The flag goes up **before** the notification: a waiter that
        // arrives after this call must still see the cancellation.
        if !self.flag.swap(true, Ordering::SeqCst) {
            self.notify.notify_waiters();
        }
    }

    /// True once [`AbortSignal::trigger`] has been called.
    pub fn is_triggered(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Wait until the signal is triggered.
    ///
    /// Returns immediately if it already was. The waiter is registered
    /// *before* the flag is re-checked, so a trigger that lands in
    /// between cannot be lost.
    pub async fn wait(&self) {
        loop {
            if self.is_triggered() {
                return;
            }
            let notified = self.notify.notified();
            tokio::pin!(notified);
            // Register the waiter now — `notified()` only registers when
            // it is first polled.
            notified.as_mut().enable();
            if self.is_triggered() {
                return;
            }
            notified.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_waiter_that_arrives_late_still_sees_the_cancellation() {
        let sig = AbortSignal::new();
        sig.trigger();
        // Would hang if the flag were not checked first.
        sig.wait().await;
        assert!(sig.is_triggered());
    }

    #[tokio::test]
    async fn a_trigger_that_lands_before_the_await_is_not_lost() {
        let sig = AbortSignal::new();
        let s2 = sig.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            s2.trigger();
        });
        sig.wait().await;
        assert!(sig.is_triggered());
    }

    #[tokio::test]
    async fn triggering_twice_is_fine() {
        let sig = AbortSignal::new();
        sig.trigger();
        sig.trigger();
        assert!(sig.is_triggered());
    }
}

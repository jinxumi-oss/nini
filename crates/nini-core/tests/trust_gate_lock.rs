//! The trust gate must not print while it holds the trust store's lock.
//!
//! invariant: bdfdc06d —— 本文件守的是「不变量：不丢行、不持锁打印」。
//! 真正的证据在 docs/invariant-verification.md：注入了什么、哪条断言红了。
//!
//! Why this is a lock and not a style question:
//!
//! While the TUI runs, the terminal guard makes stderr a **pipe**. A
//! pipe has a finite buffer, and a write to a full pipe blocks until
//! somebody reads it. The reader is the guard's thread, and it is fast
//! — but "fast" is not "always": if the reader is busy, or the guard
//! was detached, the write waits.
//!
//! The trust gate used to print that warning **inside** the lock:
//!
//! ```text
//! let store = self.store.lock()?;      // the lock is taken
//! store.check(&call)                   // ... and printed into
//! ```
//!
//! So a blocked print held the trust store's mutex. Every other tool
//! call waits for that same mutex — the agent looks frozen, the
//! transcript stops moving, and Ctrl+C does not help because the
//! thread that would be interrupted is the one blocked in the print.
//!
//! The fix is to take the decision (and the warning) **under** the lock
//! and print it after the lock is gone.
//!
//! The test uses the gate's printer seam: the printer blocks on a
//! channel, so the print is *provably* in flight while the test looks
//! at the lock. (Replacing fd 2 with a full pipe would prove less: it
//! cannot say whether the print had started.)

//! invariant: 81fac45
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nini_core::project_trust::ProjectTrustStore;
use nini_core::tool::{BeforeExecute, ToolContext};
use nini_core::trust_gate::ProjectTrustGate;

/// The gate must not hold the trust store's lock while it prints.
///
/// The print blocks (the printer waits for the test). The lock must
/// not.
#[tokio::test(flavor = "current_thread")]
async fn a_blocked_trust_warning_does_not_hold_the_store_lock() {
    let store = Arc::new(Mutex::new(ProjectTrustStore::default()));
    // The default decision is `Ask`, which is the level that warns.
    let cwd = Path::new("/tmp/nini-trust-gate-lock-test");

    // The printer blocks: it announces that it started, then waits
    // for the test to let it finish.
    let (started_tx, started_rx) = mpsc::channel::<String>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    // The channels go behind a mutex: `Sender`/`Receiver` are `Send`
    // but not `Sync`, and the printer has to be both.
    let started_tx = Mutex::new(started_tx);
    let release_rx = Mutex::new(release_rx);
    let printer: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |message: &str| {
        let _ = started_tx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .send(message.to_string());
        // Block until the test releases the print.
        let _ = release_rx.lock().unwrap_or_else(|e| e.into_inner()).recv();
    });

    let gate = ProjectTrustGate::with_printer(store.clone(), cwd, "bash", printer);

    let handle = tokio::task::spawn_blocking(move || {
        let ctx = ToolContext {
            cwd: cwd.to_path_buf(),
            ..ToolContext::default()
        };
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime")
            .block_on(gate.run(serde_json::json!({"command": "ls"}), &ctx));
        result.expect("the Ask level allows the call");
    });

    // Wait until the print is really in flight.
    let warning = tokio::task::spawn_blocking(move || {
        started_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the warning was never printed")
    })
    .await
    .expect("the waiter panicked");
    assert!(
        warning.contains("trust:"),
        "the warning should be the trust notice, got: {warning}"
    );

    // While the print is in flight, the store's lock must be free:
    // that is the whole point. Another tool call has to be able to
    // proceed.
    assert!(
        store.try_lock().is_ok(),
        "the trust store's lock was held while the warning was being \
         printed: a blocked print (stderr is a full pipe) would then \
         stall every other tool call"
    );

    // Let the print finish.
    let _ = release_tx.send(());
    tokio::time::timeout(Duration::from_secs(10), handle)
        .await
        .expect("the gate never returned")
        .expect("the gate panicked");
}

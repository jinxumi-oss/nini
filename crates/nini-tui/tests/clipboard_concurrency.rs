//! The system clipboard is entered by one caller at a time.
//!
//! Found by reading a native stack, not by reasoning. On `macos-latest`:
//!
//! ```text
//! thread #2, name = 'paste_image_inserts_description_in_prompt'
//! stop reason = EXC_BAD_ACCESS (code=1)
//! frame #0: libobjc objc_msgSend + 32
//! ```
//!
//! `arboard`'s macOS backend is Objective-C. The same test file passes
//! 17/17 with `--test-threads=1` and dies under concurrency, so the defect
//! is two threads inside `generalPasteboard` at once.
//!
//! # What this test can and cannot do
//!
//! It calls only `paste_image_from_clipboard`, so it **compiles against the
//! code that predates the fix** — the fourth time this session a test was
//! written against an API the fix adds, and the point of saying so is that
//! this one deliberately avoids it.
//!
//! On Linux it cannot fail: arboard returns `Err` when there is no display,
//! no crash either way. **The verdict for this defect is the macOS CI leg**,
//! which is why the fix is not claimed on the strength of a green local run.
//! What this test buys on every platform is that the call is safe to make
//! from several threads without the process needing `catch_unwind`.

use nini_tui::image_paste::paste_image_from_clipboard;
#[test]
fn concurrent_clipboard_reads_are_safe() {
    let mut handles = Vec::new();
    for _ in 0..8 {
        handles.push(std::thread::spawn(|| {
            for _ in 0..16 {
                // The result is irrelevant: headless returns Err or Ok(None).
                // Not crashing is the assertion.
                let _ = paste_image_from_clipboard();
            }
        }));
    }
    for h in handles {
        h.join().expect("a clipboard thread panicked");
    }
}

#[test]
fn concurrent_clipboard_reads_and_writes_are_safe_together() {
    let writer = std::thread::spawn(|| {
        for _ in 0..16 {
            let _ = nini_tui::clipboard::copy("nini");
        }
    });
    let mut readers = Vec::new();
    for _ in 0..4 {
        readers.push(std::thread::spawn(|| {
            for _ in 0..16 {
                let _ = paste_image_from_clipboard();
            }
        }));
    }
    writer.join().expect("clipboard writer panicked");
    for h in readers {
        h.join().expect("a clipboard reader panicked");
    }
}

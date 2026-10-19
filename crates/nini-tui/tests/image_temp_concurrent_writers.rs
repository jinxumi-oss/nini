//! Oracle: a pasted image's temp file must be that paste's file.
//!
//! # The defect (`9d954ef0`)
//!
//! `save_image_to_temp` derived its name from a pid and a **millisecond**
//! stamp:
//!
//! ```text
//! format!("nini-image-{}-{}", std::process::id(), d.as_millis())
//! ```
//!
//! Milliseconds are 1/1000th of the resolution the write needs, and a
//! clip-paste is a burst: several pastes inside one millisecond is not a
//! corner case, it is what a user double-pasting three screenshots does.
//! The second paste then wrote over the first — the file the user was told
//! about is not the image they pasted.
//!
//! The commit that introduced the name recorded the flake rate honestly
//! (1 failure in 10 runs, `save_image_writes_file_with_correct_extension`),
//! which is exactly why it survived: a 10% flake is a symptom you can
//! ignore for a long time.
//!
//! (It reported the collision as `os error 17 (File exists)`. `fs::write`
//! creates-or-truncates, so that errno cannot come from it. The damage is
//! silent truncation — one paste's bytes replaced another's — which is the
//! worse failure and the one this oracle asserts against.)
//!
//! # The oracle
//!
//! 16 threads x 32 pastes = 512 writes of **distinct** bytes into
//! `std::env::temp_dir()`, started together:
//!
//! 1. 512 **distinct** paths — a name that collides is not a name;
//! 2. every file exists and its bytes are **its own** payload — this is the
//!    assertion the original symptom should have had, since truncation is
//!    invisible to `exists()`;
//! 3. every name carries this **process's pid**, so two nini processes
//!    pasting at once cannot share a file either. A timestamp never did
//!    that; only identity can.
//!
//! invariant: 9d954ef0

use std::collections::BTreeSet;

use nini_tui::image_paste::save_image_to_temp;

const THREADS: usize = 16;
const PER_THREAD: usize = 32;

/// Distinct bytes per paste, still starting with the PNG magic so the
/// extension sniffer sees a real image.
fn payload(thread: usize, i: usize) -> Vec<u8> {
    let mut bytes = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    bytes.extend_from_slice(&(thread * 1000 + i).to_le_bytes());
    bytes.extend_from_slice(b"nini-image-oracle-payload");
    bytes
}

#[test]
fn concurrent_pastes_get_distinct_files_holding_their_own_bytes() {
    let mut handles = Vec::new();
    for t in 0..THREADS {
        handles.push(std::thread::spawn(move || {
            let mut saved = Vec::new();
            for i in 0..PER_THREAD {
                let bytes = payload(t, i);
                let path = save_image_to_temp(&bytes)
                    .unwrap_or_else(|e| panic!("thread {t} paste {i}: {e}"));
                saved.push((path, bytes));
            }
            saved
        }));
    }

    let mut all = Vec::new();
    for h in handles {
        all.extend(h.join().expect("writer thread did not panic"));
    }

    // 1. Distinct names. 512 pastes, 512 files.
    let distinct: BTreeSet<&std::path::PathBuf> = all.iter().map(|(p, _)| p).collect();
    assert_eq!(
        distinct.len(),
        all.len(),
        "{} of {} pastes were handed a filename another paste already took — \
         one paste silently overwrote another",
        all.len() - distinct.len(),
        all.len()
    );

    // 2. Each file exists and holds its own bytes, not a neighbour's.
    for (path, bytes) in &all {
        assert!(path.exists(), "temp image missing: {}", path.display());
        let on_disk = std::fs::read(path).unwrap_or_default();
        assert_eq!(
            on_disk.len(),
            bytes.len(),
            "truncated: {} now holds {} bytes, not {}",
            path.display(),
            on_disk.len(),
            bytes.len()
        );
        assert_eq!(
            on_disk,
            *bytes,
            "{} holds another paste's bytes",
            path.display()
        );
    }

    // 3. The name says which process wrote it.
    let pid = std::process::id().to_string();
    for (path, _) in &all {
        let name = path
            .file_name()
            .expect("temp image has a file name")
            .to_string_lossy()
            .into_owned();
        assert!(
            name.contains(&pid),
            "temp image {name} carries no process id — two nini processes \
             pasting at the same moment would share it"
        );
        assert!(
            path.extension().and_then(|e| e.to_str()) == Some("png"),
            "temp image {name} lost its sniffed extension"
        );
    }
}

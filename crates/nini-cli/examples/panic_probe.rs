//! Can the binary we ship recover from a panic?
//!
//! Run it with `cargo run --release --example panic_probe -p nini-cli`.
//! `examples/` is the only Cargo target that is built by the **real
//! `[profile.release]`** and is **not** a test harness — and that pair is
//! the whole point. Cargo silently forces `panic = "unwind"` for test
//! targets (`cargo test --release` included), so every hook-isolation test
//! in this workspace passes under `--release` whether or not the shipped
//! binary could do the same thing. An example binary is compiled and run
//! exactly like the deliverable.
//!
//! What is at stake, concretely: `catch_hook_panic`
//! (`nini-core/src/agent_hooks.rs`) returns a default and logs when a
//! **user-supplied** hook panics, and `nini-core/src/agent.rs` does the
//! same around tool calls. That is a product behaviour, and Pi has it too
//! — its hooks are TypeScript, which cannot abort the process. If
//! `panic = "abort"` ever comes back into `[profile.release]`, this
//! example dies before printing `recovered`, and the isolation is gone
//! from the shipped binary while the test suite stays green.
//!
//! Exit code 0 and `recovered` on stdout = the delivery profile unwinds.
//! Abort (exit 134, or a `panic` message with no `recovered` line) = the
//! isolation in `agent_hooks.rs` is dead code in the shipped binary.

fn main() {
    let recovered = std::panic::catch_unwind(|| -> i32 { panic!("hook under test") }).unwrap_or(7);
    if recovered == 7 {
        println!("recovered: the release profile unwinds");
    } else {
        println!("NOT recovered: the profile aborted on a caught panic");
        std::process::exit(1);
    }
}

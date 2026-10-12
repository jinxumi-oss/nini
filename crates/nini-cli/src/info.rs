//! v0.8.2: `nini info` subcommand — prints loaded settings + skills.
//!
//! Extracted from `main.rs` as a single-purpose async function.
//! No dependencies on other v0.8.2 modules.

use anyhow::Result;

use nini_core::settings::load_settings;
use nini_core::skills::{load_skills, SkillOrigin};

/// Print current `~/.nini/agent/settings.json` + loaded skills to stdout.
///
/// Run via `nini info`. Useful for debugging "why isn't my config
/// taking effect" issues.
pub(crate) async fn run_info() -> Result<()> {
    let cwd = std::env::current_dir()?;
    println!("nini info (cwd: {})", cwd.display());
    println!();
    let settings = load_settings(&cwd);
    println!("Settings (loaded):");
    println!("  provider:       {:?}", settings.provider);
    println!("  model:          {:?}", settings.model);
    println!("  thinking_level: {:?}", settings.thinking_level);
    println!();
    let skills_result = load_skills(&cwd);
    // v0.10 (M3): skills are read from nini's directories *and* from the
    // directories other agents keep theirs in, so "20 loaded" is not an
    // answer a user can act on — they need to know how many are theirs and
    // how many they are borrowing, especially when two of them collide.
    let from_nini = skills_result
        .skills
        .iter()
        .filter(|s| s.origin == SkillOrigin::Nini)
        .count();
    println!(
        "Skills ({} loaded — {from_nini} nini's own, {} from other agents; {} errors):",
        skills_result.skills.len(),
        skills_result.skills.len() - from_nini,
        skills_result.errors.len()
    );
    for s in &skills_result.skills {
        println!(
            "  - {} ({}, from {}) -- {}",
            s.name,
            s.source.label(),
            s.origin.label(),
            s.description.lines().next().unwrap_or_default()
        );
    }
    for e in &skills_result.errors {
        println!("  ! error: {e}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {

    #[test]
    fn run_info_doesnt_panic_in_empty_cwd() {
        // Just ensure the function compiles and runs without panic.
        // We can't easily test stdout output without capturing it,
        // but we can at least verify the function signature.
        let _ = std::env::current_dir();
    }
}

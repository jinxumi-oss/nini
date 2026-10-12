//! Oracle: nini reads other agents' skill directories, and depends on none.
//!
//! v0.10 (M3). `paths.rs` (R3) moved every path from `.pi` to `.nini` so
//! nini would stop being a channel into Pi. It moved the skills directory
//! with them, which closed the only cross-ecosystem door nini had: on a
//! machine with Pi installed, nini could read none of Pi's 19 skills and
//! none of Claude Code's 15.
//!
//! These tests are the falsifiable form of the rule that fixes it:
//!
//! > **Assets are shared, credentials are not; formats are compatible,
//! > runtime dependencies are not.**
//!
//! Concretely, two directions have to hold at once, and the interesting
//! cases are the ones where either alone would pass:
//!
//!   1. Another agent's skills are picked up, and
//!   2. nothing about nini breaks when that agent is not installed.
//!
//! A test that only proved (1) would also pass if nini required Pi to be
//! present. A test that only proved (2) is the state R3 left behind.
//!
//! Nothing here touches the real `$HOME`. `skill_roots` takes `home` and
//! `agent_dir` as parameters precisely so that a test can point them at a
//! scratch directory — `thinking_persistence.rs` once overwrote this
//! machine's actual `~/.nini/agent/settings.json` because a test reached
//! for the real one.

use nini_core::skills::{load_skills_from_roots, skill_roots, SkillOrigin, SkillSource};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn write_skill(skills_dir: &Path, name: &str, description: &str) {
    let dir = skills_dir.join(name);
    std::fs::create_dir_all(&dir).expect("create skill dir");
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\nbody of {name}\n"),
    )
    .expect("write SKILL.md");
}

/// A scratch machine: `<tmp>/home`, `<tmp>/agent`, `<tmp>/project`.
struct Scratch {
    _dir: TempDir,
    home: PathBuf,
    agent: PathBuf,
    project: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path().to_path_buf();
        let s = Self {
            home: root.join("home"),
            agent: root.join("agent"),
            project: root.join("project"),
            _dir: dir,
        };
        std::fs::create_dir_all(&s.home).unwrap();
        std::fs::create_dir_all(&s.agent).unwrap();
        std::fs::create_dir_all(&s.project).unwrap();
        s
    }

    fn pi_skills(&self) -> PathBuf {
        self.home.join(".pi").join("agent").join("skills")
    }

    fn claude_skills(&self) -> PathBuf {
        self.home.join(".claude").join("skills")
    }

    fn nini_skills(&self) -> PathBuf {
        self.agent.join("skills")
    }

    fn roots(&self) -> Vec<nini_core::skills::SkillRoot> {
        skill_roots(Some(&self.home), Some(&self.agent), &self.project)
    }

    fn load(&self) -> nini_core::skills::LoadSkillsResult {
        load_skills_from_roots(&self.roots())
    }
}

fn find<'a>(
    result: &'a nini_core::skills::LoadSkillsResult,
    name: &str,
) -> &'a nini_core::skills::Skill {
    result
        .skills
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| {
            panic!(
                "`{name}` was not loaded. Loaded: {:?}",
                result.skills.iter().map(|s| &s.name).collect::<Vec<_>>()
            )
        })
}

// ---------------------------------------------------------------------------
// Direction 1: another agent's assets are readable.
// ---------------------------------------------------------------------------

#[test]
fn skills_written_by_pi_are_read() {
    let s = Scratch::new();
    write_skill(&s.pi_skills(), "firecrawl", "Pi's skill");

    let result = s.load();
    let skill = find(&result, "firecrawl");
    assert_eq!(skill.description, "Pi's skill");
    assert_eq!(skill.origin, SkillOrigin::Pi);
}

#[test]
fn skills_written_by_claude_code_are_read() {
    let s = Scratch::new();
    write_skill(&s.claude_skills(), "pdf", "Claude Code's skill");

    let result = s.load();
    let skill = find(&result, "pdf");
    assert_eq!(skill.description, "Claude Code's skill");
    assert_eq!(skill.origin, SkillOrigin::Claude);
}

#[test]
fn a_skill_from_another_agent_is_reachable_by_the_model() {
    // Loading a skill is only half of it: the model has to be told where the
    // file is, or it advertises a skill it cannot open. That was v0.9.2's
    // P0-1 in a different costume.
    let s = Scratch::new();
    write_skill(&s.pi_skills(), "firecrawl", "Pi's skill");

    let result = s.load();
    let prompt = nini_core::skills::format_skills_for_prompt(&result.skills);
    let expected = s.pi_skills().join("firecrawl").join("SKILL.md");
    assert!(
        prompt.contains(&expected.display().to_string()),
        "the prompt must carry the real path so `read` can open it. prompt:\n{prompt}"
    );
}

// ---------------------------------------------------------------------------
// Direction 2: nothing depends on them being there.
// ---------------------------------------------------------------------------

#[test]
fn nini_works_on_a_machine_with_no_other_agent_installed() {
    let s = Scratch::new();
    write_skill(&s.nini_skills(), "mine", "nini's own");

    let result = s.load();
    assert_eq!(
        result.skills.len(),
        1,
        "a clean machine must still get its own skills, errors: {:?}",
        result.errors
    );
    assert_eq!(find(&result, "mine").origin, SkillOrigin::Nini);
}

#[test]
fn an_absent_ecosystem_directory_is_not_an_error() {
    let s = Scratch::new();
    write_skill(&s.nini_skills(), "mine", "nini's own");
    // ~/.pi and ~/.claude were never created.

    let result = s.load();
    assert!(
        result.errors.is_empty(),
        "missing roots must not error: {:?}",
        result.errors
    );
    assert_eq!(result.skills.len(), 1);
}

#[test]
fn removing_pis_directory_only_removes_pis_skills() {
    let s = Scratch::new();
    write_skill(&s.pi_skills(), "theirs", "Pi's");
    write_skill(&s.nini_skills(), "mine", "nini's");

    let before = s.load();
    assert_eq!(before.skills.len(), 2);

    std::fs::remove_dir_all(s.home.join(".pi")).unwrap();

    let after = s.load();
    assert_eq!(
        after.skills.len(),
        1,
        "nini's own skill must survive Pi being uninstalled"
    );
    assert_eq!(find(&after, "mine").origin, SkillOrigin::Nini);
}

// ---------------------------------------------------------------------------
// Precedence: nini's own copy is authoritative, and project beats user.
// ---------------------------------------------------------------------------

#[test]
fn a_project_skill_shadows_a_user_skill_of_the_same_name() {
    let s = Scratch::new();
    write_skill(&s.nini_skills(), "shared", "user-level");
    write_skill(
        &s.project.join(".nini").join("skills"),
        "shared",
        "project-level",
    );

    let result = s.load();
    assert_eq!(find(&result, "shared").description, "project-level");
    assert_eq!(find(&result, "shared").source, SkillSource::Project);
}

#[test]
fn ninis_copy_of_a_name_wins_over_pis_and_claudes() {
    let s = Scratch::new();
    write_skill(&s.pi_skills(), "shared", "Pi's version");
    write_skill(&s.claude_skills(), "shared", "Claude's version");
    write_skill(&s.nini_skills(), "shared", "nini's version");

    let result = s.load();
    let skill = find(&result, "shared");
    assert_eq!(skill.description, "nini's version");
    assert_eq!(skill.origin, SkillOrigin::Nini);
}

#[test]
fn a_name_present_in_one_ecosystem_is_unaffected_by_the_other() {
    // The rule above resolves collisions without silently adopting an
    // entire directory: these two names exist only once and must not be
    // dropped or merged.
    let s = Scratch::new();
    write_skill(&s.pi_skills(), "only-pi", "a");
    write_skill(&s.claude_skills(), "only-claude", "b");
    write_skill(&s.nini_skills(), "shared", "c");

    let result = s.load();
    assert_eq!(result.skills.len(), 3);
    assert_eq!(find(&result, "only-pi").origin, SkillOrigin::Pi);
    assert_eq!(find(&result, "only-claude").origin, SkillOrigin::Claude);
}

// ---------------------------------------------------------------------------
// The boundary itself: assets only, never credentials.
// ---------------------------------------------------------------------------

/// The boundary is only meaningful if it can fail. This asserts it on the
/// root list, which is the one place a new directory could be added.
///
/// Every root must end in `skills`. Pi's credential store is a sibling of
/// that directory — `~/.pi/agent/auth.json` — so a root pointing at
/// `~/.pi/agent` instead would satisfy "we read Pi's files" while sweeping
/// in the key. The last path segment is what separates the two.
#[test]
fn every_root_is_a_skills_directory_and_nothing_else() {
    let s = Scratch::new();
    for root in s.roots() {
        assert_eq!(
            root.dir.file_name().and_then(|n| n.to_str()),
            Some("skills"),
            "{} points at {} which is not a skills directory",
            root.origin.label(),
            root.dir.display()
        );
    }
}

/// The mirror image: credentials must not appear anywhere in the discovery
/// list, by name or by directory.
#[test]
fn no_root_can_reach_a_credential_store() {
    let s = Scratch::new();
    for root in s.roots() {
        let rendered = root.dir.display().to_string();
        assert!(
            !rendered.contains("auth"),
            "{} would expose a credential store: {rendered}",
            root.origin.label()
        );
    }
}

/// The three origins are a closed set. Adding a fourth agent means deciding
/// what reading its directory means, and this makes that a compile-visible
/// decision rather than a silent default.
#[test]
fn the_origin_set_is_closed_and_labelled() {
    assert_eq!(SkillOrigin::Nini.label(), "nini");
    assert_eq!(SkillOrigin::Pi.label(), "pi");
    assert_eq!(SkillOrigin::Claude.label(), "claude");
}

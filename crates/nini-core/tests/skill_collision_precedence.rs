//! Who wins a skill-name collision — and who is told about it.
//!
//! v0.16 (D1). Before this round the loader decided collisions **silently**
//! (`by_name.entry(name).or_insert(skill)` — the loser just vanished), so
//! the user had no way to know that the `review` skill they pointed
//! `--skill` at was being shadowed by a `review` skill in a directory they
//! also pointed at. Two things are pinned here:
//!
//! 1. **The precedence rule itself.** `--skill`'s help text says it
//!    "Overrides automatic skill discovery", and `load_skills_from_paths`'s
//!    own comment said the same thing — but the code did the opposite: the
//!    roots were inserted first and `or_insert` kept them, so an
//!    explicitly named `SKILL.md` **lost** to a same-named skill inside a
//!    named directory. The code was wrong; the two independent statements
//!    of intent were right.
//! 2. **The loser is reported.** Every collision produces a
//!    [`SkillCollision`] with the winner's and the loser's `SKILL.md`
//!    paths — the files the loader actually read, and the paths a user can
//!    open.
//!
//! Nothing here touches the real `$HOME`: every path is inside a
//! `TempDir`.

use nini_core::skills::{
    load_skills_from_paths, load_skills_from_roots, SkillCollision, SkillOrigin, SkillRoot,
    SkillSource,
};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Write `<dir>/<name>/SKILL.md` and return the `SKILL.md` path.
fn write_skill(dir: &Path, name: &str, description: &str) -> PathBuf {
    let skill_dir = dir.join(name);
    std::fs::create_dir_all(&skill_dir).expect("create skill dir");
    let file = skill_dir.join("SKILL.md");
    std::fs::write(
        &file,
        format!("---\nname: {name}\ndescription: {description}\n---\n\nbody of {name}\n"),
    )
    .expect("write SKILL.md");
    file
}

/// The description of the skill that was loaded under `name`.
fn description_of(result: &nini_core::skills::LoadSkillsResult, name: &str) -> Option<String> {
    result
        .skills
        .iter()
        .find(|s| s.name == name)
        .map(|s| s.description.clone())
}

/// The collision recorded for `name`, if any.
fn collision_for<'a>(collisions: &'a [SkillCollision], name: &str) -> Option<&'a SkillCollision> {
    collisions.iter().find(|c| c.name == name)
}

#[test]
fn an_explicitly_named_skill_beats_a_same_named_skill_in_a_named_directory() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();

    // `--skill <dir>/review/SKILL.md` — named explicitly.
    let explicit = write_skill(&root.join("explicit"), "review", "the explicit one");
    // `--skill <dir>/bundle/` — a directory, which contributes a root.
    let bundle = root.join("bundle");
    let from_root = write_skill(&bundle, "review", "the one inside the directory");

    let result = load_skills_from_paths(&[explicit.clone(), bundle.clone()]);

    assert_eq!(
        description_of(&result, "review").as_deref(),
        Some("the explicit one"),
        "the explicitly named skill must win the collision — `--skill` is an \
         override, and the loader's own comment said so"
    );

    let collision = collision_for(&result.collisions, "review")
        .expect("the collision must be reported, not swallowed");
    assert_eq!(collision.winner, explicit);
    assert_eq!(collision.loser, from_root);
}

#[test]
fn the_first_of_two_explicitly_named_skills_wins_and_the_second_is_reported() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();

    let first = write_skill(&root.join("a"), "review", "the first one");
    let second = write_skill(&root.join("b"), "review", "the second one");

    let result = load_skills_from_paths(&[first.clone(), second.clone()]);

    assert_eq!(
        description_of(&result, "review").as_deref(),
        Some("the first one"),
        "the first explicitly named skill wins — the old `or_insert` order"
    );
    let collision = collision_for(&result.collisions, "review")
        .expect("the second explicit skill must be reported");
    assert_eq!(collision.winner, first);
    assert_eq!(collision.loser, second);
}

#[test]
fn a_root_skill_is_loaded_when_nothing_explicit_claims_its_name() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();

    let explicit = write_skill(&root.join("explicit"), "review", "the explicit one");
    let bundle = root.join("bundle");
    write_skill(&bundle, "deploy", "the unrelated one");

    let result = load_skills_from_paths(&[explicit, bundle.clone()]);

    assert_eq!(
        description_of(&result, "deploy").as_deref(),
        Some("the unrelated one"),
        "a root skill whose name is free must still be loaded"
    );
    assert!(
        collision_for(&result.collisions, "deploy").is_none(),
        "no collision, no report"
    );
}

#[test]
fn the_later_root_wins_a_root_against_root_collision_and_the_loser_is_reported() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();

    let low = root.join("low");
    let high = root.join("high");
    let low_file = write_skill(&low, "review", "from the earlier root");
    let high_file = write_skill(&high, "review", "from the later root");

    let roots = vec![
        SkillRoot {
            dir: low.clone(),
            source: SkillSource::User,
            origin: SkillOrigin::Nini,
        },
        SkillRoot {
            dir: high.clone(),
            source: SkillSource::User,
            origin: SkillOrigin::Nini,
        },
    ];
    let result = load_skills_from_roots(&roots);

    assert_eq!(
        description_of(&result, "review").as_deref(),
        Some("from the later root"),
        "the later root wins — the loader reverses the root order and keeps \
         the first definition it sees"
    );
    let collision =
        collision_for(&result.collisions, "review").expect("the loser must be reported");
    assert_eq!(collision.winner, high_file);
    assert_eq!(collision.loser, low_file);
}

#[test]
fn a_clean_directory_produces_no_collisions() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();

    let a = write_skill(&root.join("a"), "review", "one");
    let b = write_skill(&root.join("b"), "deploy", "two");

    let result = load_skills_from_paths(&[a, b]);

    assert_eq!(result.skills.len(), 2);
    assert!(
        result.collisions.is_empty(),
        "nothing collided, so nothing is reported: {:?}",
        result.collisions
    );
}

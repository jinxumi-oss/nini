//! Oracle: the skill count `nini info` prints must be **this** machine's,
//! counted from **this** process's environment — never a developer's.
//!
//! # The defect (`6eb77154`)
//!
//! The only assertion about the skills header was:
//!
//! ```text
//! // User's ~/.pi/agent/skills should have entries (we saw 18 earlier)
//! assert!(out.lines().filter(|l| l.starts_with("  - ")).count() >= 1);
//! ```
//!
//! `>= 1` is a claim about the **author's laptop**. It is true wherever
//! somebody has ever installed an agent and false on a clean runner, so it
//! was a red build on CI for a reason that had nothing to do with nini.
//! The repair stopped the *test* from asserting a number it had no right
//! to assume — and in doing so stopped asserting a number **at all**.
//!
//! # Why the weaker assertion is still wrong
//!
//! `skill_count == header_count` (what the repair settled for) passes when
//! the header says `Skills (29 loaded …)` because it found 29 skills in
//! whatever `$HOME` the runner happened to have. It says nothing about
//! whether those 29 came from *this* process's environment.
//!
//! # The oracle
//!
//! Drive the real binary with a **fully specified** environment and assert
//! the numbers against what was planted, not against a minimum:
//!
//! | environment | asserted header | asserted list length |
//! |---|---|---|
//! | nothing planted anywhere | `0 loaded — 0 nini's own, 0 from other agents` | 0 |
//! | 2 in `$NINI_AGENT_DIR/skills`, 1 in `$HOME/.pi/agent/skills` | `3 loaded — 2 nini's own, 1 from other agents` | 3 |
//!
//! Row 2 is the load-bearing one: on a machine that has 29 skills in the
//! real `~`, a process whose `$HOME` is a scratch directory must still
//! print `3`. If any code path reads the ambient environment instead of the
//! process's, the number moves and this goes red on **every** machine —
//! which is what makes it a test rather than a snapshot.
//!
//! invariant: 6eb77154

use std::path::Path;
use std::process::Command;

/// Run `nini info` with `home` / `agent_dir` / `cwd` as its *only* idea of
/// where anything lives, and return stdout.
fn info_with_env(home: &Path, agent_dir: &Path, cwd: &Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_nini"))
        .arg("info")
        .current_dir(cwd)
        // env_clear, not env: an inherited `HOME` (or an inherited
        // `NINI_AGENT_DIR` from whatever runner this is) is precisely the
        // input under test.
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", home)
        .env("NINI_AGENT_DIR", agent_dir)
        .output()
        .expect("nini info runs");
    assert!(
        out.status.success(),
        "nini info exited {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// `(loaded, nini's own, from other agents)` as printed on the header line.
///
/// The line is
/// `Skills (3 loaded — 2 nini's own, 1 from other agents; 0 errors):`
/// so the numbers sit in fixed grammatical slots. Parsed positionally so a
/// change to the prose does not silently pass and a change to a number
/// cannot.
fn header_counts(out: &str) -> (usize, usize, usize) {
    let line = out
        .lines()
        .find(|l| l.starts_with("Skills ("))
        .unwrap_or_else(|| panic!("no Skills header in:\n{out}"));
    let tok: Vec<&str> = line.split_whitespace().collect();
    let num = |i: usize| -> usize {
        tok.get(i)
            .unwrap_or_else(|| panic!("token {i} missing from header: {line}"))
            .trim_matches(|c: char| !c.is_ascii_digit())
            .parse()
            .unwrap_or_else(|_| panic!("token {i} is not a number in header: {line}"))
    };
    // tok: 0=Skills 1=(N 2=loaded 3=— 4=nini's-own 5=own, 6=own 7=borrowed
    (num(1), num(4), num(7))
}

/// How many skill entries were actually rendered.
fn listed(out: &str) -> usize {
    out.lines().filter(|l| l.starts_with("  - ")).count()
}

/// Plant one skill: `<root>/<name>/SKILL.md` with frontmatter.
fn plant_skill(root: &Path, name: &str) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: planted by the oracle\n---\n\nbody\n"),
    )
    .unwrap();
}

/// An empty, fully controlled universe: empty `$HOME`, empty agent dir,
/// empty project directory. Nothing planted, so nothing may be reported.
#[test]
fn an_empty_environment_reports_zero_skills() {
    let base = tempfile::tempdir().unwrap();
    let home = base.path().join("home");
    let agent = base.path().join("agent");
    let cwd = base.path().join("project");
    for d in [&home, &agent, &cwd] {
        std::fs::create_dir_all(d).unwrap();
    }

    let out = info_with_env(&home, &agent, &cwd);
    assert_eq!(
        header_counts(&out),
        (0, 0, 0),
        "an environment with no skills planted must report none, whatever \
         the machine running this test has installed:\n{out}"
    );
    assert_eq!(
        listed(&out),
        0,
        "header said 0 but lines were rendered:\n{out}"
    );
}

/// The count follows the *process's* environment, and the origin split is
/// distinguished. Two skills nini's own, one borrowed from Pi's directory:
/// `3 loaded — 2 nini's own, 1 from other agents`.
#[test]
fn the_count_follows_the_process_environment_not_the_machine() {
    let base = tempfile::tempdir().unwrap();
    let home = base.path().join("home");
    let agent = base.path().join("agent");
    let cwd = base.path().join("project");
    for d in [&home, &agent, &cwd] {
        std::fs::create_dir_all(d).unwrap();
    }

    let skills = agent.join("skills");
    plant_skill(&skills, "mine-one");
    plant_skill(&skills, "mine-two");
    // Pi's directory, which nini reads for assets but never writes.
    plant_skill(&home.join(".pi").join("agent").join("skills"), "borrowed");

    let out = info_with_env(&home, &agent, &cwd);
    assert_eq!(
        header_counts(&out),
        (3, 2, 1),
        "the header must describe exactly the skills planted in this \
         process's environment — not the ambient one:\n{out}"
    );
    assert_eq!(
        listed(&out),
        3,
        "header and rendered list must agree:\n{out}"
    );
    for name in ["mine-one", "mine-two", "borrowed"] {
        assert!(
            out.contains(name),
            "planted skill {name} missing from `nini info`:\n{out}"
        );
    }
}

/// A skill planted *outside* the process's environment is invisible: the
/// same binary, one more directory in the machine's real `$HOME`, must not
/// move the number. This is the direction the old `>= 1` got backwards —
/// it asserted that the machine contributed, this asserts it cannot.
#[test]
fn a_skill_outside_the_process_environment_is_not_counted() {
    let base = tempfile::tempdir().unwrap();
    let home = base.path().join("home");
    let agent = base.path().join("agent");
    let cwd = base.path().join("project");
    for d in [&home, &agent, &cwd] {
        std::fs::create_dir_all(d).unwrap();
    }
    // The machine's real user-level skills, inherited the way a shell
    // would inherit them — except through a variable nini is not reading.
    plant_skill(&home.join(".claude").join("skills"), "on-another-agent");
    plant_skill(&cwd.join(".nini").join("skills"), "in-the-project");

    let out = info_with_env(&home, &agent, &cwd);
    assert_eq!(
        header_counts(&out),
        (2, 1, 1),
        "nini's own project skill plus the borrowed one, and nothing else:\n{out}"
    );
}

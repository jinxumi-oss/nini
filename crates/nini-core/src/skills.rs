//! Skills loader: parse `SKILL.md` files from nini's own directories and
//! from the directories other agents keep theirs in.
//!
//! # The boundary this module draws
//!
//! nini is a rewrite of Pi, not a channel into one. Two consequences, and
//! they point in opposite directions:
//!
//!   * **Runtime paths are nini's own** (`~/.nini/agent/`). Nothing here
//!     starts, requires, or reads the private state of another agent.
//!   * **Skill *files* are shared, portable assets.** `SKILL.md` is the
//!     Anthropic agent-skills format, not a Pi invention. A skill directory
//!     is prose on disk; reading one creates no dependency on the agent that
//!     wrote it.
//!
//! v0.10 (M3) fixed what R3 broke. R3 moved every path from `.pi` to `.nini`
//! to stop nini being a channel, and it moved the skills directory with
//! them. That was the right rule applied one step too far: it closed the
//! only cross-ecosystem door nini had. On a machine with Pi installed, nini
//! could see zero of the 19 skills Pi had, and none of the 15 Claude Code
//! had either, while its status bar sat there claiming a clean install.
//!
//! The rule this module now implements, in one line:
//!
//! > **Assets are shared, credentials are not; formats are compatible,
//! > runtime dependencies are not.**
//!
//! The falsifiable form of that rule is the one `clean_home.rs` checks:
//! **delete `~/.pi` entirely and nini must keep working**, with fewer
//! skills. A skill directory that vanishes must never be load-bearing.
//!
//! What is deliberately *not* read from another agent's directory:
//! `auth.json`, any credential, any key. Those are the channel R3 closed
//! and `6abadd9` kept closed — see the revert for why parity does not
//! settle it.
//!
//! Mirrors spec `packages/coding-agent/src/core/skills.ts`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Skill metadata from the frontmatter.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, rename = "disable-model-invocation")]
    pub disable_model_invocation: Option<bool>,
}

/// A loaded skill.
#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
    pub base_dir: PathBuf,
    pub source: SkillSource,
    /// Which agent's directory this skill was read from.
    ///
    /// Carried rather than discarded because "nini loaded 34 skills" is a
    /// different claim from "nini loaded 34 skills, 19 of which Pi wrote",
    /// and only the second one can be acted on when two of them disagree.
    pub origin: SkillOrigin,
    pub disable_model_invocation: bool,
}

/// Where a skill was loaded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillSource {
    User,    // a user-level directory, e.g. ~/.nini/agent/skills/
    Project, // a project-level directory, e.g. <cwd>/.nini/skills/
}

impl SkillSource {
    pub fn label(&self) -> &'static str {
        match self {
            SkillSource::User => "user",
            SkillSource::Project => "project",
        }
    }
}

/// Which agent's directory a skill came from.
///
/// This is about *provenance*, not priority — [`SkillSource`] decides which
/// directory wins a name collision, and within one level nini's own
/// directory wins over an ecosystem one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillOrigin {
    /// nini's own directory.
    Nini,
    /// Pi's directory. Read-only, data only.
    Pi,
    /// Claude Code's directory. Read-only, data only.
    Claude,
}

impl SkillOrigin {
    /// The label shown to the user, e.g. in `nini info`.
    pub fn label(&self) -> &'static str {
        match self {
            SkillOrigin::Nini => "nini",
            SkillOrigin::Pi => "pi",
            SkillOrigin::Claude => "claude",
        }
    }
}

/// One directory that skills are loaded from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRoot {
    pub source: SkillSource,
    pub origin: SkillOrigin,
    pub dir: PathBuf,
}

/// Result of loading skills.
#[derive(Debug, Clone, Default)]
pub struct LoadSkillsResult {
    pub skills: Vec<Skill>,
    pub errors: Vec<String>,
}

/// Load skills from every directory nini will read.
///
/// Own directories first-class; other agents' directories read for their
/// `SKILL.md` files only. See the module docs for why that is a dependency
/// on nothing.
pub fn load_skills(cwd: &Path) -> LoadSkillsResult {
    load_skills_from_roots(&skill_roots(
        crate::paths::home_dir().as_deref(),
        crate::paths::agent_dir().as_deref(),
        cwd,
    ))
}

/// Every directory skills are loaded from, **ordered lowest priority
/// first** — later entries shadow earlier ones on a name collision.
///
/// The order encodes two rules, in this sequence:
///   1. project beats user, and
///   2. within a level, nini's own directory beats an ecosystem's.
///
/// Rule 2 is what keeps a nini user's edits authoritative without making
/// Pi's directory unreadable: a name present in both resolves to nini's
/// copy, and every other name is picked up.
///
/// `home` and `agent_dir` are parameters rather than lookups so a test can
/// point them at a scratch directory. Reading the real `$HOME` from a test
/// is how `thinking_persistence.rs` once overwrote this machine's actual
/// settings file.
pub fn skill_roots(home: Option<&Path>, agent_dir: Option<&Path>, cwd: &Path) -> Vec<SkillRoot> {
    let mut roots = Vec::new();
    if let Some(h) = home {
        roots.push(SkillRoot {
            source: SkillSource::User,
            origin: SkillOrigin::Pi,
            dir: h.join(".pi").join("agent").join("skills"),
        });
        roots.push(SkillRoot {
            source: SkillSource::User,
            origin: SkillOrigin::Claude,
            dir: h.join(".claude").join("skills"),
        });
    }
    if let Some(a) = agent_dir {
        roots.push(SkillRoot {
            source: SkillSource::User,
            origin: SkillOrigin::Nini,
            dir: a.join("skills"),
        });
    }
    roots.push(SkillRoot {
        source: SkillSource::Project,
        origin: SkillOrigin::Pi,
        dir: cwd.join(".pi").join("skills"),
    });
    roots.push(SkillRoot {
        source: SkillSource::Project,
        origin: SkillOrigin::Claude,
        dir: cwd.join(".claude").join("skills"),
    });
    roots.push(SkillRoot {
        source: SkillSource::Project,
        origin: SkillOrigin::Nini,
        dir: crate::paths::project_file(cwd, "skills"),
    });
    roots
}

/// Load from an explicit root list. The seam `load_skills` is built on, so
/// the precedence rules above are testable without a `$HOME`.
/// v0.10 (fix): load skills from paths the user named explicitly
/// (`--skill a,b,c`).
///
/// Distinct from [`load_skills`] in two ways that matter, both of which
/// `--help` promises:
///
/// * **"Overrides automatic skill discovery."** A user who names one
///   directory gets that directory. Appending it to the discovered set would
///   mean the flag cannot be used to *narrow* anything, which is the only
///   reason to reach for it.
/// * **A path that is not there is an error.** `load_from_dir` returns
///   silently on a missing directory, which is right for discovery (most
///   candidate roots do not exist on most machines) and wrong for an
///   explicit path: the user typed it, and an empty skill set that looks
///   like "I have no skills installed" sends them looking in the wrong
///   place.
pub fn load_skills_from_paths(paths: &[std::path::PathBuf]) -> LoadSkillsResult {
    let mut result = LoadSkillsResult::default();
    let mut roots = Vec::new();
    for p in paths {
        match std::fs::metadata(p) {
            Ok(meta) if meta.is_file() => {
                // `--skill path/to/SKILL.md` — one skill, named directly.
                match load_skill_file(p, SkillSource::User, SkillOrigin::Nini) {
                    Ok(sk) => result.skills.push(sk),
                    Err(e) => result.errors.push(format!("{}: {e}", p.display())),
                }
            }
            Ok(meta) if meta.is_dir() => {
                // A directory *containing* SKILL.md is itself one skill —
                // which is what `~/.claude/skills/my-skill` is, and what a
                // user who copied one skill somewhere expects to be able to
                // name. Anything else is a skills *root*, whose
                // subdirectories each hold a SKILL.md.
                if p.join("SKILL.md").is_file() {
                    match load_skill_file(&p.join("SKILL.md"), SkillSource::User, SkillOrigin::Nini)
                    {
                        Ok(sk) => result.skills.push(sk),
                        Err(e) => result.errors.push(format!("{}: {e}", p.display())),
                    }
                } else {
                    roots.push(SkillRoot {
                        source: SkillSource::User,
                        origin: SkillOrigin::Nini,
                        dir: p.clone(),
                    });
                }
            }
            Ok(_) => {
                result
                    .errors
                    .push(format!("--skill {}: not a file or directory", p.display()));
            }
            Err(e) => result.errors.push(format!("--skill {}: {e}", p.display())),
        }
    }
    // Merge rather than replace: some paths contributed one skill directly
    // (a named SKILL.md, or a directory that is itself one) and the rest
    // contributed roots. Explicitly named skills win a name collision —
    // `--skill` is an override, so it should not be shadowed by a root
    // listed after it.
    let loaded = load_skills_from_roots(&roots);
    let mut by_name: std::collections::BTreeMap<String, Skill> = loaded
        .skills
        .into_iter()
        .map(|s| (s.name.clone(), s))
        .collect();
    for sk in result.skills {
        by_name.entry(sk.name.clone()).or_insert(sk);
    }
    result.skills = by_name.into_values().collect();
    result.errors.extend(loaded.errors);
    result
}

pub fn load_skills_from_roots(roots: &[SkillRoot]) -> LoadSkillsResult {
    let mut result = LoadSkillsResult::default();

    let mut all: Vec<Skill> = Vec::new();
    for root in roots {
        load_from_dir(
            &root.dir,
            root.source,
            root.origin,
            &mut all,
            &mut result.errors,
        );
    }

    // `all` is in root order (lowest first), so reversing and taking the
    // first `or_insert` makes the last definition win.
    let mut by_name: std::collections::BTreeMap<String, Skill> = std::collections::BTreeMap::new();
    for s in all.into_iter().rev() {
        by_name.entry(s.name.clone()).or_insert(s);
    }
    result.skills = by_name.into_values().collect();
    result
}

fn load_from_dir(
    dir: &Path,
    source: SkillSource,
    origin: SkillOrigin,
    out: &mut Vec<Skill>,
    errors: &mut Vec<String>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skill_md = path.join("SKILL.md");
        if !skill_md.is_file() {
            continue;
        }
        match load_skill_file(&skill_md, source, origin) {
            Ok(skill) => out.push(skill),
            Err(e) => errors.push(format!("{}: {}", skill_md.display(), e)),
        }
    }
}

fn load_skill_file(path: &Path, source: SkillSource, origin: SkillOrigin) -> Result<Skill, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let (fm, body) = split_frontmatter(&text);
    let fm: SkillFrontmatter = match fm {
        Some(s) => serde_yaml::from_str(&s).map_err(|e| e.to_string())?,
        None => SkillFrontmatter::default(),
    };
    let base_dir = path
        .parent()
        .ok_or_else(|| "no parent".to_string())?
        .to_path_buf();
    let name = fm
        .name
        .clone()
        .or_else(|| {
            base_dir
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string())
        })
        .ok_or_else(|| "no name".to_string())?;
    let description = fm.description.unwrap_or_default();
    Ok(Skill {
        name,
        description,
        body,
        base_dir,
        source,
        origin,
        disable_model_invocation: fm.disable_model_invocation.unwrap_or(false),
    })
}

/// Split `SKILL.md` into frontmatter (between `---` lines) and body.
fn split_frontmatter(text: &str) -> (Option<String>, String) {
    let trimmed = text.trim_start_matches('\n');
    if !trimmed.starts_with("---") {
        return (None, text.to_string());
    }
    let after_first = &trimmed[3..];
    let after_first = after_first.trim_start_matches('\n');
    if let Some(end_idx) = after_first.find("\n---") {
        let fm = &after_first[..end_idx];
        let rest_start = end_idx + 4;
        let body = after_first[rest_start..]
            .trim_start_matches('\n')
            .to_string();
        return (Some(fm.to_string()), body);
    }
    (None, text.to_string())
}

/// nini's own user-level skills directory: `~/.nini/agent/skills/`.
///
/// This is nini's directory and the one it writes to. It is **not** the
/// only one it reads — see [`skill_roots`] for the full list.
pub fn user_skills_dir() -> Option<PathBuf> {
    crate::paths::agent_file("skills")
}

/// nini's own project-level skills directory: `<cwd>/.nini/skills/`.
///
/// Not the only one read either; `<cwd>/.pi/skills/` and
/// `<cwd>/.claude/skills/` are read for their `SKILL.md` files. See
/// [`skill_roots`].
pub fn project_skills_dir(cwd: &Path) -> Option<PathBuf> {
    Some(crate::paths::project_file(cwd, "skills"))
}

/// Render the loaded skills into a system-prompt fragment (Pi-style
/// `<available_skills>` XML with `<name>`, `<description>`, `<location>`).
///
/// The `<location>` element is critical: it gives the model the file path
/// it needs to use the `read` tool to actually load the skill content.
/// Without it, skills are advertised but unreachable (the model knows
/// they exist but can't fetch them).
pub fn format_skills_for_prompt(skills: &[Skill]) -> String {
    let visible: Vec<&Skill> = skills
        .iter()
        .filter(|s| !s.disable_model_invocation)
        .collect();
    if visible.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n\nThe following skills provide specialized instructions for specific tasks.\n\
         Use the read tool to load a skill's file when the task matches its description.\n\
         When a skill file references a relative path, resolve it against the skill directory.\n\
         \n\
         <available_skills>\n",
    );
    for s in visible {
        // v0.8.6: skill file lives at base_dir/SKILL.md (see load_skill).
        let skill_path = s.base_dir.join("SKILL.md");
        out.push_str(&format!(
            "  <skill>\n    <name>{}</name>\n    <description>{}</description>\n    <location>{}</location>\n  </skill>\n",
            xml_escape(&s.name),
            xml_escape(&s.description),
            xml_escape(&skill_path.display().to_string()),
        ));
    }
    out.push_str("</available_skills>\n");
    out
}

/// XML-escape special characters in skill name/description/path.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_frontmatter_basic() {
        let text = "---\nname: test\ndescription: hello\n---\nbody text\n";
        let (fm, body) = split_frontmatter(text);
        assert_eq!(fm.as_deref(), Some("name: test\ndescription: hello"));
        assert_eq!(body, "body text\n");
    }

    #[test]
    fn split_frontmatter_absent() {
        let (fm, body) = split_frontmatter("just a body\n");
        assert!(fm.is_none());
        assert_eq!(body, "just a body\n");
    }

    #[test]
    fn format_empty_returns_empty() {
        assert_eq!(format_skills_for_prompt(&[]), "");
    }

    // v0.8.6: skill prompt now emits <available_skills> XML with
    // <location> so the model can `read` the SKILL.md file when the
    // task matches the skill description.
    #[test]
    fn format_skills_includes_location_xml() {
        let skill = Skill {
            name: "firecrawl".into(),
            description: "Search & scrape the web".into(),
            body: String::new(),
            base_dir: PathBuf::from("/home/jin/.nini/agent/skills/firecrawl"),
            source: SkillSource::User,
            origin: SkillOrigin::Nini,
            disable_model_invocation: false,
        };
        let out = format_skills_for_prompt(&[skill]);
        assert!(out.contains("<available_skills>"), "missing wrapper: {out}");
        assert!(
            out.contains("<name>firecrawl</name>"),
            "missing name: {out}"
        );
        assert!(
            out.contains("Search &amp; scrape"),
            "missing escaped desc: {out}"
        );
        assert!(
            out.contains("<location>/home/jin/.nini/agent/skills/firecrawl/SKILL.md</location>"),
            "missing location path: {out}"
        );
        assert!(out.contains("</available_skills>"), "missing closer: {out}");
    }

    #[test]
    fn format_skills_xml_escapes_special_chars() {
        let skill = Skill {
            name: "a&b".into(),
            description: "<script>alert(1)</script>".into(),
            body: String::new(),
            base_dir: PathBuf::from("/tmp"),
            source: SkillSource::User,
            origin: SkillOrigin::Nini,
            disable_model_invocation: false,
        };
        let out = format_skills_for_prompt(&[skill]);
        // & < > " should be escaped to their XML entities.
        assert!(out.contains("a&amp;b"), "& must escape to &amp;: {out}");
        assert!(
            out.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
            "<> must escape to &lt;/&gt;: {out}"
        );
    }

    #[test]
    fn format_skills_skips_disabled_model_invocation() {
        let visible = Skill {
            name: "visible".into(),
            description: "shown".into(),
            body: String::new(),
            base_dir: PathBuf::from("/tmp/visible"),
            source: SkillSource::User,
            origin: SkillOrigin::Nini,
            disable_model_invocation: false,
        };
        let hidden = Skill {
            name: "hidden".into(),
            disable_model_invocation: true,
            ..visible.clone()
        };
        let out = format_skills_for_prompt(&[visible, hidden]);
        assert!(out.contains("<name>visible</name>"));
        assert!(!out.contains("<name>hidden</name>"));
    }
}

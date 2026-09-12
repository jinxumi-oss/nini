//! Skills loader: parse `SKILL.md` files from `~/.pi/agent/skills/` and
//! `.pi/skills/`, including YAML frontmatter.
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
    pub disable_model_invocation: bool,
}

/// Where a skill was loaded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillSource {
    User,    // ~/.pi/agent/skills/
    Project, // .pi/skills/
}

impl SkillSource {
    pub fn label(&self) -> &'static str {
        match self {
            SkillSource::User => "user",
            SkillSource::Project => "project",
        }
    }
}

/// Result of loading skills.
#[derive(Debug, Clone, Default)]
pub struct LoadSkillsResult {
    pub skills: Vec<Skill>,
    pub errors: Vec<String>,
}

/// Load skills from user-level (`~/.pi/agent/skills/`) and project-level
/// (`.pi/skills/`) directories. Project-level skills shadow user-level
/// when names collide.
pub fn load_skills(cwd: &Path) -> LoadSkillsResult {
    let mut result = LoadSkillsResult::default();
    let user_dir = user_skills_dir();
    let project_dir = project_skills_dir(cwd);

    let mut all: Vec<Skill> = Vec::new();
    if let Some(d) = user_dir {
        load_from_dir(&d, SkillSource::User, &mut all, &mut result.errors);
    }
    if let Some(d) = project_dir {
        load_from_dir(&d, SkillSource::Project, &mut all, &mut result.errors);
    }

    // Project-level shadows user-level on name collision
    let mut by_name: std::collections::BTreeMap<String, Skill> = std::collections::BTreeMap::new();
    for s in all.into_iter().rev() {
        by_name.entry(s.name.clone()).or_insert(s);
    }
    result.skills = by_name.into_values().collect();
    result
}

fn load_from_dir(dir: &Path, source: SkillSource, out: &mut Vec<Skill>, errors: &mut Vec<String>) {
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
        match load_skill_file(&skill_md, source) {
            Ok(skill) => out.push(skill),
            Err(e) => errors.push(format!("{}: {}", skill_md.display(), e)),
        }
    }
}

fn load_skill_file(path: &Path, source: SkillSource) -> Result<Skill, String> {
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

/// User-level skills directory: `~/.pi/agent/skills/`.
pub fn user_skills_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)?;
    Some(home.join(".pi").join("agent").join("skills"))
}

/// Project-level skills directory: `<cwd>/.pi/skills/`.
pub fn project_skills_dir(cwd: &Path) -> Option<PathBuf> {
    Some(cwd.join(".pi").join("skills"))
}

/// Render the loaded skills into a system-prompt fragment (markdown section
/// listing each visible skill).
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
         When a skill file references a relative path, resolve it against the skill directory.\n",
    );
    for s in visible {
        out.push_str(&format!("\n- {}: {}\n", s.name, s.description));
    }
    out
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
}

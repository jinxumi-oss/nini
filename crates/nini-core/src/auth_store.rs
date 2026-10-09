//! Reading Pi's credential store.
//!
//! v0.10 (R2j): read-only, and deliberately so.
//!
//! Pi stores credentials in `auth.json` under its agent directory,
//! written with `mode: 0o600` and then `chmodSync(0o600)`
//! (`auth-storage.ts:23,63-64`). Its shape is
//! `Record<provider, Credential>`, where a credential carries `type` and -
//! for an API key - `key`. Real file on this machine:
//!
//! ```json
//! { "minimax": { "type": "apiKey", "key": "..." },
//!   "aiio-anthropic-direct": { "type": "apiKey", "key": "..." } }
//! ```
//!
//! **This module writes nothing.** `/login` still prints an `export` line
//! rather than storing a credential, and that is a product decision about
//! putting secrets on disk, not a wiring gap - it is not taken here by
//! accident. Reading is the half that needs no new attack surface: a user
//! who has already given Pi a key gets it in nini too, and nothing is
//! created or modified.
//!
//! Malformed input is ignored rather than fatal, and the key is never
//! logged. A corrupt store should not stop someone who has an env var.

use std::path::{Path, PathBuf};

/// Pi's `ENV_AGENT_DIR`: `${APP_NAME.toUpperCase()}_CODING_AGENT_DIR`.
pub const ENV_AGENT_DIR: &str = "PI_CODING_AGENT_DIR";

/// Pi's `getAgentDir()` (`config.ts:515-521`): the env var if set, otherwise
/// `~/.pi/agent`.
pub fn agent_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(ENV_AGENT_DIR) {
        let s = dir.to_string_lossy().into_owned();
        if !s.trim().is_empty() {
            return PathBuf::from(expand_tilde(&s));
        }
    }
    home_dir().join(".pi").join("agent")
}

pub fn auth_path() -> PathBuf {
    agent_dir().join("auth.json")
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    PathBuf::from(p)
}

/// The stored API key for a provider, if there is one.
///
/// Matches Pi's provider ids, which are the same strings nini's catalog
/// uses: `anthropic`, `openai`, `deepseek`, `groq`, `mistral`, `google`.
/// `cohere` has no counterpart — Pi ships no cohere provider, and nini's
/// `cohere/command-r-plus` was one of the invented model ids the
/// unwired-fields sweep turned up.
pub fn stored_key(provider: &str) -> Option<String> {
    read_key_at(&auth_path(), provider)
}

fn read_key_at(path: &Path, provider: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let entry = value.get(provider)?;
    // `type` is present in Pi's shape; only an API key is usable here.
    // An OAuth credential has no `key` and is ignored rather than
    // misread - nini cannot use an OAuth token without the flow that
    // obtains one.
    let key = entry.get("key")?.as_str()?.trim();
    if key.is_empty() {
        return None;
    }
    Some(key.to_string())
}

/// Which providers have a stored credential. For the error message, so a
/// user can be told what is available rather than only what is missing.
pub fn stored_providers() -> Vec<String> {
    let text = match std::fs::read_to_string(auth_path()) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let Some(map) = value.as_object() else {
        return Vec::new();
    };
    let mut names: Vec<String> = map
        .iter()
        .filter(|(_, v)| {
            v.get("key")
                .and_then(|k| k.as_str())
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false)
        })
        .map(|(k, _)| k.clone())
        .collect();
    names.sort();
    names
}

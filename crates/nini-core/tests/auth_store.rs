//! R2j: nini can use a credential Pi already stored.
//!
//! Pi keeps credentials in `auth.json` under its agent directory, written
//! with `mode: 0o600` and then `chmodSync(0o600)`
//! (`auth-storage.ts:23,63-64`). The shape is `Record<provider, Credential>`
//! with `type` and, for an API key, `key`. The file on this machine:
//!
//! ```json
//! { "minimax":                  { "type": "apiKey", "key": "..." },
//!   "aiio-anthropic-direct":   { "type": "apiKey", "key": "..." } }
//! ```
//!
//! Reading is the half that adds no attack surface. Writing is a decision
//! about putting secrets on disk and is **not** taken here - `/login` still
//! prints an `export` line. There is a test below asserting that nothing in
//! this path writes, because "read-only" is a claim that should be checked
//! rather than believed.
//!
//! Fixtures are built from the real shape and written into a temporary
//! agent directory, which `PI_CODING_AGENT_DIR` points at - the same
//! override Pi's `getAgentDir()` honours (`config.ts:515-521`).

use nini_core::auth_store;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// These tests all set `PI_CODING_AGENT_DIR`, and Rust runs tests in
/// parallel threads sharing one environment. Without this they interleave
/// and read each other's directories - which is exactly what happened the
/// first time: an OAuth fixture picked up a sibling test's key.
fn agent_dir_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// The shape of a real entry, from the file on this machine.
fn auth_json(entries: &[(&str, &str)]) -> String {
    let body: Vec<String> = entries
        .iter()
        .map(|(provider, key)| format!(r#"  "{provider}": {{ "type": "apiKey", "key": "{key}" }}"#))
        .collect();
    format!("{{\n{}\n}}", body.join(",\n"))
}

/// Run `f` with a temporary agent directory holding `contents`.
///
/// Serialised through `agent_dir_lock`: the override is process-global, so
/// a parallel sibling would otherwise swap the directory underneath this
/// one. That is not hypothetical - it is how the OAuth test first read a
/// neighbouring fixture's key and reported a failure that had nothing to
/// do with OAuth.
fn with_agent_dir<T>(contents: &str, f: impl FnOnce() -> T) -> T {
    let _guard = agent_dir_lock();
    let dir = std::env::temp_dir().join(format!(
        "nini-auth-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("auth.json"), contents).unwrap();

    let saved = std::env::var_os(auth_store::ENV_AGENT_DIR);
    // SAFETY: the override is read only by the code under test, in this
    // thread, and restored before returning.
    unsafe { std::env::set_var(auth_store::ENV_AGENT_DIR, &dir) };
    let out = f();
    match saved {
        Some(v) => unsafe { std::env::set_var(auth_store::ENV_AGENT_DIR, v) },
        None => unsafe { std::env::remove_var(auth_store::ENV_AGENT_DIR) },
    }
    let _ = std::fs::remove_dir_all(&dir);
    out
}

/// The headline.
#[test]
fn a_stored_credential_is_found() {
    with_agent_dir(&auth_json(&[("anthropic", "sk-ant-123")]), || {
        assert_eq!(
            auth_store::stored_key("anthropic").as_deref(),
            Some("sk-ant-123")
        );
    });
}

/// A provider with no entry is absent, not an empty string. A blank key
/// would be handed to a provider as if it were a real one.
#[test]
fn a_provider_with_no_entry_has_no_key() {
    with_agent_dir(&auth_json(&[("anthropic", "sk-ant-123")]), || {
        assert_eq!(auth_store::stored_key("groq"), None);
        assert_eq!(auth_store::stored_key("openai"), None);
    });
}

/// An empty `key` is treated as absent, for the same reason.
#[test]
fn an_empty_stored_key_counts_as_absent() {
    with_agent_dir(&auth_json(&[("groq", "   ")]), || {
        assert_eq!(
            auth_store::stored_key("groq"),
            None,
            "a blank key must not be handed to a provider as if it were real"
        );
        assert!(
            auth_store::stored_providers().is_empty(),
            "and must not be advertised as available"
        );
    });
}

/// An OAuth credential has no `key` and is ignored rather than misread.
/// nini cannot use an OAuth token without the flow that obtains one.
#[test]
fn an_oauth_credential_is_ignored_rather_than_misread() {
    let body = r#"{
  "anthropic": { "type": "oauth", "access": "at-...", "refresh": "rt-..." }
}"#;
    with_agent_dir(body, || {
        assert_eq!(auth_store::stored_key("anthropic"), None);
        assert!(auth_store::stored_providers().is_empty());
    });
}

/// A corrupt store must not stop someone who has an env var.
#[test]
fn a_corrupt_store_is_ignored_rather_than_fatal() {
    with_agent_dir("{ this is not json", || {
        assert_eq!(auth_store::stored_key("anthropic"), None);
        assert!(auth_store::stored_providers().is_empty());
    });
}

/// A missing file is the normal case for most people.
#[test]
fn a_missing_file_is_not_an_error() {
    let _guard = agent_dir_lock();
    let saved = std::env::var_os(auth_store::ENV_AGENT_DIR);
    // SAFETY: as above.
    unsafe { std::env::set_var(auth_store::ENV_AGENT_DIR, "/nonexistent/nini/agent") };
    let key = auth_store::stored_key("anthropic");
    assert_eq!(key, None);
    assert!(auth_store::stored_providers().is_empty());
    match saved {
        Some(v) => unsafe { std::env::set_var(auth_store::ENV_AGENT_DIR, v) },
        None => unsafe { std::env::remove_var(auth_store::ENV_AGENT_DIR) },
    }
}

/// The store is read-only. This is the claim that matters most and the one
/// a comment alone would not carry: if anything here wrote, the fixture
/// would change underneath the test.
#[test]
fn reading_the_store_writes_nothing() {
    let _guard = agent_dir_lock();
    let dir = std::env::temp_dir().join(format!("nini-auth-ro-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let auth = dir.join("auth.json");
    let original = auth_json(&[("anthropic", "sk-ant-123")]);
    std::fs::write(&auth, &original).unwrap();

    let saved = std::env::var_os(auth_store::ENV_AGENT_DIR);
    unsafe { std::env::set_var(auth_store::ENV_AGENT_DIR, &dir) };

    let _ = auth_store::stored_key("anthropic");
    let _ = auth_store::stored_providers();
    let _ = auth_store::auth_path();

    let after = std::fs::read_to_string(&auth).unwrap();
    assert_eq!(
        after, original,
        "reading the credential store must not modify it"
    );
    let entries: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
    assert_eq!(
        entries.len(),
        1,
        "and must not create anything beside it: {:?}",
        entries
            .iter()
            .filter_map(|e| e.as_ref().ok().map(|e| e.path()))
            .collect::<Vec<_>>()
    );

    match saved {
        Some(v) => unsafe { std::env::set_var(auth_store::ENV_AGENT_DIR, v) },
        None => unsafe { std::env::remove_var(auth_store::ENV_AGENT_DIR) },
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The path follows Pi: the override if set, otherwise `~/.pi/agent`.
#[test]
fn the_path_matches_pis() {
    let _guard = agent_dir_lock();
    let saved = std::env::var_os(auth_store::ENV_AGENT_DIR);
    unsafe { std::env::set_var(auth_store::ENV_AGENT_DIR, "/tmp/somewhere") };
    assert_eq!(
        auth_store::auth_path(),
        std::path::PathBuf::from("/tmp/somewhere/auth.json")
    );
    assert_eq!(auth_store::ENV_AGENT_DIR, "PI_CODING_AGENT_DIR");
    match saved {
        Some(v) => unsafe { std::env::set_var(auth_store::ENV_AGENT_DIR, v) },
        None => unsafe { std::env::remove_var(auth_store::ENV_AGENT_DIR) },
    }
}

/// The default path, with no override set.
///
/// An earlier version of this file set `PI_CODING_AGENT_DIR` in every test,
/// so `~/.pi/agent` was never actually executed - changing it to
/// `~/.nini/agent` passed 8/8. The default is the path every user without
/// the override actually takes, and it is Pi's.
#[test]
fn without_an_override_the_path_is_pis_default() {
    let _guard = agent_dir_lock();
    let saved = std::env::var_os(auth_store::ENV_AGENT_DIR);
    unsafe { std::env::remove_var(auth_store::ENV_AGENT_DIR) };

    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .expect("a home directory");
    let expected = std::path::PathBuf::from(home)
        .join(".pi")
        .join("agent")
        .join("auth.json");
    assert_eq!(
        auth_store::auth_path(),
        expected,
        "Pi's getAgentDir() is `join(homedir(), \".pi\", \"agent\")` \
         (config.ts:520)"
    );

    match saved {
        Some(v) => unsafe { std::env::set_var(auth_store::ENV_AGENT_DIR, v) },
        None => unsafe { std::env::remove_var(auth_store::ENV_AGENT_DIR) },
    }
}

/// An override that is set but blank falls through to the default, rather
/// than resolving the store to the current directory.
#[test]
fn a_blank_override_falls_through_to_the_default() {
    let _guard = agent_dir_lock();
    let saved = std::env::var_os(auth_store::ENV_AGENT_DIR);
    unsafe { std::env::set_var(auth_store::ENV_AGENT_DIR, "   ") };

    let path = auth_store::auth_path();
    assert!(
        !path.to_string_lossy().starts_with("   "),
        "a blank override must not become the path: {}",
        path.display()
    );
    assert_eq!(
        auth_store::agent_dir(),
        std::env::var("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join(".pi")
            .join("agent")
    );

    match saved {
        Some(v) => unsafe { std::env::set_var(auth_store::ENV_AGENT_DIR, v) },
        None => unsafe { std::env::remove_var(auth_store::ENV_AGENT_DIR) },
    }
}

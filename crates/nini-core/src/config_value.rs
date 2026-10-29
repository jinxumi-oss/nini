//! Pi's configuration-value resolution.
//!
//! # What this is for
//!
//! A `models.json` may spell a credential three ways, and Pi accepts all
//! three:
//!
//!   * a literal (`"sk-abc"`),
//!   * an environment reference (`"$MY_KEY"` or `"${MY_KEY}"`),
//!   * a shell command (`"!op read op://vault/item"`).
//!
//! nini read the file but not the values: a user's `apiKey: "$MY_KEY"`
//! was taken literally, and the request went out with the six characters
//! `$MY_KEY` as the bearer token. The file parsed; the credential did
//! not.
//!
//! # The rules, copied from Pi
//!
//! Pi's `resolve-config-value.ts`:
//!
//!   * a value that starts with `!` is a **command**: the rest is run by
//!     the shell and its stdout (trimmed) is the value. No output, a
//!     non-zero exit, or a timeout all mean "no value";
//!   * otherwise the value is a **template**: `$NAME` and `${NAME}` are
//!     replaced by the environment variable of that name;
//!   * `$$` is a literal `$` and `$!` is a literal `!`;
//!   * a variable name that is not a legal identifier (`${not-a-name}`)
//!     is kept verbatim;
//!   * a variable that is **not set** makes the whole value undefined —
//!     not an empty string, and not a half-expanded string.
//!
//! The last rule is the one that is easy to get wrong: a credential
//! that is half-expanded is a credential that goes out to the network.
//!
//! # Deliberately not implemented
//!
//! Pi caches a command's result for the life of the process. nini
//! evaluates each provider's value once, so the cache is not observable;
//! it is not implemented.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long a `!command` may run before it is killed.
///
/// Pi's `execSync` uses the same number (`timeout: 10000`).
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

/// Why a configured value could not be resolved.
///
/// Pi's `resolveConfigValueOrThrow` (`resolve-config-value.js:199-217`)
/// throws instead of returning `undefined`, and the message says **why**:
/// a shell command that failed, or the names of the environment variables
/// that were not set. nini needs the same distinction, because "the value
/// is missing" and "the value was configured but could not be read" are
/// different failures — the first falls back, the second is an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigValueError {
    /// A `!command` that failed, timed out, or printed nothing.
    ///
    /// The payload is the command **without** its leading `!`, which is
    /// how Pi reports it (`reference.config.slice(1)`).
    Command(String),
    /// Environment variables the template referenced but that are not set.
    MissingEnvVars(Vec<String>),
}

impl ConfigValueError {
    /// The sentence Pi would print for this failure.
    ///
    /// `description` is Pi's wording for the field being resolved, e.g.
    /// `API key for provider "my-gw"`.
    pub fn message(&self, description: &str) -> String {
        match self {
            ConfigValueError::Command(command) => {
                format!("Failed to resolve {description} from shell command: {command}")
            }
            ConfigValueError::MissingEnvVars(names) if names.len() == 1 => {
                format!(
                    "Failed to resolve {description} from environment variable: {}",
                    names[0]
                )
            }
            ConfigValueError::MissingEnvVars(names) => {
                format!(
                    "Failed to resolve {description} from environment variables: {}",
                    names.join(", ")
                )
            }
        }
    }
}

/// Resolve a configuration value, reporting **why** it could not.
///
/// `Ok(None)` means "there is no value here"; `Err` means "there was a
/// value here and it could not be read". Pi makes the same distinction
/// (`resolveConfigValue` vs `resolveConfigValueOrThrow`).
pub fn resolve_config_value_detailed(config: &str) -> Result<String, ConfigValueError> {
    resolve_with_detailed(config, |name| {
        // Pi's lookup is `env?.[name] || process.env[name] || undefined`:
        // a variable that is set but empty counts as **not set**.
        std::env::var(name).ok().filter(|v| !v.is_empty())
    })
}

/// The resolution with the environment lookup injected.
///
/// The lookup is a parameter so the rules can be tested without mutating
/// the process environment (which the test harness shares between
/// threads).
fn resolve_with_detailed<F: Fn(&str) -> Option<String>>(
    config: &str,
    env: F,
) -> Result<String, ConfigValueError> {
    if let Some(command) = config.strip_prefix('!') {
        return match execute_command(command, COMMAND_TIMEOUT) {
            Some(value) => Ok(value),
            None => Err(ConfigValueError::Command(command.to_string())),
        };
    }
    match resolve_with(config, &env) {
        Some(value) => Ok(value),
        // The resolution stopped at a variable that is not set. Which
        // ones those are is a separate question, and Pi answers it
        // separately too (`getMissingConfigValueEnvVarNames`,
        // `resolve-config-value.js:107-109`) — so that the message can
        // name **all** of them, not just the first.
        None => {
            let missing: Vec<String> = config_value_env_var_names(config)
                .into_iter()
                .filter(|name| env(name).is_none())
                .collect();
            Err(ConfigValueError::MissingEnvVars(missing))
        }
    }
}

/// The environment variables a template references, in the order they
/// appear, without duplicates.
///
/// Pi: `getConfigValueEnvVarNames` (`resolve-config-value.js:103-106`).
fn config_value_env_var_names(config: &str) -> Vec<String> {
    let mut names = Vec::new();
    collect_env_var_names(config, &mut names);
    names
}

/// Walk the template, collecting the names of the environment variables it
/// references. Only legal identifiers count: `${not-a-name}` is literal.
fn collect_env_var_names(config: &str, names: &mut Vec<String>) {
    let bytes = config.as_bytes();
    let mut index = 0usize;
    while index < config.len() {
        let Some(offset) = config[index..].find('$') else {
            return;
        };
        let dollar = index + offset;
        match bytes.get(dollar + 1).copied() {
            Some(b'$') | Some(b'!') => index = dollar + 2,
            Some(b'{') => match config[dollar + 2..].find('}') {
                Some(rel_end) => {
                    let end = dollar + 2 + rel_end;
                    let name = &config[dollar + 2..end];
                    if is_env_var_name(name) && !names.iter().any(|n| n == name) {
                        names.push(name.to_string());
                    }
                    index = end + 1;
                }
                None => index = dollar + 1,
            },
            _ => {
                let rest = &config[dollar + 1..];
                let name_len = rest
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(rest.len());
                let name = &rest[..name_len];
                if !name.is_empty() && is_env_var_name(name) && !names.iter().any(|n| n == name) {
                    names.push(name.to_string());
                }
                index = dollar + 1 + name_len;
            }
        }
    }
}

/// The resolution itself, with the environment lookup injected.
///
/// The lookup is a parameter so the rules can be tested without mutating
/// the process environment (which the test harness shares between
/// threads).
fn resolve_with<F: Fn(&str) -> Option<String>>(config: &str, env: F) -> Option<String> {
    if let Some(command) = config.strip_prefix('!') {
        return execute_command(command, COMMAND_TIMEOUT);
    }
    resolve_template(config, &env)
}

/// The template rules: `$VAR` and `${VAR}` are replaced by the
/// environment variable of that name, `$$` and `$!` are literal, and a
/// variable that is not set makes the whole value undefined.
///
/// The resolution stops at the first variable that is missing — the same
/// way Pi's `resolveTemplate` does (`resolve-config-value.js:89-92`).
/// The **list** of missing variables is a separate question, and is
/// answered by `config_value_env_var_names`.
fn resolve_template<F: Fn(&str) -> Option<String>>(config: &str, env: &F) -> Option<String> {
    let mut out = String::new();
    let mut index = 0usize;
    while index < config.len() {
        let Some(offset) = config[index..].find('$') else {
            out.push_str(&config[index..]);
            break;
        };
        let dollar = index + offset;
        out.push_str(&config[index..dollar]);

        match config.as_bytes().get(dollar + 1).copied() {
            // `$$` is a literal `$`, `$!` is a literal `!`.
            Some(b'$') | Some(b'!') => {
                out.push(config.as_bytes()[dollar + 1] as char);
                index = dollar + 2;
            }
            Some(b'{') => match config[dollar + 2..].find('}') {
                Some(rel_end) => {
                    let end = dollar + 2 + rel_end;
                    let name = &config[dollar + 2..end];
                    if is_env_var_name(name) {
                        out.push_str(&env(name)?);
                    } else {
                        // Not a legal identifier: the whole `${...}` stays.
                        out.push_str(&config[dollar..=end]);
                    }
                    index = end + 1;
                }
                None => {
                    // No closing brace: the `$` is a literal.
                    out.push('$');
                    index = dollar + 1;
                }
            },
            _ => {
                let rest = &config[dollar + 1..];
                let name_len = rest
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(rest.len());
                let name = &rest[..name_len];
                if !name.is_empty() && is_env_var_name(name) {
                    out.push_str(&env(name)?);
                    index = dollar + 1 + name_len;
                } else {
                    out.push('$');
                    index = dollar + 1;
                }
            }
        }
    }
    Some(out)
}

/// Is this a legal environment-variable name?
///
/// Pi: `/^[A-Za-z_][A-Za-z0-9_]*$/`.
fn is_env_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Run a `!command` and return its trimmed stdout.
///
/// The command runs under `/bin/sh -c`. A non-zero exit, no output, a
/// failure to start, and a run that outlives `timeout` all mean "no
/// value" — the same four ways Pi's `execSync` wrapper can fail.
fn execute_command(command: &str, timeout: Duration) -> Option<String> {
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    // Read the output on another thread: a command that prints more than
    // a pipe buffer would otherwise block forever, and the timeout below
    // would never be reached.
    let mut pipe = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = pipe.read_to_string(&mut buf);
        buf
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            // The reader is **not** joined: the shell is gone, but a
            // command that spawned a child of its own keeps the pipe
            // open, and waiting for that would defeat the timeout.
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };

    let output = reader.join().unwrap_or_default();
    if !status?.success() {
        return None;
    }
    let trimmed = output.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name: &str| map.get(name).cloned()
    }

    #[test]
    fn a_missing_variable_is_reported_with_its_name() {
        // Pi's `resolveConfigValueOrThrow`
        // (`resolve-config-value.js:199-217`) says **which** variable was
        // missing, and names all of them, not just the first.
        let err = resolve_with_detailed("$NINI_ABSENT_ONE", env(&[])).unwrap_err();
        assert_eq!(
            err.message("API key for provider \"my-gw\""),
            "Failed to resolve API key for provider \"my-gw\" from environment variable: NINI_ABSENT_ONE"
        );
    }

    #[test]
    fn every_missing_variable_is_reported() {
        let err =
            resolve_with_detailed("$NINI_ABSENT_ONE-${NINI_ABSENT_TWO}", env(&[])).unwrap_err();
        assert_eq!(
            err.message("API key for provider \"my-gw\""),
            "Failed to resolve API key for provider \"my-gw\" from environment variables: NINI_ABSENT_ONE, NINI_ABSENT_TWO"
        );
    }

    #[test]
    fn a_command_that_fails_is_reported_as_a_command() {
        let err = resolve_with_detailed("!exit 3", env(&[])).unwrap_err();
        assert_eq!(
            err.message("API key for provider \"my-gw\""),
            "Failed to resolve API key for provider \"my-gw\" from shell command: exit 3"
        );
    }

    #[test]
    fn a_plain_string_is_a_literal() {
        assert_eq!(
            resolve_with("sk-plain", env(&[])),
            Some("sk-plain".to_string())
        );
    }

    #[test]
    fn a_dollar_variable_is_expanded() {
        assert_eq!(
            resolve_with("$NINI_TEST_KEY", env(&[("NINI_TEST_KEY", "from-env")])),
            Some("from-env".to_string())
        );
    }

    #[test]
    fn a_braced_variable_is_expanded() {
        assert_eq!(
            resolve_with("${NINI_TEST_KEY}", env(&[("NINI_TEST_KEY", "from-env")])),
            Some("from-env".to_string())
        );
    }

    #[test]
    fn a_missing_variable_yields_nothing_at_all() {
        assert_eq!(resolve_with("$NINI_TEST_ABSENT", env(&[])), None);
    }

    #[test]
    fn a_missing_variable_does_not_partially_expand() {
        assert_eq!(
            resolve_with(
                "prefix-$NINI_TEST_ABSENT-suffix",
                env(&[("NINI_TEST_OTHER", "x")])
            ),
            None
        );
    }

    #[test]
    fn a_double_dollar_is_a_literal_dollar() {
        assert_eq!(resolve_with("$$HOME", env(&[])), Some("$HOME".to_string()));
    }

    #[test]
    fn a_dollar_bang_is_a_literal_bang() {
        assert_eq!(
            resolve_with("$!literal", env(&[])),
            Some("!literal".to_string())
        );
    }

    #[test]
    fn an_invalid_braced_name_is_kept_literally() {
        assert_eq!(
            resolve_with("${not-a-name}", env(&[])),
            Some("${not-a-name}".to_string())
        );
    }

    #[test]
    fn a_bare_dollar_is_kept_literally() {
        assert_eq!(resolve_with("100$", env(&[])), Some("100$".to_string()));
    }

    #[test]
    fn a_dollar_followed_by_a_digit_is_kept_literally() {
        assert_eq!(resolve_with("$1", env(&[])), Some("$1".to_string()));
    }

    #[test]
    fn a_command_is_run_and_its_output_is_the_value() {
        assert_eq!(
            resolve_with("!echo hello", env(&[])),
            Some("hello".to_string())
        );
    }

    #[test]
    fn a_command_that_fails_yields_nothing() {
        assert_eq!(resolve_with("!exit 3", env(&[])), None);
    }

    #[test]
    fn a_command_with_no_output_yields_nothing() {
        assert_eq!(resolve_with("!printf ''", env(&[])), None);
    }

    #[test]
    fn a_command_that_runs_too_long_yields_nothing() {
        assert_eq!(
            execute_command("sleep 5", Duration::from_millis(200)),
            None,
            "a command that outlives its timeout must not be waited for"
        );
    }

    #[test]
    fn a_command_that_cannot_start_yields_nothing() {
        // A directory is not a program: the shell fails, so there is no
        // value.
        assert_eq!(resolve_with("!/definitely/not/a/program", env(&[])), None);
    }
}

//! Where nini keeps its files.
//!
//! v0.10 (R3): every one of these used to say `.pi`. nini read its settings,
//! models, keybindings, prompts, skills and sessions out of
//! `~/.nini/agent/`, wrote its sessions and exports back into it, and scanned
//! Pi's session history to offer it in `/resume`. 894 references across 81
//! files, 14 of them actual path construction and the rest commentary
//! following from them.
//!
//! That made nini a *channel* rather than a rewrite: two installs shared
//! one directory, so editing settings in one changed the other, and nini
//! did not work on a machine where Pi had never been installed.
//!
//! The boundary this module draws:
//!
//!   * **Runtime paths are nini's own.** `~/.nini/agent/`, and `.nini/` in
//!     a project. Nothing here reads Pi's installation or its config
//!     directory.
//!   * **File *formats* stay Pi-compatible.** The session JSONL schema and
//!     the model-metadata schema are unchanged. Being able to open a Pi
//!     session file is a feature; needing Pi to be installed to have a
//!     config directory is a dependency. Those are different things, and
//!     only the second one was wrong.
//!
//! `NINI_AGENT_DIR` overrides the user directory, the way Pi's
//! `PI_CODING_AGENT_DIR` does, because tests and sandboxes need it and
//! hardcoding `$HOME` makes that impossible.

use std::path::{Path, PathBuf};

/// Override for the user-level agent directory.
pub const ENV_AGENT_DIR: &str = "NINI_AGENT_DIR";

/// The directory name under `$HOME`.
pub const HOME_DIR_NAME: &str = ".nini";

/// The directory name under the project root and under the home directory.
pub const PROJECT_DIR_NAME: &str = ".nini";

/// The subdirectory that holds the agent's own state, mirroring Pi's
/// `~/.nini/agent/` so the two layouts are legible side by side.
pub const AGENT_SUBDIR: &str = "agent";

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// `~/.nini/agent`, or `$NINI_AGENT_DIR` when set.
pub fn agent_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(ENV_AGENT_DIR) {
        let s = dir.to_string_lossy().into_owned();
        if !s.trim().is_empty() {
            return Some(expand_tilde(&s));
        }
    }
    Some(home_dir()?.join(HOME_DIR_NAME).join(AGENT_SUBDIR))
}

/// `<cwd>/.nini`.
pub fn project_dir(cwd: &Path) -> PathBuf {
    cwd.join(PROJECT_DIR_NAME)
}

fn expand_tilde(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home_dir().unwrap_or_else(|| PathBuf::from(".")).join(rest),
        None => PathBuf::from(p),
    }
}

/// A file in the user-level agent directory.
pub fn agent_file(name: &str) -> Option<PathBuf> {
    Some(agent_dir()?.join(name))
}

/// A file in the project-level directory.
pub fn project_file(cwd: &Path, name: &str) -> PathBuf {
    project_dir(cwd).join(name)
}

/// `~/.nini/agent/sessions` - where a conversation is recorded.
pub fn sessions_dir() -> Option<PathBuf> {
    agent_dir().map(|d| d.join("sessions"))
}

/// `~/.nini/agent/exports` - where `/export` writes its HTML snapshot.
pub fn exports_dir() -> Option<PathBuf> {
    agent_dir().map(|d| d.join("exports"))
}

/// `~/.nini/agent/trust.json` - per-project trust decisions.
pub fn trust_file() -> Option<PathBuf> {
    agent_file("trust.json")
}

/// The candidates for a project file, project first then user-level.
///
/// Project-level overrides user-level, which is what every one of these
/// loaders did before, and the order is kept so a project can override a
/// machine-wide default without deleting it.
pub fn candidates(cwd: &Path, name: &str) -> Vec<PathBuf> {
    let mut out = vec![project_file(cwd, name)];
    if let Some(user) = agent_file(name) {
        if !out.contains(&user) {
            out.push(user);
        }
    }
    out
}

/// Points `NINI_AGENT_DIR` at a fresh temporary directory, and restores it
/// on drop.
///
/// Everything nini persists is written by whichever code happens to hold
/// the process-wide `NINI_AGENT_DIR`, and `SettingsManager::flush`
/// resolves its path at call time. A test that exercises a persisting
/// command therefore writes to the developer's real `~/.nini/agent/` unless
/// it redirects that variable - which `slash_command_e2e.rs` did not, and
/// `/thinking high` duly overwrote a real settings file before this guard
/// existed.
///
/// The lock is process-wide because the environment is: cargo runs one
/// binary on many threads, and two tests redirecting the same variable at
/// once would see each other's directories. It is held for the guard's
/// whole lifetime, which is why this is a guard and not a closure.
pub struct AgentDirGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    saved: Option<std::ffi::OsString>,
    path: PathBuf,
}

impl Drop for AgentDirGuard {
    fn drop(&mut self) {
        unsafe {
            match self.saved.take() {
                Some(v) => std::env::set_var(ENV_AGENT_DIR, v),
                None => std::env::remove_var(ENV_AGENT_DIR),
            }
        }
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

impl AgentDirGuard {
    /// The sandbox directory. Point a `SettingsManager` at
    /// `<this>/settings.json`, or just call the code under test.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub fn agent_lock() -> AgentDirGuard {
    use std::sync::{Mutex, MutexGuard, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let lock: MutexGuard<'static, ()> = LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let mut path = std::env::temp_dir();
    path.push(format!("nini-agent-sandbox-{}-{}", std::process::id(), n));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("agent sandbox directory");

    let saved = std::env::var_os(ENV_AGENT_DIR);
    unsafe { std::env::set_var(ENV_AGENT_DIR, &path) };

    AgentDirGuard {
        _lock: lock,
        saved,
        path,
    }
}

/// 新会话的落盘路径：`<agent dir>/sessions/<project>/<timestamp>_<suffix>.jsonl`
///
/// v0.10.8 (A5)：TUI **启动时**就该知道自己的会话写在哪儿——此前 nini
/// 启动后不建会话文件（footer 显示 `[no session]`），对话在退出后**整个消失**，
/// 而 Pi 每次启动一个会话就有一个文件。
///
/// 名字带一个随机后缀：秒级时间戳在同一秒里开两个会话会撞名
/// （Pi 的做法是 `{timestamp}_{session id}`）。
pub fn new_session_path(cwd: &std::path::Path) -> Option<PathBuf> {
    let base = sessions_dir()?;
    let project = cwd
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "default".to_string());
    let dir = base.join(&project);
    let _ = std::fs::create_dir_all(&dir);
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    // 不用 uuid：nini-core 没有这个依赖，而「进程号 + 纳秒」已经足够唯一
    // （同一个进程里同一纳秒开两个会话的概率可以忽略）。
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let suffix = format!("{:x}{:03x}", std::process::id(), nanos & 0xfff);
    Some(dir.join(format!("{ts}_{suffix}.jsonl")))
}

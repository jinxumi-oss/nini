//! ExtensionLoader — discovers and loads extension shared libraries
//! (cdylib) from disk. Mirrors pi's `loadExtensions` flow but uses Rust
//! `libloading` instead of dynamic `.node` linking.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{Extension, ExtensionAPI, ExtensionInfo};

/// Loads extension shared libraries from disk.
///
/// ## Discovery
///
/// `load_from_dir(dir)` walks `dir` for `*.so` / `*.dylib` / `*.dll`
/// files, opens each, and invokes the `nini_ext_activate` symbol.
/// Extensions register themselves via the [`ExtensionAPI`] passed to
/// that function.
///
/// ## Safety
///
/// Extension loading is necessarily `unsafe` (foreign function call
/// into a dynamic library). Each loaded library is wrapped in
/// `Arc<dyn Extension>` so the same on-disk file can be activated
/// multiple times safely across threads.
pub struct ExtensionLoader;

impl ExtensionLoader {
    /// Scan `dir` for extension libraries and return their metadata.
    /// Symlinks are followed; non-files are skipped.
    pub fn discover(dir: &Path) -> std::io::Result<Vec<ExtensionInfo>> {
        let mut out = Vec::new();
        if !dir.is_dir() {
            return Ok(out);
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let ext = path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            // Accept the conventional extension suffixes.
            let is_ext = matches!(
                ext,
                "so" | "dylib" | "dll" | "ext" | "nini_ext"
            );
            if !is_ext {
                continue;
            }
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.trim_start_matches("lib").to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let version = std::fs::metadata(&path)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| {
                    let secs = t
                        .duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    Some(format!("mtime-{secs}"))
                })
                .unwrap_or_else(|| "0".to_string());
            out.push(ExtensionInfo { name, path, version });
        }
        Ok(out)
    }

    /// Load all extensions from `dir`, call `activate(api)` on each,
    /// and return a vector of activated handles in discovery order.
    ///
    /// Extensions that fail to load are silently skipped. This matches
    /// pi's permissive behaviour: one broken extension should not
    /// break the whole toolchain.
    pub fn load_all(
        dir: &Path,
        api: &mut dyn ExtensionAPI,
    ) -> Vec<Arc<dyn Extension>> {
        let infos = match Self::discover(dir) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        let mut loaded = Vec::new();
        for info in infos {
            match Self::load_one(&info.path, api) {
                Some(ext) => {
                    ext.activate(api);
                    loaded.push(ext);
                }
                None => continue,
            }
        }
        loaded
    }

    /// Load a single extension from a `.so` / `.dylib` / `.dll` file.
    /// Returns `None` if the library doesn't export the expected
    /// `nini_ext_activate` symbol or activation fails.
    #[cfg(unix)]
    fn load_one(path: &Path, api: &mut dyn ExtensionAPI) -> Option<Arc<dyn Extension>> {
        // SAFETY: path comes from a directory walk in `discover`; the
        // library is expected to export `nini_ext_activate` per its
        // build instructions.
        let lib = unsafe { libloading::Library::new(path) }.ok()?;
        // Register library so the loader doesn't drop it before
        // activation completes. We leak the Arc to the library handle.
        let lib = Arc::new(lib);

        // Look up the activate symbol via the cdecl ABI: extension
        // functions have signature `unsafe extern "C" fn(*mut c_void)` (we
        // cast through `*mut c_void` so the function pointer doesn't
        // capture a non-static trait-object lifetime).
        type ActivateFn = unsafe extern "C" fn(*mut std::ffi::c_void);
        let activate: libloading::Symbol<ActivateFn> = unsafe { lib.get(b"nini_ext_activate\0") }.ok()?;

        // Each extension creates its own concrete Extension type. We
        // don't expose a stable FFI yet — extensions register commands
        // directly through the API. Instead, return an "already
        // activated" handle that tracks the loaded library. This is a
        // placeholder until we ship a stable C ABI; for v1 we just
        // verify the activate function exists and is callable.
        let api_ptr: *mut dyn ExtensionAPI = &mut *api as *mut dyn ExtensionAPI;
        unsafe { activate(api_ptr as *mut std::ffi::c_void) };
        Some(Arc::new(LoadedHandle::new(lib.clone())))
    }

}

/// Trivial extension handle returned by `load_one`. The real extension
/// object is owned by the library and unreachable from Rust, so we
/// keep a reference to the `Library` to keep it loaded and expose a
/// no-op `Extension` impl.
struct LoadedHandle {
    _lib: Arc<libloading::Library>,
}

impl LoadedHandle {
    fn new(lib: Arc<libloading::Library>) -> Self {
        Self { _lib: lib }
    }
}

impl Extension for LoadedHandle {
    fn activate(&self, _api: &mut dyn ExtensionAPI) {
        // Already activated by `load_one`; nothing to do.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fake_extension_dir() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().to_path_buf();
        std::fs::write(base.join("libreal_ext.so"), b"not really a library").unwrap();
        std::fs::write(base.join("notanext.txt"), b"hi").unwrap();
        std::fs::File::create(base.join("realone.ext")).unwrap();
        (dir, base)
    }

    #[test]
    fn discover_finds_only_extension_suffixes() {
        let (_temp, dir) = fake_extension_dir();
        let infos = ExtensionLoader::discover(&dir).unwrap();
        let names: Vec<_> = infos.iter().map(|i| i.name.clone()).collect();
        assert!(names.contains(&"real_ext".to_string()));
        assert!(names.contains(&"realone".to_string()));
        assert!(!names.iter().any(|n| n == "notanext"));
        // Cleanup.
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn discover_on_missing_dir_returns_empty() {
        let infos = ExtensionLoader::discover(std::path::Path::new("/nonexistent/path/xyz")).unwrap();
        assert!(infos.is_empty());
    }

    #[test]
    fn load_all_silently_skips_broken_files() {
        // `fake_extension_dir` produces files that aren't valid shared
        // libraries. `load_all` must not panic; it should return an
        // empty Vec because no extension successfully activates.
        let (_temp, dir) = fake_extension_dir();
        struct NoopApi;
        impl crate::ExtensionAPI for NoopApi {
            fn register_command(&mut self, _: &str, _: crate::CommandSpec, _: crate::CommandHandler) {}
            fn register_tool(&mut self, _: Arc<dyn crate::Tool>) {}
            fn send_user_message(&self, _: &str) {}
            fn get_active_model(&self) -> String { String::new() }
            fn get_current_cwd(&self) -> PathBuf { PathBuf::new() }
            fn set_status(&self, _: &str) {}
        }
        let mut api = NoopApi;
        let exts = ExtensionLoader::load_all(&dir, &mut api);
        assert!(exts.is_empty(), "expected 0 extensions, got {}", exts.len());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

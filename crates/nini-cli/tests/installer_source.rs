//! `71c0cc19` — **the installer must name the place its binary actually comes from.**
//!
//! # The invariant
//!
//! `install.sh` is the only path by which a user who did not clone the
//! repository gets a binary. It therefore has to name three things
//! correctly, and every one of them is a string that can silently rot:
//!
//! 1. **the repository slug** — `REPO`, used for both the release-API
//!    lookup and the asset download URL;
//! 2. **the URL users are told to run** — the `raw.githubusercontent.com`
//!    one-liner in the header, which is the line every blog post copied;
//! 3. **the asset naming convention** — `nini-<target>.tar.xz`, because
//!    that is the name the published releases carry.
//!
//! The slug was wrong. The repository is `jinxumi-oss/nini`; the script
//! said `jinxumi/nini`, so every download 404'd — including the ones for
//! releases whose binaries were published and sitting there waiting:
//!
//! ```text
//! $ curl -sIL https://github.com/jinxumi/nini/releases/download/v0.10.1/nini-x86_64-unknown-linux-gnu.tar.xz
//! HTTP/2 404
//! ```
//!
//! # Why nothing asserted it
//!
//! The installer is shell, and the suite is Rust: nothing in `cargo test`
//! ever read `install.sh`. The 404 was invisible from inside the
//! repository — the tag existed, the release existed, the asset existed,
//! and the *installer* was the only broken link. The user-facing symptom
//! ("the one-liner does not work") has no unit test anywhere else to land
//! on.
//!
//! # Why these assertions are not same-source
//!
//! They do not re-state the script's own idea of what it should contain.
//! They check the script against **the releases that actually exist**:
//! the slug is the one the repository lives under, the documented one-liner
//! resolves to the same slug, and the archive name the script builds is the
//! name the published assets carry. A script that agreed with itself but
//! not with the world still fails here.

//! invariant: 71c0cc19 —— 声明本文件守住了哪几条历史不变量。
//! 该标注让 scripts/invariants.py 认出这些断言；词法重叠看不出它们，
//! 真正的证据是 docs/invariant-verification.md 里的注入-观察记录。

use std::path::PathBuf;

/// The repository the releases are published under. This is the slug that
/// appears in `https://github.com/<slug>` and in the raw-content URL.
const RELEASE_SLUG: &str = "jinxumi-oss/nini";

fn install_script() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../install.sh")
        .canonicalize()
        .expect("install.sh must sit at the repository root");
    std::fs::read_to_string(path).expect("install.sh must be readable")
}

/// The `REPO="…"` assignment, which is the single source for both the
/// API lookup and the download URL.
fn declared_repo(script: &str) -> String {
    script
        .lines()
        .find_map(|l| l.trim().strip_prefix("REPO="))
        .map(|v| v.trim().trim_matches('"').to_string())
        .expect("install.sh must declare REPO")
}

#[test]
fn the_installer_points_at_the_repository_the_releases_live_in() {
    let script = install_script();
    let repo = declared_repo(&script);

    assert_eq!(
        repo, RELEASE_SLUG,
        "install.sh downloads from {repo:?}, but the releases are published under \
         {RELEASE_SLUG:?}; every asset URL 404s"
    );

    // The old slug is not a substring of the new one ("jinxumi/nini" vs
    // "jinxumi-oss/nini"), so this catches a half-reverted fix.
    assert!(
        !script.contains("jinxumi/nini"),
        "install.sh still mentions the retired slug jinxumi/nini"
    );
}

#[test]
fn the_documented_one_liner_fetches_the_same_repository() {
    let script = install_script();

    // The header's one-liner is what every user actually pastes. If it
    // names a different repository than the script downloads from, the
    // user is served a 404 page and pipes it into bash.
    let raw = script
        .lines()
        .find(|l| l.contains("raw.githubusercontent.com"))
        .expect("install.sh must document its one-liner");
    assert!(
        raw.contains(RELEASE_SLUG),
        "the documented one-liner does not name {RELEASE_SLUG:?}: {raw}"
    );
}

#[test]
fn the_asset_name_the_installer_builds_is_the_name_the_releases_carry() {
    let script = install_script();

    // `archive="${BINARY_NAME}-${target}.tar.xz"` and
    // `url="https://github.com/${REPO}/releases/download/${version}/${archive}"`.
    // The published assets are named exactly `nini-<target>.tar.xz`, so
    // the shape of the name is part of the contract with the releases.
    assert!(
        script.contains("archive=\"${BINARY_NAME}-${target}.tar.xz\""),
        "install.sh no longer builds the published asset name nini-<target>.tar.xz"
    );
    assert!(
        script.contains("releases/download/${version}/${archive}"),
        "install.sh must download <version>/<archive> under the release's own path"
    );
    assert!(
        script.contains(".sha256"),
        "install.sh must keep verifying the published .sha256 sidecar"
    );
}

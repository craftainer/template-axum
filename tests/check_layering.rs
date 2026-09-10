//! Exercises `.github/scripts/check_layering.py` (the automated gate
//! backing `docs/adrs/0009`/`docs/nfrs/NFR-0018` and the `check-layering`
//! prek hook) against synthetic trees rather than the real `src/`, so
//! this doesn't need updating every time `src/` grows a file -- only
//! when the checker's own rules change. No external service, no
//! network.

use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

const SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/.github/scripts/check_layering.py"
);

fn run_against(root: &std::path::Path) -> std::process::Output {
    Command::new("python3")
        .arg(SCRIPT)
        .arg(root)
        .output()
        .expect("failed to run check_layering.py")
}

/// A directory under the OS temp dir, unique per call (pid + a counter,
/// same reasoning as `tests/common::unique_suffix`), removed on drop --
/// this crate has no `tempfile` dependency, so this rolls its own rather
/// than adding one for a single test file.
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new() -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("check-layering-test-{}-{n}", std::process::id()));
        fs::create_dir_all(&path).expect("failed to create a temp dir");
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A fresh temp dir with one `.rs` file at `generic/controllers/x.rs`
/// (or wherever `rel_path` says) containing `contents`.
fn synthetic_src(rel_path: &str, contents: &str) -> TempDir {
    let dir = TempDir::new();
    let file_path = dir.path().join(rel_path);
    fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    fs::write(&file_path, contents).unwrap();
    dir
}

#[test]
fn the_real_src_tree_has_no_layering_violations() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let output = run_against(&root);
    assert!(
        output.status.success(),
        "check_layering.py failed against the real src/ tree:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_generic_module_importing_hero_is_flagged() {
    let dir = synthetic_src(
        "generic/controllers/x.rs",
        "use crate::hero::controllers::AppState;\n",
    );
    let output = run_against(dir.path());
    assert!(
        !output.status.success(),
        "generic -> hero must be rejected, even before any layer-order check"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("hero::controllers"),
        "expected the violation to name the offending category, got: {stderr}"
    );
}

#[test]
fn a_hero_module_importing_generic_is_allowed() {
    let dir = synthetic_src(
        "hero/controllers/x.rs",
        "use crate::generic::controllers::health;\n",
    );
    let output = run_against(dir.path());
    assert!(
        output.status.success(),
        "hero -> generic (same nominal layer, opposite package) must be allowed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn an_upward_import_within_a_single_package_is_flagged() {
    // models is below controllers in the layer order -- a models file
    // reaching "up" into controllers must be rejected regardless of the
    // generic/hero split.
    let dir = synthetic_src(
        "generic/models/x.rs",
        "use crate::generic::controllers::health;\n",
    );
    let output = run_against(dir.path());
    assert!(
        !output.status.success(),
        "models importing controllers (upward) must be rejected"
    );
}

#[test]
fn a_reference_inside_a_comment_is_ignored() {
    let dir = synthetic_src(
        "generic/controllers/x.rs",
        "//! see crate::hero::controllers::AppState for the concrete state\npub fn f() {}\n",
    );
    let output = run_against(dir.path());
    assert!(
        output.status.success(),
        "a doc-comment mention isn't a real dependency and must not be flagged:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_dyn_repository_macro_invocation_is_not_treated_as_a_module_path() {
    let dir = synthetic_src(
        "hero/controllers/x.rs",
        "crate::dyn_repository!(DynX, model = M, create = C, update = U);\n",
    );
    let output = run_against(dir.path());
    assert!(
        output.status.success(),
        "a #[macro_export] macro invocation isn't a module-layering dependency:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

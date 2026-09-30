use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn track_git_path(root: &Path, name: &str) {
    if let Some(path) = git(root, &["rev-parse", "--git-path", name]) {
        println!("cargo:rerun-if-changed={}", root.join(path).display());
    }
}

fn main() {
    println!("cargo:rerun-if-env-changed=ATTUNE_BUILD_GIT_SHA");
    println!("cargo:rerun-if-changed=build.rs");
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf();
    track_git_path(&root, "HEAD");
    track_git_path(&root, "packed-refs");
    if let Some(reference) = git(&root, &["symbolic-ref", "-q", "HEAD"]) {
        track_git_path(&root, &reference);
    }
    let sha = env::var("ATTUNE_BUILD_GIT_SHA")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| git(&root, &["rev-parse", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string());
    assert!(
        sha == "unknown"
            || matches!(sha.len(), 40 | 64) && sha.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "ATTUNE_BUILD_GIT_SHA must be a full Git commit SHA or 'unknown'"
    );
    println!("cargo:rustc-env=ATTUNE_BUILD_GIT_SHA={sha}");
}

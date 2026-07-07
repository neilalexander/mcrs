use std::{env, path::Path, process::Command};

fn main() {
    println!("cargo:rustc-env=MESHCORE_FIRMWARE_VERSION={}", version());
}

fn version() -> String {
    let package = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION must be set");
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set");
    let manifest_dir = Path::new(&manifest_dir);

    register_git_reruns(manifest_dir);

    let Some(commit) = git(manifest_dir, &["rev-parse", "--short=8", "HEAD"]) else {
        return package;
    };

    if git_status(manifest_dir, &["diff-index", "--quiet", "HEAD", "--"]) == Some(false) {
        format!("{package}+{commit}.dirty")
    } else {
        format!("{package}+{commit}")
    }
}

fn register_git_reruns(manifest_dir: &Path) {
    if let Some(git_dir) = git(manifest_dir, &["rev-parse", "--absolute-git-dir"]) {
        println!("cargo:rerun-if-changed={git_dir}/HEAD");
        println!("cargo:rerun-if-changed={git_dir}/index");
        if let Some(ref_name) = git(manifest_dir, &["symbolic-ref", "--quiet", "HEAD"]) {
            println!("cargo:rerun-if-changed={git_dir}/{ref_name}");
        }
    }
}

fn git(manifest_dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .current_dir(manifest_dir)
        .args(args)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.into())
    }
}

fn git_status(manifest_dir: &Path, args: &[&str]) -> Option<bool> {
    Command::new("git")
        .current_dir(manifest_dir)
        .args(args)
        .status()
        .ok()
        .map(|status| status.success())
}

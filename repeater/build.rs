use std::{env, path::Path, process::Command};

fn main() {
    configure_defaults();
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

fn configure_defaults() {
    println!("cargo:rustc-check-cfg=cfg(mcrs_profile)");
    println!("cargo:rerun-if-env-changed=MCRS_PROFILE");
    // Track firmware sources as well as the defaults themselves. Declaring
    // any rerun-if-changed disables Cargo's default whole-package tracking.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=../profiles/defaults.conf");
    let manifest = env::var("CARGO_MANIFEST_DIR").unwrap();
    let workspace = Path::new(&manifest).parent().unwrap();
    if let Some(selected) = env::var_os("MCRS_PROFILE").filter(|value| !value.is_empty()) {
        let file = Path::new(&selected);
        // Bare filenames refer to the profiles directory. Explicit paths are
        // resolved from the workspace root (absolute paths work unchanged).
        let path = if file.components().count() == 1 {
            workspace.join("profiles").join(file)
        } else {
            workspace.join(file)
        };
        println!("cargo:rerun-if-changed={}", path.display());
        let path = path.canonicalize().unwrap_or_else(|error| {
            panic!(
                "PROFILE must name a single file; cannot open {}: {error}",
                path.display()
            )
        });
        println!("cargo:rerun-if-changed={}", path.display());
        let path = path.to_str().expect("profile path must be valid UTF-8");
        assert!(
            !path.contains(['\n', '\r']),
            "profile path must not contain newlines"
        );
        println!("cargo:rustc-env=MCRS_PROFILE_PATH={path}");
        println!("cargo:rustc-cfg=mcrs_profile");
    }
}

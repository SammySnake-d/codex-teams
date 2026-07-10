//! Build script: stamp a Teams-fork display version so `codex --version` and the
//! TUI header reflect exactly which team build is running.
//!
//! Upstream keeps `[workspace.package].version = "0.0.0"` as a source-build
//! sentinel (see `is_source_build_version`), and the real release version is
//! injected by CI. We must not touch that. Instead we compute an ADDITIONAL
//! display string of the form `<upstream>-<team.N>+<git-hash>`, e.g.
//! `0.143.0-alpha.10-team.1+9993fb8`, and expose it as `CODEX_TEAMS_VERSION`.
//!
//! Inputs (all best-effort; missing pieces degrade gracefully):
//! - `codex-rs/TEAMS_VERSION`  -> the manually-bumped `team.N` tag.
//! - `git describe --tags --match 'rust-v*'` -> the upstream release anchor.
//! - `git rev-parse --short HEAD` -> the current commit hash.
use std::path::PathBuf;
use std::process::Command;

fn main() {
    // codex-rs/ (the workspace root) is one level up from tui/.
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let workspace_root = manifest_dir
        .parent()
        .map(PathBuf::from)
        .unwrap_or(manifest_dir);
    let teams_version_path = workspace_root.join("TEAMS_VERSION");

    // Rebuild whenever the team version file or the git commit changes. Watch
    // both HEAD (branch switch) and the branch's ref file (new commit on the
    // same branch), so the embedded hash never goes stale.
    println!("cargo:rerun-if-changed={}", teams_version_path.display());
    if let Some(git_dir) = find_git_dir(&workspace_root) {
        watch_git_commit(&git_dir);
    }

    let team_tag = std::fs::read_to_string(&teams_version_path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "team.dev".to_string());

    let upstream = git(
        &workspace_root,
        &["describe", "--tags", "--match", "rust-v*", "--abbrev=0"],
    )
    .map(|s| {
        s.trim()
            .trim_start_matches("rust-v")
            .trim_start_matches("rust-v")
            .to_string()
    })
    .filter(|s| !s.is_empty());

    let hash = git(&workspace_root, &["rev-parse", "--short", "HEAD"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let display = match (upstream, hash) {
        (Some(up), Some(h)) => format!("{up}-{team_tag}+{h}"),
        (Some(up), None) => format!("{up}-{team_tag}"),
        (None, Some(h)) => format!("{team_tag}+{h}"),
        (None, None) => team_tag,
    };

    println!("cargo:rustc-env=CODEX_TEAMS_VERSION={display}");
}

fn find_git_dir(start: &std::path::Path) -> Option<PathBuf> {
    let mut dir = Some(start.to_path_buf());
    while let Some(d) = dir {
        let candidate = d.join(".git");
        if candidate.is_dir() {
            return Some(candidate);
        }
        dir = d.parent().map(PathBuf::from);
    }
    None
}

/// Emit `rerun-if-changed` for both `HEAD` and the ref file it points at, so a
/// new commit on the current branch (which only rewrites the ref) still
/// retriggers the build script and refreshes the embedded hash.
fn watch_git_commit(git_dir: &std::path::Path) {
    let head = git_dir.join("HEAD");
    println!("cargo:rerun-if-changed={}", head.display());
    if let Ok(contents) = std::fs::read_to_string(&head)
        && let Some(reference) = contents.strip_prefix("ref:").map(str::trim)
    {
        println!(
            "cargo:rerun-if-changed={}",
            git_dir.join(reference).display()
        );
    }
}

fn git(cwd: &std::path::Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

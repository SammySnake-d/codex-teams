/// The current Codex CLI version as embedded at compile time. Stays `0.0.0` for
/// source builds — `is_source_build_version` relies on this exact value, so do
/// NOT fold the Teams display suffix into it.
pub const CODEX_CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Teams-fork display version, e.g. `0.143.0-alpha.10-team.1+9993fb8`.
///
/// Computed by `tui/build.rs` from `codex-rs/TEAMS_VERSION` (the manually
/// bumped `team.N` tag), the upstream `git describe` release anchor, and the
/// current commit hash. Falls back to `team.dev` when the version file or git
/// metadata is unavailable. Bump `TEAMS_VERSION` on each meaningful change so
/// this string moves and you can confirm at a glance which build is running.
#[allow(dead_code)]
pub const CODEX_TEAMS_VERSION: &str = env!("CODEX_TEAMS_VERSION");

/// The version string shown in the TUI header: the source/upstream base plus the
/// Teams display suffix, e.g. `0.0.0 (0.143.0-alpha.10-team.1+9993fb8)`.
pub const CODEX_DISPLAY_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("CODEX_TEAMS_VERSION"),
    ")"
);

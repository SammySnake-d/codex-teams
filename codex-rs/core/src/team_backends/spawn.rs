//! Pure helpers for assembling a teammate spawn.
//!
//! Ports the input-preparation pieces of Claude Code's `spawnMultiAgent.ts`
//! (`getTeammateCommand`, `buildInheritedEnvVars`, `assignTeammateColor`,
//! `sanitizeAgentName`, `generateUniqueTeammateName`). The actual
//! `cd && env && <bin> teammate <flags>` command string and its send-keys
//! delivery live in [`super::tmux`] (`TmuxBackend::launch_teammate` /
//! `build_launch_line`); this module only prepares the values that backend
//! consumes, so there is no duplicated command-assembly logic.

use std::path::Path;
use std::path::PathBuf;

use super::tmux::AgentColor;

/// Round-robin teammate palette. Order matches Claude `AGENT_COLORS`
/// (`red, blue, green, yellow, purple, orange, pink, cyan`) and the
/// [`AgentColor`] variant order, so a spawn index maps to a stable color.
pub(crate) const AGENT_COLOR_PALETTE: [AgentColor; 8] = [
    AgentColor::Red,
    AgentColor::Blue,
    AgentColor::Green,
    AgentColor::Yellow,
    AgentColor::Purple,
    AgentColor::Orange,
    AgentColor::Pink,
    AgentColor::Cyan,
];

/// Claude `assignTeammateColor`: round-robin over [`AGENT_COLOR_PALETTE`] by
/// spawn index.
pub(crate) fn teammate_color(index: usize) -> AgentColor {
    AGENT_COLOR_PALETTE[index % AGENT_COLOR_PALETTE.len()]
}

/// Claude `sanitizeAgentName`: replace `@` with `-` so the name is safe as an
/// inbox-file / agent-id segment.
pub(crate) fn sanitize_agent_name(name: &str) -> String {
    name.replace('@', "-")
}

/// Claude `generateUniqueTeammateName`: if `base` collides (case-insensitively)
/// with an existing member name, append `-2`, `-3`, … until unique.
pub(crate) fn unique_teammate_name(base: &str, existing: &[String]) -> String {
    let taken: std::collections::HashSet<String> =
        existing.iter().map(|name| name.to_lowercase()).collect();
    if !taken.contains(&base.to_lowercase()) {
        return base.to_string();
    }
    let mut suffix: u32 = 2;
    loop {
        let candidate = format!("{base}-{suffix}");
        if !taken.contains(&candidate.to_lowercase()) {
            return candidate;
        }
        suffix += 1;
    }
}

/// Claude `getTeammateCommand`: honor the `CODEX_TEAMMATE_COMMAND` override,
/// else launch this running executable.
pub(crate) fn teammate_binary() -> std::io::Result<PathBuf> {
    if let Ok(value) = std::env::var("CODEX_TEAMMATE_COMMAND") {
        if !value.is_empty() {
            return Ok(PathBuf::from(value));
        }
    }
    std::env::current_exe()
}

/// Proxy / TLS-cert vars forwarded to a teammate process so it has the same
/// network egress as the lead (the codex-relevant subset of Claude
/// `TEAMMATE_ENV_VARS`).
const FORWARDED_ENV_VARS: [&str; 11] = [
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "NO_PROXY",
    "no_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
];

/// Claude `buildInheritedEnvVars` analog: mark teammate mode, pin the same
/// `$CODEX_HOME` (so the teammate reads the same team store + config), and
/// forward proxy / cert vars that are set and non-empty.
pub(crate) fn build_inherited_env_vars(codex_home: &Path) -> Vec<(String, String)> {
    let mut env = vec![
        ("CODEX_TEAMMATE".to_string(), "1".to_string()),
        (
            "CODEX_HOME".to_string(),
            codex_home.to_string_lossy().into_owned(),
        ),
    ];
    for key in FORWARDED_ENV_VARS {
        if let Ok(value) = std::env::var(key) {
            if !value.is_empty() {
                env.push((key.to_string(), value));
            }
        }
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_replaces_at_with_dash() {
        assert_eq!(sanitize_agent_name("alice@team"), "alice-team");
        assert_eq!(sanitize_agent_name("bob"), "bob");
    }

    #[test]
    fn color_is_round_robin_over_eight() {
        assert_eq!(teammate_color(0), AgentColor::Red);
        assert_eq!(teammate_color(7), AgentColor::Cyan);
        assert_eq!(teammate_color(8), AgentColor::Red);
        assert_eq!(teammate_color(9), teammate_color(1));
    }

    #[test]
    fn unique_name_appends_suffix_case_insensitively() {
        let existing = vec!["Worker".to_string(), "worker-2".to_string()];
        assert_eq!(unique_teammate_name("fresh", &existing), "fresh");
        assert_eq!(unique_teammate_name("worker", &existing), "worker-3");
        assert_eq!(unique_teammate_name("WORKER", &existing), "WORKER-3");
    }

    #[test]
    fn env_marks_teammate_and_pins_codex_home() {
        let env = build_inherited_env_vars(Path::new("/tmp/codex-home"));
        assert!(env.contains(&("CODEX_TEAMMATE".to_string(), "1".to_string())));
        assert!(
            env.iter()
                .any(|(k, v)| k == "CODEX_HOME" && v == "/tmp/codex-home")
        );
    }
}

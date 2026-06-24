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
use std::process::Command;
use std::process::Stdio;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use crate::team_store;

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

const TEAMMATE_HELP_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

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
/// else launch the configured Codex CLI executable. Codex.app can carry an
/// older bundled CLI that treats `teammate` as a plain prompt, so candidates
/// must prove they expose the hidden teammate subcommand before they are used.
pub(crate) fn teammate_binary(codex_self_exe: Option<&Path>) -> std::io::Result<PathBuf> {
    if let Ok(value) = std::env::var("CODEX_TEAMMATE_COMMAND")
        && !value.is_empty()
    {
        let candidate = PathBuf::from(value);
        if supports_teammate_subcommand(&candidate) {
            return Ok(candidate);
        }
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "CODEX_TEAMMATE_COMMAND does not support `codex teammate`: {}",
                candidate.display()
            ),
        ));
    }

    let mut candidates = Vec::new();
    if let Some(path) = codex_self_exe {
        candidates.push(teammate_binary_without_override(path));
    }
    if let Ok(path) = std::env::current_exe() {
        candidates.push(teammate_binary_without_override(path.as_path()));
    }
    if let Some(path_env) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_env) {
            let candidate = dir.join(format!("codex{}", std::env::consts::EXE_SUFFIX));
            if candidate.is_file() {
                candidates.push(candidate);
            }
        }
    }

    candidates.dedup();
    for candidate in candidates {
        if supports_teammate_subcommand(&candidate) {
            return Ok(candidate);
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "no `codex` binary with `codex teammate` support found; set CODEX_TEAMMATE_COMMAND to a Teams-capable Codex CLI",
    ))
}

fn teammate_binary_without_override(path: &Path) -> PathBuf {
    let Some(parent) = path.parent() else {
        return path.to_path_buf();
    };
    let Some(bin_dir) = (if parent.file_name().is_some_and(|name| name == "deps") {
        parent.parent()
    } else {
        Some(parent)
    }) else {
        return path.to_path_buf();
    };
    let candidate = bin_dir.join(format!("codex{}", std::env::consts::EXE_SUFFIX));
    if candidate.is_file() {
        candidate
    } else {
        path.to_path_buf()
    }
}

fn supports_teammate_subcommand(path: &Path) -> bool {
    let mut child = match Command::new(path)
        .arg("teammate")
        .arg("--help")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return false,
    };
    let deadline = Instant::now() + TEAMMATE_HELP_PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
    let output = child.wait_with_output();
    let Ok(output) = output else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.contains("Usage: codex teammate") && stdout.contains("--agent-id")
}

/// Proxy / TLS-cert/auth vars forwarded to a teammate process so it has the
/// same network egress and auth source as the lead (the codex-relevant subset
/// of Claude `TEAMMATE_ENV_VARS`, plus Codex's own env auth entrypoints).
const FORWARDED_ENV_VARS: [&str; 14] = [
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
    codex_login::CODEX_API_KEY_ENV_VAR,
    codex_login::CODEX_ACCESS_TOKEN_ENV_VAR,
    codex_login::OPENAI_API_KEY_ENV_VAR,
];

fn select_teammate_config_home(
    teams_root: &Path,
    explicit_codex_home: bool,
    default_codex_home: Option<&Path>,
) -> PathBuf {
    if explicit_codex_home || teams_root.join(codex_config::CONFIG_TOML_FILE).exists() {
        return teams_root.to_path_buf();
    }

    let Some(default_codex_home) = default_codex_home else {
        return teams_root.to_path_buf();
    };
    if default_codex_home != teams_root
        && default_codex_home
            .join(codex_config::CONFIG_TOML_FILE)
            .exists()
    {
        default_codex_home.to_path_buf()
    } else {
        teams_root.to_path_buf()
    }
}

fn teammate_config_home(teams_root: &Path) -> PathBuf {
    let default_codex_home = codex_utils_home_dir::find_codex_home()
        .ok()
        .map(|home| home.to_path_buf());
    select_teammate_config_home(
        teams_root,
        std::env::var_os("CODEX_HOME").is_some(),
        default_codex_home.as_deref(),
    )
}

/// Claude `buildInheritedEnvVars` analog: mark teammate mode, pin the same
/// Teams store root, resolve the config home, and forward proxy / cert / auth
/// vars that are set and non-empty.
pub(crate) fn build_inherited_env_vars(
    teams_root: &Path,
    provider_env_keys: &[&str],
) -> Vec<(String, String)> {
    let config_home = teammate_config_home(teams_root);
    build_inherited_env_vars_with_config_home(teams_root, &config_home, provider_env_keys)
}

pub(crate) fn build_inherited_env_vars_for_config_home(
    teams_root: &Path,
    config_home: &Path,
    provider_env_keys: &[&str],
) -> Vec<(String, String)> {
    build_inherited_env_vars_with_config_home(teams_root, config_home, provider_env_keys)
}

fn build_inherited_env_vars_with_config_home(
    teams_root: &Path,
    config_home: &Path,
    provider_env_keys: &[&str],
) -> Vec<(String, String)> {
    let mut env = vec![
        ("CODEX_TEAMMATE".to_string(), "1".to_string()),
        (
            team_store::TEAM_STORE_ROOT_ENV_VAR.to_string(),
            teams_root.to_string_lossy().into_owned(),
        ),
        (
            "CODEX_HOME".to_string(),
            config_home.to_string_lossy().into_owned(),
        ),
    ];
    for key in FORWARDED_ENV_VARS
        .iter()
        .copied()
        .chain(provider_env_keys.iter().copied())
    {
        if env.iter().any(|(existing, _)| existing == key) {
            continue;
        }
        if let Ok(value) = std::env::var(key)
            && !value.is_empty()
        {
            env.push((key.to_string(), value));
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
        let codex_home = tempfile::tempdir().expect("temp codex home");
        std::fs::write(codex_home.path().join(codex_config::CONFIG_TOML_FILE), "")
            .expect("write config");

        let env = build_inherited_env_vars(codex_home.path(), &[]);
        let expected_home = codex_home.path().to_string_lossy().into_owned();

        assert!(env.contains(&("CODEX_TEAMMATE".to_string(), "1".to_string())));
        assert!(
            env.iter()
                .any(|(k, v)| k == team_store::TEAM_STORE_ROOT_ENV_VAR && v == &expected_home)
        );
        assert!(
            env.iter()
                .any(|(k, v)| k == "CODEX_HOME" && v == &expected_home)
        );
    }

    #[test]
    fn config_home_falls_back_to_default_when_runtime_home_has_no_config() {
        let runtime_home = tempfile::tempdir().expect("runtime home");
        let default_home = tempfile::tempdir().expect("default home");
        std::fs::write(
            default_home.path().join(codex_config::CONFIG_TOML_FILE),
            "model_provider = \"custom\"",
        )
        .expect("write default config");

        let selected = select_teammate_config_home(
            runtime_home.path(),
            /*explicit_codex_home*/ false,
            Some(default_home.path()),
        );

        assert_eq!(selected, default_home.path());
    }

    #[test]
    fn env_can_separate_team_store_root_from_config_home() {
        let runtime_home = tempfile::tempdir().expect("runtime home");
        let config_home = tempfile::tempdir().expect("config home");

        let env =
            build_inherited_env_vars_for_config_home(runtime_home.path(), config_home.path(), &[]);
        let runtime_home = runtime_home.path().to_string_lossy().into_owned();
        let config_home = config_home.path().to_string_lossy().into_owned();

        assert_ne!(runtime_home, config_home);
        assert!(env.contains(&(
            team_store::TEAM_STORE_ROOT_ENV_VAR.to_string(),
            runtime_home
        )));
        assert!(env.contains(&("CODEX_HOME".to_string(), config_home)));
    }

    #[test]
    fn explicit_codex_home_keeps_runtime_home_even_without_config() {
        let runtime_home = tempfile::tempdir().expect("runtime home");
        let default_home = tempfile::tempdir().expect("default home");
        std::fs::write(
            default_home.path().join(codex_config::CONFIG_TOML_FILE),
            "model_provider = \"custom\"",
        )
        .expect("write default config");

        let selected = select_teammate_config_home(
            runtime_home.path(),
            /*explicit_codex_home*/ true,
            Some(default_home.path()),
        );

        assert_eq!(selected, runtime_home.path());
    }

    #[test]
    fn env_forwards_provider_auth_key_when_present() {
        let codex_home = tempfile::tempdir().expect("temp codex home");
        std::fs::write(codex_home.path().join(codex_config::CONFIG_TOML_FILE), "")
            .expect("write config");
        let path = std::env::var("PATH").expect("PATH should be set in test environment");

        let env = build_inherited_env_vars(codex_home.path(), &["PATH"]);

        assert!(env.contains(&("PATH".to_string(), path)));
    }

    #[test]
    fn codex_auth_env_vars_are_forwarded_by_default() {
        assert!(FORWARDED_ENV_VARS.contains(&codex_login::CODEX_API_KEY_ENV_VAR));
        assert!(FORWARDED_ENV_VARS.contains(&codex_login::CODEX_ACCESS_TOKEN_ENV_VAR));
        assert!(FORWARDED_ENV_VARS.contains(&codex_login::OPENAI_API_KEY_ENV_VAR));
    }

    #[test]
    fn teammate_binary_escapes_cargo_deps_test_binary() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let debug_dir = temp.path().join("target").join("debug");
        let deps_dir = debug_dir.join("deps");
        std::fs::create_dir_all(&deps_dir).expect("create deps");
        let codex = debug_dir.join(format!("codex{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&codex, "").expect("create codex binary placeholder");
        let test_binary = deps_dir.join("codex_core-53de15f594a0a30f");

        assert_eq!(teammate_binary_without_override(&test_binary), codex);
    }

    #[test]
    fn teammate_binary_rejects_non_teammate_cli() {
        assert!(!supports_teammate_subcommand(Path::new("/bin/echo")));
    }
}

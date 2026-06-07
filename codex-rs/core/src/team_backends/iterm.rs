//! iTerm2 split-pane backend for codex teams.
//!
//! Faithful Rust port of Claude Code's `it2`-CLI iTerm2 backend. Truth sources
//! (from `ChinaSiro/claude-code-sourcemap`):
//! - `restored-src/src/utils/swarm/backends/ITermBackend.ts` — the backend itself.
//! - `restored-src/src/utils/swarm/backends/detection.ts` — `IT2_COMMAND`,
//!   `isInITerm2`, `isIt2CliAvailable`.
//! - `restored-src/src/utils/swarm/backends/it2Setup.ts` — `detectPythonPackageManager`,
//!   `installIt2`, `verifyIt2Setup`, `getPythonApiInstructions`, the setup flags.
//! - `restored-src/src/utils/execFileNoThrow.ts` — the never-throw subprocess helper.
//!
//! Every `it2` argv vector, env-var name, regex, and instruction string below is quoted
//! verbatim from those files. Subprocesses are spawned as argv vectors via
//! [`tokio::process::Command`] (never a shell) exactly like Claude's `execFileNoThrow`;
//! only the per-pane *spawn command* (built elsewhere) is shell-interpreted.
//!
//! This module is intentionally self-contained and stays unwired until the lead adds
//! `pub(crate) mod iterm;` to `team_backends/mod.rs` and routes `spawn_member` through it.
#![allow(dead_code)]

use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;

use serde::Deserialize;
use serde::Serialize;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::sync::Semaphore;

use crate::team_store;

/// The iTerm2 control CLI. Verbatim from Claude `detection.ts` (`IT2_COMMAND = 'it2'`).
pub(crate) const IT2_COMMAND: &str = "it2";

/// iTerm2 sets this in the environment; format `"wXtYpZ:UUID"` — the session id is the
/// substring after the first `:`. Mirrors Claude `getLeaderSessionId`.
const ITERM_SESSION_ID_ENV: &str = "ITERM_SESSION_ID";

// ---------------------------------------------------------------------------
// Backend type / result shapes (mirror Claude `BackendType` / `CreatePaneResult`).
// These live in `team_backends/mod.rs` in the full design; redefined here so the file
// is self-contained and compile-ready before the lead wires the shared module. When
// `mod.rs` lands, replace these with imports from the parent module.
// ---------------------------------------------------------------------------

/// Mirror of Claude `BackendType` (`'tmux' | 'iterm2' | 'in-process'`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BackendType {
    Tmux,
    Iterm2,
    InProcess,
}

impl BackendType {
    /// Claude's wire value. Note: Claude emits `"iterm2"` here even though
    /// `TEAMS_CLAUDE_PORT_SPEC.md` uses `"iterm"` for the persisted `backend_type`
    /// member field — see the integrator unknown about reconciling the two.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Tmux => "tmux",
            Self::Iterm2 => "iterm2",
            Self::InProcess => "in_process",
        }
    }
}

/// Opaque pane handle. For iTerm2 this is the `it2` session UUID. Mirror of `PaneId`.
pub(crate) type PaneId = String;

/// Mirror of Claude `CreatePaneResult`.
#[derive(Clone, Debug)]
pub(crate) struct CreatePaneResult {
    pub(crate) pane_id: PaneId,
    pub(crate) is_first_teammate: bool,
}

// ---------------------------------------------------------------------------
// Subprocess helper (mirror execFileNoThrow): never errors on nonzero exit.
// ---------------------------------------------------------------------------

/// Result of a never-throw subprocess run. Mirrors `execFileNoThrow`'s
/// `{ stdout, stderr, code }` return.
#[derive(Clone, Debug, Default)]
pub(crate) struct ExecResult {
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) code: i32,
}

/// Run a program with an argv vector, capturing stdout/stderr and the exit code, never
/// returning an error for a nonzero exit (mirror of Claude `execFileNoThrow`).
///
/// A spawn failure (binary missing, no PATH entry) maps to `code = 127` with the OS error
/// text on stderr, matching the shell convention `execFileNoThrow` relies on.
async fn exec_file_no_throw(file: &str, args: &[&str]) -> ExecResult {
    let output = Command::new(file)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;

    match output {
        Ok(out) => ExecResult {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            code: out.status.code().unwrap_or(-1),
        },
        Err(err) => ExecResult {
            stdout: String::new(),
            stderr: err.to_string(),
            code: 127,
        },
    }
}

/// Convenience wrapper that always runs [`IT2_COMMAND`]. Mirrors the many
/// `execFileNoThrow(IT2_COMMAND, [...])` call sites in `ITermBackend.ts`.
async fn run_it2(args: &[&str]) -> ExecResult {
    exec_file_no_throw(IT2_COMMAND, args).await
}

// ---------------------------------------------------------------------------
// Parsing / detection helpers (mirror parseSplitOutput / getLeaderSessionId / detection.ts).
// ---------------------------------------------------------------------------

/// Mirror of `parseSplitOutput`: find `Created new pane: <id>` and return the trimmed id,
/// else the empty string. Claude uses the regex `/Created new pane:\s*(.+)/`; this matches
/// the same shape without pulling in a regex crate.
fn parse_split_output(output: &str) -> String {
    const MARKER: &str = "Created new pane:";
    for line in output.lines() {
        if let Some(idx) = line.find(MARKER) {
            // Skip the marker, then any leading whitespace (the `\s*` in Claude's regex).
            let rest = line[idx + MARKER.len()..].trim_start();
            // `(.+)` requires at least one char; mirror `.trim()` on the capture.
            let trimmed = rest.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    String::new()
}

/// Mirror of `getLeaderSessionId`: read `ITERM_SESSION_ID` and take the substring after the
/// first `:` (format `"wXtYpZ:UUID"`). Returns `None` when unset or malformed/empty.
fn get_leader_session_id() -> Option<String> {
    let raw = std::env::var(ITERM_SESSION_ID_ENV).ok()?;
    let after = raw.split_once(':').map(|(_, rest)| rest).unwrap_or(&raw);
    let trimmed = after.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Mirror of `isInITerm2`: `TERM_PROGRAM == "iTerm.app"` OR `ITERM_SESSION_ID` is present.
pub(crate) fn is_in_iterm2() -> bool {
    if std::env::var("TERM_PROGRAM").as_deref() == Ok("iTerm.app") {
        return true;
    }
    std::env::var(ITERM_SESSION_ID_ENV).is_ok()
}

/// Mirror of `isIt2CliAvailable`: `it2 session list` exits 0. Uses `session list` (NOT
/// `--version`) because `--version` succeeds even when the iTerm2 Python API is disabled.
pub(crate) async fn is_it2_cli_available() -> bool {
    run_it2(&["session", "list"]).await.code == 0
}

// ---------------------------------------------------------------------------
// The backend itself (mirror ITermBackend.ts).
// ---------------------------------------------------------------------------

/// Module-global mutable state in Claude is `teammateSessionIds: string[]` plus
/// `firstPaneUsed: bool`. Here it lives behind a [`Mutex`] inside the backend so that
/// `create_teammate_pane_in_swarm_view` is serialized — the lock replaces Claude's
/// `acquirePaneCreationLock` promise chain.
#[derive(Default)]
struct It2State {
    teammate_session_ids: Vec<String>,
    first_pane_used: bool,
}

/// The iTerm2 pane backend. The registry must cache exactly one instance per session
/// (mirror Claude's `cachedBackend`), otherwise parallel spawns race the iTerm layout.
pub(crate) struct ITermBackend {
    state: Mutex<It2State>,
    creation_lock: Semaphore,
}

impl Default for ITermBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl ITermBackend {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(It2State::default()),
            creation_lock: Semaphore::new(1),
        }
    }

    // -- Static descriptors (mirror PaneBackend metadata) -------------------

    pub(crate) fn backend_type(&self) -> BackendType {
        BackendType::Iterm2
    }

    pub(crate) fn display_name(&self) -> &'static str {
        "iTerm2"
    }

    pub(crate) fn supports_hide_show(&self) -> bool {
        false
    }

    // -- Availability -------------------------------------------------------

    /// Mirror of `isAvailable`: in iTerm2 AND the `it2` CLI can talk to the Python API.
    pub(crate) async fn is_available(&self) -> bool {
        is_in_iterm2() && is_it2_cli_available().await
    }

    /// Mirror of `isRunningInside`: just the terminal probe (no subprocess).
    pub(crate) fn is_running_inside(&self) -> bool {
        is_in_iterm2()
    }

    // -- Pane creation (the core algorithm) ---------------------------------

    /// Mirror of `createTeammatePaneInSwarmView`. Holds the state lock for the whole body
    /// (replaces Claude's pane-creation lock) and retries past dead targets.
    ///
    /// Splitting strategy (verbatim from `ITermBackend.ts`):
    /// - first teammate: vertical split off the leader session (`-v`).
    /// - subsequent teammates: horizontal split off the *last* teammate session, so panes
    ///   stack downward on screen.
    ///
    /// `color` is accepted to match the tmux backend shape but is ignored: iTerm2 styling
    /// each spawns a separate Python process, so it is skipped for performance — exactly as
    /// Claude does. The loop is bounded: every dead-target `continue` shrinks
    /// `teammate_session_ids` by one, giving O(N+1) iterations before it terminates.
    pub(crate) async fn create_teammate_pane_in_swarm_view(
        &self,
        name: &str,
        color: &str,
    ) -> anyhow::Result<CreatePaneResult> {
        let _ = (name, color); // accepted for parity; iTerm ignores both.
        let leader = get_leader_session_id();
        let _permit =
            self.creation_lock.acquire().await.map_err(|err| {
                anyhow::anyhow!("failed to acquire iTerm2 pane creation lock: {err}")
            })?;

        loop {
            let (is_first_teammate, last_id) = {
                let state = self.state.lock().await;
                (
                    !state.first_pane_used,
                    state.teammate_session_ids.last().cloned(),
                )
            };

            // Build the split argv plus an optional targeted-teammate id (for dead-target
            // recovery). Owned `String`s are held so the borrowed argv slice stays valid.
            let leader_id = leader.clone();

            let mut split_args: Vec<&str> = vec!["session", "split"];
            let mut targeted_teammate_id: Option<String> = None;

            if is_first_teammate {
                split_args.push("-v");
                if let Some(id) = leader_id.as_deref() {
                    split_args.push("-s");
                    split_args.push(id);
                }
            } else if let Some(id) = last_id.as_deref() {
                split_args.push("-s");
                split_args.push(id);
                targeted_teammate_id = Some(id.to_string());
            }
            // (else: no last id → fall back to splitting the active session.)

            let r = run_it2(&split_args).await;

            if r.code != 0 {
                // Dead-target recovery: only prune a targeted teammate we can *confirm* is
                // gone. Re-probe with `session list`; if it succeeds and the target is
                // absent, drop it and retry. Otherwise (alive, or can't tell) do not prune.
                if let Some(dead_candidate) = targeted_teammate_id.as_deref() {
                    let probe = run_it2(&["session", "list"]).await;
                    if probe.code == 0 && !probe.stdout.contains(dead_candidate) {
                        let mut state = self.state.lock().await;
                        state.teammate_session_ids.retain(|id| id != dead_candidate);
                        if state.teammate_session_ids.is_empty() {
                            state.first_pane_used = false;
                        }
                        continue;
                    }
                }
                return Err(anyhow::anyhow!(
                    "Failed to create iTerm2 split pane: {}",
                    r.stderr
                ));
            }

            let pane_id = parse_split_output(&r.stdout);
            if pane_id.is_empty() {
                return Err(anyhow::anyhow!(
                    "Failed to parse session ID from split output: {}",
                    r.stdout
                ));
            }

            {
                let mut state = self.state.lock().await;
                if is_first_teammate {
                    state.first_pane_used = true;
                }
                state.teammate_session_ids.push(pane_id.clone());
            }

            // Skip color & title (each it2 call spawns a Python process — perf).
            return Ok(CreatePaneResult {
                pane_id,
                is_first_teammate,
            });
        }
    }

    // -- Command dispatch ---------------------------------------------------

    /// Mirror of `sendCommandToPane`: `it2 session run -s <pane> <command>` (or without
    /// `-s` when `pane_id` is empty). `it2` appends the newline. `use_external_session` is
    /// tmux-only and ignored here. A nonzero exit is surfaced as an error.
    pub(crate) async fn send_command_to_pane(
        &self,
        pane_id: &str,
        command: &str,
        _use_external_session: bool,
    ) -> anyhow::Result<()> {
        let args: Vec<&str> = if pane_id.is_empty() {
            vec!["session", "run", command]
        } else {
            vec!["session", "run", "-s", pane_id, command]
        };
        let r = run_it2(&args).await;
        if r.code != 0 {
            return Err(anyhow::anyhow!(
                "Failed to send command to iTerm2 pane: {}",
                r.stderr
            ));
        }
        Ok(())
    }

    // -- No-op styling / layout (iTerm2 handles these natively) -------------

    /// No-op (perf): each styling call would spawn a Python process. Mirror Claude.
    pub(crate) async fn set_pane_border_color(
        &self,
        _pane_id: &str,
        _color: &str,
        _use_external_session: bool,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// No-op (perf). Mirror Claude.
    pub(crate) async fn set_pane_title(
        &self,
        _pane_id: &str,
        _name: &str,
        _color: &str,
        _use_external_session: bool,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// No-op: iTerm2 shows session titles in its tab/pane chrome already.
    pub(crate) async fn enable_pane_border_status(
        &self,
        _window_target: Option<&str>,
        _use_external_session: bool,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// No-op: iTerm2 auto-balances split panes.
    pub(crate) async fn rebalance_panes(
        &self,
        _window_target: &str,
        _has_leader: bool,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    // -- Teardown -----------------------------------------------------------

    /// Mirror of `killPane`: `it2 session close -f -s <pane>` (`-f` is required, else
    /// iTerm2's "Confirm before closing" prompt blocks). The pane is *always* removed from
    /// the tracked id list regardless of exit code; if the list empties, `first_pane_used`
    /// is reset. Returns whether the close exited 0.
    pub(crate) async fn kill_pane(
        &self,
        pane_id: &str,
        _use_external_session: bool,
    ) -> anyhow::Result<bool> {
        let r = run_it2(&["session", "close", "-f", "-s", pane_id]).await;

        let mut state = self.state.lock().await;
        state.teammate_session_ids.retain(|id| id != pane_id);
        if state.teammate_session_ids.is_empty() {
            state.first_pane_used = false;
        }

        Ok(r.code == 0)
    }

    /// Unsupported: iTerm2 has no equivalent to tmux `break-pane`. Mirror Claude (`false`).
    pub(crate) async fn hide_pane(
        &self,
        _pane_id: &str,
        _use_external_session: bool,
    ) -> anyhow::Result<bool> {
        Ok(false)
    }

    /// Unsupported: iTerm2 has no equivalent to tmux `join-pane`. Mirror Claude (`false`).
    pub(crate) async fn show_pane(
        &self,
        _pane_id: &str,
        _target_window_or_pane: &str,
        _use_external_session: bool,
    ) -> anyhow::Result<bool> {
        Ok(false)
    }
}

// ---------------------------------------------------------------------------
// Setup-check flow (mirror it2Setup.ts).
// ---------------------------------------------------------------------------

/// Mirror of Claude's detected package-manager union. `pip`/`pip3` both collapse to
/// [`PythonPackageManager::Pip`] (Claude maps `which pip3` → `'pip'`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PythonPackageManager {
    Uvx,
    Pipx,
    Pip,
}

impl PythonPackageManager {
    /// Human label used in the setup UI (`Installing it2 using {packageManager}…`).
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Uvx => "uvx",
            Self::Pipx => "pipx",
            Self::Pip => "pip",
        }
    }

    /// The verbatim manual install command shown on failure (mirror `it2Setup.ts`).
    pub(crate) fn install_command_hint(self) -> &'static str {
        match self {
            Self::Uvx => "uv tool install it2",
            Self::Pipx => "pipx install it2",
            Self::Pip => "pip install --user it2",
        }
    }
}

/// Mirror of Claude `installIt2`'s result.
#[derive(Clone, Debug)]
pub(crate) struct It2InstallResult {
    pub(crate) success: bool,
    pub(crate) error: Option<String>,
    pub(crate) package_manager: Option<PythonPackageManager>,
}

/// Mirror of Claude `verifyIt2Setup`'s result.
#[derive(Clone, Debug)]
pub(crate) struct It2VerifyResult {
    pub(crate) success: bool,
    pub(crate) error: Option<String>,
    pub(crate) needs_python_api_enabled: bool,
}

/// `true` iff `which <bin>` resolves the binary on PATH. Mirrors Claude's `which`-based
/// probes. Uses the `which` crate already in `core/Cargo.toml` (no shell, no subprocess).
fn which_resolves(bin: &str) -> bool {
    which::which(bin).is_ok()
}

/// Mirror of `detectPythonPackageManager`: `which uv` → Uvx; else `which pipx` → Pipx;
/// else `which pip` → Pip; else `which pip3` → Pip; else `None`.
pub(crate) async fn detect_python_package_manager() -> Option<PythonPackageManager> {
    if which_resolves("uv") {
        Some(PythonPackageManager::Uvx)
    } else if which_resolves("pipx") {
        Some(PythonPackageManager::Pipx)
    } else if which_resolves("pip") || which_resolves("pip3") {
        Some(PythonPackageManager::Pip)
    } else {
        None
    }
}

/// Mirror of the CLI detection used in setup: `which it2` resolves (code 0 ⇒ installed).
pub(crate) async fn is_it2_cli_available_via_which() -> bool {
    which_resolves("it2")
}

/// Run an install program from the user's home directory. `cwd = home_dir()` is a security
/// requirement (mirror Claude): it stops a project-level `pip.conf` / `uv.toml` from
/// redirecting the PyPI index during install.
async fn run_install(file: &str, args: &[&str]) -> ExecResult {
    let cwd: PathBuf = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let output = Command::new(file)
        .args(args)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;

    match output {
        Ok(out) => ExecResult {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            code: out.status.code().unwrap_or(-1),
        },
        Err(err) => ExecResult {
            stdout: String::new(),
            stderr: err.to_string(),
            code: 127,
        },
    }
}

/// Mirror of `installIt2`: install the `it2` CLI using the chosen package manager, always
/// from `home_dir()`. The `pip` path retries with `pip3` on failure (mirror Claude). Never
/// panics — a missing binary degrades to `success: false` with the captured stderr.
pub(crate) async fn install_it2(pm: PythonPackageManager) -> It2InstallResult {
    let result = match pm {
        PythonPackageManager::Uvx => run_install("uv", &["tool", "install", "it2"]).await,
        PythonPackageManager::Pipx => run_install("pipx", &["install", "it2"]).await,
        PythonPackageManager::Pip => {
            let first = run_install("pip", &["install", "--user", "it2"]).await;
            if first.code == 0 {
                first
            } else {
                // Retry with pip3 (mirror Claude's fallback).
                run_install("pip3", &["install", "--user", "it2"]).await
            }
        }
    };

    if result.code == 0 {
        It2InstallResult {
            success: true,
            error: None,
            package_manager: Some(pm),
        }
    } else {
        let error = if result.stderr.trim().is_empty() {
            format!(
                "it2 install via {} exited with code {}",
                pm.label(),
                result.code
            )
        } else {
            result.stderr.trim().to_string()
        };
        It2InstallResult {
            success: false,
            error: Some(error),
            package_manager: Some(pm),
        }
    }
}

/// Mirror of `verifyIt2Setup`: confirm `which it2` resolves, then `it2 session list`. On a
/// nonzero `session list`, lowercase stderr and flag `needs_python_api_enabled` when it
/// mentions the Python-API failure keywords (`api`, `python`, `connection refused`,
/// `not enabled`).
pub(crate) async fn verify_it2_setup() -> It2VerifyResult {
    if !is_it2_cli_available_via_which().await {
        return It2VerifyResult {
            success: false,
            error: Some("it2 CLI not found on PATH".to_string()),
            needs_python_api_enabled: false,
        };
    }

    let r = run_it2(&["session", "list"]).await;
    if r.code == 0 {
        return It2VerifyResult {
            success: true,
            error: None,
            needs_python_api_enabled: false,
        };
    }

    let lower = r.stderr.to_lowercase();
    let needs_python_api_enabled = ["api", "python", "connection refused", "not enabled"]
        .iter()
        .any(|kw| lower.contains(kw));

    It2VerifyResult {
        success: false,
        error: Some(if r.stderr.trim().is_empty() {
            format!("it2 session list exited with code {}", r.code)
        } else {
            r.stderr.trim().to_string()
        }),
        needs_python_api_enabled,
    }
}

/// Mirror of `getPythonApiInstructions`: EXACTLY these five lines (two are blank), verbatim
/// from `it2Setup.ts`.
pub(crate) fn get_python_api_instructions() -> Vec<String> {
    vec![
        "Almost done! Enable the Python API in iTerm2:".to_string(),
        String::new(),
        "  iTerm2 → Settings → General → Magic → Enable Python API".to_string(),
        String::new(),
        "After enabling, you may need to restart iTerm2.".to_string(),
    ]
}

// ---------------------------------------------------------------------------
// Setup-flag persistence (mirror getGlobalConfig/saveGlobalConfig flags).
//
// Claude stores `iterm2It2SetupComplete` + `preferTmuxOverIterm2` in its global config.
// team_store has no config field for this today, so this ports the behavior to a small
// JSON sidecar under the teams root (`<teams_root>/teams/it2_setup.json`). The integrator
// may instead choose two booleans in `~/.codex/config.toml` (`[teams]`) — see the summary.
// ---------------------------------------------------------------------------

/// Persisted iTerm2 setup flags. camelCase on disk to match Claude's config keys.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct It2SetupFlags {
    #[serde(default)]
    iterm2_it2_setup_complete: bool,
    #[serde(default)]
    prefer_tmux_over_iterm2: bool,
}

/// Sidecar path: `<teams_root>/teams/it2_setup.json`. Reuses team_store's `teams/`
/// directory so all team state lives under one root.
fn it2_setup_flags_path(teams_root: &Path) -> PathBuf {
    teams_root.join("teams").join("it2_setup.json")
}

fn read_it2_setup_flags(teams_root: &Path) -> It2SetupFlags {
    match std::fs::read(it2_setup_flags_path(teams_root)) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => It2SetupFlags::default(),
    }
}

fn write_it2_setup_flags(teams_root: &Path, flags: &It2SetupFlags) -> std::io::Result<()> {
    let path = it2_setup_flags_path(teams_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(flags).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, &path)
}

/// Mirror of `markIt2SetupComplete`: persist `iterm2It2SetupComplete = true`.
/// Best-effort — a write failure is reported but never panics.
pub(crate) fn mark_it2_setup_complete(teams_root: &Path) -> std::io::Result<()> {
    let mut flags = read_it2_setup_flags(teams_root);
    flags.iterm2_it2_setup_complete = true;
    write_it2_setup_flags(teams_root, &flags)
}

/// Mirror of `getIt2SetupComplete` (the read side of `markIt2SetupComplete`).
pub(crate) fn get_it2_setup_complete(teams_root: &Path) -> bool {
    read_it2_setup_flags(teams_root).iterm2_it2_setup_complete
}

/// Mirror of `setPreferTmuxOverIterm2`: persist the user's preference.
pub(crate) fn set_prefer_tmux_over_iterm2(teams_root: &Path, prefer: bool) -> std::io::Result<()> {
    let mut flags = read_it2_setup_flags(teams_root);
    flags.prefer_tmux_over_iterm2 = prefer;
    write_it2_setup_flags(teams_root, &flags)
}

/// Mirror of `getPreferTmuxOverIterm2`.
pub(crate) fn get_prefer_tmux_over_iterm2(teams_root: &Path) -> bool {
    read_it2_setup_flags(teams_root).prefer_tmux_over_iterm2
}

// ---------------------------------------------------------------------------
// team_store integration helper.
// ---------------------------------------------------------------------------

/// Persist a created pane onto the member record (mirror Claude reusing the tmux pane field
/// for both backends). Writes `tmux_pane_id = pane_id` and `backend_type = "iterm2"` for the
/// named member via [`team_store::update_config`]. No-op if the member is absent.
pub(crate) fn persist_pane_to_member(
    teams_root: &Path,
    team: &str,
    member_name: &str,
    pane_id: &str,
) -> std::io::Result<()> {
    team_store::update_config(teams_root, team, |cfg| {
        if let Some(member) = cfg.members.iter_mut().find(|m| m.name == member_name) {
            member.tmux_pane_id = pane_id.to_string();
            member.backend_type = Some(BackendType::Iterm2.as_str().to_string());
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pane_id_from_split_output() {
        assert_eq!(
            parse_split_output("Created new pane:  ABC-123-DEF "),
            "ABC-123-DEF"
        );
        assert_eq!(
            parse_split_output("noise\nCreated new pane: w0t1p2:UUID\nmore"),
            "w0t1p2:UUID"
        );
        assert_eq!(parse_split_output("no marker here"), "");
        assert_eq!(parse_split_output("Created new pane:"), "");
        assert_eq!(parse_split_output("Created new pane:   "), "");
    }

    #[test]
    fn package_manager_strings_are_verbatim() {
        assert_eq!(
            PythonPackageManager::Uvx.install_command_hint(),
            "uv tool install it2"
        );
        assert_eq!(
            PythonPackageManager::Pipx.install_command_hint(),
            "pipx install it2"
        );
        assert_eq!(
            PythonPackageManager::Pip.install_command_hint(),
            "pip install --user it2"
        );
    }

    #[test]
    fn python_api_instructions_are_exactly_five_lines() {
        let lines = get_python_api_instructions();
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0], "Almost done! Enable the Python API in iTerm2:");
        assert!(lines[1].is_empty());
        assert_eq!(
            lines[2],
            "  iTerm2 → Settings → General → Magic → Enable Python API"
        );
        assert!(lines[3].is_empty());
        assert_eq!(lines[4], "After enabling, you may need to restart iTerm2.");
    }

    #[test]
    fn backend_type_wire_value_is_iterm2() {
        assert_eq!(BackendType::Iterm2.as_str(), "iterm2");
    }

    #[test]
    fn setup_flags_round_trip() {
        let root = std::env::temp_dir().join(format!(
            "codex-it2-flags-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert!(!get_it2_setup_complete(&root));
        assert!(!get_prefer_tmux_over_iterm2(&root));

        mark_it2_setup_complete(&root).unwrap();
        set_prefer_tmux_over_iterm2(&root, true).unwrap();

        assert!(get_it2_setup_complete(&root));
        assert!(get_prefer_tmux_over_iterm2(&root));

        let _ = std::fs::remove_dir_all(&root);
    }
}

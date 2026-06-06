//! Hidden `codex teammate` subcommand: boots a headless embedded Codex session
//! driven by an on-disk team inbox (Phase 2 of the Claude-Code Teams port).
//!
//! A teammate is NOT the interactive TUI and NOT the one-shot `exec` flow — it
//! is a long-lived [`ThreadManager`]/`CodexThread` session whose turns are
//! pulled from `$CODEX_HOME/teams/{team}/inboxes/{agent}.json`. The lead spawns
//! one such process per teammate (Phase 3); this module is what that process
//! runs.
//!
//! The session-construction sequence mirrors the canonical headless caller
//! (`codex-rs/thread-manager-sample`), with [`Config`](codex_core::config::Config)
//! built through [`ConfigBuilder`] (the `codex exec` idiom) instead of a
//! hand-rolled struct literal.

mod runner;

use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use clap::Parser;
use codex_arg0::Arg0DispatchPaths;
use codex_core::ThreadManager;
use codex_core::config::ConfigBuilder;
use codex_core::config::ConfigOverrides;
use codex_core::config::find_codex_home;
use codex_core::init_state_db;
use codex_core::resolve_installation_id;
use codex_core::thread_store_from_config;
use codex_core::NewThread;
use codex_exec_server::EnvironmentManager;
use codex_exec_server::ExecServerRuntimePaths;
use codex_extension_api::empty_extension_registry;
use codex_login::AuthManager;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::SessionSource;
use codex_utils_cli::CliConfigOverrides;

use runner::TeammateRuntime;

/// Hidden: run this `codex` process as a team member driven by its on-disk
/// inbox. The lead emits exactly these flags when spawning a teammate process
/// (the Phase 3 spawn contract).
#[derive(Debug, Parser)]
pub struct TeammateCommand {
    /// `"{name}@{team}"` — internal identity (`team_store::agent_id`).
    #[arg(long = "agent-id")]
    pub agent_id: String,

    /// Display name used for messaging/tasks and the inbox file.
    #[arg(long = "agent-name")]
    pub agent_name: String,

    /// Team this member belongs to.
    #[arg(long = "team-name")]
    pub team_name: String,

    /// Round-robin pane/pill color assigned by the lead.
    #[arg(long = "agent-color")]
    pub agent_color: Option<String>,

    /// Lead session UUID that spawned this teammate.
    #[arg(long = "parent-session-id")]
    pub parent_session_id: Option<String>,

    /// Optional agent role/type label.
    #[arg(long = "agent-type")]
    pub agent_type: Option<String>,

    /// Require plan-mode approval before acting (reserved; Phase 4 wiring).
    #[arg(long = "plan-mode-required", default_value_t = false)]
    pub plan_mode_required: bool,

    /// Teammate launch-mode hint (`auto` | `tmux` | `in-process`); accepted for
    /// spawn-contract parity, not consumed by the run loop itself.
    #[arg(long = "teammate-mode")]
    pub teammate_mode: Option<String>,

    /// Initial prompt (Claude `config.prompt`) — the first turn's input.
    #[arg(long = "prompt")]
    pub prompt: Option<String>,

    #[clap(flatten)]
    pub config_overrides: CliConfigOverrides,
}

/// Build a headless embedded session for this teammate and run its inbox loop.
pub async fn run_main(mut cmd: TeammateCommand, arg0_paths: Arg0DispatchPaths) -> Result<()> {
    // A teammate is by definition a Codex Teams session; force-enable the
    // (default-off) `teams` feature so its tool set includes the team tools
    // (team_send, etc.). The lead's global `--enable teams` flag is parsed but
    // NOT folded into this subcommand's own `-c` overrides, so set it directly.
    cmd.config_overrides
        .raw_overrides
        .push("features.teams=true".to_string());

    let cli_kv_overrides = cmd
        .config_overrides
        .parse_overrides()
        .map_err(anyhow::Error::msg)?;

    // Mark this process as a Codex Teams teammate so core team tools route
    // cross-process via the on-disk file mailbox (a teammate's in-memory team
    // registry is empty — it never ran `create_team`, so the registry-backed
    // `team_send`/`team_message_list` path cannot resolve the team here).
    codex_core::set_teammate_identity(cmd.team_name.clone(), cmd.agent_name.clone());

    // Headless harness overrides: never block on approvals; thread arg0 paths
    // through so the teammate can find its own binary / sandbox helper.
    let overrides = ConfigOverrides {
        approval_policy: Some(AskForApproval::Never),
        codex_self_exe: arg0_paths.codex_self_exe.clone(),
        codex_linux_sandbox_exe: arg0_paths.codex_linux_sandbox_exe.clone(),
        main_execve_wrapper_exe: arg0_paths.main_execve_wrapper_exe.clone(),
        ..Default::default()
    };

    let codex_home = find_codex_home().context("find Codex home")?;
    let config = ConfigBuilder::default()
        .codex_home(codex_home.to_path_buf())
        .cli_overrides(cli_kv_overrides)
        .harness_overrides(overrides)
        .build()
        .await
        .context("build teammate config")?;

    let state_db = init_state_db(&config).await;
    let auth_manager =
        AuthManager::shared_from_config(&config, /* enable_codex_api_key_env */ true).await;
    let local_runtime_paths = ExecServerRuntimePaths::from_optional_paths(
        config.codex_self_exe.clone(),
        config.codex_linux_sandbox_exe.clone(),
    )?;
    let thread_store = thread_store_from_config(&config, state_db.clone());
    let environment_manager = Arc::new(
        EnvironmentManager::from_codex_home(config.codex_home.clone(), Some(local_runtime_paths))
            .await
            .context("init environment manager")?,
    );
    let installation_id = resolve_installation_id(&config.codex_home).await?;

    // `teams_root` is the $CODEX_HOME root; `team_store` joins `teams/` itself.
    // Capture it before `config` is moved into `start_thread`.
    let teams_root = config.codex_home.to_path_buf();

    let thread_manager = ThreadManager::new(
        &config,
        auth_manager,
        SessionSource::Cli,
        environment_manager,
        empty_extension_registry(),
        /* analytics_events_client */ None,
        Arc::clone(&thread_store),
        state_db,
        installation_id,
        /* attestation_provider */ None,
    );

    let NewThread { thread, .. } = thread_manager
        .start_thread(config)
        .await
        .context("start teammate Codex thread")?;

    let runtime = TeammateRuntime {
        teams_root,
        team: cmd.team_name,
        agent_name: cmd.agent_name,
        color: cmd.agent_color,
    };

    // `thread_manager` is held until the loop returns; dropping it tears down
    // the session, so keep it in scope for the lifetime of the teammate.
    let result = runner::run_teammate_loop(runtime, thread, cmd.prompt).await;
    drop(thread_manager);
    result
}

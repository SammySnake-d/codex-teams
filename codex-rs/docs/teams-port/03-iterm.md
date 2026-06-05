# Phase 6 — iTerm2 backend port spec (`it2` CLI)

Faithful Rust port of Claude Code's iTerm2 pane backend + setup flow. This is the
**直接照搬 (direct-port)** reference: every Claude identifier, file path, command string,
and JSON shape below is quoted from the truth source `ChinaSiro/claude-code-sourcemap`.

## 0. Claude source files (truth)

| Concern | Claude file path |
|---|---|
| Backend impl | `restored-src/src/utils/swarm/backends/ITermBackend.ts` |
| Backend trait + types | `restored-src/src/utils/swarm/backends/types.ts` (`PaneBackend`, `BackendType`, `PaneId`, `CreatePaneResult`, `BackendDetectionResult`) |
| Detection helpers | `restored-src/src/utils/swarm/backends/detection.ts` (`IT2_COMMAND`, `isInITerm2`, `isIt2CliAvailable`) |
| Setup helpers | `restored-src/src/utils/swarm/backends/it2Setup.ts` (`detectPythonPackageManager`, `installIt2`, `verifyIt2Setup`, `getPythonApiInstructions`, `markIt2SetupComplete`, `setPreferTmuxOverIterm2`, `getPreferTmuxOverIterm2`) |
| Setup prompt UI | `restored-src/src/utils/swarm/It2SetupPrompt.tsx` |
| Registry / detect flow | `restored-src/src/utils/swarm/backends/registry.ts` (`detectAndGetBackend`, `getBackendByType`, `registerITermBackend`) |
| Spawn-command build | `restored-src/src/utils/swarm/backends/PaneBackendExecutor.ts` (`spawnTeammate` → `sendCommandToPane`) |
| Subprocess helper | `restored-src/src/utils/execFileNoThrow.ts` |

## 1. The exact `it2` command strings (照搬 — these are the load-bearing facts)

`IT2_COMMAND = 'it2'`. All commands run via `execFileNoThrow(IT2_COMMAND, args)` (argv vectors,
**not** a shell). Verbatim argv arrays from `ITermBackend.ts` / `detection.ts`:

| Operation | argv (verbatim) | Notes |
|---|---|---|
| availability / API probe | `['session', 'list']` | `isIt2CliAvailable` uses `session list` (NOT `--version`) because `--version` succeeds even when the Python API is disabled. |
| 1st teammate split (have leader id) | `['session', 'split', '-v', '-s', <leaderSessionId>]` | vertical split off the leader. |
| 1st teammate split (no leader id) | `['session', 'split', '-v']` | fallback: split active session. |
| Nth teammate split (have last id) | `['session', 'split', '-s', <targetedTeammateId>]` | horizontal split off the **last** teammate session (stacks vertically on screen). |
| Nth teammate split (no last id) | `['session', 'split']` | fallback: split active session. |
| dead-target probe (recovery) | `['session', 'list']` | only when a targeted split returns nonzero — confirm the target is gone before pruning. |
| send command to pane | `['session', 'run', '-s', <paneId>, <command>]` | adds newline automatically. If `paneId` empty: `['session', 'run', <command>]`. |
| kill pane | `['session', 'close', '-f', '-s', <paneId>]` | `-f` (force) is REQUIRED, else iTerm2's "Confirm before closing" prompt blocks. |

Setup-path commands (from `it2Setup.ts`), run with `which` for PATH lookup:
- detect pkg mgr: `which uv` → `'uvx'`; else `which pipx` → `'pipx'`; else `which pip` → `'pip'`; else `which pip3` → `'pip'`; else `None`.
- detect cli: `which it2` (code 0 ⇒ installed).
- install: `'uvx'` → `uv tool install it2`; `'pipx'` → `pipx install it2`; `'pip'` → `pip install --user it2` (on fail, retry `pip3 install --user it2`). **All run with `cwd = home_dir()`** (security: avoid project-level `pip.conf`/`uv.toml` redirecting PyPI).
- verify: `which it2` then `it2 session list`; on nonzero, lowercase stderr and if it contains any of `"api" | "python" | "connection refused" | "not enabled"` ⇒ `needs_python_api_enabled = true`.

Leader session id is parsed from env `ITERM_SESSION_ID` (format `"wXtYpZ:UUID"` — take the substring **after the first `:`**). iTerm2 detection: `TERM_PROGRAM == "iTerm.app"` OR `ITERM_SESSION_ID` present OR terminal == `iTerm.app`.

`parseSplitOutput` (verbatim regex): match `/Created new pane:\s*(.+)/`, return capture group 1 `.trim()`, else `""`.

## 2. Codex target files / module path

- **NEW** `codex-rs/core/src/team_backends/mod.rs` — declares the backend trait + submodules (mirrors Claude `types.ts` + `registry.ts`). If P3 (tmux) lands first, it already exists; this phase only adds `pub mod iterm;` and an `Iterm2` arm.
- **NEW** `codex-rs/core/src/team_backends/iterm.rs` — the iTerm2 backend (this spec's core).
- Module wiring: add `mod team_backends;` in `codex-rs/core/src/lib.rs` next to `mod team_store;` (line ~101). Keep `pub(crate)` visibility — backend is internal to core.
- Setup-prompt UI is TUI-side (Phase 5/6 TUI work): see §6. No new file required in this phase if you only emit instruction strings; if you build the interactive prompt, target `codex-rs/tui/src/chatwidget/team_ui.rs` (already exists) or a sibling `team_setup.rs`.

Deps already present in `core/Cargo.toml`: `which` (PATH lookup), `tokio` (async process), `serde`. Use `tokio::process::Command` for the async subprocess (mirror `execFileNoThrow`). No new crate needed.

## 3. Shared trait (mirror `PaneBackend`) — `team_backends/mod.rs`

Codex has no `AgentColorName` enum in `team.rs` (grep confirmed none). Use the existing
color representation already used by `team_store::TeamFileMember.color: Option<String>` — i.e.
pass color as `&str`. (The iTerm backend ignores color anyway — see no-ops below.)

```rust
// codex-rs/core/src/team_backends/mod.rs
pub(crate) mod iterm;
// pub(crate) mod tmux;   // P3

/// Mirror of Claude `BackendType` ('tmux' | 'iterm2' | 'in-process').
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BackendType { Tmux, Iterm2, InProcess }

impl BackendType {
    pub(crate) fn as_str(self) -> &'static str {
        match self { Self::Tmux => "tmux", Self::Iterm2 => "iterm2", Self::InProcess => "in_process" }
    }
}

/// Mirror of Claude `PaneId` (opaque). For iTerm2 this is the it2 session UUID.
pub(crate) type PaneId = String;

/// Mirror of Claude `CreatePaneResult`.
#[derive(Clone, Debug)]
pub(crate) struct CreatePaneResult {
    pub(crate) pane_id: PaneId,
    pub(crate) is_first_teammate: bool,
}

/// Mirror of Claude `PaneBackend`. Async (Claude methods are all Promise-returning).
#[async_trait::async_trait]
pub(crate) trait PaneBackend: Send + Sync {
    fn backend_type(&self) -> BackendType;
    fn display_name(&self) -> &'static str;
    fn supports_hide_show(&self) -> bool;

    async fn is_available(&self) -> bool;
    async fn is_running_inside(&self) -> bool;

    async fn create_teammate_pane_in_swarm_view(
        &self,
        name: &str,
        color: &str,
    ) -> anyhow::Result<CreatePaneResult>;

    async fn send_command_to_pane(
        &self,
        pane_id: &str,
        command: &str,
        use_external_session: bool, // tmux-only; iTerm ignores
    ) -> anyhow::Result<()>;

    async fn set_pane_border_color(&self, pane_id: &str, color: &str, use_external_session: bool) -> anyhow::Result<()>;
    async fn set_pane_title(&self, pane_id: &str, name: &str, color: &str, use_external_session: bool) -> anyhow::Result<()>;
    async fn enable_pane_border_status(&self, window_target: Option<&str>, use_external_session: bool) -> anyhow::Result<()>;
    async fn rebalance_panes(&self, window_target: &str, has_leader: bool) -> anyhow::Result<()>;

    async fn kill_pane(&self, pane_id: &str, use_external_session: bool) -> anyhow::Result<bool>;
    async fn hide_pane(&self, pane_id: &str, use_external_session: bool) -> anyhow::Result<bool>;
    async fn show_pane(&self, pane_id: &str, target_window_or_pane: &str, use_external_session: bool) -> anyhow::Result<bool>;
}
```
> `async_trait` is the idiomatic way to get async trait methods; if the tmux P3 spec chose
> a non-trait design (enum dispatch), mirror that instead — keep the two backends symmetric.

## 4. `team_backends/iterm.rs` — Rust signatures (照搬 ITermBackend.ts)

Module-level mutable state in Claude is two globals (`teammateSessionIds: string[]`, `firstPaneUsed: bool`)
plus a promise-chain lock. In Rust, hold them behind a `tokio::sync::Mutex` *inside* the struct so
`create_teammate_pane_in_swarm_view` is serialized (the lock replaces Claude's `acquirePaneCreationLock`).

```rust
// codex-rs/core/src/team_backends/iterm.rs
use tokio::sync::Mutex;

pub(crate) const IT2_COMMAND: &str = "it2";

#[derive(Default)]
struct IT2State {
    teammate_session_ids: Vec<String>,
    first_pane_used: bool,
}

pub(crate) struct ITermBackend {
    state: Mutex<IT2State>,
}

impl ITermBackend {
    pub(crate) fn new() -> Self { Self { state: Mutex::new(IT2State::default()) } }
}

/// Mirror of execFileNoThrow: never errors on nonzero exit; returns (stdout, stderr, code).
/// Use tokio::process::Command::new(file).args(args).output().await.
async fn run_it2(args: &[&str]) -> ExecResult { /* ... */ }

struct ExecResult { stdout: String, stderr: String, code: i32 }

/// Mirror parseSplitOutput: regex r"Created new pane:\s*(.+)" -> trimmed group 1, else "".
fn parse_split_output(output: &str) -> String { /* ... */ }

/// Mirror getLeaderSessionId: env ITERM_SESSION_ID, take substring after first ':'.
fn get_leader_session_id() -> Option<String> { /* ... */ }

// Free fns mirroring detection.ts (or place in team_backends/detection.rs):
fn is_in_iterm2() -> bool;                 // TERM_PROGRAM=="iTerm.app" || ITERM_SESSION_ID set
async fn is_it2_cli_available() -> bool;   // run_it2(&["session","list"]).code == 0
```

### 4.1 `create_teammate_pane_in_swarm_view` — the core algorithm (照搬 verbatim logic)

Hold `self.state.lock().await` for the whole body (replaces the JS lock). Then loop:

1. `is_first_teammate = !state.first_pane_used`.
2. Build `split_args` + optional `targeted_teammate_id`:
   - first + leader id present → `["session","split","-v","-s", leader]`, `targeted=None`.
   - first + no leader id → `["session","split","-v"]`, `targeted=None`.
   - else + last id present → `["session","split","-s", last]`, `targeted=Some(last)`.
   - else + no last id → `["session","split"]`, `targeted=None`.
3. `let r = run_it2(&split_args).await;`
4. If `r.code != 0`:
   - If `targeted = Some(id)`: run `["session","list"]`; if that `code==0 && !stdout.contains(id)` ⇒ **confirmed dead** — remove `id` from `teammate_session_ids`; if now empty set `first_pane_used=false`; `continue` the loop (retry next-to-last/leader). Else (alive or can't tell) ⇒ **don't prune**, fall through to error.
   - `return Err(anyhow!("Failed to create iTerm2 split pane: {}", r.stderr));`
5. If `is_first_teammate` ⇒ `state.first_pane_used = true`.
6. `let pane_id = parse_split_output(&r.stdout);` if empty ⇒ `return Err(anyhow!("Failed to parse session ID from split output: {}", r.stdout));`
7. `state.teammate_session_ids.push(pane_id.clone());`
8. **Skip color & title** (perf — each `it2` call spawns a Python process). Return `CreatePaneResult { pane_id, is_first_teammate }`.

> Loop bound: each `continue` shrinks `teammate_session_ids` by 1 → O(N+1) iterations, terminates (Claude's own comment).

### 4.2 Other methods (照搬, mostly no-ops)

| Method | Body |
|---|---|
| `backend_type` | `BackendType::Iterm2` |
| `display_name` | `"iTerm2"` |
| `supports_hide_show` | `false` |
| `is_available` | `is_in_iterm2() && is_it2_cli_available().await` |
| `is_running_inside` | `is_in_iterm2()` |
| `send_command_to_pane` | argv = if pane non-empty `["session","run","-s",pane,cmd]` else `["session","run",cmd]`; nonzero ⇒ `Err`. |
| `set_pane_border_color` | **no-op** `Ok(())` (perf). |
| `set_pane_title` | **no-op** `Ok(())` (perf). |
| `enable_pane_border_status` | **no-op** `Ok(())` (iTerm2 shows titles in tabs). |
| `rebalance_panes` | **no-op** `Ok(())` (iTerm2 auto-balances). |
| `kill_pane` | run `["session","close","-f","-s",pane]`; **always** remove `pane` from `teammate_session_ids` (lock); if empty set `first_pane_used=false`; return `r.code == 0`. |
| `hide_pane` | **unsupported** — `Ok(false)` (no iTerm2 equivalent to tmux `break-pane`). |
| `show_pane` | **unsupported** — `Ok(false)` (no iTerm2 equivalent to tmux `join-pane`). |

## 5. Setup-check flow (照搬 it2Setup.ts + registry detect priority)

Detection priority (`registry.ts::detectAndGetBackend`) the codex equivalent must follow:
1. **Inside tmux** → tmux (even in iTerm2).
2. **In iTerm2** and `get_prefer_tmux_over_iterm2()==false` and `is_it2_cli_available()` → **Iterm2** backend (`is_native=true`, `needs_it2_setup=false`).
3. In iTerm2, it2 missing, tmux available → tmux fallback, `needs_it2_setup = !prefer_tmux`.
4. In iTerm2, no it2 & no tmux → error: `"iTerm2 detected but it2 CLI not installed. Install it2 with: pip install it2"`.
5. Not tmux/iTerm2, tmux available → tmux external session.
6. Else → tmux install-instructions error.

Setup helper signatures to port (place in `team_backends/iterm.rs` or a `team_backends/it2_setup.rs`):

```rust
#[derive(Clone, Copy)] pub(crate) enum PythonPackageManager { Uvx, Pipx, Pip }

pub(crate) struct It2InstallResult { pub success: bool, pub error: Option<String>, pub package_manager: Option<PythonPackageManager> }
pub(crate) struct It2VerifyResult  { pub success: bool, pub error: Option<String>, pub needs_python_api_enabled: bool }

pub(crate) async fn detect_python_package_manager() -> Option<PythonPackageManager>; // which uv|pipx|pip|pip3
pub(crate) async fn is_it2_cli_available_via_which() -> bool;                        // which it2
pub(crate) async fn install_it2(pm: PythonPackageManager) -> It2InstallResult;       // cwd=home_dir()
pub(crate) async fn verify_it2_setup() -> It2VerifyResult;                           // which it2 + it2 session list
pub(crate) fn get_python_api_instructions() -> Vec<String>;                          // exact 5 lines below
pub(crate) fn mark_it2_setup_complete();                                             // persist flag (see config note)
pub(crate) fn set_prefer_tmux_over_iterm2(prefer: bool);
pub(crate) fn get_prefer_tmux_over_iterm2() -> bool;
```

`get_python_api_instructions()` returns EXACTLY (verbatim from `it2Setup.ts`):
```
Almost done! Enable the Python API in iTerm2:
<blank>
  iTerm2 → Settings → General → Magic → Enable Python API
<blank>
After enabling, you may need to restart iTerm2.
```

**Config persistence note (blocking-ish):** Claude stores `iterm2It2SetupComplete: true` and
`preferTmuxOverIterm2: bool` in its global config (`getGlobalConfig`/`saveGlobalConfig`). Codex
must pick the analog: either two booleans in `~/.codex/config.toml` (e.g. `[teams] it2_setup_complete`,
`prefer_tmux_over_iterm2`) or a small JSON sidecar under `$CODEX_HOME`. team_store has no config
field for this today — see §7 unknown #1.

## 6. Setup-prompt UI strings (照搬 It2SetupPrompt.tsx — verbatim render text)

Step machine: `initial → installing → install-failed | api-instructions → verifying → success | failed`.
Title (Pane, bold): `iTerm2 Split Pane Setup`. After verify success: `setTimeout(onDone, 1500, 'installed')`.

| Step | Verbatim text |
|---|---|
| initial | `To use native iTerm2 split panes for teammates, you need the it2 CLI tool.` / `This enables teammates to appear as split panes within your current window.` |
| initial options | `Install it2 now` (desc `Uses {pm} to install the it2 CLI tool` or `Requires Python (uvx, pipx, or pip)`); if tmux available `Use tmux instead` (desc `Opens teammates in a separate tmux session`); `Cancel` (desc `Skip teammate spawning for now`) |
| installing | `Installing it2 using {packageManager}…` / `This may take a moment.` |
| install-failed | `Installation failed` / `You can try installing manually: {uv tool install it2 \| pipx install it2 \| pip install --user it2}`; options `Try again` / `Use tmux instead` / `Cancel` |
| api-instructions | `✓ it2 installed successfully` + the 5 `get_python_api_instructions()` lines + `Press Enter when ready to verify…` |
| verifying | `Verifying it2 can communicate with iTerm2…` |
| success | `✓ iTerm2 split pane support is ready` / `Teammates will now appear as split panes.` |
| failed | `Verification failed` / `Make sure:` / `· Python API is enabled in iTerm2 preferences` / `· You may need to restart iTerm2 after enabling`; options `Try again` / `Use tmux instead` / `Cancel` |

`onDone` result enum: `'installed' | 'use-tmux' | 'cancelled'`. On `Use tmux instead` →
`set_prefer_tmux_over_iterm2(true)` then `use-tmux`; caller resets backend detection and re-fetches.
On verify success → `mark_it2_setup_complete()`.

## 7. Wiring to team_store / team.rs / team tools

- **team_store (`core/src/team_store.rs`)** — backend writes the resulting pane id + backend type onto
  the member record. Use existing fields: `TeamFileMember.tmux_pane_id: String` (store the it2 session
  UUID here — Claude reuses the same field for both backends; the field name is tmux-historical but the
  payload is the pane/session id) and `backend_type: Option<String>` = `"iterm"` (per
  `TEAMS_CLAUDE_PORT_SPEC.md` enum `tmux|iterm|in_process`; `BackendType::Iterm2.as_str()` returns
  `"iterm2"` to match Claude's wire value — **reconcile** with the spec's `"iterm"`, see unknown #2).
  Persist via `team_store::update_config(teams_root, team, |cfg| { member.tmux_pane_id = pane_id; member.backend_type = Some("iterm2".into()); })`.
- **team.rs (`TeamRegistry::spawn_member`, line ~345)** — P3 replaces the in-process thread spawn with
  pane spawn. The iTerm path is reached when `detect_and_get_backend()` selects `Iterm2`. Flow:
  1. `let res = backend.create_teammate_pane_in_swarm_view(name, color).await?;`
  2. build spawn command (see below), `backend.send_command_to_pane(&res.pane_id, &cmd, /*use_external_session=*/ false).await?;` (iTerm ignores the flag).
  3. write `tmux_pane_id = res.pane_id`, `backend_type="iterm2"`, `backend_type` member fields to config.json.
- **Spawn command string (照搬 PaneBackendExecutor.spawnTeammate)** — shell string sent into the pane:
  `cd <quote(cwd)> && env <envStr> <quote(binaryPath)> <teammateArgs><flagsStr>`
  where `teammateArgs` = `--agent-id "<name>@<team>" --agent-name <display> --team-name <team> --agent-color <color> --parent-session-id <leadSessionUuid> [--agent-type <t>] [--plan-mode-required]`
  (identity flags per `TEAMS_CLAUDE_PORT_SPEC.md` §Identity), `binaryPath` = the codex teammate binary
  (`getTeammateCommand` analog), `envStr` from a `build_inherited_env_vars()` analog, `flagsStr` may
  add `--model <m>`. This is shared with the tmux backend (P3) — define once in `team_backends/mod.rs`
  or a `spawn_command.rs`, not per-backend.
- **team tools (`core/src/tools/handlers/team.rs`)** — `team_spawn_member` (line ~384) keeps emitting
  `TeamSpawnMemberResult { member }` unchanged so the model + TUI observer are unaffected; only the
  registry's spawn implementation changes underneath. `team_member_stop` (line ~706) on an iterm member
  calls `backend.kill_pane(&member.tmux_pane_id, false)`.
- **teams_root** — all team_store calls take `teams_root: &Path` = `$CODEX_HOME` (default `~/.codex`),
  resolved by the existing core config (same value team_store callers already pass).

## 8. Notes / risks
- `run_it2` MUST NOT use a shell (argv vector via `tokio::process::Command`); only the *spawn command*
  string in §7 is shell-interpreted (it's sent as keystrokes into the pane, exactly as Claude does).
- `it2 session list` is the canonical health probe everywhere (availability, verify, dead-target). Do
  not substitute `it2 --version`.
- The module-global lock in Claude is process-wide; in Rust it lives in the `ITermBackend` instance —
  ensure the registry caches **one** `ITermBackend` instance for the session (mirror `cachedBackend`),
  else parallel spawns from different instances race the iTerm layout.

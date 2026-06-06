//! tmux teammate-pane backend (Phase 3a of the Claude Code teams port).
//!
//! Faithful Rust port of Claude Code's
//! `restored-src/src/utils/swarm/backends/TmuxBackend.ts`
//! (with constants from `swarm/constants.ts`, detection from
//! `swarm/backends/detection.ts`, and the launch-command shape from
//! `swarm/backends/PaneBackendExecutor.ts` / `spawnMultiAgent.ts`).
//!
//! Like codex's own `terminal-detection` module (`tmux_display_message`), this
//! backend shells out synchronously via [`std::process::Command`], so it needs
//! no async runtime and matches Claude's per-call `execFile` shape.
//!
//! The only intentional deviation from Claude is the swarm session name:
//! `"claude-swarm"` becomes `"codex-swarm"` (and the socket likewise becomes
//! `"codex-swarm-{pid}"`), per `TEAMS_CLAUDE_PORT_SPEC.md` P3. The window name
//! `"swarm-view"` is kept verbatim.
//!
//! This file is a pure backend: it owns no team state. It stays unwired until
//! the lead integrates it (it does not declare any modules and reuses
//! [`crate::team_store`] only conceptually — no direct calls live here yet).
#![allow(dead_code)]

use std::cell::RefCell;
use std::io;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// tmux binary name (`TMUX_COMMAND` in `swarm/constants.ts`).
const TMUX_COMMAND: &str = "tmux";
/// Detached swarm session name. Claude: `"claude-swarm"` (`SWARM_SESSION_NAME`).
const SWARM_SESSION_NAME: &str = "codex-swarm";
/// Swarm window name (`SWARM_VIEW_WINDOW_NAME` in `swarm/constants.ts`).
const SWARM_VIEW_WINDOW_NAME: &str = "swarm-view";
/// Sleep after a split so the new pane's shell is ready (`PANE_SHELL_INIT_DELAY_MS`).
const PANE_SHELL_INIT_DELAY_MS: u64 = 200;

// ---------------------------------------------------------------------------
// Shared backend types (mirrors `swarm/types.ts`).
//
// NOTE: the spec places these in a sibling `team_backends/mod.rs` so a later
// `iterm.rs` can reuse them. Because this phase may only create the single
// target file, they live here for now; the integrator should hoist them up to
// `mod.rs` when wiring `iterm.rs` (P6). They are deliberately self-contained.
// ---------------------------------------------------------------------------

/// Mirrors Claude `BackendType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendType {
    /// tmux split-pane backend (this file).
    Tmux,
    /// iTerm2 split-pane backend (Phase 6).
    Iterm,
    /// In-process teammate (no external multiplexer).
    InProcess,
}

/// Mirrors Claude `AgentColorName`.
///
/// Stored on disk as the lowercase string ([`crate::team_store::TeamFileMember`]'s
/// `color` is `Option<String>`); convert via [`AgentColor::from_name`] /
/// [`AgentColor::as_name`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentColor {
    /// `red`.
    Red,
    /// `blue`.
    Blue,
    /// `green`.
    Green,
    /// `yellow`.
    Yellow,
    /// `purple` (maps to tmux `magenta`).
    Purple,
    /// `orange` (maps to tmux `colour208`).
    Orange,
    /// `pink` (maps to tmux `colour205`).
    Pink,
    /// `cyan`.
    Cyan,
}

impl AgentColor {
    /// `getTmuxColorName` — agent color to tmux color name (see Claude
    /// `TmuxBackend.getTmuxColorName`).
    pub fn tmux_color(self) -> &'static str {
        match self {
            AgentColor::Red => "red",
            AgentColor::Blue => "blue",
            AgentColor::Green => "green",
            AgentColor::Yellow => "yellow",
            AgentColor::Purple => "magenta",
            AgentColor::Orange => "colour208",
            AgentColor::Pink => "colour205",
            AgentColor::Cyan => "cyan",
        }
    }

    /// Parse the on-disk `TeamFileMember.color` string; unknown -> `None`.
    pub fn from_name(s: &str) -> Option<Self> {
        match s {
            "red" => Some(AgentColor::Red),
            "blue" => Some(AgentColor::Blue),
            "green" => Some(AgentColor::Green),
            "yellow" => Some(AgentColor::Yellow),
            "purple" => Some(AgentColor::Purple),
            "orange" => Some(AgentColor::Orange),
            "pink" => Some(AgentColor::Pink),
            "cyan" => Some(AgentColor::Cyan),
            _ => None,
        }
    }

    /// The lowercase on-disk name (`"red"`..`"cyan"`).
    pub fn as_name(self) -> &'static str {
        match self {
            AgentColor::Red => "red",
            AgentColor::Blue => "blue",
            AgentColor::Green => "green",
            AgentColor::Yellow => "yellow",
            AgentColor::Purple => "purple",
            AgentColor::Orange => "orange",
            AgentColor::Pink => "pink",
            AgentColor::Cyan => "cyan",
        }
    }
}

/// Mirrors Claude `CreatePaneResult`.
#[derive(Clone, Debug)]
pub struct CreatePaneResult {
    /// The new tmux pane id (e.g. `"%3"`).
    pub pane_id: String,
    /// `true` when this was the very first teammate pane in the window/session.
    pub is_first_teammate: bool,
    /// `true` when launched in the detached `codex-swarm` session (outside tmux).
    pub used_external_session: bool,
}

// ---------------------------------------------------------------------------
// execFileNoThrow analog.
// ---------------------------------------------------------------------------

/// Result of an `execFileNoThrow`-style tmux call: captured output that never
/// errors on non-zero exit (mirrors `utils/execFileNoThrow.ts`).
#[derive(Clone, Debug, Default)]
struct TmuxOutput {
    stdout: String,
    stderr: String,
    code: i32,
}

/// `execFileNoThrow` analog: run a command, capturing stdout/stderr; never
/// returns `Err`. `code` is `status.code().unwrap_or(-1)`; a spawn failure
/// (e.g. binary missing) yields `code = -1` and the OS error string in `stderr`.
fn exec_file_no_throw(cmd: &str, args: &[&str]) -> TmuxOutput {
    match Command::new(cmd).args(args).output() {
        Ok(output) => TmuxOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            code: output.status.code().unwrap_or(-1),
        },
        Err(e) => TmuxOutput {
            stdout: String::new(),
            stderr: e.to_string(),
            code: -1,
        },
    }
}

// ---------------------------------------------------------------------------
// TmuxBackend.
// ---------------------------------------------------------------------------

/// tmux implementation of Claude's `PaneBackend`.
///
/// Detection state (`inside_tmux`, `leader_pane_id`, pid) is captured once at
/// construction to mirror Claude's module-load capture of `$TMUX` / `$TMUX_PANE`
/// in `detection.ts`. Construct this from the lead session's startup snapshot.
#[derive(Debug)]
pub struct TmuxBackend {
    /// `ORIGINAL_TMUX_PANE` (captured `$TMUX_PANE`); `None` when not inside tmux.
    leader_pane_id: Option<String>,
    /// `ORIGINAL_USER_TMUX` (captured `$TMUX` non-empty) via codex detection.
    inside_tmux: bool,
    /// `getSwarmSocketName()` => `"codex-swarm-{pid}"`.
    swarm_socket: String,
    /// `cachedLeaderWindowTarget` — memoized `session:window_index`.
    cached_window_target: RefCell<Option<String>>,
}

impl TmuxBackend {
    /// Captures `$TMUX` (via codex `terminal-detection`), `$TMUX_PANE`, and pid
    /// ONCE — mirrors the module-load capture in `detection.ts`.
    ///
    /// Inside/outside tmux is decided by
    /// `matches!(terminal_info().multiplexer, Some(Multiplexer::Tmux { .. }))`,
    /// which is set when `TMUX` or `TMUX_PANE` is non-empty — matching Claude's
    /// `!!ORIGINAL_USER_TMUX`. The leader pane id is read straight from
    /// `$TMUX_PANE` because `terminal_info()` does not expose it. For exact
    /// Claude parity this should run at lead-session startup.
    pub fn new() -> Self {
        let inside_tmux = matches!(
            codex_terminal_detection::terminal_info().multiplexer,
            Some(codex_terminal_detection::Multiplexer::Tmux { .. })
        );
        let leader_pane_id = std::env::var("TMUX_PANE")
            .ok()
            .filter(|s| !s.is_empty());
        Self::from_parts(inside_tmux, leader_pane_id, std::process::id())
    }

    /// Test/construction seam: build with explicit detection state and pid.
    pub fn from_parts(inside_tmux: bool, leader_pane_id: Option<String>, pid: u32) -> Self {
        Self {
            leader_pane_id,
            inside_tmux,
            swarm_socket: format!("{SWARM_SESSION_NAME}-{pid}"),
            cached_window_target: RefCell::new(None),
        }
    }

    // ---- detection ----

    /// `isInsideTmux()` — whether the lead session is itself inside tmux.
    pub fn is_inside_tmux(&self) -> bool {
        self.inside_tmux
    }

    /// `getLeaderPaneId()` — the captured `$TMUX_PANE`.
    fn get_leader_pane_id(&self) -> Option<String> {
        self.leader_pane_id.clone()
    }

    /// `getSwarmSocketName()` — `"codex-swarm-{pid}"`.
    fn swarm_socket_name(&self) -> &str {
        &self.swarm_socket
    }

    // ---- tmux runners ----

    /// `runTmuxInUserSession(args)` — invoke `tmux <args>` in the user's session.
    fn run_in_user_session(&self, args: &[&str]) -> TmuxOutput {
        exec_file_no_throw(TMUX_COMMAND, args)
    }

    /// `runTmuxInSwarm(args)` — prepends `["-L", swarm_socket]` then args so the
    /// call targets the detached swarm server.
    fn run_in_swarm(&self, args: &[&str]) -> TmuxOutput {
        let mut full: Vec<&str> = Vec::with_capacity(args.len() + 2);
        full.push("-L");
        full.push(&self.swarm_socket);
        full.extend_from_slice(args);
        exec_file_no_throw(TMUX_COMMAND, &full)
    }

    /// Dispatch a raw tmux call to either the user session or the swarm server.
    fn run(&self, use_external: bool, args: &[&str]) -> TmuxOutput {
        if use_external {
            self.run_in_swarm(args)
        } else {
            self.run_in_user_session(args)
        }
    }

    // ---- queries ----

    /// `getCurrentPaneId()` — prefer the captured leader pane id, falling back
    /// to `display-message -p #{pane_id}`.
    fn get_current_pane_id(&self) -> Option<String> {
        if let Some(pane) = self.get_leader_pane_id() {
            return Some(pane);
        }
        let out = self.run_in_user_session(&["display-message", "-p", "#{pane_id}"]);
        non_empty_trimmed(&out.stdout)
    }

    /// `getCurrentWindowTarget()` — memoized `session:window_index` for the
    /// leader's window. Uses the leader pane as `-t` target when known.
    fn get_current_window_target(&self) -> Option<String> {
        if let Some(cached) = self.cached_window_target.borrow().clone() {
            return Some(cached);
        }
        let fmt = "#{session_name}:#{window_index}";
        let out = match self.get_leader_pane_id() {
            Some(leader) => {
                self.run_in_user_session(&["display-message", "-t", &leader, "-p", fmt])
            }
            None => self.run_in_user_session(&["display-message", "-p", fmt]),
        };
        let target = non_empty_trimmed(&out.stdout)?;
        *self.cached_window_target.borrow_mut() = Some(target.clone());
        Some(target)
    }

    /// `getCurrentWindowPaneCount()` — count non-empty `#{pane_id}` lines for the
    /// given window target. When `window_target` is `None`, resolves it first.
    fn get_current_window_pane_count(
        &self,
        window_target: Option<&str>,
        use_swarm: bool,
    ) -> Option<usize> {
        let resolved;
        let target = match window_target {
            Some(t) => t,
            None => {
                resolved = self.get_current_window_target()?;
                &resolved
            }
        };
        let out = self.run(use_swarm, &["list-panes", "-t", target, "-F", "#{pane_id}"]);
        Some(split_pane_ids(&out.stdout).len())
    }

    // ---- pane styling / io (public PaneBackend surface) ----

    /// `setPaneBorderColor` (tmux 3.2+ pane options).
    pub fn set_pane_border_color(
        &self,
        pane_id: &str,
        color: AgentColor,
        use_external: bool,
    ) -> io::Result<()> {
        let tmux_color = color.tmux_color();
        // select-pane -t <pane> -P bg=default,fg=<color>
        let style = format!("bg=default,fg={tmux_color}");
        self.run(use_external, &["select-pane", "-t", pane_id, "-P", &style]);
        // set-option -p -t <pane> pane-border-style fg=<color>
        let border = format!("fg={tmux_color}");
        self.run(
            use_external,
            &[
                "set-option",
                "-p",
                "-t",
                pane_id,
                "pane-border-style",
                &border,
            ],
        );
        // set-option -p -t <pane> pane-active-border-style fg=<color>
        self.run(
            use_external,
            &[
                "set-option",
                "-p",
                "-t",
                pane_id,
                "pane-active-border-style",
                &border,
            ],
        );
        Ok(())
    }

    /// `setPaneTitle`.
    pub fn set_pane_title(
        &self,
        pane_id: &str,
        name: &str,
        color: AgentColor,
        use_external: bool,
    ) -> io::Result<()> {
        let tmux_color = color.tmux_color();
        // select-pane -t <pane> -T <name>
        self.run(use_external, &["select-pane", "-t", pane_id, "-T", name]);
        // set-option -p -t <pane> pane-border-format "#[fg=<color>,bold] #{pane_title} #[default]"
        let format = format!("#[fg={tmux_color},bold] #{{pane_title}} #[default]");
        self.run(
            use_external,
            &[
                "set-option",
                "-p",
                "-t",
                pane_id,
                "pane-border-format",
                &format,
            ],
        );
        Ok(())
    }

    /// `sendCommandToPane` — `send-keys -t <pane> <command> Enter`. The literal
    /// final arg is the string `"Enter"` (Claude uses `'Enter'`, not `C-m`).
    /// A non-zero tmux exit code yields `Err` with the captured stderr.
    pub fn send_command_to_pane(
        &self,
        pane_id: &str,
        command: &str,
        use_external: bool,
    ) -> io::Result<()> {
        let out = self.run(
            use_external,
            &["send-keys", "-t", pane_id, command, "Enter"],
        );
        ok_or_tmux_err(&out, "send-keys")
    }

    // ---- layout ----

    /// `rebalancePanesWithLeader` — only acts when the window has more than two
    /// panes; lays out `main-vertical` and pins the leader (`panes[0]`) to 30%.
    fn rebalance_panes_with_leader(&self, window_target: &str) {
        let out =
            self.run_in_user_session(&["list-panes", "-t", window_target, "-F", "#{pane_id}"]);
        let panes = split_pane_ids(&out.stdout);
        if panes.len() <= 2 {
            return;
        }
        self.run_in_user_session(&["select-layout", "-t", window_target, "main-vertical"]);
        self.run_in_user_session(&["resize-pane", "-t", &panes[0], "-x", "30%"]);
    }

    // ---- creation (the entry point) ----

    /// `createTeammatePaneInSwarmView` — dispatch on [`Self::is_inside_tmux`].
    pub fn create_teammate_pane(
        &self,
        teammate_name: &str,
        color: AgentColor,
    ) -> io::Result<CreatePaneResult> {
        if self.is_inside_tmux() {
            self.create_teammate_pane_with_leader(teammate_name, color)
        } else {
            self.create_teammate_pane_external(teammate_name, color)
        }
    }

    /// `createTeammatePaneWithLeader` — split within the leader's window.
    fn create_teammate_pane_with_leader(
        &self,
        teammate_name: &str,
        color: AgentColor,
    ) -> io::Result<CreatePaneResult> {
        let current_pane = self.get_current_pane_id().ok_or_else(|| {
            io::Error::other("tmux: could not resolve current pane id")
        })?;
        let window_target = self.get_current_window_target().ok_or_else(|| {
            io::Error::other("tmux: could not resolve current window target")
        })?;
        let pane_count = self
            .get_current_window_pane_count(Some(&window_target), false)
            .ok_or_else(|| io::Error::other("tmux: could not count window panes"))?;
        let is_first = pane_count == 1;

        let split_out = if is_first {
            // split-window -t <currentPane> -h -l 70% -P -F #{pane_id}
            self.run_in_user_session(&[
                "split-window",
                "-t",
                &current_pane,
                "-h",
                "-l",
                "70%",
                "-P",
                "-F",
                "#{pane_id}",
            ])
        } else {
            // list-panes -t <window> -F #{pane_id}; drop leader (index 0).
            let list = self.run_in_user_session(&[
                "list-panes",
                "-t",
                &window_target,
                "-F",
                "#{pane_id}",
            ]);
            let panes = split_pane_ids(&list.stdout);
            let teammate_panes: Vec<String> = panes.into_iter().skip(1).collect();
            let (vertical, target_pane) = pick_split(&teammate_panes);
            let dir = if vertical { "-v" } else { "-h" };
            // split-window -t <targetPane> (-v|-h) -P -F #{pane_id}
            self.run_in_user_session(&[
                "split-window",
                "-t",
                &target_pane,
                dir,
                "-P",
                "-F",
                "#{pane_id}",
            ])
        };

        if split_out.code != 0 {
            return Err(tmux_err(&split_out, "split-window"));
        }
        let pane_id = non_empty_trimmed(&split_out.stdout).ok_or_else(|| {
            io::Error::other("tmux: split-window returned no pane id")
        })?;

        self.set_pane_border_color(&pane_id, color, false)?;
        self.set_pane_title(&pane_id, teammate_name, color, false)?;
        self.rebalance_panes_with_leader(&window_target);
        wait_for_pane_shell_ready();

        Ok(CreatePaneResult {
            pane_id,
            is_first_teammate: is_first,
            used_external_session: false,
        })
    }

    /// `createTeammatePaneExternal` — operate in the detached `codex-swarm`
    /// session via `run_in_swarm`. First teammate reuses the session's initial
    /// pane; later teammates split within `swarm-view`.
    fn create_teammate_pane_external(
        &self,
        teammate_name: &str,
        color: AgentColor,
    ) -> io::Result<CreatePaneResult> {
        let initial_pane = self.ensure_swarm_session()?;
        let window_target = SWARM_VIEW_WINDOW_NAME.to_string();

        let pane_count = self
            .get_current_window_pane_count(Some(&window_target), true)
            .unwrap_or(1);
        let is_first = pane_count == 1;

        let pane_id = if is_first {
            // Reuse the session's initial pane.
            initial_pane
        } else {
            let list = self.run_in_swarm(&[
                "list-panes",
                "-t",
                &window_target,
                "-F",
                "#{pane_id}",
            ]);
            let teammate_panes = split_pane_ids(&list.stdout);
            let (vertical, target_pane) = pick_split(&teammate_panes);
            let dir = if vertical { "-v" } else { "-h" };
            let split_out = self.run_in_swarm(&[
                "split-window",
                "-t",
                &target_pane,
                dir,
                "-P",
                "-F",
                "#{pane_id}",
            ]);
            if split_out.code != 0 {
                return Err(tmux_err(&split_out, "split-window"));
            }
            non_empty_trimmed(&split_out.stdout).ok_or_else(|| {
                io::Error::other("tmux: split-window returned no pane id")
            })?
        };

        self.set_pane_border_color(&pane_id, color, true)?;
        self.set_pane_title(&pane_id, teammate_name, color, true)?;
        wait_for_pane_shell_ready();

        Ok(CreatePaneResult {
            pane_id,
            is_first_teammate: is_first,
            used_external_session: true,
        })
    }

    /// `createExternalSwarmSession` — ensure a detached `codex-swarm` session
    /// with a `swarm-view` window exists, returning its first pane id.
    ///
    /// `new-session -d -s codex-swarm -n swarm-view -P -F #{pane_id}` errors when
    /// the session already exists; that is treated as "already created" and the
    /// existing pane is queried instead. The port does NOT auto-attach — the
    /// user views the session through the TUI (P5).
    fn ensure_swarm_session(&self) -> io::Result<String> {
        let create = self.run_in_swarm(&[
            "new-session",
            "-d",
            "-s",
            SWARM_SESSION_NAME,
            "-n",
            SWARM_VIEW_WINDOW_NAME,
            "-P",
            "-F",
            "#{pane_id}",
        ]);
        if create.code == 0 {
            if let Some(pane) = non_empty_trimmed(&create.stdout) {
                return Ok(pane);
            }
        }
        // Already exists (or no pane id echoed): query the existing first pane.
        let window_target = SWARM_VIEW_WINDOW_NAME;
        let list = self.run_in_swarm(&[
            "list-panes",
            "-t",
            window_target,
            "-F",
            "#{pane_id}",
        ]);
        split_pane_ids(&list.stdout)
            .into_iter()
            .next()
            .ok_or_else(|| io::Error::other("tmux: codex-swarm session has no panes"))
    }

    // ---- launch helper (PaneBackendExecutor.spawnTeammateInPane) ----

    /// Build the teammate launch line and `send_command_to_pane` it.
    ///
    /// Final line (see [`build_launch_line`]):
    /// `cd <q(cwd)> && env <K=q(V) …> <q(bin)> <teammate_flags joined by space>`.
    pub fn launch_teammate(
        &self,
        pane_id: &str,
        use_external: bool,
        cwd: &Path,
        env: &[(String, String)],
        bin: &Path,
        teammate_flags: &[String],
    ) -> io::Result<()> {
        let line = build_launch_line(cwd, env, bin, teammate_flags)?;
        self.send_command_to_pane(pane_id, &line, use_external)
    }
}

impl Default for TmuxBackend {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Free helpers (extracted for unit testing without a live tmux server).
// ---------------------------------------------------------------------------

/// `waitForPaneShellReady()` — `sleep(PANE_SHELL_INIT_DELAY_MS)`.
fn wait_for_pane_shell_ready() {
    std::thread::sleep(Duration::from_millis(PANE_SHELL_INIT_DELAY_MS));
}

/// Trim a tmux stdout line; `None` when empty after trimming.
fn non_empty_trimmed(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Split tmux `list-panes` output into pane ids, dropping empty lines.
fn split_pane_ids(stdout: &str) -> Vec<String> {
    stdout
        .split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// Build an `io::Error` from a failed `TmuxOutput`.
fn tmux_err(out: &TmuxOutput, op: &str) -> io::Error {
    let stderr = out.stderr.trim();
    io::Error::other(format!("tmux {op} failed (code {}): {stderr}", out.code))
}

/// `Ok(())` on a zero exit code, otherwise an `io::Error` carrying stderr.
fn ok_or_tmux_err(out: &TmuxOutput, op: &str) -> io::Result<()> {
    if out.code == 0 {
        Ok(())
    } else {
        Err(tmux_err(out, op))
    }
}

/// Pane-selection math for the additional-teammate split (Claude
/// `createTeammatePaneWithLeader`).
///
/// Given the existing *teammate* panes (leader already dropped):
/// `vertical = teammateCount % 2 == 1`, `idx = (teammateCount - 1) / 2`,
/// `target = teammatePanes[idx]` (falls back to the last pane on out-of-range).
///
/// Parity table: 0 panes -> `(false, "")` (caller takes the leader/first path);
/// 1 -> `-v` target=[0]; 2 -> `-h` target=[0]; 3 -> `-v` target=[1];
/// 4 -> `-h` target=[1].
fn pick_split(teammate_panes: &[String]) -> (bool, String) {
    let count = teammate_panes.len();
    if count == 0 {
        return (false, String::new());
    }
    let vertical = count % 2 == 1;
    let idx = (count - 1) / 2;
    let target = teammate_panes
        .get(idx)
        .or_else(|| teammate_panes.last())
        .cloned()
        .unwrap_or_default();
    (vertical, target)
}

/// Build the single shell line Claude's `spawnTeammateInPane` sends to the pane:
/// `cd <q(cwd)> && env <K=q(V) …> <q(bin)> <q(flag) …>`.
///
/// `cwd`, `bin`, each env VALUE, AND each teammate flag token are quoted with
/// [`shlex::try_quote`] — flag VALUES can contain spaces (e.g. a team name like
/// `local tmux teams smoke`), so they must be quoted or the pane shell would
/// split them into stray positional arguments. A quoting failure (interior NUL)
/// is surfaced as an `io::Error`.
pub(crate) fn build_launch_line(
    cwd: &Path,
    env: &[(String, String)],
    bin: &Path,
    teammate_flags: &[String],
) -> io::Result<String> {
    let cwd_q = quote_path(cwd)?;
    let bin_q = quote_path(bin)?;

    let mut line = format!("cd {cwd_q} && env");
    for (key, value) in env {
        let value_q = shlex::try_quote(value).map_err(|e| {
            io::Error::other(format!("failed to quote env value for {key}: {e:?}"))
        })?;
        line.push(' ');
        line.push_str(key);
        line.push('=');
        line.push_str(&value_q);
    }
    line.push(' ');
    line.push_str(&bin_q);
    for flag in teammate_flags {
        let flag_q = shlex::try_quote(flag).map_err(|e| {
            io::Error::other(format!("failed to quote teammate flag {flag:?}: {e:?}"))
        })?;
        line.push(' ');
        line.push_str(&flag_q);
    }
    Ok(line)
}

/// shlex-quote a path's lossy UTF-8 form.
fn quote_path(path: &Path) -> io::Result<String> {
    let s = path.to_string_lossy();
    shlex::try_quote(&s)
        .map(|cow| cow.into_owned())
        .map_err(|e| io::Error::other(format!("failed to quote path {s:?}: {e:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn pick_split_parity_table() {
        // 0 teammate panes -> caller uses the first/leader path.
        assert_eq!(pick_split(&[]), (false, String::new()));

        let p = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect::<Vec<_>>();

        // 1 -> -v, target index 0.
        assert_eq!(pick_split(&p(&["%1"])), (true, "%1".to_string()));
        // 2 -> -h, target index 0.
        assert_eq!(pick_split(&p(&["%1", "%2"])), (false, "%1".to_string()));
        // 3 -> -v, target index 1.
        assert_eq!(
            pick_split(&p(&["%1", "%2", "%3"])),
            (true, "%2".to_string())
        );
        // 4 -> -h, target index 1.
        assert_eq!(
            pick_split(&p(&["%1", "%2", "%3", "%4"])),
            (false, "%2".to_string())
        );
    }

    #[test]
    fn agent_color_tmux_table() {
        assert_eq!(AgentColor::Red.tmux_color(), "red");
        assert_eq!(AgentColor::Blue.tmux_color(), "blue");
        assert_eq!(AgentColor::Green.tmux_color(), "green");
        assert_eq!(AgentColor::Yellow.tmux_color(), "yellow");
        assert_eq!(AgentColor::Purple.tmux_color(), "magenta");
        assert_eq!(AgentColor::Orange.tmux_color(), "colour208");
        assert_eq!(AgentColor::Pink.tmux_color(), "colour205");
        assert_eq!(AgentColor::Cyan.tmux_color(), "cyan");
    }

    #[test]
    fn agent_color_name_round_trip() {
        for color in [
            AgentColor::Red,
            AgentColor::Blue,
            AgentColor::Green,
            AgentColor::Yellow,
            AgentColor::Purple,
            AgentColor::Orange,
            AgentColor::Pink,
            AgentColor::Cyan,
        ] {
            assert_eq!(AgentColor::from_name(color.as_name()), Some(color));
        }
        assert_eq!(AgentColor::from_name("chartreuse"), None);
    }

    #[test]
    fn build_launch_line_basic() {
        let line = build_launch_line(
            &PathBuf::from("/home/user/project"),
            &[("CODEX_API_KEY".to_string(), "abc123".to_string())],
            &PathBuf::from("/usr/local/bin/codex"),
            &[
                "--agent-id".to_string(),
                "alice@rocket".to_string(),
                "--agent-color".to_string(),
                "green".to_string(),
            ],
        )
        .unwrap();
        assert_eq!(
            line,
            "cd /home/user/project && env CODEX_API_KEY=abc123 \
             /usr/local/bin/codex --agent-id alice@rocket --agent-color green"
        );
    }

    #[test]
    fn build_launch_line_quotes_spaces() {
        let line = build_launch_line(
            &PathBuf::from("/home/user/my project"),
            &[("TOKEN".to_string(), "a b c".to_string())],
            &PathBuf::from("/opt/code x/codex"),
            &["--team-name".to_string(), "rocket".to_string()],
        )
        .unwrap();
        assert!(
            line.contains("cd '/home/user/my project'"),
            "cwd not quoted: {line}"
        );
        assert!(line.contains("TOKEN='a b c'"), "env not quoted: {line}");
        assert!(
            line.contains("'/opt/code x/codex'"),
            "bin not quoted: {line}"
        );
        assert!(line.ends_with("--team-name rocket"), "flags wrong: {line}");
    }

    #[test]
    fn split_pane_ids_drops_empty_lines() {
        assert_eq!(split_pane_ids("%1\n%2\n\n%3\n"), vec!["%1", "%2", "%3"]);
        assert!(split_pane_ids("").is_empty());
        assert!(split_pane_ids("\n  \n").is_empty());
    }

    #[test]
    fn from_parts_builds_socket_name() {
        let backend = TmuxBackend::from_parts(true, Some("%0".to_string()), 4242);
        assert_eq!(backend.swarm_socket_name(), "codex-swarm-4242");
        assert!(backend.is_inside_tmux());
        assert_eq!(backend.get_leader_pane_id(), Some("%0".to_string()));
    }
}

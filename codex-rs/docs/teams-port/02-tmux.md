# Teams port — Phase 3a: TMUX backend (`core/src/team_backends/tmux.rs`)

Faithful Rust port of Claude Code's `restored-src/src/utils/swarm/backends/TmuxBackend.ts`
(+ `swarm/constants.ts`, `swarm/backends/detection.ts`, `swarm/backends/PaneBackendExecutor.ts`).
Target the codex repo at `/Users/snakesammy/Desktop/project/codex-teams/codex-rs`. **New file only**;
do not touch existing sources. Everything below uses `std::process::Command` (sync) like codex's own
`terminal-detection` module already does (`tmux_display_message`), which keeps this backend free of any
async runtime requirement and matches Claude's per-call `execFile` shape.

---

## 0. Claude source map (truth source: ChinaSiro/claude-code-sourcemap)

| Claude symbol | File | Notes |
|---|---|---|
| `class TmuxBackend implements PaneBackend` | `swarm/backends/TmuxBackend.ts` | the port target |
| `TMUX_COMMAND = 'tmux'` | `swarm/constants.ts` | binary name |
| `SWARM_SESSION_NAME = 'claude-swarm'` | `swarm/constants.ts` | → codex `"codex-swarm"` |
| `SWARM_VIEW_WINDOW_NAME = 'swarm-view'` | `swarm/constants.ts` | window name (keep) |
| `PANE_SHELL_INIT_DELAY_MS = 200` | `swarm/constants.ts` | sleep after split |
| `ORIGINAL_TMUX_PANE` (from `TMUX_PANE` at module load) | `detection.ts` | leader pane id |
| `ORIGINAL_USER_TMUX` (from `TMUX` at module load) | `detection.ts` | inside-tmux flag |
| `isInsideTmux()` / `isInsideTmuxFromDetection()` | `detection.ts` / `TmuxBackend.ts` | `!!ORIGINAL_USER_TMUX` |
| `getLeaderPaneId()` | `detection.ts` | returns `ORIGINAL_TMUX_PANE` |
| `getSwarmSocketName()` | `TmuxBackend.ts` | `` `claude-swarm-${process.pid}` `` → `format!("codex-swarm-{pid}")` |
| `runTmuxInUserSession(args)` | `TmuxBackend.ts` | `execFileNoThrow('tmux', args)` |
| `runTmuxInSwarm(args)` | `TmuxBackend.ts` | prepends `['-L', getSwarmSocketName()]` then args |
| `execFileNoThrow(cmd,args)` | `utils/execFileNoThrow.ts` | returns `{stdout,stderr,code}`, never throws |
| `getTmuxColorName(color)` | `TmuxBackend.ts` | agent color → tmux color name |
| `waitForPaneShellReady()` | `TmuxBackend.ts` | `sleep(200)` |
| `spawnTeammateInPane` / cmd build | `swarm/backends/PaneBackendExecutor.ts`, `spawnMultiAgent.ts` | `cd … && env … <bin> <flags>` |

### Naming change for the port (intentional, only deviation)
- `'claude-swarm'` → **`"codex-swarm"`** (per `TEAMS_CLAUDE_PORT_SPEC.md` P3). Socket name
  `getSwarmSocketName` likewise becomes `"codex-swarm-{pid}"`.
- `'swarm-view'` window name: keep verbatim as `"swarm-view"` (no user-facing brand string).

---

## 1. Exact tmux argument vectors (verbatim from TmuxBackend.ts)

These are the strings the Rust port MUST emit. `<paneId>` is a tmux pane id like `%3`;
`<windowTarget>` is `session:window_index` (e.g. `main:0`).

```
# createTeammatePaneWithLeader — FIRST teammate (paneCount == 1):
split-window -t <currentPaneId> -h -l 70% -P -F #{pane_id}

# createTeammatePaneWithLeader — list existing teammate panes:
list-panes -t <windowTarget> -F #{pane_id}
#   panes = stdout split '\n' (drop empty); teammatePanes = panes[1..]
#   teammateCount = teammatePanes.len()
#   splitVertically = teammateCount % 2 == 1
#   targetPaneIndex = floor((teammateCount - 1) / 2)
#   targetPane = teammatePanes[targetPaneIndex] (fallback: last)

# createTeammatePaneWithLeader — ADDITIONAL teammate:
split-window -t <targetPane> (-v if splitVertically else -h) -P -F #{pane_id}

# getCurrentPaneId fallback (only if TMUX_PANE missing):
display-message -p #{pane_id}

# getCurrentWindowTarget (cached). With leader pane known:
display-message -t <leaderPane> -p #{session_name}:#{window_index}
#   without leader pane:
display-message -p #{session_name}:#{window_index}

# getCurrentWindowPaneCount:
list-panes -t <windowTarget> -F #{pane_id}     # then count non-empty lines

# setPaneBorderColor (tmux 3.2+ pane options):
select-pane -t <paneId> -P bg=default,fg=<tmuxColor>
set-option -p -t <paneId> pane-border-style fg=<tmuxColor>
set-option -p -t <paneId> pane-active-border-style fg=<tmuxColor>

# setPaneTitle:
select-pane -t <paneId> -T <name>
set-option -p -t <paneId> pane-border-format #[fg=<tmuxColor>,bold] #{pane_title} #[default]

# sendCommandToPane (press Enter == literal arg "Enter"):
send-keys -t <paneId> <command> Enter

# rebalancePanesWithLeader (only when panes.len() > 2):
list-panes -t <windowTarget> -F #{pane_id}
select-layout -t <windowTarget> main-vertical
resize-pane -t <panes[0]> -x 30%

# createExternalSwarmSession (NOT inside tmux) — runTmuxInSwarm, i.e. prefixed with -L <socket>:
new-session -d -s codex-swarm -n swarm-view -P -F #{pane_id}
#   (if the session already exists, this errors; treat as "already created" and query its pane)
```

`pane-border-format` value string is, EXACTLY (note leading/trailing spaces around `#{pane_title}`):
`#[fg=<tmuxColor>,bold] #{pane_title} #[default]`.

### `getTmuxColorName` mapping (verbatim)
```
red    -> "red"
blue   -> "blue"
green  -> "green"
yellow -> "yellow"
purple -> "magenta"
orange -> "colour208"
pink   -> "colour205"
cyan   -> "cyan"
```

### Teammate launch command string (PaneBackendExecutor.ts `spawnTeammateInPane`)
Claude builds, then `sendCommandToPane`s, this single shell line:
```
cd <quote(workingDir)> && env <envStr> <quote(binaryPath)> <teammateArgs><flagsStr>
```
- `teammateArgs` flags (per `TEAMS_CLAUDE_PORT_SPEC.md` "Identity"):
  `--agent-id "{name}@{team}" --agent-name <display> --team-name <team> --agent-color <color>
   --parent-session-id <lead session uuid> [--agent-type <t>] [--plan-mode-required]`
- `flagsStr` = inherited CLI flags incl. `--model <model>`.
- `envStr` = space-joined `KEY=quote(VALUE)` propagated env (auth/provider — codex
  `buildInheritedEnvVars` analog; out of scope for this file, supplied by caller as a `Vec<(String,String)>`).

---

## 2. Target codex module + wiring

- **New file:** `core/src/team_backends/tmux.rs`
- **New file:** `core/src/team_backends/mod.rs` (just `pub mod tmux;` plus the shared
  `BackendType`, `CreatePaneResult`, `AgentColor` enums below — keep them here so a later
  `iterm.rs` (P6) reuses them).
- **Register in `core/src/lib.rs`:** add `mod team_backends;` adjacent to the existing
  `mod team;` / `mod team_store;` (lines 100–101). Place it alphabetically near `mod team`.
  *(Spec note — actual edit happens in the implementation phase, not here.)*
- Module path from elsewhere in `core`: `crate::team_backends::tmux::TmuxBackend`.

### Dependencies — already present, no Cargo.toml change needed
- `shlex = { workspace = true }` (core/Cargo.toml:101) → use `shlex::try_quote` for `quote([...])`.
- `codex-terminal-detection = { workspace = true }` (core/Cargo.toml:64). **Reuse for tmux
  detection** instead of re-reading `$TMUX`. See §4.

---

## 3. Rust types and function signatures to add

```rust
//! core/src/team_backends/mod.rs
pub mod tmux;

/// Mirrors Claude `BackendType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendType { Tmux, Iterm, InProcess }

/// Mirrors Claude `AgentColorName`. Stored on disk as the lowercase string
/// (TeamFileMember.color is `Option<String>`); map via `AgentColor::from_str`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentColor { Red, Blue, Green, Yellow, Purple, Orange, Pink, Cyan }

impl AgentColor {
    /// `getTmuxColorName` — see §1 table.
    pub fn tmux_color(self) -> &'static str;
    /// Parse the on-disk `TeamFileMember.color` string; unknown -> None.
    pub fn from_name(s: &str) -> Option<Self>;
    pub fn as_name(self) -> &'static str; // "red" .. "cyan"
}

/// Mirrors Claude `CreatePaneResult`.
#[derive(Clone, Debug)]
pub struct CreatePaneResult {
    pub pane_id: String,       // tmux "%N"
    pub is_first_teammate: bool,
    pub used_external_session: bool, // true when launched in the detached codex-swarm session
}
```

```rust
//! core/src/team_backends/tmux.rs
use std::process::Command;
use std::time::Duration;

const TMUX_COMMAND: &str = "tmux";
const SWARM_SESSION_NAME: &str = "codex-swarm";   // Claude: "claude-swarm"
const SWARM_VIEW_WINDOW_NAME: &str = "swarm-view";
const PANE_SHELL_INIT_DELAY_MS: u64 = 200;

/// `execFileNoThrow` analog: run tmux, never error on non-zero exit.
struct TmuxOutput { stdout: String, stderr: String, code: i32 }
fn exec_file_no_throw(cmd: &str, args: &[&str]) -> TmuxOutput; // captures stdout/stderr, code = status.code().unwrap_or(-1)

#[derive(Debug)]
pub struct TmuxBackend {
    /// `ORIGINAL_TMUX_PANE` (captured `$TMUX_PANE`); None when not inside tmux.
    leader_pane_id: Option<String>,
    /// `ORIGINAL_USER_TMUX` (captured `$TMUX` non-empty).
    inside_tmux: bool,
    /// `getSwarmSocketName()` => "codex-swarm-{pid}".
    swarm_socket: String,
    /// `cachedLeaderWindowTarget`.
    cached_window_target: std::cell::RefCell<Option<String>>,
}

impl TmuxBackend {
    /// Captures `$TMUX`, `$TMUX_PANE`, and pid ONCE (mirrors module-load capture in detection.ts).
    /// Prefer feeding values in from the lead session's startup snapshot rather than reading
    /// env late, to match Claude's "captured at module load" semantics.
    pub fn new() -> Self;
    /// Test/construction seam.
    pub fn from_parts(inside_tmux: bool, leader_pane_id: Option<String>, pid: u32) -> Self;

    // ---- detection ----
    pub fn is_inside_tmux(&self) -> bool;                 // -> self.inside_tmux
    fn get_leader_pane_id(&self) -> Option<String>;       // -> self.leader_pane_id.clone()
    fn swarm_socket_name(&self) -> &str;                  // "codex-swarm-{pid}"

    // ---- tmux runners ----
    fn run_in_user_session(&self, args: &[&str]) -> TmuxOutput;
    fn run_in_swarm(&self, args: &[&str]) -> TmuxOutput;  // prepends ["-L", self.swarm_socket]

    // ---- queries ----
    fn get_current_pane_id(&self) -> Option<String>;
    fn get_current_window_target(&self) -> Option<String>;
    fn get_current_window_pane_count(&self, window_target: Option<&str>, use_swarm: bool) -> Option<usize>;

    // ---- pane styling / io (public PaneBackend surface) ----
    pub fn set_pane_border_color(&self, pane_id: &str, color: AgentColor, use_external: bool) -> std::io::Result<()>;
    pub fn set_pane_title(&self, pane_id: &str, name: &str, color: AgentColor, use_external: bool) -> std::io::Result<()>;
    pub fn send_command_to_pane(&self, pane_id: &str, command: &str, use_external: bool) -> std::io::Result<()>;

    // ---- layout ----
    fn rebalance_panes_with_leader(&self, window_target: &str);

    // ---- creation (the entry point) ----
    /// `createTeammatePaneInSwarmView`: dispatch on `is_inside_tmux()`.
    pub fn create_teammate_pane(&self, teammate_name: &str, color: AgentColor) -> std::io::Result<CreatePaneResult>;
    fn create_teammate_pane_with_leader(&self, teammate_name: &str, color: AgentColor) -> std::io::Result<CreatePaneResult>;
    fn create_teammate_pane_external(&self, teammate_name: &str, color: AgentColor) -> std::io::Result<CreatePaneResult>;
    /// `createExternalSwarmSession`: ensure detached "codex-swarm"/"swarm-view"; return its first pane id.
    fn ensure_swarm_session(&self) -> std::io::Result<String>;

    // ---- launch helper (PaneBackendExecutor.spawnTeammateInPane) ----
    /// Builds `cd <q(cwd)> && env <env> <q(bin)> <flags>` and sends it via send_command_to_pane.
    pub fn launch_teammate(
        &self,
        pane_id: &str,
        use_external: bool,
        cwd: &std::path::Path,
        env: &[(String, String)],
        bin: &std::path::Path,
        teammate_flags: &[String],   // already-tokenized: --agent-id, X@T, --agent-name, ...
    ) -> std::io::Result<()>;
}

fn wait_for_pane_shell_ready() { std::thread::sleep(Duration::from_millis(PANE_SHELL_INIT_DELAY_MS)); }
```

### Behavioral notes the port MUST preserve
- `create_teammate_pane`: if `is_inside_tmux()` → `_with_leader`; else `_external`.
- `_with_leader`: get current pane id + window target → error if either missing; pane count
  → `is_first = count == 1`. First: `split-window -t <pane> -h -l 70% -P -F #{pane_id}`.
  Else: list panes, drop index 0 (leader), pick target by parity formula, split `-v`/`-h`.
  On non-zero `code` from the split, return `Err` with stderr. After split: trim stdout for
  `pane_id`, `set_pane_border_color`, `set_pane_title`, `rebalance_panes_with_leader`,
  `wait_for_pane_shell_ready()`, return `{pane_id, is_first, used_external: false}`.
- `_external`: `ensure_swarm_session()` first; all tmux calls go through `run_in_swarm`
  (i.e. `use_external = true`); on first teammate reuse the session's initial pane,
  otherwise split within `swarm-view`. Color/title set with `use_external = true`.
  Return `used_external: true`.
- `send_command_to_pane`: `send-keys -t <pane> <command> Enter`; non-zero `code` → `Err`.
  The literal final arg is the string `"Enter"` (NOT `C-m`) — Claude uses `'Enter'`.
- `launch_teammate` quoting: use `shlex::try_quote` for `cwd`, `bin`, and each env VALUE;
  join env as `KEY=<quoted value>`; final line:
  `cd <q(cwd)> && env <env joined> <q(bin)> <teammate_flags joined by space>`.

---

## 4. Reuse codex `terminal-detection` for the `$TMUX` check (do NOT re-read env ad hoc)

`core/src/terminal-detection` already detects tmux:
- `codex_terminal_detection::terminal_info()` returns `TerminalInfo { multiplexer: Option<Multiplexer>, .. }`.
- `Multiplexer::Tmux { .. }` is set when **`TMUX` or `TMUX_PANE` is non-empty** (see
  `detect_multiplexer`, lib.rs:390-407). That matches Claude's `!!ORIGINAL_USER_TMUX`.

Caveat (Claude parity): Claude's `getLeaderPaneId` needs the raw **`TMUX_PANE`** value, which
`terminal_info()` does NOT expose. So:
- For the inside/outside boolean, prefer
  `matches!(codex_terminal_detection::terminal_info().multiplexer, Some(Multiplexer::Tmux { .. }))`.
- For the leader pane id, capture `std::env::var("TMUX_PANE").ok().filter(|s| !s.is_empty())`
  directly in `TmuxBackend::new()` (there is no codex helper for it). Document that this read
  should happen at lead-session startup to mirror Claude's module-load capture.

`terminal_info()` is memoized via a process-wide `OnceLock`, so calling it from `TmuxBackend::new`
is cheap and consistent with how codex already shells out to `tmux display-message`.

---

## 5. How it wires to `team_store` / `team.rs` / the team tools (context only — implemented in later steps)

This file is a **pure backend**; it does not own team state. The integration points (P3 of the
plan, separate edits) are:

1. **`team_store` (already exists, `core/src/team_store.rs`):** after `create_teammate_pane`
   succeeds, the spawn path calls `team_store::update_config(teams_root, team, |cfg| { ... })`
   to set on the matching `TeamFileMember`: `tmux_pane_id = result.pane_id`,
   `backend_type = Some("tmux".into())` (`BackendType::Tmux`), `is_active = Some(true)`,
   `color = Some(color.as_name().into())`. `teams_root` = `Config.codex_home`
   (`core/src/config/mod.rs:840 pub codex_home: AbsolutePathBuf`).
2. **`team.rs` registry / `tools/handlers/team.rs::team_spawn_member`:** the existing in-process
   `registry.spawn_member(...)` path (handler at team.rs:384-437) is the call site that P3 swaps to:
   write member into `config.json` → `create_teammate_pane` → `launch_teammate(... codex bin + flags ...)`
   → persist pane id. **Keep `TeamSpawnMemberResult { member: TeamMember }` shape unchanged** so the
   model output and TUI observer are unaffected (team.rs:256-258).
3. **`team_send` (team.rs:439-502):** unchanged by this file; cross-process delivery is
   `team_store::write_to_mailbox` (P4), not tmux.
4. **TUI (P5, `tui/src/app/teammate_panes.rs`):** consumes `tmux_pane_id` / `backend_type` from
   `config.json` for view/hide; hide/show panes are `kill-pane` / re-split (Claude
   `hidePane`/`showPane`) — out of scope for this file but should live as
   `TmuxBackend::hide_pane`/`show_pane` later (signatures: `(&self, pane_id, use_external) -> io::Result<()>`).

---

## 6. Unit-test plan (pure, no live tmux)

Tmux is not available in CI sandboxes, so split logic must be unit-testable without a server:
- Extract the **pane-selection math** into a free fn
  `fn pick_split(teammate_panes: &[String]) -> (bool /*vertical*/, String /*target*/)` and test the
  parity table: 0 panes → first uses leader path; 1 → `-h` target=[0]; 2 → `-v` target=[0];
  3 → `-h` target=[1]; 4 → `-v` target=[1] (i.e. `vertical = count%2==1`, `idx=(count-1)/2`).
- Test `AgentColor::tmux_color` table (all 8) and `from_name`/`as_name` round-trip.
- Test `launch_teammate` **command-string builder** (extract `fn build_launch_line(cwd, env, bin, flags) -> String`)
  for correct `cd … && env K=v … <bin> <flags>` and shlex quoting of paths with spaces.
- Live tmux paths (`run_in_*`, `create_*`) guard behind `#[cfg(test)]` only where a fake
  `exec_file_no_throw` can be injected; otherwise leave untested (matches codex's own untested
  `tmux_display_message`).

---

## 7. Blocking unknowns / decisions for the implementer

- **`launch_teammate` arg source.** The env list, codex binary path, and inherited flags
  (`buildInheritedEnvVars` / `--model`) are produced by the spawn site (P3), not this file. This
  spec defines the *shape* (`&[(String,String)]`, `&Path`, `&[String]`) but the population logic
  lives in the `team_spawn_member` rewrite. **Low risk** — interface is explicit.
- **Detached-session attach.** Claude's `createExternalSwarmSession` creates the detached
  `codex-swarm` session but the de-minified source does not clearly show an automatic
  `tmux attach`/`switch-client`; the user views it via the TUI "view teammate output" action (P5).
  The port should NOT auto-attach. **Moderate confidence** — if a future trace shows an attach
  call, add it in P5, not here.
- **`exec_file_no_throw` injection seam.** To unit-test creation flows you need to inject a fake
  runner; the signatures above keep `exec_file_no_throw` as a free fn. If full creation-flow tests
  are desired, refactor `TmuxBackend` to hold a `Box<dyn Fn(&str,&[&str]) -> TmuxOutput>`. Left as
  an implementer choice; default to the free-fn + extracted-pure-logic approach in §6 (no live tmux
  in CI). **Low risk.**

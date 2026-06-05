//! tmux / iTerm2 teammate-pane backends (ports Claude Code's swarm backends).
//!
//! Each teammate is an independent `codex` process launched into its own multiplexer
//! pane; these backends own the multiplexer-specific pane creation and command
//! delivery. Unwired into the spawn path until the integration phase.
#![allow(dead_code)]

pub mod iterm;
pub mod spawn;
pub mod tmux;

//! Port of Claude's agentColorManager.ts + teammateLayoutManager.ts color logic.
//!
//! Provides the `AGENT_COLOR_TO_THEME_COLOR` → ratatui mapping
//! (`agent_color_to_tui`) and the `PERMISSION_MODE_CONFIG` symbol/color table
//! (`mode_symbol_and_color`) used by the Teams dialog.

#[cfg(test)]
use std::collections::HashMap;

use ratatui::style::Color;

/// Round-robin palette — order matches Claude `AGENT_COLORS`.
///
/// The order is load-bearing: it is the index used by `TeammateColors::assign`
/// to hand out colors in first-seen order.
#[cfg(test)]
const AGENT_COLORS: [&str; 8] = [
    "red", "blue", "green", "yellow", "purple", "orange", "pink", "cyan",
];

/// Deterministic per-team color assignment. Mirrors `assignTeammateColor`:
/// stable per `teammate_id`, round-robin by first-seen order.
#[cfg(test)]
#[derive(Default)]
struct TeammateColors {
    assignments: HashMap<String, &'static str>,
    next_index: usize,
}

#[cfg(test)]
impl TeammateColors {
    /// Assign (or return the existing) color for `teammate_id`.
    ///
    /// The first time an id is seen it is given `AGENT_COLORS[next_index %
    /// len]` and `next_index` is bumped; subsequent calls return the same color.
    pub(crate) fn assign(&mut self, teammate_id: &str) -> &'static str {
        if let Some(existing) = self.assignments.get(teammate_id) {
            return existing;
        }
        let color = AGENT_COLORS[self.next_index % AGENT_COLORS.len()];
        self.assignments.insert(teammate_id.to_string(), color);
        self.next_index += 1;
        color
    }

    /// Return the previously assigned color for `teammate_id`, if any.
    ///
    /// Part of the ported `getTeammateColor` surface; currently exercised by
    /// tests and kept for callers that need a read-only lookup (e.g. echoing the
    /// persisted color without mutating the round-robin index).
    #[allow(dead_code)]
    pub(crate) fn get(&self, teammate_id: &str) -> Option<&'static str> {
        self.assignments.get(teammate_id).copied()
    }

    /// Clear all assignments and reset the round-robin index. Mirrors
    /// `clearTeammateColors`.
    pub(crate) fn clear(&mut self) {
        self.assignments.clear();
        self.next_index = 0;
    }
}

/// `AGENT_COLOR_TO_THEME_COLOR` → ratatui. Codex TUI keeps this on ANSI colors so
/// it renders consistently across terminal themes.
pub(crate) fn agent_color_to_tui(name: &str) -> Color {
    match name {
        "red" => Color::Red,
        "blue" => Color::Blue,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "purple" => Color::Magenta,
        "orange" => Color::Yellow,
        "pink" => Color::Magenta,
        "cyan" => Color::Cyan,
        _ => Color::White,
    }
}

/// Port of `PERMISSION_MODE_CONFIG`: `(symbol, ratatui color)`.
///
/// `default` (and any unknown mode) renders no pill: `("", Color::Reset)`.
pub(crate) fn mode_symbol_and_color(mode: &str) -> (&'static str, Color) {
    match mode {
        // plan → PAUSE_ICON in planMode color
        "plan" => ("\u{23F8}", Color::Cyan),
        // acceptEdits → ⏵⏵ in autoAccept color
        "acceptEdits" => ("\u{23F5}\u{23F5}", Color::Green),
        // bypassPermissions / dontAsk → ⏵⏵ in error color
        "bypassPermissions" | "dontAsk" => ("\u{23F5}\u{23F5}", Color::Red),
        // default / unknown → no pill
        _ => ("", Color::Reset),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assign_is_round_robin_in_first_seen_order() {
        let mut colors = TeammateColors::default();
        // First eight distinct ids walk the palette in order.
        for (i, expected) in AGENT_COLORS.iter().enumerate() {
            let id = format!("teammate-{i}");
            assert_eq!(colors.assign(&id), *expected);
        }
        // The ninth distinct id wraps around to the start of the palette.
        assert_eq!(colors.assign("teammate-8"), AGENT_COLORS[0]);
    }

    #[test]
    fn assign_is_stable_per_id() {
        let mut colors = TeammateColors::default();
        let first = colors.assign("alice");
        // A different id advances the index...
        let _ = colors.assign("bob");
        // ...but re-asking for the original id returns the same color.
        assert_eq!(colors.assign("alice"), first);
        assert_eq!(colors.assign("alice"), first);
        // get() agrees with assign() and is read-only.
        assert_eq!(colors.get("alice"), Some(first));
        assert_eq!(colors.get("unknown"), None);
    }

    #[test]
    fn clear_resets_assignments_and_index() {
        let mut colors = TeammateColors::default();
        // Burn through a couple of palette slots.
        assert_eq!(colors.assign("a"), AGENT_COLORS[0]);
        assert_eq!(colors.assign("b"), AGENT_COLORS[1]);
        colors.clear();
        // After clear(), assignments are gone and the index restarts at 0.
        assert_eq!(colors.get("a"), None);
        assert_eq!(colors.get("b"), None);
        assert_eq!(colors.assign("c"), AGENT_COLORS[0]);
    }

    #[test]
    fn agent_color_to_tui_known_and_unknown() {
        assert_eq!(agent_color_to_tui("red"), Color::Red);
        assert_eq!(agent_color_to_tui("blue"), Color::Blue);
        assert_eq!(agent_color_to_tui("green"), Color::Green);
        assert_eq!(agent_color_to_tui("yellow"), Color::Yellow);
        assert_eq!(agent_color_to_tui("purple"), Color::Magenta);
        assert_eq!(agent_color_to_tui("orange"), Color::Yellow);
        assert_eq!(agent_color_to_tui("pink"), Color::Magenta);
        assert_eq!(agent_color_to_tui("cyan"), Color::Cyan);
        // Every palette name must map to something other than the White fallback.
        for name in AGENT_COLORS {
            assert_ne!(agent_color_to_tui(name), Color::White);
        }
        // Unknown names fall back to White.
        assert_eq!(agent_color_to_tui("magenta"), Color::White);
        assert_eq!(agent_color_to_tui(""), Color::White);
    }

    #[test]
    fn mode_symbol_and_color_matches_config() {
        // default / unknown → no pill.
        assert_eq!(mode_symbol_and_color("default"), ("", Color::Reset));
        assert_eq!(mode_symbol_and_color("totally-unknown"), ("", Color::Reset));
        assert_eq!(mode_symbol_and_color(""), ("", Color::Reset));
        // plan → PAUSE_ICON in cyan (planMode).
        assert_eq!(mode_symbol_and_color("plan"), ("\u{23F8}", Color::Cyan));
        // acceptEdits → ⏵⏵ in green (autoAccept).
        assert_eq!(
            mode_symbol_and_color("acceptEdits"),
            ("\u{23F5}\u{23F5}", Color::Green)
        );
        // bypassPermissions / dontAsk → ⏵⏵ in red (error).
        assert_eq!(
            mode_symbol_and_color("bypassPermissions"),
            ("\u{23F5}\u{23F5}", Color::Red)
        );
        assert_eq!(
            mode_symbol_and_color("dontAsk"),
            ("\u{23F5}\u{23F5}", Color::Red)
        );
    }
}

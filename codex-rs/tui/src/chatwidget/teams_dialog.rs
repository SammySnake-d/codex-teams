//! Port of Claude's `components/teams/TeamsDialog.tsx` (Phase 6 §B.3).
//!
//! A bottom-pane overlay with two levels mirroring Claude's `dialogLevel`:
//! a teammate list (`TeamDetailView` / `TeammateListItem`) and a per-teammate
//! detail (`TeammateDetailView`). The roster is re-read from the on-disk team
//! store every ~1s (Claude's `useInterval`), and `handle_key` returns the
//! side-effects (`viewTeammateOutput` / `toggleTeammateVisibility`) that the
//! `App` executes because they need tmux/exec.
//!
//! Colors + the permission-mode pill come from `super::team_colors` so the
//! dialog renders identically to the footer pills (Phase 6 §B.1).

use std::path::Path;
use std::time::Instant;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

const PROMPT_COLLAPSED_WIDTH: usize = 80;

/// Which level of the dialog is currently focused (Claude's `dialogLevel`).
pub(crate) enum TeamsDialogLevel {
    TeammateList { team: String },
    TeammateDetail { team: String, member_name: String },
}

/// Row backed by `team_store::TeamFileMember` + the assigned color.
///
/// Mirrors Claude's `TeammateStatus` shape consumed by `TeammateListItem`:
/// `isHidden` is derived from `teamFile.hiddenPaneIds.includes(tmuxPaneId)` and
/// `[idle]` from `isActive === false`.
pub(crate) struct TeammateRow {
    pub name: String,
    pub agent_id: String,
    pub tmux_pane_id: String,
    pub backend_type: Option<String>,
    pub model: Option<String>,
    pub prompt: Option<String>,
    pub mode: Option<String>,
    pub color: Option<String>,
    pub is_active: Option<bool>,
    pub is_hidden: bool,
}

pub(crate) struct TeamsDialog {
    level: TeamsDialogLevel,
    selected_index: usize,
    teammates: Vec<TeammateRow>,
    prompt_expanded: bool,
    last_refresh: Instant,
}

/// Side-effects requested by the dialog, executed by `App` (needs tmux/exec).
#[derive(Debug)]
pub(crate) enum TeamsDialogAction {
    /// `↵` on a detail row — port of `viewTeammateOutput(paneId, backendType)`.
    ViewTeammateOutput {
        pane_id: String,
        backend_type: Option<String>,
    },
    /// `h` — port of `toggleTeammateVisibility`'s on-disk half. `hide` is the
    /// new desired state (the negation of the row's current `is_hidden`).
    ToggleVisibility {
        team: String,
        pane_id: String,
        hide: bool,
    },
    ToggleAllVisibility {
        team: String,
        hide: bool,
    },
    KillTeammate {
        team: String,
        teammate: TeamsDialogTeammateAction,
    },
    ShutdownTeammate {
        team: String,
        name: String,
    },
    PruneIdle {
        team: String,
        teammates: Vec<TeamsDialogTeammateAction>,
    },
    /// `esc` / `q` — close the overlay (or pop back to the list from detail).
    Close,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TeamsDialogTeammateAction {
    pub(crate) pane_id: String,
    pub(crate) backend_type: Option<String>,
    pub(crate) agent_id: String,
    pub(crate) name: String,
}

impl TeamsDialog {
    /// Open the dialog at the teammate list level (Claude opens on `TeamDetailView`).
    pub(crate) fn open(team: String) -> Self {
        Self {
            level: TeamsDialogLevel::TeammateList { team },
            selected_index: 0,
            teammates: Vec::new(),
            prompt_expanded: false,
            last_refresh: Instant::now(),
        }
    }

    /// Re-read the roster from `teams/{team}/config.json` (Claude's `useInterval`,
    /// 1s). Maps `TeamFileMember` -> `TeammateRow`, deriving `is_hidden` from the
    /// team file's `hidden_pane_ids`. A read error or a missing config clears the
    /// roster (the team was stopped or never persisted) rather than panicking.
    pub(crate) fn refresh(&mut self, codex_home: &Path) {
        self.last_refresh = Instant::now();
        let team = self.team_name().to_string();
        let team_file = match crate::legacy_core::team_store::read_config(codex_home, &team) {
            Ok(Some(team_file)) => team_file,
            Ok(None) | Err(_) => {
                self.teammates.clear();
                self.clamp_selection();
                return;
            }
        };

        self.teammates = team_file
            .members
            .iter()
            .filter(|member| {
                member.name != crate::legacy_core::team_store::TEAM_LEAD_NAME
                    && !member.tmux_pane_id.trim().is_empty()
            })
            .map(|member| {
                let is_hidden = team_file
                    .hidden_pane_ids
                    .iter()
                    .any(|pane| pane == &member.tmux_pane_id);
                TeammateRow {
                    name: member.name.clone(),
                    agent_id: member.agent_id.clone(),
                    tmux_pane_id: member.tmux_pane_id.clone(),
                    backend_type: member.backend_type.clone(),
                    model: member.model.clone(),
                    prompt: member.prompt.clone(),
                    mode: member.mode.clone(),
                    color: member.color.clone(),
                    is_active: member.is_active,
                    is_hidden,
                }
            })
            .collect();
        self.clamp_selection();
    }

    /// Render `TeamDetailView` (list) or `TeammateDetailView` (detail).
    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let lines = match &self.level {
            TeamsDialogLevel::TeammateList { team } => self.list_lines(team),
            TeamsDialogLevel::TeammateDetail { team, member_name } => {
                self.detail_lines(team, member_name)
            }
        };
        Paragraph::new(lines).render(area, buf);
    }

    /// Port of `TeamsDialog.tsx` `useInput`: arrows move `selected_index`; `↵`
    /// drills into a teammate detail or focuses the pane; `k` kills; `s`
    /// requests graceful shutdown; `h`/`H` hide/show; `p` prunes idle teammates;
    /// `esc`/`q` closes (or pops detail -> list).
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> Option<TeamsDialogAction> {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return None;
        }
        // Mirror Claude's plain (unmodified) letter keys; ignore chords so a
        // ctrl+c / ctrl+t never moves the selection. Arrows are accepted with any
        // modifier (they carry no meaning here).
        let plain = key.modifiers == KeyModifiers::NONE;
        match key.code {
            KeyCode::Down => {
                self.move_selection_next();
                None
            }
            KeyCode::Up => {
                self.move_selection_prev();
                None
            }
            KeyCode::Char('j') if plain => {
                self.move_selection_next();
                None
            }
            KeyCode::Char('k') if plain => self.kill_action(),
            KeyCode::Char('s') if plain => self.shutdown_action(),
            KeyCode::Char('h') if plain => self.toggle_visibility_action(),
            KeyCode::Char('H') if plain => self.toggle_all_visibility_action(),
            KeyCode::Char('p') if plain => self.prompt_or_prune_action(),
            KeyCode::Enter => self.activate_selected(),
            KeyCode::Left => self.back_from_detail(),
            KeyCode::Esc => self.close_or_pop(),
            KeyCode::Char('q') if plain => self.close_or_pop(),
            _ => None,
        }
    }

    fn team_name(&self) -> &str {
        match &self.level {
            TeamsDialogLevel::TeammateList { team }
            | TeamsDialogLevel::TeammateDetail { team, .. } => team,
        }
    }

    /// Clamp selection when moving down past the end (Claude uses min/max).
    fn move_selection_next(&mut self) {
        if self.teammates.is_empty() {
            self.selected_index = 0;
            return;
        }
        self.selected_index = (self.selected_index + 1).min(self.teammates.len() - 1);
    }

    /// Clamp selection when moving up past the start.
    fn move_selection_prev(&mut self) {
        if self.teammates.is_empty() {
            self.selected_index = 0;
            return;
        }
        self.selected_index = self.selected_index.saturating_sub(1);
    }

    fn clamp_selection(&mut self) {
        if self.teammates.is_empty() {
            self.selected_index = 0;
        } else if self.selected_index >= self.teammates.len() {
            self.selected_index = self.teammates.len() - 1;
        }
    }

    /// `↵` — list drills into detail; detail focuses the teammate's pane.
    fn activate_selected(&mut self) -> Option<TeamsDialogAction> {
        match &self.level {
            TeamsDialogLevel::TeammateList { team } => {
                let row = self.teammates.get(self.selected_index)?;
                self.level = TeamsDialogLevel::TeammateDetail {
                    team: team.clone(),
                    member_name: row.name.clone(),
                };
                self.prompt_expanded = false;
                None
            }
            TeamsDialogLevel::TeammateDetail { .. } => {
                let row = self.selected_detail_row()?;
                Some(TeamsDialogAction::ViewTeammateOutput {
                    pane_id: row.tmux_pane_id.clone(),
                    backend_type: row.backend_type.clone(),
                })
            }
        }
    }

    /// `h` — request the on-disk visibility toggle for the active row. `hide` is
    /// the negation of the current `is_hidden` (Claude's `toggleTeammateVisibility`).
    fn toggle_visibility_action(&self) -> Option<TeamsDialogAction> {
        let row = match &self.level {
            TeamsDialogLevel::TeammateList { .. } => self.teammates.get(self.selected_index)?,
            TeamsDialogLevel::TeammateDetail { .. } => self.selected_detail_row()?,
        };
        Some(TeamsDialogAction::ToggleVisibility {
            team: self.team_name().to_string(),
            pane_id: row.tmux_pane_id.clone(),
            hide: !row.is_hidden,
        })
    }

    fn toggle_all_visibility_action(&self) -> Option<TeamsDialogAction> {
        if !matches!(self.level, TeamsDialogLevel::TeammateList { .. }) || self.teammates.is_empty()
        {
            return None;
        }
        Some(TeamsDialogAction::ToggleAllVisibility {
            team: self.team_name().to_string(),
            hide: self.teammates.iter().any(|row| !row.is_hidden),
        })
    }

    fn kill_action(&mut self) -> Option<TeamsDialogAction> {
        let teammate = self.selected_action_teammate()?;
        let team = self.team_name().to_string();
        if matches!(self.level, TeamsDialogLevel::TeammateDetail { .. }) {
            self.level = TeamsDialogLevel::TeammateList { team: team.clone() };
            self.prompt_expanded = false;
        }
        Some(TeamsDialogAction::KillTeammate { team, teammate })
    }

    fn shutdown_action(&mut self) -> Option<TeamsDialogAction> {
        let name = self.selected_action_teammate()?.name;
        let team = self.team_name().to_string();
        if matches!(self.level, TeamsDialogLevel::TeammateDetail { .. }) {
            self.level = TeamsDialogLevel::TeammateList { team: team.clone() };
            self.prompt_expanded = false;
        }
        Some(TeamsDialogAction::ShutdownTeammate { team, name })
    }

    fn prompt_or_prune_action(&mut self) -> Option<TeamsDialogAction> {
        if matches!(self.level, TeamsDialogLevel::TeammateDetail { .. }) {
            self.prompt_expanded = !self.prompt_expanded;
            return None;
        }
        self.prune_idle_action()
    }

    fn prune_idle_action(&self) -> Option<TeamsDialogAction> {
        if !matches!(self.level, TeamsDialogLevel::TeammateList { .. }) {
            return None;
        }
        let teammates = self
            .teammates
            .iter()
            .filter(|row| row.is_active == Some(false))
            .map(action_from_row)
            .collect::<Vec<_>>();
        (!teammates.is_empty()).then(|| TeamsDialogAction::PruneIdle {
            team: self.team_name().to_string(),
            teammates,
        })
    }

    /// `esc`/`q`: from detail pop back to the list (consumed, no action); from the
    /// list close the whole overlay. Claude's `TeamsDialog` is a two-level dialog,
    /// so esc on the detail level only steps back one level.
    fn close_or_pop(&mut self) -> Option<TeamsDialogAction> {
        match &self.level {
            TeamsDialogLevel::TeammateDetail { team, .. } => {
                self.level = TeamsDialogLevel::TeammateList { team: team.clone() };
                self.prompt_expanded = false;
                None
            }
            TeamsDialogLevel::TeammateList { .. } => Some(TeamsDialogAction::Close),
        }
    }

    fn back_from_detail(&mut self) -> Option<TeamsDialogAction> {
        if let TeamsDialogLevel::TeammateDetail { team, .. } = &self.level {
            self.level = TeamsDialogLevel::TeammateList { team: team.clone() };
            self.prompt_expanded = false;
        }
        None
    }

    /// The row backing the current `TeammateDetail` level, matched by name.
    fn selected_detail_row(&self) -> Option<&TeammateRow> {
        match &self.level {
            TeamsDialogLevel::TeammateDetail { member_name, .. } => {
                self.teammates.iter().find(|row| &row.name == member_name)
            }
            TeamsDialogLevel::TeammateList { .. } => self.teammates.get(self.selected_index),
        }
    }

    fn selected_action_teammate(&self) -> Option<TeamsDialogTeammateAction> {
        match &self.level {
            TeamsDialogLevel::TeammateList { .. } => {
                self.teammates.get(self.selected_index).map(action_from_row)
            }
            TeamsDialogLevel::TeammateDetail { .. } => {
                self.selected_detail_row().map(action_from_row)
            }
        }
    }

    fn list_lines(&self, team: &str) -> Vec<Line<'static>> {
        let mut lines = vec![
            Line::from(format!("Team: {team}").bold_span()),
            Line::from(String::new()),
        ];
        if self.teammates.is_empty() {
            lines.push(Line::from("No teammates yet.".dim_span()));
            return lines;
        }
        for (index, row) in self.teammates.iter().enumerate() {
            lines.push(self.teammate_list_item(row, index == self.selected_index));
        }
        lines.push(Line::from(String::new()));
        lines.push(Line::from(
            "↑/↓ select · ↵ view · k kill · s shutdown · p prune idle · h hide/show · H hide/show all · esc close".dim_span(),
        ));
        lines
    }

    fn detail_lines(&self, team: &str, member_name: &str) -> Vec<Line<'static>> {
        let Some(row) = self.teammates.iter().find(|row| row.name == member_name) else {
            return vec![Line::from(
                format!("@{member_name} is no longer in {team}.").dim_span(),
            )];
        };

        let mut lines = vec![
            Line::from(self.teammate_name_span(row)),
            Line::from(String::new()),
        ];
        lines.push(detail_field("teammate id", &row.agent_id));
        lines.push(detail_field("pane", &row.tmux_pane_id));
        if let Some(backend_type) = row.backend_type.as_deref() {
            lines.push(detail_field("backend", backend_type));
        }
        if let Some(model) = row.model.as_deref() {
            lines.push(detail_field("model", model));
        }
        if let Some(mode) = row.mode.as_deref() {
            lines.push(detail_field("mode", mode));
        }
        if let Some(prompt) = row.prompt.as_deref().filter(|prompt| !prompt.is_empty()) {
            lines.push(Line::from(String::new()));
            lines.push(Line::from("Prompt".bold_span()));
            lines.push(prompt_line(prompt, self.prompt_expanded));
        }
        lines.push(detail_field(
            "status",
            if row.is_active == Some(false) {
                "idle"
            } else {
                "running"
            },
        ));
        lines.push(detail_field(
            "visibility",
            if row.is_hidden { "hidden" } else { "visible" },
        ));
        lines.push(Line::from(String::new()));
        lines.push(Line::from(
            "↵ view output · k kill · s shutdown · p expand prompt · h hide/show · esc back"
                .dim_span(),
        ));
        lines
    }

    /// Port of `TeammateListItem` (§A.4):
    /// `{pointer}{[hidden] }{[idle] }{modeSymbol in modeColor}@{name}{ (model)}`,
    /// dimmed when idle && not selected, selected row in the highlight color.
    fn teammate_list_item(&self, row: &TeammateRow, is_selected: bool) -> Line<'static> {
        let is_idle = row.is_active == Some(false);
        let should_dim = is_idle && !is_selected;
        let base_style = if is_selected {
            // Claude renders the selected row in the theme "suggestion" color; no
            // theme is threaded into this overlay, so use a reversed highlight.
            Style::default().add_modifier(Modifier::REVERSED)
        } else if should_dim {
            Style::default().add_modifier(Modifier::DIM)
        } else {
            Style::default()
        };

        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.push(Span::styled(
            if is_selected { "› " } else { "  " }.to_string(),
            base_style,
        ));
        if row.is_hidden {
            spans.push(Span::styled("[hidden] ".to_string(), base_style));
        }
        if is_idle {
            spans.push(Span::styled("[idle] ".to_string(), base_style));
        }
        self.push_mode_and_name(&mut spans, row, base_style);
        if let Some(model) = row.model.as_deref() {
            spans.push(Span::styled(
                format!(" ({model})"),
                base_style.add_modifier(Modifier::DIM),
            ));
        }
        Line::from(spans)
    }

    /// The `@{name}` span (with the leading mode pill) used in the detail header,
    /// always rendered un-dimmed.
    fn teammate_name_span(&self, row: &TeammateRow) -> Vec<Span<'static>> {
        let mut spans: Vec<Span<'static>> = Vec::new();
        self.push_mode_and_name(&mut spans, row, Style::default());
        spans
    }

    /// Push the mode symbol (in mode color) followed by `@{name}` in the teammate
    /// color, honoring `base_style` for selection/dim. Calls into
    /// `super::team_colors` so the shared color API is exercised.
    fn push_mode_and_name(
        &self,
        spans: &mut Vec<Span<'static>>,
        row: &TeammateRow,
        base_style: Style,
    ) {
        let mode = row.mode.as_deref().unwrap_or("default");
        let (mode_symbol, mode_color) = super::team_colors::mode_symbol_and_color(mode);
        if !mode_symbol.is_empty() {
            spans.push(Span::styled(
                format!("{mode_symbol} "),
                base_style.fg(mode_color),
            ));
        }
        let name_color =
            super::team_colors::agent_color_to_tui(row.color.as_deref().unwrap_or(&row.name));
        spans.push(Span::styled(
            format!("@{}", row.name),
            base_style.fg(name_color),
        ));
    }
}

fn detail_field(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label}: "),
            Style::default().add_modifier(Modifier::DIM),
        ),
        Span::raw(value.to_string()),
    ])
}

fn action_from_row(row: &TeammateRow) -> TeamsDialogTeammateAction {
    TeamsDialogTeammateAction {
        pane_id: row.tmux_pane_id.clone(),
        backend_type: row.backend_type.clone(),
        agent_id: row.agent_id.clone(),
        name: row.name.clone(),
    }
}

fn prompt_line(prompt: &str, expanded: bool) -> Line<'static> {
    if expanded {
        return Line::from(prompt.to_string());
    }
    let truncated = truncate_to_width(prompt, PROMPT_COLLAPSED_WIDTH);
    let needs_expand_hint = UnicodeWidthStr::width(prompt) > PROMPT_COLLAPSED_WIDTH;
    if needs_expand_hint {
        Line::from(vec![truncated.into(), " (p to expand)".dim_span()])
    } else {
        Line::from(truncated)
    }
}

fn truncate_to_width(text: &str, max_width: usize) -> String {
    let mut width = 0;
    let mut out = String::new();
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_width > max_width {
            break;
        }
        out.push(ch);
        width += ch_width;
    }
    out
}

/// Tiny styling helpers so the render code reads like the rest of the TUI without
/// pulling `Stylize` in (string -> styled `Span`).
trait DialogSpan {
    fn bold_span(self) -> Span<'static>;
    fn dim_span(self) -> Span<'static>;
}

impl DialogSpan for String {
    fn bold_span(self) -> Span<'static> {
        Span::styled(self, Style::default().add_modifier(Modifier::BOLD))
    }
    fn dim_span(self) -> Span<'static> {
        Span::styled(self, Style::default().add_modifier(Modifier::DIM))
    }
}

impl DialogSpan for &str {
    fn bold_span(self) -> Span<'static> {
        self.to_string().bold_span()
    }
    fn dim_span(self) -> Span<'static> {
        self.to_string().dim_span()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str) -> TeammateRow {
        TeammateRow {
            name: name.to_string(),
            agent_id: format!("agent-{name}"),
            tmux_pane_id: format!("%{name}"),
            backend_type: Some("tmux".to_string()),
            model: Some("gpt-5".to_string()),
            prompt: None,
            mode: None,
            color: None,
            is_active: None,
            is_hidden: false,
        }
    }

    fn dialog_with(rows: Vec<TeammateRow>) -> TeamsDialog {
        let mut dialog = TeamsDialog::open("Rocket".to_string());
        dialog.teammates = rows;
        dialog
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn rendered_text(dialog: &TeamsDialog) -> String {
        let area = Rect::new(0, 0, 60, 16);
        let mut buf = Buffer::empty(area);
        dialog.render(area, &mut buf);
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| {
                        let symbol = buf[(x, y)].symbol();
                        if symbol.is_empty() {
                            " ".to_string()
                        } else {
                            symbol.to_string()
                        }
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn refresh_excludes_team_lead_and_empty_pane_rows() {
        let codex_home = tempfile::tempdir().expect("tempdir");
        crate::legacy_core::team_store::write_config(
            codex_home.path(),
            "Rocket",
            &crate::legacy_core::team_store::TeamFile {
                name: "Rocket".to_string(),
                lead_agent_id: "team-lead@Rocket".to_string(),
                members: vec![
                    crate::legacy_core::team_store::TeamFileMember {
                        agent_id: "team-lead@Rocket".to_string(),
                        name: crate::legacy_core::team_store::TEAM_LEAD_NAME.to_string(),
                        ..Default::default()
                    },
                    crate::legacy_core::team_store::TeamFileMember {
                        agent_id: "empty@Rocket".to_string(),
                        name: "empty".to_string(),
                        tmux_pane_id: String::new(),
                        ..Default::default()
                    },
                    crate::legacy_core::team_store::TeamFileMember {
                        agent_id: "alice@Rocket".to_string(),
                        name: "alice".to_string(),
                        tmux_pane_id: "%7".to_string(),
                        backend_type: Some("tmux".to_string()),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
        )
        .expect("write team config");
        let mut dialog = TeamsDialog::open("Rocket".to_string());

        dialog.refresh(codex_home.path());

        assert_eq!(dialog.teammates.len(), 1);
        assert_eq!(dialog.teammates[0].name, "alice");
        let text = rendered_text(&dialog);
        assert!(text.contains("@alice"), "missing @alice in:\n{text}");
        assert!(!text.contains("@team-lead"), "lead leaked into:\n{text}");
        assert!(!text.contains("@empty"), "empty pane leaked into:\n{text}");
    }

    #[test]
    fn arrow_selection_clamps() {
        let mut dialog = dialog_with(vec![row("alice"), row("bob"), row("carol")]);
        assert_eq!(dialog.selected_index, 0);

        assert!(dialog.handle_key(press(KeyCode::Char('j'))).is_none());
        assert_eq!(dialog.selected_index, 1);
        dialog.handle_key(press(KeyCode::Down));
        assert_eq!(dialog.selected_index, 2);
        // Down past the end stays at the last row.
        dialog.handle_key(press(KeyCode::Char('j')));
        assert_eq!(dialog.selected_index, 2);
        dialog.handle_key(press(KeyCode::Up));
        assert_eq!(dialog.selected_index, 1);
        dialog.handle_key(press(KeyCode::Up));
        assert_eq!(dialog.selected_index, 0);
        // Up from the top stays at the first row.
        dialog.handle_key(press(KeyCode::Up));
        assert_eq!(dialog.selected_index, 0);
    }

    #[test]
    fn selection_clamps_after_roster_shrinks() {
        let mut dialog = dialog_with(vec![row("alice"), row("bob"), row("carol")]);
        dialog.selected_index = 2;
        dialog.teammates = vec![row("alice")];
        dialog.clamp_selection();
        assert_eq!(dialog.selected_index, 0);
    }

    #[test]
    fn enter_drills_into_detail_then_views_output() {
        let mut dialog = dialog_with(vec![row("alice"), row("bob")]);
        dialog.selected_index = 1;

        // Enter on the list drills in (no action emitted yet).
        assert!(dialog.handle_key(press(KeyCode::Enter)).is_none());
        match &dialog.level {
            TeamsDialogLevel::TeammateDetail { team, member_name } => {
                assert_eq!(team, "Rocket");
                assert_eq!(member_name, "bob");
            }
            TeamsDialogLevel::TeammateList { .. } => panic!("expected detail level"),
        }

        // Enter on the detail focuses the teammate's pane.
        match dialog.handle_key(press(KeyCode::Enter)) {
            Some(TeamsDialogAction::ViewTeammateOutput {
                pane_id,
                backend_type,
            }) => {
                assert_eq!(pane_id, "%bob");
                assert_eq!(backend_type.as_deref(), Some("tmux"));
            }
            other => panic!("expected ViewTeammateOutput, got {:?}", other.is_some()),
        }
    }

    #[test]
    fn h_toggles_visibility_with_hide_matching_is_hidden() {
        // Visible row -> request hide=true.
        let mut dialog = dialog_with(vec![row("alice")]);
        match dialog.handle_key(press(KeyCode::Char('h'))) {
            Some(TeamsDialogAction::ToggleVisibility {
                team,
                pane_id,
                hide,
            }) => {
                assert_eq!(team, "Rocket");
                assert_eq!(pane_id, "%alice");
                assert!(hide, "visible row should request hide=true");
            }
            _ => panic!("expected ToggleVisibility"),
        }

        // Hidden row -> request hide=false (i.e. show).
        let mut hidden = row("bob");
        hidden.is_hidden = true;
        let mut dialog = dialog_with(vec![hidden]);
        match dialog.handle_key(press(KeyCode::Char('h'))) {
            Some(TeamsDialogAction::ToggleVisibility { hide, .. }) => {
                assert!(!hide, "hidden row should request hide=false");
            }
            _ => panic!("expected ToggleVisibility"),
        }
    }

    #[test]
    fn k_kills_selected_teammate() {
        let mut dialog = dialog_with(vec![row("alice")]);

        match dialog.handle_key(press(KeyCode::Char('k'))) {
            Some(TeamsDialogAction::KillTeammate { team, teammate }) => {
                assert_eq!(team, "Rocket");
                assert_eq!(
                    teammate,
                    TeamsDialogTeammateAction {
                        pane_id: "%alice".to_string(),
                        backend_type: Some("tmux".to_string()),
                        agent_id: "agent-alice".to_string(),
                        name: "alice".to_string(),
                    }
                );
            }
            _ => panic!("expected KillTeammate"),
        }
    }

    #[test]
    fn s_sends_shutdown_to_selected_teammate() {
        let mut dialog = dialog_with(vec![row("alice")]);

        match dialog.handle_key(press(KeyCode::Char('s'))) {
            Some(TeamsDialogAction::ShutdownTeammate { team, name }) => {
                assert_eq!(team, "Rocket");
                assert_eq!(name, "alice");
            }
            _ => panic!("expected ShutdownTeammate"),
        }
    }

    #[test]
    fn uppercase_h_toggles_all_visibility() {
        let mut hidden = row("bob");
        hidden.is_hidden = true;
        let mut dialog = dialog_with(vec![row("alice"), hidden]);

        match dialog.handle_key(press(KeyCode::Char('H'))) {
            Some(TeamsDialogAction::ToggleAllVisibility { team, hide }) => {
                assert_eq!(team, "Rocket");
                assert!(hide, "any visible row should hide all");
            }
            _ => panic!("expected ToggleAllVisibility"),
        }

        let mut hidden_a = row("alice");
        hidden_a.is_hidden = true;
        let mut hidden_b = row("bob");
        hidden_b.is_hidden = true;
        let mut dialog = dialog_with(vec![hidden_a, hidden_b]);
        match dialog.handle_key(press(KeyCode::Char('H'))) {
            Some(TeamsDialogAction::ToggleAllVisibility { hide, .. }) => {
                assert!(!hide, "all hidden rows should show all");
            }
            _ => panic!("expected ToggleAllVisibility"),
        }
    }

    #[test]
    fn p_prunes_idle_teammates() {
        let mut idle = row("alice");
        idle.is_active = Some(false);
        let mut active = row("bob");
        active.is_active = Some(true);
        let mut dialog = dialog_with(vec![idle, active]);

        match dialog.handle_key(press(KeyCode::Char('p'))) {
            Some(TeamsDialogAction::PruneIdle { team, teammates }) => {
                assert_eq!(team, "Rocket");
                assert_eq!(teammates.len(), 1);
                assert_eq!(teammates[0].name, "alice");
            }
            _ => panic!("expected PruneIdle"),
        }
    }

    #[test]
    fn esc_pops_detail_then_closes_from_list() {
        let mut dialog = dialog_with(vec![row("alice")]);
        dialog.handle_key(press(KeyCode::Enter)); // -> detail

        // Esc from detail pops back to the list (consumed, no action).
        assert!(dialog.handle_key(press(KeyCode::Esc)).is_none());
        assert!(matches!(
            dialog.level,
            TeamsDialogLevel::TeammateList { .. }
        ));

        // Esc / q from the list closes.
        assert!(matches!(
            dialog.handle_key(press(KeyCode::Char('q'))),
            Some(TeamsDialogAction::Close)
        ));
    }

    #[test]
    fn idle_and_hidden_pills_present_in_render() {
        let mut idle = row("alice");
        idle.is_active = Some(false);
        let mut hidden = row("bob");
        hidden.is_hidden = true;
        let dialog = dialog_with(vec![idle, hidden]);

        let text = rendered_text(&dialog);
        assert!(text.contains("[idle]"), "missing [idle] pill in:\n{text}");
        assert!(
            text.contains("[hidden]"),
            "missing [hidden] pill in:\n{text}"
        );
        assert!(text.contains("@alice"), "missing @alice in:\n{text}");
        assert!(text.contains("@bob"), "missing @bob in:\n{text}");
    }

    #[test]
    fn active_row_is_not_idle() {
        // is_active == Some(true) and None both render without the [idle] pill.
        let mut active = row("alice");
        active.is_active = Some(true);
        let dialog = dialog_with(vec![active]);
        assert!(!rendered_text(&dialog).contains("[idle]"));
    }
}

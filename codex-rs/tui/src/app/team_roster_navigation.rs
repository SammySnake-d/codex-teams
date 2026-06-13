//! Claude-style Teams status selection for the lead TUI.
//!
//! This state is intentionally separate from generic subagent navigation. It is
//! populated only from Teams teammate spawn events. Split-pane/process Teams use
//! a Teams-only footer selector so switching teammates does not add them to the
//! ordinary `/agent` picker.

use codex_protocol::ThreadId;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;

use crate::chatwidget::TeamTeammateViewHeader;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TeamRosterMember {
    pub(crate) thread_id: ThreadId,
    pub(crate) name: String,
    pub(crate) tmux_pane_id: String,
    pub(crate) backend_type: Option<String>,
    pub(crate) color: Option<String>,
    pub(crate) mode: Option<String>,
    pub(crate) is_active: Option<bool>,
    pub(crate) prompt: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TeamRosterMemberInput {
    pub(crate) thread_id: ThreadId,
    pub(crate) name: String,
    pub(crate) tmux_pane_id: Option<String>,
    pub(crate) backend_type: Option<String>,
    pub(crate) color: Option<String>,
    pub(crate) mode: Option<String>,
    pub(crate) is_active: Option<bool>,
    pub(crate) prompt: Option<String>,
}

impl TeamRosterMember {
    pub(crate) fn new(input: TeamRosterMemberInput) -> Option<Self> {
        let TeamRosterMemberInput {
            thread_id,
            name,
            tmux_pane_id,
            backend_type,
            color,
            mode,
            is_active,
            prompt,
        } = input;
        let tmux_pane_id = tmux_pane_id?;
        let tmux_pane_id = tmux_pane_id.trim().to_string();
        if tmux_pane_id.is_empty() {
            return None;
        }
        Some(Self {
            thread_id,
            name,
            tmux_pane_id,
            backend_type,
            color,
            mode,
            is_active,
            prompt: prompt.and_then(trimmed_non_empty),
        })
    }
}

#[derive(Debug, Default)]
pub(crate) struct TeamRosterNavigationState {
    team_name: Option<String>,
    members: Vec<TeamRosterMember>,
    selected_footer_index: Option<usize>,
    viewed_member_thread_id: Option<ThreadId>,
    roster_collapsed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TeamRosterDirection {
    Previous,
    Next,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TeamRosterSelectionAction {
    SelectThread(ThreadId),
    ViewTeammate(ThreadId),
    KillTeammate {
        team_name: String,
        name: String,
        pane_id: String,
        backend_type: Option<String>,
    },
    CollapseRoster,
}

struct RosterTreeLine {
    label: String,
    index: usize,
    viewed: bool,
    color: Option<String>,
    idle: bool,
    hide_row: bool,
}

impl TeamRosterNavigationState {
    pub(crate) fn set_active_team(&mut self, team_name: String) {
        if self.team_name.as_deref() != Some(team_name.as_str()) {
            self.members.clear();
            self.selected_footer_index = None;
            self.viewed_member_thread_id = None;
            self.roster_collapsed = false;
        }
        self.team_name = Some(team_name);
    }

    pub(crate) fn register_member(&mut self, member: TeamRosterMember) {
        if let Some(existing) = self
            .members
            .iter_mut()
            .find(|existing| existing.thread_id == member.thread_id)
        {
            *existing = member;
        } else {
            self.members.push(member);
        }
        self.members.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.thread_id.to_string().cmp(&right.thread_id.to_string()))
        });
        self.clamp_selection();
        self.clamp_viewed_member();
    }

    pub(crate) fn replace_members(&mut self, mut members: Vec<TeamRosterMember>) {
        members.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.thread_id.to_string().cmp(&right.thread_id.to_string()))
        });
        self.members = members;
        self.clamp_selection();
        self.clamp_viewed_member();
    }

    pub(crate) fn clear(&mut self) {
        self.team_name = None;
        self.members.clear();
        self.selected_footer_index = None;
        self.viewed_member_thread_id = None;
        self.roster_collapsed = false;
    }

    pub(crate) fn active_team_name(&self) -> Option<String> {
        self.team_name.clone()
    }

    pub(crate) fn teammate_mention_names(&self) -> Vec<String> {
        self.members
            .iter()
            .map(|member| member.name.clone())
            .collect()
    }

    pub(crate) fn footer_spans(
        &self,
        current_thread_id: Option<ThreadId>,
        primary_thread_id: Option<ThreadId>,
    ) -> Option<Vec<Span<'static>>> {
        let _ = (current_thread_id, primary_thread_id);
        if self.members.is_empty() {
            return None;
        }

        let teammate_count = self.members.len();
        let suffix = if teammate_count == 1 {
            "teammate"
        } else {
            "teammates"
        };
        let mut spans = Vec::new();
        let label = format!("{teammate_count} {suffix}");
        let mut status = Span::from(label.clone());
        if self.selected_footer_index.is_some() && !self.roster_collapsed {
            status = Span::styled(label, Style::default().add_modifier(Modifier::REVERSED));
        }
        spans.push(status);
        if self.selected_footer_index.is_some() && !self.roster_collapsed {
            spans.push(" · ".dim());
            spans.push("Enter to view".dim());
        }
        Some(spans)
    }

    pub(crate) fn roster_tree_lines(
        &self,
        current_thread_id: Option<ThreadId>,
        primary_thread_id: Option<ThreadId>,
    ) -> Option<Vec<Line<'static>>> {
        if self.members.is_empty() || self.selected_footer_index.is_none() || self.roster_collapsed
        {
            return None;
        }

        let mut lines = Vec::new();
        let viewed_member_thread_id = self.viewed_member_thread_id.or_else(|| {
            current_thread_id.filter(|thread_id| Some(*thread_id) != primary_thread_id)
        });
        self.push_roster_tree_line(
            &mut lines,
            RosterTreeLine {
                label: "team-lead".to_string(),
                index: 0,
                viewed: self.viewed_member_thread_id.is_none()
                    && current_thread_id == primary_thread_id,
                color: None,
                idle: false,
                hide_row: false,
            },
        );
        for (member_index, member) in self.members.iter().enumerate() {
            self.push_roster_tree_line(
                &mut lines,
                RosterTreeLine {
                    label: format!("@{}", member.name),
                    index: member_index + 1,
                    viewed: Some(member.thread_id) == viewed_member_thread_id,
                    color: member.color.clone(),
                    idle: member.is_active == Some(false),
                    hide_row: false,
                },
            );
        }
        self.push_roster_tree_line(
            &mut lines,
            RosterTreeLine {
                label: "hide".to_string(),
                index: self.members.len() + 1,
                viewed: false,
                color: None,
                idle: false,
                hide_row: true,
            },
        );
        Some(lines)
    }

    pub(crate) fn selected_footer_index(&self) -> Option<usize> {
        self.selected_footer_index
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selected_footer_index = None;
    }

    pub(crate) fn clear_viewed_teammate(&mut self) -> bool {
        let was_viewing = self.viewed_member_thread_id.take().is_some();
        if was_viewing {
            self.selected_footer_index = None;
        }
        was_viewing
    }

    pub(crate) fn activate_selection(
        &mut self,
        primary_thread_id: Option<ThreadId>,
    ) -> Option<TeamRosterSelectionAction> {
        let selected_index = self.selected_footer_index?;
        self.selected_footer_index = None;
        if selected_index == 0 {
            self.viewed_member_thread_id = None;
            return primary_thread_id.map(TeamRosterSelectionAction::SelectThread);
        }
        if selected_index == self.members.len() + 1 {
            self.viewed_member_thread_id = None;
            self.roster_collapsed = true;
            return Some(TeamRosterSelectionAction::CollapseRoster);
        }
        let member = self.members.get(selected_index - 1)?;
        self.viewed_member_thread_id = Some(member.thread_id);
        Some(TeamRosterSelectionAction::ViewTeammate(member.thread_id))
    }

    pub(crate) fn activate_selected_teammate_view(&mut self) -> Option<TeamRosterSelectionAction> {
        let selected_index = self.selected_footer_index?;
        if selected_index == 0 || selected_index == self.members.len() + 1 {
            return None;
        }
        let member = self.members.get(selected_index - 1)?;
        self.selected_footer_index = None;
        self.viewed_member_thread_id = Some(member.thread_id);
        Some(TeamRosterSelectionAction::ViewTeammate(member.thread_id))
    }

    pub(crate) fn kill_selected_teammate(&mut self) -> Option<TeamRosterSelectionAction> {
        let team_name = self.team_name.clone()?;
        let selected_index = self.selected_footer_index?;
        if selected_index == 0 || selected_index == self.members.len() + 1 {
            return None;
        }
        let member = self.members.get(selected_index - 1)?;
        if member.is_active != Some(true) {
            return None;
        }
        let action = TeamRosterSelectionAction::KillTeammate {
            team_name,
            name: member.name.clone(),
            pane_id: member.tmux_pane_id.clone(),
            backend_type: member.backend_type.clone(),
        };
        self.selected_footer_index = None;
        if self.viewed_member_thread_id == Some(member.thread_id) {
            self.viewed_member_thread_id = None;
        }
        Some(action)
    }

    #[cfg(test)]
    pub(crate) fn collapse_roster(&mut self) {
        self.selected_footer_index = None;
        self.roster_collapsed = true;
    }

    pub(crate) fn step_selection(
        &mut self,
        direction: TeamRosterDirection,
        current_thread_id: Option<ThreadId>,
        primary_thread_id: Option<ThreadId>,
    ) -> bool {
        if self.members.is_empty() {
            self.selected_footer_index = None;
            return false;
        }

        let _ = (current_thread_id, primary_thread_id);
        self.roster_collapsed = false;
        let item_count = self.footer_item_count();
        let selected = match self.selected_footer_index {
            None => 0,
            Some(index) => match direction {
                TeamRosterDirection::Previous => {
                    if index == 0 {
                        item_count - 1
                    } else {
                        index - 1
                    }
                }
                TeamRosterDirection::Next => (index + 1) % item_count,
            },
        };
        self.selected_footer_index = Some(selected);
        true
    }

    pub(crate) fn is_teammate_thread(&self, thread_id: Option<ThreadId>) -> bool {
        thread_id.is_some_and(|thread_id| {
            self.members
                .iter()
                .any(|member| member.thread_id == thread_id)
        })
    }

    pub(crate) fn teammate_view_header(
        &self,
        thread_id: Option<ThreadId>,
    ) -> Option<TeamTeammateViewHeader> {
        let thread_id = self.viewed_member_thread_id.or(thread_id)?;
        self.members
            .iter()
            .find(|member| member.thread_id == thread_id)
            .map(|member| TeamTeammateViewHeader {
                name: member.name.clone(),
                color: member.color.clone(),
                prompt: member.prompt.clone(),
            })
    }

    fn footer_item_count(&self) -> usize {
        if self.members.is_empty() {
            0
        } else {
            // main + each teammate + hide
            self.members.len() + 2
        }
    }

    fn clamp_selection(&mut self) {
        let item_count = self.footer_item_count();
        if item_count == 0
            || self
                .selected_footer_index
                .is_some_and(|index| index >= item_count)
        {
            self.selected_footer_index = None;
        }
    }

    fn clamp_viewed_member(&mut self) {
        if self.viewed_member_thread_id.is_some_and(|thread_id| {
            !self
                .members
                .iter()
                .any(|member| member.thread_id == thread_id)
        }) {
            self.viewed_member_thread_id = None;
        }
    }

    fn push_roster_tree_line(&self, lines: &mut Vec<Line<'static>>, row: RosterTreeLine) {
        let selected = self.selected_footer_index == Some(row.index);
        let mut style = row.color.as_deref().map(color_style).unwrap_or_default();
        if selected {
            style = style.add_modifier(Modifier::BOLD);
        }
        if row.viewed {
            style = style.add_modifier(Modifier::BOLD);
        }
        if row.idle && !selected && !row.viewed {
            style = style.dim();
        }

        let mut spans = vec![
            if selected { "› " } else { "  " }.into(),
            if row.hide_row { "└─ " } else { "├─ " }.dim(),
            Span::styled(row.label, style),
        ];
        if selected {
            spans.push(" · ".dim());
            let hint = if row.hide_row {
                "enter to collapse"
            } else {
                "Enter to view"
            };
            spans.push(hint.dim());
        }
        lines.push(Line::from(spans));
    }
}

fn trimmed_non_empty(value: String) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn color_style(color: &str) -> Style {
    let color = match color {
        "red" => Color::Red,
        "blue" => Color::Blue,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "purple" | "pink" => Color::Magenta,
        "orange" => Color::Yellow,
        "cyan" => Color::Cyan,
        _ => Color::Reset,
    };
    if color == Color::Reset {
        Style::default()
    } else {
        Style::default().fg(color)
    }
}

#[cfg(test)]
#[path = "team_roster_navigation_tests.rs"]
mod tests;

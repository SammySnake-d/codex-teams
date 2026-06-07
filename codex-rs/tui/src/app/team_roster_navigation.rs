//! Claude-style Teams roster selection for the lead TUI.
//!
//! This state is intentionally separate from generic subagent navigation. It is
//! populated only from Teams teammate spawn events, and it models the footer
//! roster as `@main`, teammates sorted by name, and Claude's transient `hide`
//! row while the roster is actively selected.

use std::collections::HashMap;

use codex_protocol::ThreadId;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Span;

#[derive(Debug, Default)]
pub(crate) struct TeamRosterNavigationState {
    team_name: Option<String>,
    members: HashMap<ThreadId, TeamRosterMember>,
    selected_footer_index: Option<usize>,
    roster_collapsed: bool,
}

#[derive(Debug)]
struct TeamRosterMember {
    name: String,
    pane: Option<TeamRosterPaneTarget>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TeamRosterPaneTarget {
    pane_id: String,
    backend_type: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TeamRosterDirection {
    Previous,
    Next,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SelectedTeamRosterTarget {
    Thread(ThreadId),
    Pane {
        pane_id: String,
        backend_type: Option<String>,
    },
    Hide,
}

impl TeamRosterNavigationState {
    pub(crate) fn set_active_team(&mut self, team_name: String) {
        self.team_name = Some(team_name);
    }

    pub(crate) fn register_member(
        &mut self,
        thread_id: ThreadId,
        name: String,
        tmux_pane_id: Option<String>,
        backend_type: Option<String>,
    ) {
        let pane = tmux_pane_id
            .filter(|pane_id| !pane_id.trim().is_empty())
            .map(|pane_id| TeamRosterPaneTarget {
                pane_id,
                backend_type,
            });
        self.members
            .insert(thread_id, TeamRosterMember { name, pane });
        self.clamp_selection();
    }

    pub(crate) fn clear(&mut self) {
        self.team_name = None;
        self.members.clear();
        self.selected_footer_index = None;
        self.roster_collapsed = false;
    }

    pub(crate) fn footer_label(&self) -> Option<String> {
        self.team_name
            .as_ref()
            .map(|team_name| format!("Teams: {team_name}"))
    }

    pub(crate) fn footer_spans(
        &self,
        current_thread_id: Option<ThreadId>,
        primary_thread_id: Option<ThreadId>,
    ) -> Option<Vec<Span<'static>>> {
        if self.members.is_empty() || self.roster_collapsed {
            return None;
        }

        let viewed_footer_index = self.viewed_footer_index(current_thread_id, primary_thread_id);
        let mut spans = Vec::new();
        self.push_footer_pill(&mut spans, 0, "@main".to_string(), viewed_footer_index);
        for (member_index, (_, member)) in self.ordered_members().into_iter().enumerate() {
            spans.push(" · ".into());
            self.push_footer_pill(
                &mut spans,
                member_index + 1,
                format!("@{}", member.name),
                viewed_footer_index,
            );
        }
        if self.selected_footer_index.is_some() {
            spans.push(" · ".into());
            self.push_footer_pill(
                &mut spans,
                self.hide_footer_index(),
                "hide".to_string(),
                viewed_footer_index,
            );
        }
        Some(spans)
    }

    pub(crate) fn selected_footer_index(&self) -> Option<usize> {
        self.selected_footer_index
    }

    pub(crate) fn selected_target(
        &self,
        primary_thread_id: Option<ThreadId>,
    ) -> Option<SelectedTeamRosterTarget> {
        match self.selected_footer_index? {
            0 => primary_thread_id.map(SelectedTeamRosterTarget::Thread),
            index => {
                if index == self.hide_footer_index() {
                    return Some(SelectedTeamRosterTarget::Hide);
                }
                let ordered_members = self.ordered_members();
                let (thread_id, member) = ordered_members.get(index.saturating_sub(1))?;
                if let Some(pane) = member.pane.as_ref() {
                    Some(SelectedTeamRosterTarget::Pane {
                        pane_id: pane.pane_id.clone(),
                        backend_type: pane.backend_type.clone(),
                    })
                } else {
                    Some(SelectedTeamRosterTarget::Thread(*thread_id))
                }
            }
        }
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selected_footer_index = None;
    }

    pub(crate) fn collapse_roster(&mut self) {
        self.selected_footer_index = None;
        self.roster_collapsed = true;
    }

    pub(crate) fn step_selection(&mut self, direction: TeamRosterDirection) -> bool {
        let item_count = self.footer_item_count();
        if item_count <= 1 {
            self.selected_footer_index = None;
            return false;
        }

        let Some(current) = self.selected_footer_index else {
            self.roster_collapsed = false;
            self.selected_footer_index = Some(0);
            return true;
        };
        let next = match direction {
            TeamRosterDirection::Next => (current + 1) % item_count,
            TeamRosterDirection::Previous => {
                if current == 0 {
                    item_count - 1
                } else {
                    current - 1
                }
            }
        };
        self.roster_collapsed = false;
        self.selected_footer_index = Some(next);
        true
    }

    pub(crate) fn is_teammate_thread(&self, thread_id: Option<ThreadId>) -> bool {
        thread_id.is_some_and(|thread_id| self.members.contains_key(&thread_id))
    }

    pub(crate) fn viewed_footer_index(
        &self,
        current_thread_id: Option<ThreadId>,
        primary_thread_id: Option<ThreadId>,
    ) -> Option<usize> {
        let current_thread_id = current_thread_id?;
        if Some(current_thread_id) == primary_thread_id {
            return (!self.members.is_empty()).then_some(0);
        }
        self.ordered_members()
            .iter()
            .position(|(thread_id, _)| *thread_id == current_thread_id)
            .map(|index| index + 1)
    }

    fn footer_item_count(&self) -> usize {
        if self.members.is_empty() {
            0
        } else {
            self.members.len() + 2
        }
    }

    fn hide_footer_index(&self) -> usize {
        self.members.len() + 1
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

    fn ordered_members(&self) -> Vec<(ThreadId, &TeamRosterMember)> {
        let mut members = self
            .members
            .iter()
            .map(|(thread_id, member)| (*thread_id, member))
            .collect::<Vec<_>>();
        members.sort_by(|(_, left), (_, right)| left.name.cmp(&right.name));
        members
    }

    fn push_footer_pill(
        &self,
        spans: &mut Vec<Span<'static>>,
        index: usize,
        label: String,
        viewed_footer_index: Option<usize>,
    ) {
        let selected = self.selected_footer_index == Some(index);
        let viewed = viewed_footer_index == Some(index);
        let style = match (selected, viewed) {
            (true, true) => Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            (true, false) => Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            (false, true) => Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
            (false, false) => Style::default(),
        };
        spans.push(Span::styled(label, style));
    }
}

#[cfg(test)]
#[path = "team_roster_navigation_tests.rs"]
mod tests;

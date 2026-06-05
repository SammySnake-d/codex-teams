//! Lightweight Teams UI state derived from team tool calls.
//!
//! The core Teams runtime is model/tool driven. Until app-server exposes a dedicated
//! Teams notification stream, the TUI can still make Teams visible by observing the raw
//! team tool call/output items that already flow through the app-server event stream.

use std::collections::HashMap;

use codex_protocol::models::ResponseItem;
use serde_json::Value;

use super::ChatWidget;
use crate::app_event::AppEvent;

#[derive(Default)]
pub(super) struct TeamUiState {
    pending_calls: HashMap<String, PendingTeamCall>,
    active_team: Option<TeamUiSummary>,
    routed_message_count: usize,
    /// Stable per-teammate color assignment (Phase 6 §B.2). Mirrors Claude's
    /// `teammateColorAssignments`: round-robin in first-seen order, cleared on
    /// `TeamStop`.
    colors: crate::chatwidget::team_colors::TeammateColors,
}

#[derive(Clone, Debug)]
struct PendingTeamCall {
    tool: TeamToolKind,
    team_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TeamToolKind {
    CreateTeam,
    ListTeams,
    TeamStatus,
    TeamSpawnMember,
    TeamSend,
    TeamMemberStop,
    TeamStop,
}

#[derive(Clone, Debug, Default)]
struct TeamUiSummary {
    id: String,
    name: String,
    status: String,
    members: Vec<TeamMemberUiSummary>,
}

#[derive(Clone, Debug)]
pub(super) struct TeamMemberUiSummary {
    id: String,
    name: String,
    agent_thread_id: String,
    status: String,
    agent_status: String,
    /// Assigned (or persisted) teammate color name; one of `team_colors::AGENT_COLORS`.
    color: Option<&'static str>,
    /// Permission mode if surfaced by the team tool output; `None` → `default`.
    mode: Option<String>,
    /// Mirrors `hidden_pane_ids`; surfaced by the team tool output when present.
    is_hidden: bool,
}

#[derive(Debug)]
pub(super) enum TeamUiEvent {
    TeamCreated {
        name: String,
    },
    MemberSpawned {
        name: String,
        agent_thread_id: String,
    },
    MessageSent,
    TeamStopped {
        name: String,
    },
    Updated,
}

impl TeamUiState {
    pub(super) fn footer_label(&self) -> Option<String> {
        let team = self.active_team.as_ref()?;
        if team.status == "stopped" {
            return Some(format!("Teams: {} stopped", team.name));
        }

        let teammate_count = team.members.len();
        let mut member_labels = team
            .members
            .iter()
            .take(3)
            .map(|member| format!("@{} {}", member.name, member.status_label()))
            .collect::<Vec<_>>();
        if team.members.len() > member_labels.len() {
            member_labels.push(format!(
                "+{} more",
                team.members.len() - member_labels.len()
            ));
        }

        let teammate_word = if teammate_count == 1 {
            "1 teammate".to_string()
        } else {
            format!("{teammate_count} teammates")
        };

        let mut parts = vec![format!("Teams: {}", team.name)];
        if !member_labels.is_empty() {
            parts.push(member_labels.join(", "));
        }
        parts.push(teammate_word);
        parts.push("ctrl+t teammates".to_string());
        Some(parts.join(" · "))
    }

    pub(super) fn observe_response_item(&mut self, item: &ResponseItem) -> Option<TeamUiEvent> {
        match item {
            ResponseItem::FunctionCall {
                name,
                arguments,
                call_id,
                ..
            } => {
                let Some(tool) = TeamToolKind::from_name(name) else {
                    return None;
                };
                let team_id = parse_argument_string(arguments, "team_id");
                self.pending_calls
                    .insert(call_id.clone(), PendingTeamCall { tool, team_id });
                None
            }
            ResponseItem::FunctionCallOutput { call_id, output } => {
                let pending = self.pending_calls.remove(call_id)?;
                let text = output.text_content()?;
                let value = serde_json::from_str::<Value>(text).ok()?;
                self.apply_team_tool_output(pending, &value)
            }
            _ => None,
        }
    }

    /// Snapshot the live roster as composer-facing `@`-mention candidates.
    ///
    /// Stopped teams and stopped members are excluded so the popup never offers a
    /// teammate that can no longer receive messages. The mention token is derived
    /// from the member name, falling back to the member id when the name has no
    /// characters valid in a mention token.
    /// Snapshot the live roster as composer-facing `@`-mention candidates.
    ///
    /// Stopped teams and stopped members are excluded so the popup never offers a
    /// teammate that can no longer receive messages. The mention token is derived
    /// from the member name, falling back to the member id when the name has no
    /// characters valid in a mention token.
    /// Snapshot the live roster as composer-facing `@`-mention candidates.
    ///
    /// Stopped teams and stopped members are excluded so the popup never offers a
    /// teammate that can no longer receive messages. The mention token is derived
    /// from the member name, falling back to the member id when the name has no
    /// characters valid in a mention token.
    /// Snapshot the live roster as composer-facing `@`-mention candidates.
    ///
    /// Stopped teams and stopped members are excluded so the popup never offers a
    /// teammate that can no longer receive messages. The mention token is derived
    /// from the member name, falling back to the member id when the name has no
    /// characters valid in a mention token.
    fn apply_team_tool_output(
        &mut self,
        pending: PendingTeamCall,
        value: &Value,
    ) -> Option<TeamUiEvent> {
        match pending.tool {
            TeamToolKind::CreateTeam => {
                let team = parse_team(value.get("team")?)?;
                let name = team.name.clone();
                self.active_team = Some(team);
                self.assign_member_colors();
                Some(TeamUiEvent::TeamCreated { name })
            }
            TeamToolKind::ListTeams => {
                let teams = value.get("teams")?.as_array()?;
                let team = teams.iter().filter_map(parse_team).last()?;
                self.active_team = Some(team);
                self.assign_member_colors();
                Some(TeamUiEvent::Updated)
            }
            TeamToolKind::TeamStatus => {
                let team = parse_team(value.get("snapshot")?.get("team")?)?;
                self.active_team = Some(team);
                self.assign_member_colors();
                Some(TeamUiEvent::Updated)
            }
            TeamToolKind::TeamSpawnMember => {
                let member = parse_member(value.get("member")?)?;
                let name = member.name.clone();
                let agent_thread_id = member.agent_thread_id.clone();
                if let Some(team) = self
                    .active_team
                    .as_mut()
                    .filter(|team| pending.team_id.as_deref() == Some(team.id.as_str()))
                {
                    upsert_member(&mut team.members, member);
                }
                self.assign_member_colors();
                Some(TeamUiEvent::MemberSpawned {
                    name,
                    agent_thread_id,
                })
            }
            TeamToolKind::TeamSend => {
                self.routed_message_count = self.routed_message_count.saturating_add(1);
                Some(TeamUiEvent::MessageSent)
            }
            TeamToolKind::TeamMemberStop | TeamToolKind::TeamStop => {
                let team = parse_team(value.get("snapshot")?.get("team")?)?;
                let stopped =
                    matches!(pending.tool, TeamToolKind::TeamStop) || team.status == "stopped";
                let name = team.name.clone();
                self.active_team = Some(team);
                if stopped {
                    // Mirror Claude's `clearTeammateColors` on team teardown so a
                    // fresh team restarts the round-robin palette from the top.
                    self.colors.clear();
                    Some(TeamUiEvent::TeamStopped { name })
                } else {
                    self.assign_member_colors();
                    Some(TeamUiEvent::Updated)
                }
            }
        }
    }

    /// Fill a stable color for every member of the active team that does not
    /// already carry a persisted one (Phase 6 §B.2). Colors are assigned by
    /// member id in first-seen order so they match the on-disk
    /// `TeamFileMember.color` the lead persists.
    fn assign_member_colors(&mut self) {
        let Some(team) = self.active_team.as_mut() else {
            return;
        };
        for member in &mut team.members {
            if member.color.is_none() {
                member.color = Some(self.colors.assign(&member.id));
            }
        }
    }

    /// Name of the active (non-stopped) team, for opening the Teams dialog.
    pub(super) fn active_team_name(&self) -> Option<String> {
        self.active_team
            .as_ref()
            .filter(|team| team.status != "stopped")
            .map(|team| team.name.clone())
    }

    /// Styled footer roster (Phase 6 §B.2). Each pill = mode symbol (in its mode
    /// color) + `@name` (in the teammate color) + a status suffix, joined by
    /// ` · `. Returns `None` when there is no active team (the plain
    /// [`Self::footer_label`] path still drives the textual footer).
    pub(super) fn footer_spans(&self) -> Option<Vec<ratatui::text::Span<'static>>> {
        use ratatui::style::Style;
        use ratatui::text::Span;

        let team = self.active_team.as_ref()?;
        if team.status == "stopped" || team.members.is_empty() {
            return None;
        }

        let mut spans: Vec<Span<'static>> = Vec::new();
        for (index, member) in team.members.iter().take(3).enumerate() {
            if index > 0 {
                spans.push(Span::raw(" · "));
            }
            let mode = member.mode.as_deref().unwrap_or("default");
            let (mode_symbol, mode_color) =
                crate::chatwidget::team_colors::mode_symbol_and_color(mode);
            if !mode_symbol.is_empty() {
                spans.push(Span::styled(
                    format!("{mode_symbol} "),
                    Style::default().fg(mode_color),
                ));
            }
            if member.is_hidden {
                spans.push(Span::raw("[hidden] "));
            }
            let name_color = crate::chatwidget::team_colors::agent_color_to_tui(
                member.color.unwrap_or(member.name.as_str()),
            );
            spans.push(Span::styled(
                format!("@{}", member.name),
                Style::default().fg(name_color),
            ));
            spans.push(Span::raw(format!(" {}", member.status_label())));
        }
        if team.members.len() > 3 {
            spans.push(Span::raw(format!(" · +{} more", team.members.len() - 3)));
        }
        Some(spans)
    }
}

impl ChatWidget {
    /// Name of the active (non-stopped) Codex team, if any. Used by `App` to
    /// open the Teams dialog (Phase 6 §B.6).
    pub(crate) fn active_team_name(&self) -> Option<String> {
        self.team_ui.active_team_name()
    }

    pub(super) fn handle_raw_response_item_for_team_ui(
        &mut self,
        item: ResponseItem,
        from_replay: bool,
    ) {
        // Gate: the observer is a strict no-op for any item that is not a team
        // tool call/output (see `observe_response_item`, which returns `None` for
        // every non-team `FunctionCall`/`FunctionCallOutput`). A session that is
        // not running a team — including one running native subagents — therefore
        // renders and behaves exactly like upstream, because it never receives
        // team-shaped tool output to begin with.
        let Some(event) = self.team_ui.observe_response_item(&item) else {
            return;
        };
        // Footer pills must reflect the latest roster on both live events and
        // replay (e.g. after `codex resume` rebuilds team state).
        self.sync_footer_context_label();
        // Transcript notices and external panes are live-only side effects: replaying
        // historical tool output must not re-announce teams or re-open teammate panes.
        if from_replay {
            return;
        }
        match event {
            TeamUiEvent::TeamCreated { name } => {
                self.add_info_message(
                    format!("Codex team started: {name}"),
                    Some(
                        "Spawn teammates to make them appear in the footer and teammate picker."
                            .to_string(),
                    ),
                );
                self.app_event_tx
                    .send(AppEvent::TeamBecameActive { team: name });
            }
            TeamUiEvent::MemberSpawned {
                name,
                agent_thread_id,
            } => {
                self.add_info_message(
                    format!("Teammate @{name} started"),
                    Some(format!(
                        "Agent thread {agent_thread_id}; use ctrl+t to switch teammates or @{name} to message them."
                    )),
                );
                self.app_event_tx.send(AppEvent::OpenTeammatePane {
                    member_name: name,
                    agent_thread_id,
                });
            }
            TeamUiEvent::MessageSent => {
                self.add_info_message(
                    "Teams message routed".to_string(),
                    Some(
                        "The teammate footer is updated from the live team tool output."
                            .to_string(),
                    ),
                );
            }
            TeamUiEvent::TeamStopped { name } => {
                self.add_info_message(format!("Codex team stopped: {name}"), None);
                self.app_event_tx.send(AppEvent::TeamBecameInactive);
            }
            TeamUiEvent::Updated => {}
        }
    }

    pub(super) fn sync_footer_context_label(&mut self) {
        let mut labels = Vec::new();
        if let Some(active_agent_label) = self.active_agent_label.as_ref() {
            labels.push(active_agent_label.clone());
        }
        if let Some(team_label) = self.team_ui.footer_label() {
            labels.push(team_label);
        }
        let combined = (!labels.is_empty()).then(|| labels.join(" · "));
        self.bottom_pane.set_active_agent_label(combined);
        // Colored teammate pills render alongside the plain label (Phase 6 §B.6);
        // the plain `footer_label` String path above is left untouched so the
        // textual footer keeps working when spans are unavailable.
        self.bottom_pane
            .set_active_team_pills(self.team_ui.footer_spans());
    }
}

impl TeamMemberUiSummary {
    pub(super) fn status_label(&self) -> &'static str {
        if self.status == "stopped" {
            return "stopped";
        }
        match self.agent_status.as_str() {
            "pending_init" => "starting",
            "running" => "running",
            "interrupted" | "completed" => "idle",
            "shutdown" | "not_found" => "stopped",
            "errored" => "error",
            _ => "active",
        }
    }
}

impl TeamToolKind {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "create_team" => Some(Self::CreateTeam),
            "list_teams" => Some(Self::ListTeams),
            "team_status" => Some(Self::TeamStatus),
            "team_spawn_member" => Some(Self::TeamSpawnMember),
            "team_send" => Some(Self::TeamSend),
            "team_member_stop" => Some(Self::TeamMemberStop),
            "team_stop" => Some(Self::TeamStop),
            _ => None,
        }
    }
}

fn parse_team(value: &Value) -> Option<TeamUiSummary> {
    let members = value
        .get("members")
        .and_then(Value::as_array)
        .map(|members| members.iter().filter_map(parse_member).collect())
        .unwrap_or_default();

    Some(TeamUiSummary {
        id: value.get("id")?.as_str()?.to_string(),
        name: value.get("name")?.as_str()?.to_string(),
        status: value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("active")
            .to_string(),
        members,
    })
}

fn parse_member(value: &Value) -> Option<TeamMemberUiSummary> {
    Some(TeamMemberUiSummary {
        id: value.get("id")?.as_str()?.to_string(),
        name: value.get("name")?.as_str()?.to_string(),
        agent_thread_id: value.get("agent_thread_id")?.as_str()?.to_string(),
        status: value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("active")
            .to_string(),
        agent_status: normalized_agent_status(value.get("agent_status")),
        // Persisted color name (TeamFileMember.color) when the tool output carries
        // it; otherwise left None so the round-robin assigner fills it in.
        color: value
            .get("color")
            .and_then(Value::as_str)
            .and_then(canonical_color_name),
        mode: value
            .get("mode")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        is_hidden: value
            .get("is_hidden")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// Map a free-form color string back to one of the static `AGENT_COLORS` names so
/// `TeamMemberUiSummary.color` can stay `&'static str` (matching the assigner).
/// Unknown names are dropped, falling through to the round-robin assignment.
fn canonical_color_name(name: &str) -> Option<&'static str> {
    crate::chatwidget::team_colors::AGENT_COLORS
        .iter()
        .copied()
        .find(|candidate| *candidate == name)
}

fn normalized_agent_status(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(status)) => status.clone(),
        Some(Value::Object(map)) => map
            .keys()
            .next()
            .cloned()
            .unwrap_or_else(|| "active".to_string()),
        _ => "active".to_string(),
    }
}

fn parse_argument_string(arguments: &str, key: &str) -> Option<String> {
    serde_json::from_str::<Value>(arguments)
        .ok()?
        .get(key)?
        .as_str()
        .map(ToString::to_string)
}

fn upsert_member(members: &mut Vec<TeamMemberUiSummary>, member: TeamMemberUiSummary) {
    if let Some(existing) = members.iter_mut().find(|existing| existing.id == member.id) {
        *existing = member;
    } else {
        members.push(member);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::models::FunctionCallOutputPayload;

    fn call(name: &str, call_id: &str, arguments: &str) -> ResponseItem {
        ResponseItem::FunctionCall {
            id: None,
            name: name.to_string(),
            namespace: None,
            arguments: arguments.to_string(),
            call_id: call_id.to_string(),
        }
    }

    fn output(call_id: &str, json: &str) -> ResponseItem {
        ResponseItem::FunctionCallOutput {
            call_id: call_id.to_string(),
            output: FunctionCallOutputPayload::from_text(json.to_string()),
        }
    }

    #[test]
    fn footer_tracks_team_tool_output() {
        let mut state = TeamUiState::default();
        assert!(state.footer_label().is_none());

        // create_team makes the team visible in the footer even before any teammates.
        state.observe_response_item(&call("create_team", "c1", "{}"));
        let created = state.observe_response_item(&output(
            "c1",
            r#"{"team":{"id":"team-1","name":"Rocket","status":"active","members":[]}}"#,
        ));
        assert!(matches!(created, Some(TeamUiEvent::TeamCreated { .. })));
        assert!(
            state
                .footer_label()
                .is_some_and(|label| label.contains("Teams: Rocket"))
        );

        // team_spawn_member adds the teammate to the footer.
        state.observe_response_item(&call(
            "team_spawn_member",
            "c2",
            r#"{"team_id":"team-1"}"#,
        ));
        let spawned = state.observe_response_item(&output(
            "c2",
            r#"{"member":{"id":"m1","name":"alice","agent_thread_id":"thr-1","status":"active","agent_status":"running"}}"#,
        ));
        assert!(matches!(
            spawned,
            Some(TeamUiEvent::MemberSpawned { .. })
        ));

        let footer = state.footer_label().expect("footer label after spawn");
        assert!(footer.contains("Teams: Rocket"), "footer was {footer}");
        assert!(footer.contains("@alice running"), "footer was {footer}");
        assert!(footer.contains("1 teammate"), "footer was {footer}");
        assert!(footer.contains("ctrl+t teammates"), "footer was {footer}");

        // Stopping the team shows a stopped footer.
        state.observe_response_item(&call("team_stop", "c3", r#"{"team_id":"team-1"}"#));
        state.observe_response_item(&output(
            "c3",
            r#"{"snapshot":{"team":{"id":"team-1","name":"Rocket","status":"stopped","members":[]}}}"#,
        ));
        assert_eq!(
            state.footer_label().as_deref(),
            Some("Teams: Rocket stopped")
        );
    }

    #[test]
    fn footer_spans_carry_color_after_member_spawned() {
        use ratatui::style::Color;

        let mut state = TeamUiState::default();
        // No team yet → no colored pills.
        assert!(state.footer_spans().is_none());

        state.observe_response_item(&call("create_team", "c1", "{}"));
        state.observe_response_item(&output(
            "c1",
            r#"{"team":{"id":"team-1","name":"Rocket","status":"active","members":[]}}"#,
        ));
        // A team with no members still renders no pills (only the plain label).
        assert!(state.footer_spans().is_none());

        state.observe_response_item(&call(
            "team_spawn_member",
            "c2",
            r#"{"team_id":"team-1"}"#,
        ));
        state.observe_response_item(&output(
            "c2",
            r#"{"member":{"id":"m1","name":"alice","agent_thread_id":"thr-1","status":"active","agent_status":"running"}}"#,
        ));

        let spans = state.footer_spans().expect("colored pills after spawn");
        let text: String = spans.iter().map(|span| span.content.as_ref()).collect();
        assert!(text.contains("@alice"), "spans were {text}");
        // The first assigned color is `red` (AGENT_COLORS[0]) → ratatui Red, and
        // it must actually be applied to the @name span (the whole point of the
        // colored-pill path).
        assert!(
            spans
                .iter()
                .any(|span| span.content.contains("@alice") && span.style.fg == Some(Color::Red)),
            "expected @alice pill in red, spans were {spans:?}"
        );

        // Stopping the team clears the colored pills and resets the palette.
        state.observe_response_item(&call("team_stop", "c3", r#"{"team_id":"team-1"}"#));
        state.observe_response_item(&output(
            "c3",
            r#"{"snapshot":{"team":{"id":"team-1","name":"Rocket","status":"stopped","members":[]}}}"#,
        ));
        assert!(state.footer_spans().is_none());
    }

    #[test]
    fn footer_spans_use_persisted_color_when_present() {
        use ratatui::style::Color;

        let mut state = TeamUiState::default();
        state.observe_response_item(&call("create_team", "c1", "{}"));
        state.observe_response_item(&output(
            "c1",
            r#"{"team":{"id":"team-1","name":"Rocket","status":"active","members":[]}}"#,
        ));
        state.observe_response_item(&call(
            "team_spawn_member",
            "c2",
            r#"{"team_id":"team-1"}"#,
        ));
        // The tool output carries a persisted color ("cyan"); it must win over
        // the round-robin assignment.
        state.observe_response_item(&output(
            "c2",
            r#"{"member":{"id":"m1","name":"alice","agent_thread_id":"thr-1","status":"active","agent_status":"running","color":"cyan"}}"#,
        ));

        let spans = state.footer_spans().expect("colored pills after spawn");
        assert!(
            spans
                .iter()
                .any(|span| span.content.contains("@alice") && span.style.fg == Some(Color::Cyan)),
            "expected @alice pill in cyan (persisted), spans were {spans:?}"
        );
    }
}


use super::*;
use pretty_assertions::assert_eq;

fn thread_id(suffix: u128) -> ThreadId {
    ThreadId::from_string(&format!("00000000-0000-0000-0000-{suffix:012}"))
        .expect("valid thread id")
}

fn span_text(spans: &[Span<'static>]) -> String {
    spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}

fn line_text(lines: &[ratatui::text::Line<'static>]) -> String {
    lines
        .iter()
        .map(|line| span_text(&line.spans))
        .collect::<Vec<_>>()
        .join("\n")
}

fn member(suffix: u128, name: &str, pane: &str) -> TeamRosterMember {
    TeamRosterMember::new(TeamRosterMemberInput {
        thread_id: thread_id(suffix),
        name: name.to_string(),
        tmux_pane_id: Some(pane.to_string()),
        backend_type: Some("tmux".to_string()),
        color: None,
        mode: None,
        is_active: None,
        prompt: None,
    })
    .expect("pane-backed member")
}

#[test]
fn footer_status_shows_leader_teammates_and_hide_row() {
    let mut state = TeamRosterNavigationState::default();
    let main_thread_id = thread_id(1);

    state.set_active_team("Rocket".to_string());
    assert_eq!(
        state.footer_spans(Some(main_thread_id), Some(main_thread_id)),
        None
    );

    state.register_member(member(2, "bob", "%bob"));
    state.register_member(member(3, "alice", "%alice"));

    let spans = state
        .footer_spans(Some(main_thread_id), Some(main_thread_id))
        .expect("members should render Teams status");
    assert_eq!(span_text(&spans), "2 teammates");
}

#[test]
fn selection_cycles_leader_teammates_and_hide_row() {
    let mut state = TeamRosterNavigationState::default();
    let main_thread_id = thread_id(1);

    state.register_member(member(2, "bob", "%bob"));
    state.register_member(member(3, "alice", "%alice"));

    assert!(state.step_selection(TeamRosterDirection::Next, None, None));
    assert_eq!(state.selected_footer_index(), Some(0));

    let spans = state
        .footer_spans(Some(main_thread_id), Some(main_thread_id))
        .expect("selected status should render hint");
    assert_eq!(span_text(&spans), "2 teammates · Enter to view");
    let lines = state
        .roster_tree_lines(Some(main_thread_id), Some(main_thread_id))
        .expect("selected roster should render tree");
    assert_eq!(
        line_text(&lines),
        "› ├─ team-lead · Enter to view\n  ├─ @alice\n  ├─ @bob\n  └─ hide"
    );

    assert!(state.step_selection(TeamRosterDirection::Next, None, None));
    assert_eq!(state.selected_footer_index(), Some(1));

    assert!(state.step_selection(TeamRosterDirection::Next, None, None));
    assert_eq!(state.selected_footer_index(), Some(2));

    assert!(state.step_selection(TeamRosterDirection::Next, None, None));
    assert_eq!(state.selected_footer_index(), Some(3));
    let spans = state
        .footer_spans(Some(main_thread_id), Some(main_thread_id))
        .expect("hide row should render collapse hint");
    assert_eq!(span_text(&spans), "2 teammates · Enter to view");
    let lines = state
        .roster_tree_lines(Some(main_thread_id), Some(main_thread_id))
        .expect("selected roster should render tree");
    assert_eq!(
        line_text(&lines),
        "  ├─ team-lead\n  ├─ @alice\n  ├─ @bob\n› └─ hide · enter to collapse"
    );

    assert!(state.step_selection(TeamRosterDirection::Next, None, None));
    assert_eq!(state.selected_footer_index(), Some(0));

    assert!(state.step_selection(TeamRosterDirection::Previous, None, None));
    assert_eq!(state.selected_footer_index(), Some(3));

    state.clear_selection();
    assert_eq!(state.selected_footer_index(), None);
}

#[test]
fn selection_is_disabled_without_teammates() {
    let mut state = TeamRosterNavigationState::default();

    assert!(!state.step_selection(TeamRosterDirection::Next, None, None));
    assert_eq!(state.selected_footer_index(), None);
    assert_eq!(state.footer_spans(None, None), None);
}

#[test]
fn stepping_reopens_collapsed_roster() {
    let mut state = TeamRosterNavigationState::default();
    let main_thread_id = thread_id(1);
    state.register_member(member(3, "alice", "%alice"));

    state.collapse_roster();
    assert_eq!(
        span_text(
            &state
                .footer_spans(Some(main_thread_id), Some(main_thread_id))
                .expect("collapsed roster still shows status")
        ),
        "1 teammate"
    );

    assert!(state.step_selection(TeamRosterDirection::Next, None, None));
    assert_eq!(state.selected_footer_index(), Some(0));
    let spans = state
        .footer_spans(Some(main_thread_id), Some(main_thread_id))
        .expect("stepping should reopen the collapsed status");
    assert_eq!(span_text(&spans), "1 teammate · Enter to view");
    let lines = state
        .roster_tree_lines(Some(main_thread_id), Some(main_thread_id))
        .expect("stepping should reopen the roster tree");
    assert_eq!(
        line_text(&lines),
        "› ├─ team-lead · Enter to view\n  ├─ @alice\n  └─ hide"
    );
}

#[test]
fn idle_and_mode_metadata_keep_footer_to_roster_items() {
    let mut state = TeamRosterNavigationState::default();
    state.register_member(
        TeamRosterMember::new(TeamRosterMemberInput {
            thread_id: thread_id(3),
            name: "alice".to_string(),
            tmux_pane_id: Some("%alice".to_string()),
            backend_type: Some("tmux".to_string()),
            color: Some("red".to_string()),
            mode: Some("plan".to_string()),
            is_active: Some(false),
            prompt: None,
        })
        .expect("pane-backed member"),
    );

    let spans = state
        .footer_spans(Some(thread_id(1)), Some(thread_id(1)))
        .expect("idle member should render");
    assert_eq!(span_text(&spans), "1 teammate");
}

#[test]
fn kill_selection_ignores_idle_teammates() {
    let mut state = TeamRosterNavigationState::default();
    state.set_active_team("Rocket".to_string());
    state.register_member(
        TeamRosterMember::new(TeamRosterMemberInput {
            thread_id: thread_id(3),
            name: "alice".to_string(),
            tmux_pane_id: Some("%alice".to_string()),
            backend_type: Some("tmux".to_string()),
            color: None,
            mode: None,
            is_active: Some(false),
            prompt: None,
        })
        .expect("pane-backed member"),
    );

    assert!(state.step_selection(TeamRosterDirection::Next, None, None));
    assert!(state.step_selection(TeamRosterDirection::Next, None, None));
    assert_eq!(state.selected_footer_index(), Some(1));
    assert_eq!(state.kill_selected_teammate(), None);
    assert_eq!(state.selected_footer_index(), Some(1));
}

#[test]
fn kill_selection_ignores_unknown_activity_teammates() {
    let mut state = TeamRosterNavigationState::default();
    state.set_active_team("Rocket".to_string());
    state.register_member(member(3, "alice", "%alice"));

    assert!(state.step_selection(TeamRosterDirection::Next, None, None));
    assert!(state.step_selection(TeamRosterDirection::Next, None, None));
    assert_eq!(state.selected_footer_index(), Some(1));
    assert_eq!(state.kill_selected_teammate(), None);
    assert_eq!(state.selected_footer_index(), Some(1));
}

#[test]
fn teammate_view_header_is_available_only_for_teammate_thread() {
    let mut state = TeamRosterNavigationState::default();
    let main_thread_id = thread_id(1);
    let alice_thread_id = thread_id(3);
    state.register_member(
        TeamRosterMember::new(TeamRosterMemberInput {
            thread_id: alice_thread_id,
            name: "alice".to_string(),
            tmux_pane_id: Some("%alice".to_string()),
            backend_type: Some("tmux".to_string()),
            color: Some("red".to_string()),
            mode: Some("plan".to_string()),
            is_active: Some(true),
            prompt: Some("Inspect issue #15 and report the proof path.".to_string()),
        })
        .expect("pane-backed member"),
    );

    assert_eq!(state.teammate_view_header(Some(main_thread_id)), None);
    let header = state
        .teammate_view_header(Some(alice_thread_id))
        .expect("teammate thread should expose view header");
    assert_eq!(header.name, "alice");
    assert_eq!(header.color.as_deref(), Some("red"));
    assert_eq!(
        header.prompt.as_deref(),
        Some("Inspect issue #15 and report the proof path.")
    );
}

#[test]
fn activating_leader_teammate_and_hide_returns_expected_action() {
    let mut state = TeamRosterNavigationState::default();
    let main_thread_id = thread_id(1);
    state.register_member(member(2, "bob", "%bob"));
    state.register_member(member(3, "alice", "%alice"));

    assert!(state.step_selection(TeamRosterDirection::Next, None, None));
    assert_eq!(
        state.activate_selection(Some(main_thread_id)),
        Some(TeamRosterSelectionAction::SelectThread(main_thread_id))
    );

    assert!(state.step_selection(TeamRosterDirection::Next, None, None));
    assert!(state.step_selection(TeamRosterDirection::Next, None, None));
    assert_eq!(
        state.activate_selection(Some(main_thread_id)),
        Some(TeamRosterSelectionAction::ViewTeammate(thread_id(3)))
    );
    assert_eq!(
        state
            .teammate_view_header(Some(main_thread_id))
            .expect("viewed teammate header")
            .name,
        "alice"
    );
    assert!(state.clear_viewed_teammate());
    assert_eq!(state.teammate_view_header(Some(main_thread_id)), None);

    assert!(state.step_selection(TeamRosterDirection::Previous, None, None));
    assert!(state.step_selection(TeamRosterDirection::Previous, None, None));
    assert_eq!(
        state.activate_selection(Some(main_thread_id)),
        Some(TeamRosterSelectionAction::CollapseRoster)
    );
    assert_eq!(
        state.footer_spans(Some(main_thread_id), Some(main_thread_id)),
        Some(vec!["2 teammates".into()])
    );
}

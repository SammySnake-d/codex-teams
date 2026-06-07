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

#[test]
fn footer_roster_is_main_plus_teammates_sorted_by_name() {
    let mut state = TeamRosterNavigationState::default();
    let main_thread_id = thread_id(1);
    let bob_thread_id = thread_id(2);
    let alice_thread_id = thread_id(3);

    state.set_active_team("Rocket".to_string());
    assert_eq!(state.footer_label(), Some("Teams: Rocket".to_string()));
    assert_eq!(
        state.footer_spans(Some(main_thread_id), Some(main_thread_id)),
        None
    );

    state.register_member(bob_thread_id, "bob".to_string(), None, None);
    state.register_member(alice_thread_id, "alice".to_string(), None, None);

    let spans = state
        .footer_spans(Some(main_thread_id), Some(main_thread_id))
        .expect("members should render footer roster");
    assert_eq!(span_text(&spans), "@main · @alice · @bob");
    assert_eq!(
        state.viewed_footer_index(Some(main_thread_id), Some(main_thread_id)),
        Some(0)
    );
    assert_eq!(
        state.viewed_footer_index(Some(alice_thread_id), Some(main_thread_id)),
        Some(1)
    );
    assert_eq!(
        state.viewed_footer_index(Some(bob_thread_id), Some(main_thread_id)),
        Some(2)
    );
}

#[test]
fn selection_cycles_through_main_sorted_teammates_and_hide_row() {
    let mut state = TeamRosterNavigationState::default();
    let main_thread_id = thread_id(1);
    let bob_thread_id = thread_id(2);
    let alice_thread_id = thread_id(3);

    state.register_member(bob_thread_id, "bob".to_string(), None, None);
    state.register_member(alice_thread_id, "alice".to_string(), None, None);

    assert!(state.step_selection(TeamRosterDirection::Next));
    assert_eq!(state.selected_footer_index(), Some(0));
    assert_eq!(
        state.selected_target(Some(main_thread_id)),
        Some(SelectedTeamRosterTarget::Thread(main_thread_id))
    );

    assert!(state.step_selection(TeamRosterDirection::Next));
    assert_eq!(state.selected_footer_index(), Some(1));
    assert_eq!(
        state.selected_target(Some(main_thread_id)),
        Some(SelectedTeamRosterTarget::Thread(alice_thread_id))
    );

    assert!(state.step_selection(TeamRosterDirection::Next));
    assert_eq!(state.selected_footer_index(), Some(2));
    assert_eq!(
        state.selected_target(Some(main_thread_id)),
        Some(SelectedTeamRosterTarget::Thread(bob_thread_id))
    );

    assert!(state.step_selection(TeamRosterDirection::Next));
    assert_eq!(state.selected_footer_index(), Some(3));
    assert_eq!(
        state.selected_target(Some(main_thread_id)),
        Some(SelectedTeamRosterTarget::Hide)
    );

    let spans = state
        .footer_spans(Some(main_thread_id), Some(main_thread_id))
        .expect("selected roster should render hide row");
    assert_eq!(span_text(&spans), "@main · @alice · @bob · hide");

    state.collapse_roster();
    assert_eq!(
        state.footer_spans(Some(main_thread_id), Some(main_thread_id)),
        None
    );

    assert!(state.step_selection(TeamRosterDirection::Next));
    assert_eq!(state.selected_footer_index(), Some(0));
    let spans = state
        .footer_spans(Some(main_thread_id), Some(main_thread_id))
        .expect("stepping should reopen the collapsed roster");
    assert_eq!(span_text(&spans), "@main · @alice · @bob · hide");

    assert!(state.step_selection(TeamRosterDirection::Next));
    assert_eq!(state.selected_footer_index(), Some(1));

    assert!(state.step_selection(TeamRosterDirection::Previous));
    assert_eq!(state.selected_footer_index(), Some(0));

    assert!(state.step_selection(TeamRosterDirection::Previous));
    assert_eq!(state.selected_footer_index(), Some(3));
    assert_eq!(
        state.selected_target(Some(main_thread_id)),
        Some(SelectedTeamRosterTarget::Hide)
    );

    state.clear_selection();
    assert_eq!(state.selected_footer_index(), None);
    assert_eq!(state.selected_target(Some(main_thread_id)), None);
}

#[test]
fn selection_is_disabled_without_teammates() {
    let mut state = TeamRosterNavigationState::default();

    assert!(!state.step_selection(TeamRosterDirection::Next));
    assert_eq!(state.selected_footer_index(), None);
    assert_eq!(state.footer_spans(None, None), None);
}

#[test]
fn pane_backed_teammates_select_pane_target() {
    let mut state = TeamRosterNavigationState::default();
    let alice_thread_id = thread_id(3);

    state.register_member(
        alice_thread_id,
        "alice".to_string(),
        Some("%9".to_string()),
        Some("tmux".to_string()),
    );

    assert!(state.step_selection(TeamRosterDirection::Next));
    assert!(state.step_selection(TeamRosterDirection::Next));
    assert_eq!(
        state.selected_target(Some(thread_id(1))),
        Some(SelectedTeamRosterTarget::Pane {
            pane_id: "%9".to_string(),
            backend_type: Some("tmux".to_string()),
        })
    );
}

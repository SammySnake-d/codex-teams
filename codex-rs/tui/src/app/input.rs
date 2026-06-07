//! Keyboard input, external editor, and status-line dispatch for the TUI app.
//!
//! This module owns global key bindings that sit above ChatWidget, including transcript overlay
//! entry, Ctrl-L clear, external editor launch, and agent navigation shortcuts.

use super::*;

const SIDE_EDIT_PREVIOUS_UNAVAILABLE_MESSAGE: &str =
    "Editing previous prompts is unavailable in side conversations.";

#[derive(Debug, Eq, PartialEq)]
enum TeamRosterKeyAction {
    Redraw,
    SwitchThread(ThreadId),
    FocusPane {
        pane_id: String,
        backend_type: Option<String>,
    },
    Consumed,
}

impl App {
    pub(super) async fn launch_external_editor(&mut self, tui: &mut tui::Tui) {
        let editor_cmd = match external_editor::resolve_editor_command() {
            Ok(cmd) => cmd,
            Err(external_editor::EditorError::MissingEditor) => {
                self.chat_widget
                    .add_to_history(history_cell::new_error_event(
                    "Cannot open external editor: set $VISUAL or $EDITOR before starting Codex."
                        .to_string(),
                ));
                self.reset_external_editor_state(tui);
                return;
            }
            Err(err) => {
                self.chat_widget
                    .add_to_history(history_cell::new_error_event(format!(
                        "Failed to open editor: {err}",
                    )));
                self.reset_external_editor_state(tui);
                return;
            }
        };

        let seed = self.chat_widget.composer_text_with_pending();
        let editor_result = tui
            .with_restored(tui::RestoreMode::KeepRaw, || async {
                external_editor::run_editor(&seed, &editor_cmd).await
            })
            .await;
        self.reset_external_editor_state(tui);

        match editor_result {
            Ok(new_text) => {
                // Trim trailing whitespace
                let cleaned = new_text.trim_end().to_string();
                self.chat_widget.apply_external_edit(cleaned);
            }
            Err(err) => {
                self.chat_widget
                    .add_to_history(history_cell::new_error_event(format!(
                        "Failed to open editor: {err}",
                    )));
            }
        }
        tui.frame_requester().schedule_frame();
    }

    pub(super) fn request_external_editor_launch(&mut self, tui: &mut tui::Tui) {
        self.chat_widget
            .set_external_editor_state(ExternalEditorState::Requested);
        self.chat_widget.set_footer_hint_override(Some(vec![(
            EXTERNAL_EDITOR_HINT.to_string(),
            String::new(),
        )]));
        tui.frame_requester().schedule_frame();
    }

    pub(super) fn reset_external_editor_state(&mut self, tui: &mut tui::Tui) {
        self.chat_widget
            .set_external_editor_state(ExternalEditorState::Closed);
        self.chat_widget.set_footer_hint_override(/*items*/ None);
        tui.frame_requester().schedule_frame();
    }

    pub(super) fn apply_raw_output_mode(
        &mut self,
        tui: &mut tui::Tui,
        enabled: bool,
        notify: bool,
    ) {
        if notify {
            self.chat_widget.set_raw_output_mode_and_notify(enabled);
        } else {
            self.chat_widget.set_raw_output_mode(enabled);
        }
        if let Err(err) = self.reflow_transcript_now(tui) {
            tracing::warn!(error = %err, "failed to reflow transcript after raw output mode toggle");
            self.chat_widget
                .add_error_message(format!("Failed to redraw transcript: {err}"));
        }
        tui.frame_requester().schedule_frame();
    }

    pub(super) async fn handle_key_event(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        key_event: KeyEvent,
    ) {
        // Some terminals, especially on macOS, encode Option+Left/Right as Option+b/f unless
        // enhanced keyboard reporting is available. We only treat those word-motion fallbacks as
        // agent-switch shortcuts when the composer is empty so we never steal the expected
        // editing behavior for moving across words inside a draft.
        let allow_agent_word_motion_fallback = !self.enhanced_keys_supported
            && self.chat_widget.composer_text_with_pending().is_empty();
        if self
            .handle_team_roster_navigation_key(tui, app_server, key_event)
            .await
        {
            return;
        }
        if self.overlay.is_none()
            && self.chat_widget.no_modal_or_popup_active()
            // Alt+Left/Right are also natural word-motion keys in the composer. Keep agent
            // fast-switch available only once the draft is empty so editing behavior wins whenever
            // there is text on screen.
            && self.chat_widget.composer_text_with_pending().is_empty()
            && previous_agent_shortcut_matches(key_event, allow_agent_word_motion_fallback)
        {
            if let Some(thread_id) = self
                .adjacent_thread_id_with_backfill(app_server, AgentNavigationDirection::Previous)
                .await
            {
                let _ = self
                    .select_agent_thread_and_discard_side(tui, app_server, thread_id)
                    .await;
            }
            return;
        }
        if self.overlay.is_none()
            && self.chat_widget.no_modal_or_popup_active()
            // Mirror the previous-agent rule above: empty drafts may use these keys for thread
            // switching, but non-empty drafts keep them for expected word-wise cursor motion.
            && self.chat_widget.composer_text_with_pending().is_empty()
            && next_agent_shortcut_matches(key_event, allow_agent_word_motion_fallback)
        {
            if let Some(thread_id) = self
                .adjacent_thread_id_with_backfill(app_server, AgentNavigationDirection::Next)
                .await
            {
                let _ = self
                    .select_agent_thread_and_discard_side(tui, app_server, thread_id)
                    .await;
            }
            return;
        }
        if side_return_shortcut_matches(key_event)
            && self.maybe_return_from_side(tui, app_server).await
        {
            return;
        }

        let app_keymap_shortcuts_available = self.app_keymap_shortcuts_available();

        if app_keymap_shortcuts_available && self.keymap.app.toggle_vim_mode.is_pressed(key_event) {
            self.chat_widget.toggle_vim_mode_and_notify();
            return;
        }

        if app_keymap_shortcuts_available
            && self.keymap.app.toggle_fast_mode.is_pressed(key_event)
            && self.chat_widget.can_toggle_fast_mode_from_keybinding()
        {
            self.chat_widget.toggle_fast_mode_from_ui();
            return;
        }

        if app_keymap_shortcuts_available && self.keymap.app.toggle_raw_output.is_pressed(key_event)
        {
            let enabled = !self.chat_widget.raw_output_mode();
            self.apply_raw_output_mode(tui, enabled, /*notify*/ false);
            return;
        }

        if app_keymap_shortcuts_available && self.keymap.app.open_transcript.is_pressed(key_event) {
            // Enter alternate screen and set viewport to full size.
            let _ = tui.enter_alt_screen();
            self.overlay = Some(Overlay::new_transcript(
                self.transcript_cells.clone(),
                self.keymap.pager.clone(),
            ));
            tui.frame_requester().schedule_frame();
            return;
        }

        if app_keymap_shortcuts_available
            && self.keymap.app.open_external_editor.is_pressed(key_event)
        {
            // Only launch the external editor if there is no overlay and the bottom pane is not in use.
            // Note that it can be launched while a task is running to enable editing while the previous turn is ongoing.
            if self.overlay.is_none()
                && self.chat_widget.can_launch_external_editor()
                && self.chat_widget.external_editor_state() == ExternalEditorState::Closed
            {
                self.request_external_editor_launch(tui);
            }
            return;
        }

        if matches!(key_event.code, KeyCode::Esc)
            && matches!(key_event.kind, KeyEventKind::Press | KeyEventKind::Repeat)
        {
            // Esc primes/advances backtracking only in normal (not working) mode
            // with the composer focused and empty. In any other state, forward
            // Esc so the active UI (e.g. status indicator, modals, popups)
            // handles it.
            if self.should_handle_backtrack_esc(key_event) {
                self.handle_backtrack_esc_key(tui);
            } else if self.should_reject_side_backtrack_esc(key_event) {
                self.reject_side_backtrack_esc();
            } else {
                self.chat_widget.handle_key_event(key_event);
            }
            return;
        }

        match key_event {
            _ if app_keymap_shortcuts_available
                && self.keymap.app.clear_terminal.is_pressed(key_event) =>
            {
                if !self.chat_widget.can_run_ctrl_l_clear_now() {
                    return;
                }
                if let Err(err) = self.clear_terminal_ui(tui, /*redraw_header*/ false) {
                    tracing::warn!(error = %err, "failed to clear terminal UI");
                    self.chat_widget
                        .add_error_message(format!("Failed to clear terminal UI: {err}"));
                } else {
                    self.reset_app_ui_state_after_clear();
                    self.queue_clear_ui_header(tui);
                    tui.frame_requester().schedule_frame();
                }
            }
            // Enter confirms backtrack when primed + count > 0. Otherwise pass to widget.
            KeyEvent {
                code: KeyCode::Enter,
                kind: KeyEventKind::Press,
                ..
            } if self.backtrack.primed
                && self.backtrack.nth_user_message != usize::MAX
                && self.chat_widget.composer_is_empty() =>
            {
                if let Some(selection) = self.confirm_backtrack_from_main() {
                    self.apply_backtrack_selection(tui, selection);
                }
            }
            KeyEvent {
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            } => {
                // Any non-Esc key press should cancel a primed backtrack.
                // This avoids stale "Esc-primed" state after the user starts typing
                // (even if they later backspace to empty).
                if key_event.code != KeyCode::Esc && self.backtrack.primed {
                    self.reset_backtrack_state();
                }
                self.chat_widget.handle_key_event(key_event);
            }
            _ => {
                self.chat_widget.handle_key_event(key_event);
            }
        };
    }

    async fn handle_team_roster_navigation_key(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        key_event: KeyEvent,
    ) -> bool {
        if self.overlay.is_some()
            || !self.chat_widget.no_modal_or_popup_active()
            || !matches!(key_event.kind, KeyEventKind::Press | KeyEventKind::Repeat)
        {
            return false;
        }

        let should_handle_vim_insert_escape = matches!(key_event.code, KeyCode::Esc)
            && self.chat_widget.should_handle_vim_insert_escape(key_event);
        if let Some(action) = apply_team_roster_navigation_key(
            &mut self.team_roster_navigation,
            key_event,
            self.primary_thread_id,
            should_handle_vim_insert_escape,
        ) {
            match action {
                TeamRosterKeyAction::Redraw => {
                    self.sync_active_agent_label();
                }
                TeamRosterKeyAction::SwitchThread(thread_id) => {
                    if self.current_displayed_thread_id() != Some(thread_id) {
                        let _ = self
                            .select_agent_thread_and_discard_side(tui, app_server, thread_id)
                            .await;
                    } else {
                        self.sync_active_agent_label();
                    }
                }
                TeamRosterKeyAction::FocusPane {
                    pane_id,
                    backend_type,
                } => {
                    self.focus_teammate_pane(&pane_id, backend_type.as_deref());
                    self.sync_active_agent_label();
                }
                TeamRosterKeyAction::Consumed => {
                    return true;
                }
            }
            tui.frame_requester().schedule_frame();
            return true;
        }

        if matches!(key_event.code, KeyCode::Esc) && !should_handle_vim_insert_escape {
            let current_thread_id = self.current_displayed_thread_id();
            if !self
                .team_roster_navigation
                .is_teammate_thread(current_thread_id)
            {
                return false;
            }
            let Some(current_thread_id) = current_thread_id else {
                return false;
            };
            if self
                .active_turn_id_for_thread(current_thread_id)
                .await
                .is_some()
            {
                return false;
            }
            let Some(primary_thread_id) = self.primary_thread_id else {
                return false;
            };
            let _ = self
                .select_agent_thread_and_discard_side(tui, app_server, primary_thread_id)
                .await;
            tui.frame_requester().schedule_frame();
            return true;
        }

        false
    }

    pub(super) fn should_handle_backtrack_esc(&self, key_event: KeyEvent) -> bool {
        !self.chat_widget.side_conversation_active()
            && self.chat_widget.is_normal_backtrack_mode()
            && self.chat_widget.composer_is_empty()
            && !self.chat_widget.should_handle_vim_insert_escape(key_event)
    }

    pub(super) fn should_reject_side_backtrack_esc(&self, key_event: KeyEvent) -> bool {
        self.chat_widget.side_conversation_active()
            && self.chat_widget.is_normal_backtrack_mode()
            && self.chat_widget.composer_is_empty()
            && !self.chat_widget.should_handle_vim_insert_escape(key_event)
    }

    pub(super) fn reject_side_backtrack_esc(&mut self) {
        self.reset_backtrack_state();
        self.chat_widget
            .add_error_message(SIDE_EDIT_PREVIOUS_UNAVAILABLE_MESSAGE.to_string());
    }

    fn app_keymap_shortcuts_available(&self) -> bool {
        self.overlay.is_none() && self.chat_widget.no_modal_or_popup_active()
    }

    pub(super) fn refresh_status_line(&mut self) {
        self.chat_widget.refresh_status_line();
    }
}

fn apply_team_roster_navigation_key(
    navigation: &mut TeamRosterNavigationState,
    key_event: KeyEvent,
    primary_thread_id: Option<ThreadId>,
    should_handle_vim_insert_escape: bool,
) -> Option<TeamRosterKeyAction> {
    if !matches!(key_event.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return None;
    }

    let is_shift = key_event.modifiers.contains(KeyModifiers::SHIFT);
    match key_event.code {
        KeyCode::Up if is_shift && navigation.step_selection(TeamRosterDirection::Previous) => {
            Some(TeamRosterKeyAction::Redraw)
        }
        KeyCode::Down if is_shift && navigation.step_selection(TeamRosterDirection::Next) => {
            Some(TeamRosterKeyAction::Redraw)
        }
        KeyCode::Enter => {
            let target = navigation.selected_target(primary_thread_id)?;
            navigation.clear_selection();
            Some(match target {
                SelectedTeamRosterTarget::Thread(thread_id) => {
                    TeamRosterKeyAction::SwitchThread(thread_id)
                }
                SelectedTeamRosterTarget::Pane {
                    pane_id,
                    backend_type,
                } => TeamRosterKeyAction::FocusPane {
                    pane_id,
                    backend_type,
                },
                SelectedTeamRosterTarget::Hide => {
                    navigation.collapse_roster();
                    TeamRosterKeyAction::Redraw
                }
            })
        }
        KeyCode::Char('f') if navigation.selected_footer_index().is_some() => {
            let target = navigation.selected_target(primary_thread_id)?;
            Some(match target {
                SelectedTeamRosterTarget::Thread(thread_id)
                    if Some(thread_id) != primary_thread_id =>
                {
                    navigation.clear_selection();
                    TeamRosterKeyAction::SwitchThread(thread_id)
                }
                SelectedTeamRosterTarget::Pane {
                    pane_id,
                    backend_type,
                } => {
                    navigation.clear_selection();
                    TeamRosterKeyAction::FocusPane {
                        pane_id,
                        backend_type,
                    }
                }
                SelectedTeamRosterTarget::Thread(_) | SelectedTeamRosterTarget::Hide => {
                    TeamRosterKeyAction::Consumed
                }
            })
        }
        KeyCode::Esc => {
            if should_handle_vim_insert_escape {
                return None;
            }
            if navigation.selected_footer_index().is_some() {
                navigation.clear_selection();
                return Some(TeamRosterKeyAction::Redraw);
            }
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::make_test_app;
    use super::*;
    use pretty_assertions::assert_eq;

    fn thread_id(suffix: u128) -> ThreadId {
        ThreadId::from_string(&format!("00000000-0000-0000-0000-{suffix:012}"))
            .expect("valid thread id")
    }

    fn roster_with_two_members() -> (TeamRosterNavigationState, ThreadId, ThreadId, ThreadId) {
        let mut navigation = TeamRosterNavigationState::default();
        let main_thread_id = thread_id(1);
        let bob_thread_id = thread_id(2);
        let alice_thread_id = thread_id(3);
        navigation.register_member(bob_thread_id, "bob".to_string(), None, None);
        navigation.register_member(alice_thread_id, "alice".to_string(), None, None);
        (navigation, main_thread_id, alice_thread_id, bob_thread_id)
    }

    #[tokio::test]
    async fn app_keymap_shortcuts_are_disabled_while_keymap_view_is_active() {
        let mut app = make_test_app().await;
        assert!(app.app_keymap_shortcuts_available());

        let keymap = app.keymap.clone();
        app.chat_widget.open_keymap_debug(&keymap);

        assert!(!app.app_keymap_shortcuts_available());
    }

    #[test]
    fn team_roster_shift_down_selects_main_footer_target() {
        let (mut navigation, main_thread_id, _, _) = roster_with_two_members();

        let action = apply_team_roster_navigation_key(
            &mut navigation,
            KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT),
            Some(main_thread_id),
            /*should_handle_vim_insert_escape*/ false,
        );

        assert_eq!(action, Some(TeamRosterKeyAction::Redraw));
        assert_eq!(navigation.selected_footer_index(), Some(0));
        assert_eq!(
            navigation.selected_target(Some(main_thread_id)),
            Some(SelectedTeamRosterTarget::Thread(main_thread_id))
        );
    }

    #[test]
    fn team_roster_shift_up_wraps_from_main_to_hide() {
        let (mut navigation, main_thread_id, _, _) = roster_with_two_members();

        assert_eq!(
            apply_team_roster_navigation_key(
                &mut navigation,
                KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT),
                Some(main_thread_id),
                /*should_handle_vim_insert_escape*/ false,
            ),
            Some(TeamRosterKeyAction::Redraw)
        );
        assert_eq!(navigation.selected_footer_index(), Some(0));

        assert_eq!(
            apply_team_roster_navigation_key(
                &mut navigation,
                KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT),
                Some(main_thread_id),
                /*should_handle_vim_insert_escape*/ false,
            ),
            Some(TeamRosterKeyAction::Redraw)
        );
        assert_eq!(
            navigation.selected_target(Some(main_thread_id)),
            Some(SelectedTeamRosterTarget::Hide)
        );
    }

    #[test]
    fn team_roster_enter_on_hide_collapses_roster() {
        let (mut navigation, main_thread_id, _, _) = roster_with_two_members();
        for _ in 0..4 {
            apply_team_roster_navigation_key(
                &mut navigation,
                KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT),
                Some(main_thread_id),
                /*should_handle_vim_insert_escape*/ false,
            );
        }
        assert_eq!(
            navigation.selected_target(Some(main_thread_id)),
            Some(SelectedTeamRosterTarget::Hide)
        );

        let action = apply_team_roster_navigation_key(
            &mut navigation,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            Some(main_thread_id),
            /*should_handle_vim_insert_escape*/ false,
        );

        assert_eq!(action, Some(TeamRosterKeyAction::Redraw));
        assert_eq!(navigation.selected_footer_index(), None);
        assert_eq!(
            navigation.footer_spans(Some(main_thread_id), Some(main_thread_id)),
            None
        );
    }

    #[test]
    fn team_roster_esc_clears_selection_before_backtrack() {
        let (mut navigation, main_thread_id, _, _) = roster_with_two_members();
        apply_team_roster_navigation_key(
            &mut navigation,
            KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT),
            Some(main_thread_id),
            /*should_handle_vim_insert_escape*/ false,
        );

        let action = apply_team_roster_navigation_key(
            &mut navigation,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            Some(main_thread_id),
            /*should_handle_vim_insert_escape*/ false,
        );

        assert_eq!(action, Some(TeamRosterKeyAction::Redraw));
        assert_eq!(navigation.selected_footer_index(), None);
    }

    #[test]
    fn team_roster_enter_on_teammate_returns_thread_switch_action() {
        let (mut navigation, main_thread_id, alice_thread_id, _) = roster_with_two_members();
        apply_team_roster_navigation_key(
            &mut navigation,
            KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT),
            Some(main_thread_id),
            /*should_handle_vim_insert_escape*/ false,
        );
        apply_team_roster_navigation_key(
            &mut navigation,
            KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT),
            Some(main_thread_id),
            /*should_handle_vim_insert_escape*/ false,
        );

        let action = apply_team_roster_navigation_key(
            &mut navigation,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            Some(main_thread_id),
            /*should_handle_vim_insert_escape*/ false,
        );

        assert_eq!(
            action,
            Some(TeamRosterKeyAction::SwitchThread(alice_thread_id))
        );
        assert_eq!(navigation.selected_footer_index(), None);
    }
}

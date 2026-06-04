use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use std::collections::BTreeMap;

fn create_collab_input_items_schema() -> JsonSchema {
    let byte_range_properties = BTreeMap::from([
        (
            "start".to_string(),
            JsonSchema::integer(Some("Start byte offset, inclusive.".to_string())),
        ),
        (
            "end".to_string(),
            JsonSchema::integer(Some("End byte offset, exclusive.".to_string())),
        ),
    ]);
    let text_element_properties = BTreeMap::from([
        (
            "byte_range".to_string(),
            JsonSchema::object(
                byte_range_properties,
                Some(vec!["start".to_string(), "end".to_string()]),
                Some(false.into()),
            ),
        ),
        (
            "placeholder".to_string(),
            JsonSchema::string(Some(
                "Optional display placeholder for the byte-range text element.".to_string(),
            )),
        ),
    ]);
    let properties = BTreeMap::from([
        (
            "type".to_string(),
            JsonSchema::string(Some(
                "Input item type: text, image, local_image, skill, or mention.".to_string(),
            )),
        ),
        (
            "text".to_string(),
            JsonSchema::string(Some("Text content when type is text.".to_string())),
        ),
        (
            "text_elements".to_string(),
            JsonSchema::array(
                JsonSchema::object(
                    text_element_properties,
                    Some(vec!["byte_range".to_string()]),
                    Some(false.into()),
                ),
                Some("Optional UI-defined spans within text for structured text items.".to_string()),
            ),
        ),
        (
            "image_url".to_string(),
            JsonSchema::string(Some("Image URL when type is image.".to_string())),
        ),
        (
            "path".to_string(),
            JsonSchema::string(Some(
                "Path when type is local_image/skill, or mention target such as app://<connector-id> when type is mention."
                    .to_string(),
            )),
        ),
        (
            "name".to_string(),
            JsonSchema::string(Some("Display name when type is skill or mention.".to_string())),
        ),
    ]);

    JsonSchema::array(
        JsonSchema::object(properties, /*required*/ None, Some(false.into())),
        Some(
            "Structured input items. Use this to pass explicit mentions (for example app:// connector paths)."
                .to_string(),
        ),
    )
}

fn function_tool(
    name: &str,
    description: &str,
    properties: BTreeMap<String, JsonSchema>,
    required: Option<Vec<String>>,
) -> ToolSpec {
    ToolSpec::Function(ResponsesApiTool {
        name: name.to_string(),
        description: description.to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, required, Some(false.into())),
        output_schema: None,
    })
}

pub(crate) fn create_team_create_tool() -> ToolSpec {
    function_tool(
        "create_team",
        "Create a live Codex team registry entry before spawning teammates. Use this when the user asks for an agent team or parallel teammates.",
        BTreeMap::from([(
            "name".to_string(),
            JsonSchema::string(Some("Human-readable team name.".to_string())),
        )]),
        Some(vec!["name".to_string()]),
    )
}

pub(crate) fn create_team_list_tool() -> ToolSpec {
    function_tool(
        "list_teams",
        "List caller-visible live-session-only Codex teams and their current member/task summaries.",
        BTreeMap::new(),
        None,
    )
}

pub(crate) fn create_team_status_tool() -> ToolSpec {
    function_tool(
        "team_status",
        "Return one team's members, messages, tasks, and event feed.",
        BTreeMap::from([(
            "team_id".to_string(),
            JsonSchema::string(Some("Team id from create_team or list_teams.".to_string())),
        )]),
        Some(vec!["team_id".to_string()]),
    )
}

pub(crate) fn create_team_spawn_member_tool() -> ToolSpec {
    function_tool(
        "team_spawn_member",
        "Team-lead-only tool. Spawn one teammate in an existing team using the existing Codex agent lifecycle, with generic Teams context prepended to the spawn prompt.",
        BTreeMap::from([
            (
                "team_id".to_string(),
                JsonSchema::string(Some("Team id from create_team.".to_string())),
            ),
            (
                "name".to_string(),
                JsonSchema::string(Some("Human-readable teammate name.".to_string())),
            ),
            (
                "profile".to_string(),
                JsonSchema::string(Some(
                    "Optional generic capability/profile note. Do not encode workflow policy here."
                        .to_string(),
                )),
            ),
            (
                "capabilities".to_string(),
                JsonSchema::array(
                    JsonSchema::string(None),
                    Some("Optional generic capability labels attached to this member.".to_string()),
                ),
            ),
            (
                "permissions".to_string(),
                JsonSchema::array(
                    JsonSchema::string(None),
                    Some("Optional generic permission labels attached to this member.".to_string()),
                ),
            ),
            (
                "message".to_string(),
                JsonSchema::string(Some(
                    "Initial plain-text spawn prompt for the teammate. Use either message or items; Teams will prepend generic team/member context."
                        .to_string(),
                )),
            ),
            ("items".to_string(), create_collab_input_items_schema()),
        ]),
        Some(vec!["team_id".to_string(), "name".to_string()]),
    )
}

pub(crate) fn create_team_send_tool() -> ToolSpec {
    function_tool(
        "team_send",
        "Send a team message to a member agent or record a message to the team lead mailbox.",
        BTreeMap::from([
            (
                "team_id".to_string(),
                JsonSchema::string(Some("Team id from create_team.".to_string())),
            ),
            (
                "target".to_string(),
                JsonSchema::string(Some(
                    "Optional target endpoint: member or lead. Defaults to member.".to_string(),
                )),
            ),
            (
                "member_id".to_string(),
                JsonSchema::string(Some(
                    "Member id from team_spawn_member or team_status. Required when target is member; omit when target is lead."
                        .to_string(),
                )),
            ),
            (
                "sender_member_id".to_string(),
                JsonSchema::string(Some(
                    "Optional member id to record as sender. Omit for the team lead.".to_string(),
                )),
            ),
            (
                "delivery_mode".to_string(),
                JsonSchema::string(Some("Optional delivery mode: queue or interrupt.".to_string())),
            ),
            (
                "message".to_string(),
                JsonSchema::string(Some(
                    "Plain-text message to route to the target endpoint. Omit sender_member_id for lead-originated messages; set sender_member_id for member-originated messages. Use either message or items. Member-targeted delivery prepends a Teams message envelope."
                        .to_string(),
                )),
            ),
            ("items".to_string(), create_collab_input_items_schema()),
        ]),
        Some(vec!["team_id".to_string()]),
    )
}

pub(crate) fn create_team_message_list_tool() -> ToolSpec {
    function_tool(
        "team_message_list",
        "List one team's routed messages, including the lead mailbox.",
        BTreeMap::from([
            (
                "team_id".to_string(),
                JsonSchema::string(Some("Team id from create_team or list_teams.".to_string())),
            ),
            (
                "target".to_string(),
                JsonSchema::string(Some(
                    "Optional target filter: all, lead, or member. Defaults to all.".to_string(),
                )),
            ),
            (
                "member_id".to_string(),
                JsonSchema::string(Some(
                    "Optional member id filter for member-targeted messages.".to_string(),
                )),
            ),
        ]),
        Some(vec!["team_id".to_string()]),
    )
}

pub(crate) fn create_team_task_create_tool() -> ToolSpec {
    function_tool(
        "team_task_create",
        "Create a generic shared task-board item for a Codex team.",
        BTreeMap::from([
            (
                "team_id".to_string(),
                JsonSchema::string(Some("Team id from create_team or list_teams.".to_string())),
            ),
            (
                "title".to_string(),
                JsonSchema::string(Some(
                    "Generic task title for the team's shared task board.".to_string(),
                )),
            ),
            (
                "assignee_member_id".to_string(),
                JsonSchema::string(Some(
                    "Optional member id from team_spawn_member or team_status.".to_string(),
                )),
            ),
            (
                "dependencies".to_string(),
                JsonSchema::array(
                    JsonSchema::string(None),
                    Some("Optional task ids this task depends on.".to_string()),
                ),
            ),
            (
                "note".to_string(),
                JsonSchema::string(Some("Optional generic task note.".to_string())),
            ),
        ]),
        Some(vec!["team_id".to_string(), "title".to_string()]),
    )
}

pub(crate) fn create_team_task_update_tool() -> ToolSpec {
    function_tool(
        "team_task_update",
        "Update a generic shared task-board item without encoding workflow policy.",
        BTreeMap::from([
            (
                "team_id".to_string(),
                JsonSchema::string(Some("Team id from create_team or list_teams.".to_string())),
            ),
            (
                "task_id".to_string(),
                JsonSchema::string(Some(
                    "Task id from team_task_create, team_task_list, or team_status.".to_string(),
                )),
            ),
            (
                "title".to_string(),
                JsonSchema::string(Some("Optional replacement task title.".to_string())),
            ),
            (
                "assignee_member_id".to_string(),
                JsonSchema::string(Some(
                    "Optional member id from team_spawn_member or team_status.".to_string(),
                )),
            ),
            (
                "clear_assignee".to_string(),
                JsonSchema::boolean(Some(
                    "When true, remove the current task assignee. Do not combine with assignee_member_id, and do not clear the assignee from a claimed task."
                        .to_string(),
                )),
            ),
            (
                "dependencies".to_string(),
                JsonSchema::array(
                    JsonSchema::string(None),
                    Some("Optional replacement list of dependency task ids.".to_string()),
                ),
            ),
            (
                "status".to_string(),
                JsonSchema::string(Some(
                    "Optional status: open, completed, or blocked. Use team_task_claim to set claimed."
                        .to_string(),
                )),
            ),
            (
                "note".to_string(),
                JsonSchema::string(Some("Optional replacement generic task note.".to_string())),
            ),
            (
                "clear_note".to_string(),
                JsonSchema::boolean(Some(
                    "When true, remove the current task note. Do not combine with note."
                        .to_string(),
                )),
            ),
        ]),
        Some(vec!["team_id".to_string(), "task_id".to_string()]),
    )
}

pub(crate) fn create_team_task_claim_tool() -> ToolSpec {
    function_tool(
        "team_task_claim",
        "Claim one open shared task-board item for a team member after dependency checks.",
        BTreeMap::from([
            (
                "team_id".to_string(),
                JsonSchema::string(Some("Team id from create_team or list_teams.".to_string())),
            ),
            (
                "task_id".to_string(),
                JsonSchema::string(Some(
                    "Task id from team_task_create, team_task_list, or team_status.".to_string(),
                )),
            ),
            (
                "member_id".to_string(),
                JsonSchema::string(Some(
                    "Member id from team_spawn_member or team_status.".to_string(),
                )),
            ),
        ]),
        Some(vec![
            "team_id".to_string(),
            "task_id".to_string(),
            "member_id".to_string(),
        ]),
    )
}

pub(crate) fn create_team_task_list_tool() -> ToolSpec {
    function_tool(
        "team_task_list",
        "List one team's shared task-board items.",
        BTreeMap::from([(
            "team_id".to_string(),
            JsonSchema::string(Some("Team id from create_team or list_teams.".to_string())),
        )]),
        Some(vec!["team_id".to_string()]),
    )
}

pub(crate) fn create_team_event_list_tool() -> ToolSpec {
    function_tool(
        "team_event_list",
        "List one team's append-only lifecycle, message, and task events.",
        BTreeMap::from([(
            "team_id".to_string(),
            JsonSchema::string(Some("Team id from create_team or list_teams.".to_string())),
        )]),
        Some(vec!["team_id".to_string()]),
    )
}

pub(crate) fn create_team_member_stop_tool() -> ToolSpec {
    function_tool(
        "team_member_stop",
        "Team-lead-only tool. Stop one teammate agent while keeping the live team readable and active.",
        BTreeMap::from([
            (
                "team_id".to_string(),
                JsonSchema::string(Some("Team id from create_team or list_teams.".to_string())),
            ),
            (
                "member_id".to_string(),
                JsonSchema::string(Some(
                    "Member id from team_spawn_member or team_status.".to_string(),
                )),
            ),
        ]),
        Some(vec!["team_id".to_string(), "member_id".to_string()]),
    )
}

pub(crate) fn create_team_stop_tool() -> ToolSpec {
    function_tool(
        "team_stop",
        "Team-lead-only tool. Stop a live Codex team and shut down its active teammate agents.",
        BTreeMap::from([(
            "team_id".to_string(),
            JsonSchema::string(Some("Team id from create_team or list_teams.".to_string())),
        )]),
        Some(vec!["team_id".to_string()]),
    )
}

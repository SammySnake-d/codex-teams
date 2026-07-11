use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use std::collections::BTreeMap;

const CODEX_TEAM_CREATE_DESCRIPTION: &str =
    "Create a new Codex Teams workspace for coordinating named teammates";
const CODEX_TEAM_SEND_DESCRIPTION: &str =
    "Send a message to a Codex Teams teammate or the team lead";
const CLAUDE_TEAM_CREATE_DESCRIPTION: &str = "Create a new Codex Teams workspace and task list. After TeamCreate, use Task tools for the team task board, spawn teammates through spawn_agent with name and optional team_name, and coordinate with SendMessage. Teammate messages are delivered automatically and queued while the receiver is busy.";
const CLAUDE_SEND_MESSAGE_DESCRIPTION: &str = "Send a message to a named teammate. Plain assistant text is not visible to teammates or the lead; use SendMessage for Teams communication. Use bare teammate names, team-lead, or \"*\" for broadcast. Messages are delivered automatically and queued while the receiver is busy.";
const CLAUDE_TASK_CREATE_DESCRIPTION: &str = "Create a task in the Codex Teams task list";
const CLAUDE_TASK_UPDATE_DESCRIPTION: &str = "Update a task in the Codex Teams task list";
const CLAUDE_TASK_LIST_DESCRIPTION: &str = "List all tasks in the Codex Teams task list";
const CLAUDE_TASK_GET_DESCRIPTION: &str = "Get a Codex Teams task by ID";

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

fn metadata_schema() -> JsonSchema {
    JsonSchema::object(BTreeMap::new(), /*required*/ None, Some(true.into()))
}

fn string_enum_schema(values: &[&str], description: &str) -> JsonSchema {
    JsonSchema::string_enum(
        values
            .iter()
            .map(|value| serde_json::json!(value))
            .collect(),
        Some(description.to_string()),
    )
}

fn structured_message_schema() -> JsonSchema {
    let shutdown_request = JsonSchema::object(
        BTreeMap::from([
            (
                "type".to_string(),
                string_enum_schema(&["shutdown_request"], "Structured message discriminator."),
            ),
            (
                "reason".to_string(),
                JsonSchema::string(Some("Optional shutdown reason.".to_string())),
            ),
        ]),
        Some(vec!["type".to_string()]),
        Some(false.into()),
    );
    let shutdown_response = JsonSchema::object(
        BTreeMap::from([
            (
                "type".to_string(),
                string_enum_schema(&["shutdown_response"], "Structured message discriminator."),
            ),
            (
                "request_id".to_string(),
                JsonSchema::string(Some("Request id from the shutdown request.".to_string())),
            ),
            (
                "approve".to_string(),
                JsonSchema::boolean(Some("Whether to approve shutdown.".to_string())),
            ),
            (
                "reason".to_string(),
                JsonSchema::string(Some("Reason when rejecting shutdown.".to_string())),
            ),
        ]),
        Some(vec![
            "type".to_string(),
            "request_id".to_string(),
            "approve".to_string(),
        ]),
        Some(false.into()),
    );
    let plan_approval_response = JsonSchema::object(
        BTreeMap::from([
            (
                "type".to_string(),
                string_enum_schema(
                    &["plan_approval_response"],
                    "Structured message discriminator.",
                ),
            ),
            (
                "request_id".to_string(),
                JsonSchema::string(Some(
                    "Request id from the plan approval request.".to_string(),
                )),
            ),
            (
                "approve".to_string(),
                JsonSchema::boolean(Some("Whether to approve the plan.".to_string())),
            ),
            (
                "feedback".to_string(),
                JsonSchema::string(Some("Feedback when rejecting the plan.".to_string())),
            ),
        ]),
        Some(vec![
            "type".to_string(),
            "request_id".to_string(),
            "approve".to_string(),
        ]),
        Some(false.into()),
    );
    JsonSchema::any_of(
        vec![
            JsonSchema::string(Some("Plain text message content.".to_string())),
            shutdown_request,
            shutdown_response,
            plan_approval_response,
        ],
        Some("Plain text or structured swarm protocol message.".to_string()),
    )
}

pub(crate) fn create_team_create_tool() -> ToolSpec {
    function_tool(
        "create_team",
        CODEX_TEAM_CREATE_DESCRIPTION,
        BTreeMap::from([
            (
                "team_name".to_string(),
                JsonSchema::string(Some("Name for the new team to create.".to_string())),
            ),
            (
                "description".to_string(),
                JsonSchema::string(Some("Team description/purpose.".to_string())),
            ),
            (
                "agent_type".to_string(),
                JsonSchema::string(Some(
                    "Type/role of the team lead (e.g., \"researcher\", \"test-runner\"). Used for the team file and teammate coordination."
                        .to_string(),
                )),
            ),
            (
                "name".to_string(),
                JsonSchema::string(Some(
                    "Deprecated alias for team_name kept for existing Codex callers.".to_string(),
                )),
            ),
        ]),
        Some(vec!["team_name".to_string()]),
    )
}

pub(crate) fn create_claude_team_create_tool() -> ToolSpec {
    function_tool(
        "TeamCreate",
        CLAUDE_TEAM_CREATE_DESCRIPTION,
        BTreeMap::from([
            (
                "team_name".to_string(),
                JsonSchema::string(Some("Name for the new team to create.".to_string())),
            ),
            (
                "description".to_string(),
                JsonSchema::string(Some("Team description/purpose.".to_string())),
            ),
            (
                "agent_type".to_string(),
                JsonSchema::string(Some(
                    "Type/role of the team lead (e.g., \"researcher\", \"test-runner\"). Used for the team file and teammate coordination."
                        .to_string(),
                )),
            ),
        ]),
        Some(vec!["team_name".to_string()]),
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
        "Launch a new teammate in an existing team.",
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
                    "Initial plain-text task for the teammate. Use either message or items. For split-pane teammates, this exact task text is delivered as the teammate's first mailbox turn; Teams identity is supplied by the teammate launch context, not by prepending visible metadata."
                        .to_string(),
                )),
            ),
            ("items".to_string(), create_collab_input_items_schema()),
        ]),
        Some(vec!["team_id".to_string(), "name".to_string()]),
    )
}

#[cfg(test)]
#[path = "team_spec_tests.rs"]
mod tests;

pub(crate) fn create_team_send_tool() -> ToolSpec {
    function_tool(
        "team_send",
        CODEX_TEAM_SEND_DESCRIPTION,
        BTreeMap::from([
            (
                "team_id".to_string(),
                JsonSchema::string(Some(
                    "Team id from create_team. Required when the team lead calls team_send; a spawned teammate process may omit it because its team is resolved from launch context."
                        .to_string(),
                )),
            ),
            (
                "to".to_string(),
                JsonSchema::string(Some(
                    "Recipient: teammate name, \"*\" for broadcast to all teammates, or team-lead."
                        .to_string(),
                )),
            ),
            (
                "summary".to_string(),
                JsonSchema::string(Some(
                    "Short message summary for UI notifications when message is plain text."
                        .to_string(),
                )),
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
                "member_name".to_string(),
                JsonSchema::string(Some(
                    "Display name of a split-pane teammate to message via its mailbox; prefer the Claude-compatible to field for new calls."
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
                    "Plain-text message to route to the target endpoint. Your plain text output is NOT visible to other teammates; use this tool to communicate. Refer to teammates by name, never by UUID. Use either message or items."
                        .to_string(),
                )),
            ),
            (
                "kind".to_string(),
                JsonSchema::string(Some(
                    "Optional message intent for receiver arbitration: \"correction\" (redirect the recipient's work — preempts routine discussion), \"report\" (a finding/observation), \"progress\" (a status update), or \"discussion\" (brainstorming, lowest priority). Use \"correction\" when you are a reviewer telling an agent it has drifted; the system decides how much authority your correction carries from your identity. Omit for ordinary chatter."
                        .to_string(),
                )),
            ),
            ("items".to_string(), create_collab_input_items_schema()),
        ]),
        None,
    )
}

pub(crate) fn create_claude_send_message_tool() -> ToolSpec {
    function_tool(
        "SendMessage",
        CLAUDE_SEND_MESSAGE_DESCRIPTION,
        BTreeMap::from([
            (
                "to".to_string(),
                JsonSchema::string(Some(
                    "Recipient: teammate name, team-lead, or \"*\" for broadcast to all teammates."
                        .to_string(),
                )),
            ),
            (
                "summary".to_string(),
                JsonSchema::string(Some(
                    "A 5-10 word summary shown as a preview in the UI (required when message is a string)."
                        .to_string(),
                )),
            ),
            ("message".to_string(), structured_message_schema()),
        ]),
        Some(vec!["to".to_string(), "message".to_string()]),
    )
}

pub(crate) fn create_team_message_list_tool() -> ToolSpec {
    function_tool(
        "team_message_list",
        "List one team's routed messages, including the lead mailbox.",
        BTreeMap::from([
            (
                "team_id".to_string(),
                JsonSchema::string(Some(
                    "Team id from create_team or list_teams. A spawned teammate process may omit it because its team is resolved from launch context."
                        .to_string(),
                )),
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
                JsonSchema::string(Some(
                    "Team id from create_team or list_teams. A spawned teammate process may omit it because its team is resolved from launch context."
                        .to_string(),
                )),
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
        Some(vec!["title".to_string()]),
    )
}

pub(crate) fn create_claude_task_create_tool() -> ToolSpec {
    function_tool(
        "TaskCreate",
        CLAUDE_TASK_CREATE_DESCRIPTION,
        BTreeMap::from([
            (
                "subject".to_string(),
                JsonSchema::string(Some("A brief title for the task.".to_string())),
            ),
            (
                "description".to_string(),
                JsonSchema::string(Some("What needs to be done.".to_string())),
            ),
            (
                "activeForm".to_string(),
                JsonSchema::string(Some(
                    "Present continuous form shown in spinner when in_progress (e.g., \"Running tests\")."
                        .to_string(),
                )),
            ),
            ("metadata".to_string(), metadata_schema()),
        ]),
        Some(vec!["subject".to_string(), "description".to_string()]),
    )
}

pub(crate) fn create_team_task_update_tool() -> ToolSpec {
    function_tool(
        "team_task_update",
        "Update a generic shared task-board item without encoding workflow policy.",
        BTreeMap::from([
            (
                "team_id".to_string(),
                JsonSchema::string(Some(
                    "Team id from create_team or list_teams. A spawned teammate process may omit it because its team is resolved from launch context."
                        .to_string(),
                )),
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
        Some(vec!["task_id".to_string()]),
    )
}

pub(crate) fn create_claude_task_update_tool() -> ToolSpec {
    function_tool(
        "TaskUpdate",
        CLAUDE_TASK_UPDATE_DESCRIPTION,
        BTreeMap::from([
            (
                "taskId".to_string(),
                JsonSchema::string(Some("The ID of the task to update.".to_string())),
            ),
            (
                "subject".to_string(),
                JsonSchema::string(Some("New subject for the task.".to_string())),
            ),
            (
                "description".to_string(),
                JsonSchema::string(Some("New description for the task.".to_string())),
            ),
            (
                "activeForm".to_string(),
                JsonSchema::string(Some(
                    "Present continuous form shown in spinner when in_progress (e.g., \"Running tests\")."
                        .to_string(),
                )),
            ),
            (
                "status".to_string(),
                string_enum_schema(
                    &["pending", "in_progress", "completed", "deleted"],
                    "New status for the task.",
                ),
            ),
            (
                "addBlocks".to_string(),
                JsonSchema::array(
                    JsonSchema::string(None),
                    Some("Task IDs that this task blocks.".to_string()),
                ),
            ),
            (
                "addBlockedBy".to_string(),
                JsonSchema::array(
                    JsonSchema::string(None),
                    Some("Task IDs that block this task.".to_string()),
                ),
            ),
            (
                "owner".to_string(),
                JsonSchema::string(Some("New owner for the task.".to_string())),
            ),
            ("metadata".to_string(), metadata_schema()),
        ]),
        Some(vec!["taskId".to_string()]),
    )
}

pub(crate) fn create_team_task_claim_tool() -> ToolSpec {
    function_tool(
        "team_task_claim",
        "Claim one open shared task-board item for a team member after dependency checks.",
        BTreeMap::from([
            (
                "team_id".to_string(),
                JsonSchema::string(Some(
                    "Team id from create_team or list_teams. A spawned teammate process may omit it because its team is resolved from launch context."
                        .to_string(),
                )),
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
                    "Member id from team_spawn_member or team_status. A spawned teammate process may omit it to claim as itself."
                        .to_string(),
                )),
            ),
        ]),
        Some(vec!["task_id".to_string()]),
    )
}

pub(crate) fn create_team_task_list_tool() -> ToolSpec {
    function_tool(
        "team_task_list",
        "List one team's shared task-board items.",
        BTreeMap::from([(
            "team_id".to_string(),
            JsonSchema::string(Some(
                "Team id from create_team or list_teams. A spawned teammate process may omit it because its team is resolved from launch context."
                    .to_string(),
            )),
        )]),
        None,
    )
}

pub(crate) fn create_claude_task_list_tool() -> ToolSpec {
    function_tool(
        "TaskList",
        CLAUDE_TASK_LIST_DESCRIPTION,
        BTreeMap::new(),
        Some(Vec::new()),
    )
}

pub(crate) fn create_claude_task_get_tool() -> ToolSpec {
    function_tool(
        "TaskGet",
        CLAUDE_TASK_GET_DESCRIPTION,
        BTreeMap::from([(
            "taskId".to_string(),
            JsonSchema::string(Some("The ID of the task to retrieve.".to_string())),
        )]),
        Some(vec!["taskId".to_string()]),
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
        "Team-lead-only tool. Stop one teammate process while keeping the live team readable and active.",
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
        "Team-lead-only tool. Stop a live Codex team and shut down its active teammate processes.",
        BTreeMap::from([(
            "team_id".to_string(),
            JsonSchema::string(Some("Team id from create_team or list_teams.".to_string())),
        )]),
        Some(vec!["team_id".to_string()]),
    )
}

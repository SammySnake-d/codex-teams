use super::member_to_lead_send_succeeded;
use pretty_assertions::assert_eq;

#[test]
fn member_to_lead_send_succeeded_accepts_legacy_message_shape() {
    let output = r#"{
        "message": {
            "content": "Member-to-lead smoke acknowledgement from local mock Responses provider.",
            "target": {"lead": true}
        }
    }"#;

    assert_eq!(member_to_lead_send_succeeded(output), true);
}

#[test]
fn member_to_lead_send_succeeded_accepts_claude_send_message_envelope() {
    let output = r#"{
        "success": true,
        "message": "Message sent to team-lead's inbox",
        "routing": {
            "sender": "mock-member",
            "target": "@team-lead",
            "summary": "smoke acknowledgement",
            "content": "Member-to-lead smoke acknowledgement from local mock Responses provider."
        }
    }"#;

    assert_eq!(member_to_lead_send_succeeded(output), true);
}

#[test]
fn member_to_lead_send_succeeded_rejects_failed_claude_send_message_envelope() {
    let output = r#"{
        "success": false,
        "message": "Message not sent to team-lead's inbox",
        "routing": {
            "sender": "mock-member",
            "target": "@team-lead",
            "content": "Member-to-lead smoke acknowledgement from local mock Responses provider."
        }
    }"#;

    assert_eq!(member_to_lead_send_succeeded(output), false);
}

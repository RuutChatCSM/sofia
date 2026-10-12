use super::*;
use crate::tools::router::ToolRouter;
use pretty_assertions::assert_eq;
use sofia_protocol::models::AgentMessageInputContent;
use sofia_protocol::models::ResponseItem;

#[test]
fn collaboration_task_payload_reaches_model_as_plaintext_on_chat_completions() {
    let author = AgentPath::root();
    let recipient = author.join("architecture").expect("recipient path");
    for (name, trigger_turn) in [
        ("spawn_agent", true),
        ("followup_task", true),
        ("send_message", false),
    ] {
        for marker in [Some(Vec::new()), None, Some(vec!["ciphertext".to_string()])] {
            let plaintext = marker.as_ref().is_some_and(Vec::is_empty);
            let call = ToolRouter::build_tool_call(ResponseItem::FunctionCall {
                id: None,
                name: name.to_string(),
                namespace: None,
                arguments: "{}".to_string(),
                encrypted_function_args: marker,
                call_id: "task-message".to_string(),
                internal_chat_message_metadata_passthrough: None,
            })
            .unwrap()
            .unwrap();
            let communication = communication_from_tool_message(
                author.clone(),
                recipient.clone(),
                "Inspect architecture read-only".to_string(),
                &call.direct_source(),
                trigger_turn,
            );
            let ResponseItem::AgentMessage { content, .. } = communication.to_model_input_item()
            else {
                panic!("expected agent message");
            };
            if plaintext {
                let message_type = if trigger_turn { "NEW_TASK" } else { "MESSAGE" };
                assert_eq!(
                    content,
                    vec![AgentMessageInputContent::InputText {
                        text: format!(
                            "Message Type: {message_type}\nTask name: /root/architecture\nSender: /root\nPayload:\nInspect architecture read-only"
                        ),
                    }]
                );
            } else {
                assert_eq!(
                    communication.encrypted_content.as_deref(),
                    Some("Inspect architecture read-only")
                );
                assert!(matches!(
                    content.last(),
                    Some(AgentMessageInputContent::EncryptedContent { .. })
                ));
            }
        }
    }
}

use crate::client::strip_responses_only_fields;
use crate::client_common::Prompt;
use sofia_protocol::error::Result;
use sofia_protocol::models::ContentItem;
use sofia_protocol::models::FunctionCallOutputContentItem;
use sofia_protocol::models::FunctionCallOutputPayload;
use sofia_protocol::models::ResponseItem;
use sofia_protocol::openai_models::ModelInfo;
use sofia_protocol::openai_models::ReasoningEffort as ReasoningEffortConfig;
use std::collections::HashSet;
use tracing::debug;
use tracing::warn;

/// Anthropic has no freeform/custom tool type, so freeform tools are bridged as
/// function tools with a single `input` string. The SSE decoder needs this set
/// to unwrap a matching `tool_use` block back into a `CustomToolCall`.
pub(crate) fn anthropic_freeform_tool_names(prompt: &Prompt) -> HashSet<String> {
    let mut names = HashSet::new();
    for tool in prompt.tools.iter() {
        match tool {
            sofia_tools::ToolSpec::Freeform(freeform) => {
                names.insert(freeform.name.clone());
            }
            sofia_tools::ToolSpec::Namespace(namespace) => {
                for inner in &namespace.tools {
                    if let sofia_tools::ResponsesApiNamespaceTool::Custom(custom) = inner {
                        names.insert(sofia_tools::flat_namespace_tool_name(
                            &namespace.name,
                            &custom.name,
                        ));
                    }
                }
            }
            sofia_tools::ToolSpec::Function(_)
            | sofia_tools::ToolSpec::ToolSearch { .. }
            | sofia_tools::ToolSpec::WebSearch { .. } => {}
        }
    }
    names
}

/// Append content blocks as a message, merging into the previous message when
/// the roles match, keeping tool-use history in one assistant turn. The
/// engine emits tool calls and their history as separate items.
fn push_anthropic_message(
    messages: &mut Vec<serde_json::Value>,
    role: &str,
    mut blocks: Vec<serde_json::Value>,
) {
    if blocks.is_empty() {
        return;
    }
    if let Some(last) = messages.last_mut()
        && last["role"] == role
        && let Some(content) = last["content"].as_array_mut()
    {
        content.append(&mut blocks);
        return;
    }
    messages.push(serde_json::json!({"role": role, "content": blocks}));
}

/// Encode an image as an Anthropic image block.
///
/// Sofia stores prepared images as `data:` URLs; remote URLs are passed through
/// as Anthropic `url` sources.
fn anthropic_image_block(image_url: &str) -> serde_json::Value {
    if let Some(data) = image_url.strip_prefix("data:")
        && let Some((meta, payload)) = data.split_once(',')
    {
        let media_type = meta.split(';').next().unwrap_or("image/png");
        return serde_json::json!({
            "type": "image",
            "source": {"type": "base64", "media_type": media_type, "data": payload}
        });
    }
    serde_json::json!({"type": "image", "source": {"type": "url", "url": image_url}})
}

/// Encode a tool result's payload as an Anthropic `tool_result` content value.
fn anthropic_tool_result_content(
    output: &FunctionCallOutputPayload,
    supports_images: bool,
) -> serde_json::Value {
    let Some(items) = output.content_items() else {
        return serde_json::json!(output.text_content().unwrap_or_default());
    };
    let mut blocks = Vec::new();
    for item in items {
        match item {
            FunctionCallOutputContentItem::InputText { text } => {
                blocks.push(serde_json::json!({"type": "text", "text": text}));
            }
            FunctionCallOutputContentItem::InputImage { image_url, .. } if supports_images => {
                blocks.push(anthropic_image_block(image_url));
            }
            FunctionCallOutputContentItem::InputImage { .. } => {
                blocks.push(serde_json::json!({
                    "type": "text",
                    "text": "Image unavailable: the selected model does not support image input."
                }));
            }
            FunctionCallOutputContentItem::InputAudio { .. }
            | FunctionCallOutputContentItem::EncryptedContent { .. } => {
                blocks.push(serde_json::json!({
                    "type": "text",
                    "text": "Non-image media cannot be represented in this tool result."
                }));
            }
        }
    }
    if blocks.len() == 1
        && let Some(text) = blocks[0].get("text").and_then(|value| value.as_str())
    {
        return serde_json::json!(text);
    }
    serde_json::json!(blocks)
}

/// Build the Anthropic `tools` array from the internal tool specs.
fn build_anthropic_tools(prompt: &Prompt) -> Vec<serde_json::Value> {
    let mut tools = Vec::new();
    for tool in prompt.tools.iter() {
        match tool {
            sofia_tools::ToolSpec::Function(func) => match serde_json::to_value(&func.parameters) {
                Ok(parameters) => {
                    tools.push(anthropic_function_tool(
                        &func.name,
                        &func.description,
                        parameters,
                    ));
                }
                Err(err) => warn!(
                    tool = %func.name,
                    error = %err,
                    "anthropic: skipping tool with unserializable parameters"
                ),
            },
            sofia_tools::ToolSpec::Freeform(freeform) => {
                tools.push(anthropic_freeform_tool(
                    &freeform.name,
                    &freeform.description,
                ));
            }
            sofia_tools::ToolSpec::Namespace(namespace) => {
                for inner in &namespace.tools {
                    match inner {
                        sofia_tools::ResponsesApiNamespaceTool::Function(func) => {
                            let Ok(parameters) = serde_json::to_value(&func.parameters) else {
                                continue;
                            };
                            let name =
                                sofia_tools::flat_namespace_tool_name(&namespace.name, &func.name);
                            let description = if func.description.trim().is_empty() {
                                namespace.description.clone()
                            } else {
                                func.description.clone()
                            };
                            tools.push(anthropic_function_tool(&name, &description, parameters));
                        }
                        sofia_tools::ResponsesApiNamespaceTool::Custom(custom) => {
                            let name = sofia_tools::flat_namespace_tool_name(
                                &namespace.name,
                                &custom.name,
                            );
                            let description = if custom.description.trim().is_empty() {
                                namespace.description.clone()
                            } else {
                                custom.description.clone()
                            };
                            tools.push(anthropic_freeform_tool(&name, &description));
                        }
                    }
                }
            }
            // Hosted/web tools have no Anthropic Messages encoding negotiated
            // here; omit them rather than risk an outright request rejection.
            sofia_tools::ToolSpec::WebSearch { .. } => {
                warn!("anthropic: web_search has no messages-api encoding; tool omitted");
            }
            sofia_tools::ToolSpec::ToolSearch { .. } => {
                warn!("anthropic: tool_search has no messages-api encoding; tool omitted");
            }
        }
    }
    tools
}

fn anthropic_function_tool(
    name: &str,
    description: &str,
    mut parameters: serde_json::Value,
) -> serde_json::Value {
    strip_responses_only_fields(&mut parameters);
    serde_json::json!({
        "name": name,
        "description": description,
        "input_schema": parameters,
    })
}

/// Anthropic has no freeform/custom tool type, so a freeform tool (for example
/// `apply_patch`) is bridged as a function tool with a single required `input`
/// string. The decoder unwraps that object back into a `CustomToolCall` so the
/// tool handler receives the raw text it expects.
fn anthropic_freeform_tool(name: &str, description: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "description": description,
        "input_schema": {
            "type": "object",
            "properties": {
                "input": {
                    "type": "string",
                    "description": "The raw input for this tool, exactly as the tool expects it (not JSON-escaped)."
                }
            },
            "required": ["input"],
            "additionalProperties": false,
        }
    })
}

/// Anthropic `thinking` configuration for a model, gated on advertised
/// reasoning capability.
///
/// Claude 4.6 and newer use adaptive thinking; earlier models need an explicit
/// token budget. When the model advertises no reasoning levels we cannot know
/// whether it accepts the field, so thinking is omitted rather than guessed.
fn anthropic_thinking_config(
    model_info: &ModelInfo,
    effort: &Option<ReasoningEffortConfig>,
) -> Option<serde_json::Value> {
    if model_info.supported_reasoning_levels.is_empty()
        || matches!(effort, Some(ReasoningEffortConfig::None))
    {
        return None;
    }
    match claude_major_version(&model_info.slug) {
        Some(major)
            if major >= 5
                || (major == 4
                    && model_info
                        .slug
                        .split('-')
                        .skip_while(|part| *part != "4")
                        .nth(1)
                        .and_then(|part| part.parse::<u32>().ok())
                        .is_some_and(|minor| minor >= 6)) =>
        {
            Some(serde_json::json!({"type": "adaptive"}))
        }
        _ => {
            let budget_tokens = match effort {
                Some(ReasoningEffortConfig::Minimal) => 1024,
                Some(ReasoningEffortConfig::Low) => 2048,
                Some(ReasoningEffortConfig::Medium) => 4096,
                Some(ReasoningEffortConfig::High) => 8192,
                Some(ReasoningEffortConfig::XHigh)
                | Some(ReasoningEffortConfig::Max)
                | Some(ReasoningEffortConfig::Ultra)
                | Some(ReasoningEffortConfig::Persistent) => 16384,
                Some(ReasoningEffortConfig::None) => return None,
                Some(ReasoningEffortConfig::Custom(_)) | None => 4096,
            };
            Some(serde_json::json!({"type": "enabled", "budget_tokens": budget_tokens}))
        }
    }
}

/// Best-effort major version extracted from a Claude model slug.
///
/// Slugs are versioned as `claude-<family>-<major>[-<minor>]-<date>` (for
/// example `claude-sonnet-4-5-20250929`), so the first bare integer token is the
/// major version. Returns `None` for slugs without a version.
fn claude_major_version(slug: &str) -> Option<u32> {
    slug.split('-').find_map(|token| token.parse::<u32>().ok())
}

/// Whether an Anthropic error body reports a `max_tokens` limit problem, so the
/// stream path can retry with a smaller cap instead of failing the turn.
pub(crate) fn anthropic_error_is_max_tokens(body: &str) -> bool {
    let body = body.to_ascii_lowercase();
    !body.contains("context")
        && !body.contains("prompt")
        && !body.contains("budget_tokens")
        && (body.contains("maximum allowed number of output tokens")
            || (body.contains("max_tokens")
                && (body.contains('>') || body.contains("maximum") || body.contains("at most"))))
}

/// Build an Anthropic Messages API request body from the internal prompt format.
pub(crate) fn build_anthropic_body(
    prompt: &Prompt,
    model_info: &ModelInfo,
    effort: &Option<ReasoningEffortConfig>,
) -> Result<serde_json::Value> {
    use serde_json::json;

    let supports_images = model_info
        .input_modalities
        .contains(&sofia_protocol::openai_models::InputModality::Image);

    // Anthropic has a single `system` field, so the base instructions and any
    // Responses-style `developer`/`system` messages are folded into it.
    let mut system_segments: Vec<String> = Vec::new();
    if !prompt.base_instructions.text.trim().is_empty() {
        system_segments.push(prompt.base_instructions.text.clone());
    }

    let mut messages: Vec<serde_json::Value> = Vec::new();
    for item in &prompt.input {
        match item {
            ResponseItem::Message { role, content, .. } => {
                let mut blocks = Vec::new();
                for part in content {
                    match part {
                        ContentItem::InputText { text } | ContentItem::OutputText { text } => {
                            blocks.push(json!({"type": "text", "text": text}));
                        }
                        ContentItem::InputImage { image_url, .. }
                            if supports_images && role == "user" =>
                        {
                            blocks.push(anthropic_image_block(image_url));
                        }
                        ContentItem::InputImage { .. } => blocks.push(json!({
                            "type": "text",
                            "text": "Image unavailable for this model or message role."
                        })),
                        ContentItem::InputAudio { .. } => blocks.push(json!({
                            "type": "text",
                            "text": "Audio input unavailable on this transport."
                        })),
                    }
                }
                if role == "developer" || role == "system" {
                    let text = blocks
                        .iter()
                        .filter_map(|block| block["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n");
                    if !text.is_empty() {
                        system_segments.push(text);
                    }
                    continue;
                }
                push_anthropic_message(&mut messages, role, blocks);
            }
            ResponseItem::FunctionCall {
                name,
                namespace,
                arguments,
                call_id,
                ..
            } => {
                if name.is_empty() {
                    continue;
                }
                let name = match namespace.as_deref() {
                    Some(namespace) if !namespace.is_empty() => {
                        sofia_tools::flat_namespace_tool_name(namespace, name)
                    }
                    _ => name.clone(),
                };
                let input = serde_json::from_str::<serde_json::Value>(arguments)
                    .unwrap_or_else(|_| json!({"input": arguments}));
                push_anthropic_message(
                    &mut messages,
                    "assistant",
                    vec![json!({
                        "type": "tool_use",
                        "id": call_id,
                        "name": name,
                        "input": input,
                    })],
                );
            }
            ResponseItem::FunctionCallOutput {
                call_id, output, ..
            } => {
                let Some(call_id) = call_id else {
                    continue;
                };
                let content = anthropic_tool_result_content(output, supports_images);
                push_anthropic_message(
                    &mut messages,
                    "user",
                    vec![json!({
                        "type": "tool_result",
                        "tool_use_id": call_id,
                        "content": content,
                        "is_error": output.success == Some(false),
                    })],
                );
            }
            ResponseItem::CustomToolCall {
                call_id,
                name,
                namespace,
                input,
                ..
            } => {
                let name = match namespace.as_deref() {
                    Some(namespace) if !namespace.is_empty() => {
                        sofia_tools::flat_namespace_tool_name(namespace, name)
                    }
                    _ => name.clone(),
                };
                push_anthropic_message(
                    &mut messages,
                    "assistant",
                    vec![json!({
                        "type": "tool_use",
                        "id": call_id,
                        "name": name,
                        // Freeform tools are bridged with a single `input`
                        // string; echo it back the same way.
                        "input": {"input": input},
                    })],
                );
            }
            ResponseItem::CustomToolCallOutput {
                call_id, output, ..
            } => {
                let content = anthropic_tool_result_content(output, supports_images);
                push_anthropic_message(
                    &mut messages,
                    "user",
                    vec![json!({
                        "type": "tool_result",
                        "tool_use_id": call_id,
                        "content": content,
                        "is_error": output.success == Some(false),
                    })],
                );
            }
            ResponseItem::LocalShellCall {
                call_id, action, ..
            } => {
                let Some(call_id) = call_id else {
                    continue;
                };
                let action = serde_json::to_value(action).unwrap_or(serde_json::Value::Null);
                push_anthropic_message(
                    &mut messages,
                    "assistant",
                    vec![json!({
                        "type": "tool_use",
                        "id": call_id,
                        "name": "local_shell",
                        "input": {"action": action},
                    })],
                );
            }
            ResponseItem::Reasoning {
                encrypted_content: Some(encoded),
                ..
            } => {
                if let Some(blocks) = sofia_api::decode_anthropic_thinking(encoded) {
                    push_anthropic_message(&mut messages, "assistant", blocks);
                }
            }
            ResponseItem::Reasoning {
                encrypted_content: None,
                ..
            } => {}
            ResponseItem::Compaction { .. } => {
                push_anthropic_message(
                    &mut messages,
                    "user",
                    vec![json!({
                        "type": "text",
                        "text": "CONTEXT SUMMARY: earlier conversation turns were compacted. Treat the following as established history.",
                    })],
                );
            }
            ResponseItem::ContextCompaction { .. }
            | ResponseItem::CompactionTrigger {}
            | ResponseItem::ConfigurationUpdate { .. }
            | ResponseItem::Other => {}
            ResponseItem::AgentMessage {
                author, content, ..
            } => {
                if let Some(text) = sofia_protocol::models::plaintext_agent_message_content(content)
                {
                    push_anthropic_message(
                        &mut messages,
                        "user",
                        vec![json!({
                            "type": "text",
                            "text": format!("Message from {author}: {text}"),
                        })],
                    );
                } else {
                    debug!("anthropic: skipping encrypted inter-agent message");
                }
            }
            ResponseItem::AdditionalTools { .. } => {}
            ResponseItem::ToolSearchCall { .. } | ResponseItem::ToolSearchOutput { .. } => {
                debug!(
                    "anthropic: tool_search has no wire equivalent; tool and result both omitted"
                );
            }
            ResponseItem::WebSearchCall { .. } | ResponseItem::ImageGenerationCall { .. } => {
                debug!("anthropic: dropping hosted-tool record with no messages item type");
            }
        }
    }

    // --- Request hygiene (mirrors the Chat Completions adapter) ---
    messages.retain(|message| {
        message
            .get("content")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|blocks| !blocks.is_empty())
    });
    for message in messages.iter_mut() {
        if message["role"] == "user"
            && let Some(blocks) = message["content"].as_array_mut()
            && blocks.len() == 1
            && blocks[0]["type"] == "text"
            && blocks[0]["text"].as_str().is_some_and(str::is_empty)
        {
            blocks[0]["text"] = json!("Continue.");
        }
    }
    // Anthropic requires the first message to use the `user` role.
    if messages
        .first()
        .is_some_and(|message| message["role"] == "assistant")
    {
        messages.insert(
            0,
            json!({"role": "user", "content": [{"type": "text", "text": "Continue."}]}),
        );
    }
    // Ensure a trailing user turn. Providers that reject a trailing assistant
    // message need one; it also structurally nudges the model to continue after
    // tool execution.
    if messages
        .last()
        .is_none_or(|message| message["role"] == "assistant")
    {
        messages.push(json!({"role": "user", "content": [{"type": "text", "text": "Continue."}]}));
    }

    let mut request = json!({
        "model": &model_info.slug,
        // Anthropic requires `max_tokens`. Use the same generous default as the
        // Chat Completions wire so long agent turns are not truncated.
        "max_tokens": 32768,
        "stream": true,
        // Automatic prompt caching: a single top-level breakpoint lets Anthropic
        // cache the longest stable prefix without per-block breakpoints.
        "cache_control": {"type": "ephemeral"},
    });

    let system = system_segments.join("\n\n");
    if !system.is_empty() {
        request["system"] = json!(system);
    }
    request["messages"] = json!(messages);

    let tools = build_anthropic_tools(prompt);
    if !tools.is_empty() {
        request["tools"] = json!(tools);
    }
    if let Some(thinking) = anthropic_thinking_config(model_info, effort) {
        if thinking["type"] == "adaptive"
            && let Some(effort) = effort
        {
            let effort = match effort {
                ReasoningEffortConfig::Minimal | ReasoningEffortConfig::Low => "low",
                ReasoningEffortConfig::Medium => "medium",
                ReasoningEffortConfig::High
                | ReasoningEffortConfig::XHigh
                | ReasoningEffortConfig::Max
                | ReasoningEffortConfig::Ultra
                | ReasoningEffortConfig::Persistent
                | ReasoningEffortConfig::Custom(_) => "high",
                ReasoningEffortConfig::None => "low",
            };
            request["output_config"] = json!({"effort": effort});
        }
        request["thinking"] = thinking;
    }

    Ok(request)
}

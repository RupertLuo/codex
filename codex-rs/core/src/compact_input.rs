use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::ResponseItem;

pub(crate) fn sanitize_for_compaction(items: &[ResponseItem]) -> Vec<ResponseItem> {
    let mut sanitized = items.to_vec();
    for item in &mut sanitized {
        match item {
            ResponseItem::Message { content, .. } => sanitize_message_content(content),
            ResponseItem::FunctionCallOutput { output, .. }
            | ResponseItem::CustomToolCallOutput { output, .. } => {
                if let FunctionCallOutputBody::ContentItems(content) = &mut output.body {
                    sanitize_tool_content(content);
                }
            }
            ResponseItem::ImageGenerationCall {
                id,
                revised_prompt,
                internal_chat_message_metadata_passthrough,
                ..
            } => {
                let mut content = Vec::new();
                if let Some(prompt) = revised_prompt.take() {
                    content.push(ContentItem::InputText {
                        text: format!("Generated image prompt: {prompt}"),
                    });
                }
                content.push(ContentItem::InputText {
                    text: omitted_images(1),
                });
                *item = ResponseItem::Message {
                    id: id.take(),
                    role: "assistant".to_string(),
                    content,
                    phase: None,
                    internal_chat_message_metadata_passthrough:
                        internal_chat_message_metadata_passthrough.take(),
                };
            }
            ResponseItem::AdditionalTools { .. }
            | ResponseItem::AgentMessage { .. }
            | ResponseItem::Reasoning { .. }
            | ResponseItem::LocalShellCall { .. }
            | ResponseItem::FunctionCall { .. }
            | ResponseItem::ToolSearchCall { .. }
            | ResponseItem::CustomToolCall { .. }
            | ResponseItem::ToolSearchOutput { .. }
            | ResponseItem::WebSearchCall { .. }
            | ResponseItem::Compaction { .. }
            | ResponseItem::CompactionTrigger { .. }
            | ResponseItem::ContextCompaction { .. }
            | ResponseItem::Other => {}
        }
    }
    sanitized
}

fn sanitize_message_content(content: &mut Vec<ContentItem>) {
    let Some(first_image) = content
        .iter()
        .position(|item| matches!(item, ContentItem::InputImage { .. }))
    else {
        return;
    };
    let image_count = content
        .iter()
        .filter(|item| matches!(item, ContentItem::InputImage { .. }))
        .count();
    content.retain(|item| !matches!(item, ContentItem::InputImage { .. }));
    content.insert(
        first_image.min(content.len()),
        ContentItem::InputText {
            text: omitted_images(image_count),
        },
    );
}

fn sanitize_tool_content(content: &mut Vec<FunctionCallOutputContentItem>) {
    let Some(first_image) = content
        .iter()
        .position(|item| matches!(item, FunctionCallOutputContentItem::InputImage { .. }))
    else {
        return;
    };
    let image_count = content
        .iter()
        .filter(|item| matches!(item, FunctionCallOutputContentItem::InputImage { .. }))
        .count();
    content.retain(|item| !matches!(item, FunctionCallOutputContentItem::InputImage { .. }));
    content.insert(
        first_image.min(content.len()),
        FunctionCallOutputContentItem::InputText {
            text: omitted_images(image_count),
        },
    );
}

fn omitted_images(count: usize) -> String {
    let noun = if count == 1 { "image" } else { "images" };
    format!("[{count} {noun} omitted during compaction]")
}

#[cfg(test)]
#[path = "compact_input_tests.rs"]
mod tests;

//! Pure request adaptations enabled by host model policy.
//!
//! Runtime selects the policy and normalizes provider wire requests. The caller applies image
//! relocation before saving the typed baseline; session ownership, delta selection, and retry
//! sequencing remain in `ModelClientSession`. These helpers never mutate stored history.

use codex_api::ApiError;
use codex_api::TransportError;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::ResponseItem;
use http::StatusCode;

pub(super) fn is_unknown_previous_response(error: &ApiError) -> bool {
    let (status, body) = match error {
        ApiError::Transport(TransportError::Http { status, body, .. }) => {
            (*status, body.as_deref().unwrap_or_default())
        }
        ApiError::Api { status, message } => (*status, message.as_str()),
        ApiError::InvalidRequest { message } => (StatusCode::BAD_REQUEST, message.as_str()),
        _ => return false,
    };
    if status != StatusCode::BAD_REQUEST && status != StatusCode::NOT_FOUND {
        return false;
    }

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        let detail = value.get("error").unwrap_or(&value);
        let code = detail
            .get("code")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let param = detail
            .get("param")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let message = detail
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        if code.contains("previous_response") && code.contains("not_found") {
            return true;
        }
        if param == "previous_response_id" && message.contains("not found") {
            return true;
        }
    }

    let body = body.to_ascii_lowercase();
    body.contains("previous_response_id")
        && (body.contains("not found") || body.contains("not_found"))
}

pub(super) fn relocate_tool_output_images(input: &mut Vec<ResponseItem>) {
    let mut relocated = Vec::with_capacity(input.len());
    for mut item in std::mem::take(input) {
        let images = match &mut item {
            ResponseItem::FunctionCallOutput { output, .. } => output
                .content_items_mut()
                .map(|items| {
                    let mut images = Vec::new();
                    items.retain(|content| match content {
                        FunctionCallOutputContentItem::InputImage { image_url, detail } => {
                            images.push(ContentItem::InputImage {
                                image_url: image_url.clone(),
                                detail: *detail,
                            });
                            false
                        }
                        _ => true,
                    });
                    images
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        relocated.push(item);
        if !images.is_empty() {
            relocated.push(ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: images,
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            });
        }
    }
    *input = relocated;
}

#[cfg(test)]
#[path = "host_http_tests.rs"]
mod tests;

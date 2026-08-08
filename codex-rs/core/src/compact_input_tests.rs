use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::InternalChatMessageMetadataPassthrough;
use codex_protocol::models::MessagePhase;
use codex_protocol::models::ResponseItem;
use pretty_assertions::assert_eq;

use super::sanitize_for_compaction;

#[test]
fn sanitizes_images_without_mutating_source_history() {
    let source = vec![
        ResponseItem::Message {
            id: Some("message-id".to_string()),
            role: "user".to_string(),
            content: vec![
                ContentItem::InputText {
                    text: "before".to_string(),
                },
                ContentItem::InputImage {
                    image_url: "https://example.com/image.png".to_string(),
                    detail: None,
                },
                ContentItem::InputImage {
                    image_url: "data:image/png;base64,message".to_string(),
                    detail: None,
                },
                ContentItem::OutputText {
                    text: "after".to_string(),
                },
            ],
            phase: Some(MessagePhase::Commentary),
            internal_chat_message_metadata_passthrough: Some(
                InternalChatMessageMetadataPassthrough {
                    turn_id: Some("message-turn".to_string()),
                },
            ),
        },
        ResponseItem::FunctionCallOutput {
            id: Some("function-output-id".to_string()),
            call_id: "function-call".to_string(),
            output: FunctionCallOutputPayload::from_content_items(vec![
                FunctionCallOutputContentItem::InputText {
                    text: "tool text".to_string(),
                },
                FunctionCallOutputContentItem::InputImage {
                    image_url: "data:image/png;base64,function".to_string(),
                    detail: None,
                },
                FunctionCallOutputContentItem::EncryptedContent {
                    encrypted_content: "encrypted tool data".to_string(),
                },
            ]),
            internal_chat_message_metadata_passthrough: Some(
                InternalChatMessageMetadataPassthrough {
                    turn_id: Some("function-turn".to_string()),
                },
            ),
        },
        ResponseItem::CustomToolCallOutput {
            id: Some("custom-output-id".to_string()),
            call_id: "custom-call".to_string(),
            name: Some("custom".to_string()),
            output: FunctionCallOutputPayload::from_content_items(vec![
                FunctionCallOutputContentItem::InputImage {
                    image_url: "data:image/png;base64,custom".to_string(),
                    detail: None,
                },
            ]),
            internal_chat_message_metadata_passthrough: Some(
                InternalChatMessageMetadataPassthrough {
                    turn_id: Some("custom-turn".to_string()),
                },
            ),
        },
        ResponseItem::ImageGenerationCall {
            id: Some("image-generation-id".to_string()),
            status: "completed".to_string(),
            revised_prompt: Some("a harbor at night".to_string()),
            result: "large-base64-image-result".repeat(1_000),
            internal_chat_message_metadata_passthrough: Some(
                InternalChatMessageMetadataPassthrough {
                    turn_id: Some("image-generation-turn".to_string()),
                },
            ),
        },
    ];
    let original = source.clone();

    assert_eq!(
        sanitize_for_compaction(&source),
        vec![
            ResponseItem::Message {
                id: Some("message-id".to_string()),
                role: "user".to_string(),
                content: vec![
                    ContentItem::InputText {
                        text: "before".to_string(),
                    },
                    ContentItem::InputText {
                        text: "[2 images omitted during compaction]".to_string(),
                    },
                    ContentItem::OutputText {
                        text: "after".to_string(),
                    },
                ],
                phase: Some(MessagePhase::Commentary),
                internal_chat_message_metadata_passthrough: Some(
                    InternalChatMessageMetadataPassthrough {
                        turn_id: Some("message-turn".to_string()),
                    },
                ),
            },
            ResponseItem::FunctionCallOutput {
                id: Some("function-output-id".to_string()),
                call_id: "function-call".to_string(),
                output: FunctionCallOutputPayload::from_content_items(vec![
                    FunctionCallOutputContentItem::InputText {
                        text: "tool text".to_string(),
                    },
                    FunctionCallOutputContentItem::InputText {
                        text: "[1 image omitted during compaction]".to_string(),
                    },
                    FunctionCallOutputContentItem::EncryptedContent {
                        encrypted_content: "encrypted tool data".to_string(),
                    },
                ]),
                internal_chat_message_metadata_passthrough: Some(
                    InternalChatMessageMetadataPassthrough {
                        turn_id: Some("function-turn".to_string()),
                    },
                ),
            },
            ResponseItem::CustomToolCallOutput {
                id: Some("custom-output-id".to_string()),
                call_id: "custom-call".to_string(),
                name: Some("custom".to_string()),
                output: FunctionCallOutputPayload::from_content_items(vec![
                    FunctionCallOutputContentItem::InputText {
                        text: "[1 image omitted during compaction]".to_string(),
                    },
                ]),
                internal_chat_message_metadata_passthrough: Some(
                    InternalChatMessageMetadataPassthrough {
                        turn_id: Some("custom-turn".to_string()),
                    },
                ),
            },
            ResponseItem::Message {
                id: Some("image-generation-id".to_string()),
                role: "assistant".to_string(),
                content: vec![
                    ContentItem::InputText {
                        text: "Generated image prompt: a harbor at night".to_string(),
                    },
                    ContentItem::InputText {
                        text: "[1 image omitted during compaction]".to_string(),
                    },
                ],
                phase: None,
                internal_chat_message_metadata_passthrough: Some(
                    InternalChatMessageMetadataPassthrough {
                        turn_id: Some("image-generation-turn".to_string()),
                    },
                ),
            },
        ]
    );
    assert_eq!(source, original);
}

#[test]
fn leaves_items_without_images_unchanged() {
    let source = vec![
        ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText {
                text: "plain text".to_string(),
            }],
            phase: Some(MessagePhase::FinalAnswer),
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::FunctionCallOutput {
            id: None,
            call_id: "function-call".to_string(),
            output: FunctionCallOutputPayload::from_content_items(vec![
                FunctionCallOutputContentItem::EncryptedContent {
                    encrypted_content: "encrypted text".to_string(),
                },
            ]),
            internal_chat_message_metadata_passthrough: None,
        },
    ];

    assert_eq!(sanitize_for_compaction(&source), source);
}

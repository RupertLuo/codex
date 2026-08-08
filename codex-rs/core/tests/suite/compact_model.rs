use codex_core::compact::SUMMARY_PREFIX;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use core_test_support::hooks::trust_discovered_hooks;
use core_test_support::responses::ResponseMock;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_completed_with_tokens;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::sse_failed;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use serde_json::Value;

const ACTIVE_MODEL: &str = "gpt-5.4";
const COMPACT_MODEL: &str = "deepseek/deepseek-v4-flash";
const COMPACT_PROMPT: &str = "Summarize the conversation as durable text state.";
const FAILED_TURN_MODEL: &str = "gpt-5.3-codex";
const IMAGE_URL: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";

fn compact_request_body(mock: &ResponseMock) -> Value {
    mock.requests()
        .iter()
        .find(|request| {
            request
                .message_input_texts("user")
                .iter()
                .any(|text| text == COMPACT_PROMPT)
        })
        .expect("compact request containing configured prompt")
        .body_json()
}

fn request_body_containing_user_text(mock: &ResponseMock, expected: &str) -> Value {
    mock.requests()
        .iter()
        .find(|request| {
            request
                .message_input_texts("user")
                .iter()
                .any(|text| text == expected)
        })
        .unwrap_or_else(|| panic!("request containing user text {expected:?}"))
        .body_json()
}

fn assert_compact_request_shape(compact_body: &Value) {
    assert_eq!(compact_body["model"], COMPACT_MODEL);
    assert!(compact_body.get("previous_response_id").is_none());
    assert_eq!(compact_body["tools"], serde_json::json!([]));
    assert!(compact_body.get("text").is_none());

    let compact_body = compact_body.to_string();
    assert!(!compact_body.contains("data:image/"));
    assert!(!compact_body.contains("base64,"));
    assert!(compact_body.contains("images omitted during compaction"));
}

fn assert_normal_request_shape(normal_body: &Value) {
    assert_eq!(normal_body["model"], ACTIVE_MODEL);

    let normal_body = normal_body.to_string();
    assert!(!normal_body.contains("<model_switch>"));
    assert!(!normal_body.contains(COMPACT_MODEL));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_compact_uses_configured_text_only_model() {
    skip_if_no_network!();

    let server = start_mock_server().await;
    let response_mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_assistant_message("first-message", "FIRST_REPLY"),
                ev_completed("first-response"),
            ]),
            sse(vec![
                ev_assistant_message("compact-message", "COMPACT_SUMMARY"),
                ev_completed("compact-response"),
            ]),
            sse(vec![
                ev_assistant_message("final-message", "FINAL_REPLY"),
                ev_completed("final-response"),
            ]),
        ],
    )
    .await;

    let mut builder = test_codex().with_model(ACTIVE_MODEL).with_config(|config| {
        config.compact_model = Some(COMPACT_MODEL.to_string());
        config.compact_prompt = Some(COMPACT_PROMPT.to_string());
        config.model_provider.name = "Catalyst-compatible test provider".to_string();
    });
    let test = builder
        .build_with_auto_env(&server)
        .await
        .expect("build codex");

    test.codex
        .submit(Op::UserInput {
            items: vec![
                UserInput::Image {
                    image_url: IMAGE_URL.to_string(),
                    detail: None,
                },
                UserInput::Image {
                    image_url: IMAGE_URL.to_string(),
                    detail: None,
                },
                UserInput::Text {
                    text: "FIRST_USER_TURN".to_string(),
                    text_elements: Vec::new(),
                },
            ],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await
        .expect("submit first user turn");
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    test.codex
        .submit(Op::Compact)
        .await
        .expect("submit compact operation");
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    test.submit_turn("POST_COMPACT_USER_TURN")
        .await
        .expect("submit post-compact turn");

    assert_eq!(response_mock.requests().len(), 3);
    assert_compact_request_shape(&compact_request_body(&response_mock));
    assert_normal_request_shape(&request_body_containing_user_text(
        &response_mock,
        "POST_COMPACT_USER_TURN",
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compact_success_resets_then_reestablishes_incremental_baseline() {
    skip_if_no_network!();

    let server = start_mock_server().await;
    let response_mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-before-compact"),
                ev_assistant_message("first-message", "FIRST_REPLY"),
                ev_completed_with_tokens("resp-before-compact", /*total_tokens*/ 500),
            ]),
            sse(vec![
                ev_assistant_message("compact-message", "COMPACT_SUMMARY"),
                ev_completed_with_tokens("compact-response", /*total_tokens*/ 100),
            ]),
            sse(vec![
                ev_response_created("resp-after-compact"),
                ev_assistant_message("second-message", "SECOND_REPLY"),
                ev_completed_with_tokens("resp-after-compact", /*total_tokens*/ 80),
            ]),
            sse(vec![
                ev_response_created("resp-incremental-again"),
                ev_assistant_message("third-message", "THIRD_REPLY"),
                ev_completed_with_tokens("resp-incremental-again", /*total_tokens*/ 80),
            ]),
        ],
    )
    .await;

    let mut builder = test_codex()
        .with_model(ACTIVE_MODEL)
        .with_model_info_override(ACTIVE_MODEL, |model_info| {
            model_info.supports_incremental_requests = true;
        })
        .with_config(|config| {
            config.compact_model = Some(COMPACT_MODEL.to_string());
            config.compact_prompt = Some(COMPACT_PROMPT.to_string());
            config.model_provider.name = "Catalyst-compatible test provider".to_string();
            config.model_auto_compact_token_limit = Some(200);
        });
    let test = builder
        .build_with_auto_env(&server)
        .await
        .expect("build codex");

    test.codex
        .submit(Op::UserInput {
            items: vec![
                UserInput::Image {
                    image_url: IMAGE_URL.to_string(),
                    detail: None,
                },
                UserInput::Image {
                    image_url: IMAGE_URL.to_string(),
                    detail: None,
                },
                UserInput::Text {
                    text: "FIRST_AUTO_USER_TURN".to_string(),
                    text_elements: Vec::new(),
                },
            ],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await
        .expect("submit first user turn");
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    test.submit_turn("POST_AUTO_COMPACT_USER_TURN")
        .await
        .expect("submit turn that triggers pre-turn compaction");
    test.submit_turn("SECOND_POST_COMPACT_USER_TURN")
        .await
        .expect("submit second post-compact turn");

    assert_eq!(response_mock.requests().len(), 4);
    let compact_body = compact_request_body(&response_mock);
    assert_compact_request_shape(&compact_body);

    let post_compact_body =
        request_body_containing_user_text(&response_mock, "POST_AUTO_COMPACT_USER_TURN");
    assert_normal_request_shape(&post_compact_body);
    assert!(post_compact_body.get("previous_response_id").is_none());
    let post_compact_text = post_compact_body.to_string();
    assert!(post_compact_text.contains(SUMMARY_PREFIX));
    assert!(post_compact_text.contains("<environment_context>"));
    assert!(post_compact_text.contains("POST_AUTO_COMPACT_USER_TURN"));
    assert!(!post_compact_text.contains("data:image/"));

    let second_post_compact_body =
        request_body_containing_user_text(&response_mock, "SECOND_POST_COMPACT_USER_TURN");
    assert_normal_request_shape(&second_post_compact_body);
    assert_eq!(
        second_post_compact_body["previous_response_id"].as_str(),
        Some("resp-after-compact")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compact_failure_preserves_incremental_baseline_and_history() {
    skip_if_no_network!();

    let server = start_mock_server().await;
    let response_mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-before-failed-compact"),
                ev_assistant_message("first-message", "FIRST_REPLY"),
                ev_completed("resp-before-failed-compact"),
            ]),
            sse_failed(
                "failed-compact-response",
                "server_error",
                "compact provider failure",
            ),
            sse(vec![
                ev_response_created("resp-after-failed-compact"),
                ev_assistant_message("follow-up-message", "FOLLOW_UP_REPLY"),
                ev_completed("resp-after-failed-compact"),
            ]),
        ],
    )
    .await;

    let mut builder = test_codex()
        .with_model_info_override(FAILED_TURN_MODEL, |model_info| {
            model_info.comp_hash = Some("failed-turn-hash".to_string());
        })
        .with_model_info_override(ACTIVE_MODEL, |model_info| {
            model_info.comp_hash = Some("active-turn-hash".to_string());
            model_info.supports_incremental_requests = true;
        })
        .with_config(|config| {
            config.compact_model = Some(COMPACT_MODEL.to_string());
            config.compact_prompt = Some(COMPACT_PROMPT.to_string());
            config.model_provider.name = "Catalyst-compatible test provider".to_string();
            config.model_provider.stream_max_retries = Some(0);
        });
    let test = builder
        .build_with_auto_env(&server)
        .await
        .expect("build codex");

    test.submit_turn("FIRST_USER_TURN")
        .await
        .expect("submit first user turn");

    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "TURN_THAT_TRIGGERS_FAILED_COMPACT".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: codex_protocol::protocol::ThreadSettingsOverrides {
                collaboration_mode: Some(codex_protocol::config_types::CollaborationMode {
                    mode: codex_protocol::config_types::ModeKind::Default,
                    settings: codex_protocol::config_types::Settings {
                        model: FAILED_TURN_MODEL.to_string(),
                        reasoning_effort: None,
                        developer_instructions: None,
                    },
                }),
                ..Default::default()
            },
        })
        .await
        .expect("submit turn that triggers failed compact");
    wait_for_event(&test.codex, |event| matches!(event, EventMsg::Error(_))).await;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    test.submit_turn("FOLLOW_UP_AFTER_FAILED_COMPACT")
        .await
        .expect("submit follow-up after failed compact");

    assert_eq!(response_mock.requests().len(), 3);
    let compact_body = compact_request_body(&response_mock);
    assert_eq!(compact_body["model"], COMPACT_MODEL);
    assert!(compact_body.get("previous_response_id").is_none());
    assert_eq!(compact_body["tools"], serde_json::json!([]));
    assert!(compact_body.get("text").is_none());
    let follow_up_body =
        request_body_containing_user_text(&response_mock, "FOLLOW_UP_AFTER_FAILED_COMPACT");
    assert_eq!(
        follow_up_body["previous_response_id"].as_str(),
        Some("resp-before-failed-compact")
    );
    assert!(!follow_up_body.to_string().contains(SUMMARY_PREFIX));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compact_abort_before_replacement_preserves_incremental_baseline_and_history() {
    skip_if_no_network!();

    let server = start_mock_server().await;
    let response_mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-before-aborted-compact"),
                ev_assistant_message("first-message", "FIRST_REPLY"),
                ev_completed("resp-before-aborted-compact"),
            ]),
            sse(vec![
                ev_response_created("resp-after-aborted-compact"),
                ev_assistant_message("follow-up-message", "FOLLOW_UP_REPLY"),
                ev_completed("resp-after-aborted-compact"),
            ]),
        ],
    )
    .await;

    let mut builder = test_codex()
        .with_pre_build_hook(|home| {
            let script_path = home.join("abort_auto_compact.py");
            std::fs::write(
                &script_path,
                r#"import json
import sys

json.load(sys.stdin)
print(json.dumps({"continue": False, "stopReason": "stop before compact"}))
"#,
            )
            .expect("write pre-compact abort hook script");
            let hooks = serde_json::json!({
                "hooks": {
                    "PreCompact": [{
                        "matcher": "auto",
                        "hooks": [{
                            "type": "command",
                            "command": format!("python3 \"{}\"", script_path.display()),
                        }]
                    }]
                }
            });
            std::fs::write(home.join("hooks.json"), hooks.to_string())
                .expect("write pre-compact abort hooks config");
        })
        .with_model_info_override(FAILED_TURN_MODEL, |model_info| {
            model_info.comp_hash = Some("aborted-turn-hash".to_string());
        })
        .with_model_info_override(ACTIVE_MODEL, |model_info| {
            model_info.comp_hash = Some("active-turn-hash".to_string());
            model_info.supports_incremental_requests = true;
        })
        .with_config(|config| {
            config.compact_model = Some(COMPACT_MODEL.to_string());
            config.compact_prompt = Some(COMPACT_PROMPT.to_string());
            config.model_provider.name = "Catalyst-compatible test provider".to_string();
            trust_discovered_hooks(config);
        });
    let test = builder
        .build_with_auto_env(&server)
        .await
        .expect("build codex");

    test.submit_turn("FIRST_USER_TURN")
        .await
        .expect("submit first user turn");

    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "TURN_THAT_TRIGGERS_ABORTED_COMPACT".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: codex_protocol::protocol::ThreadSettingsOverrides {
                collaboration_mode: Some(codex_protocol::config_types::CollaborationMode {
                    mode: codex_protocol::config_types::ModeKind::Default,
                    settings: codex_protocol::config_types::Settings {
                        model: FAILED_TURN_MODEL.to_string(),
                        reasoning_effort: None,
                        developer_instructions: None,
                    },
                }),
                ..Default::default()
            },
        })
        .await
        .expect("submit turn that triggers aborted compact");
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnAborted(_))
    })
    .await;

    test.submit_turn("FOLLOW_UP_AFTER_ABORTED_COMPACT")
        .await
        .expect("submit follow-up after aborted compact");

    assert_eq!(response_mock.requests().len(), 2);
    let follow_up_body =
        request_body_containing_user_text(&response_mock, "FOLLOW_UP_AFTER_ABORTED_COMPACT");
    assert_eq!(
        follow_up_body["previous_response_id"].as_str(),
        Some("resp-before-aborted-compact")
    );
    assert!(!follow_up_body.to_string().contains(SUMMARY_PREFIX));
}

use codex_core::ThreadManagerRuntimeOptions;
use codex_core::compact::SUMMARY_PREFIX;
use codex_core::config::ThreadStoreConfig;
use codex_core::test_support::CompactCommitTestHook;
use codex_core::test_support::auth_manager_from_auth;
use codex_core::test_support::with_compact_commit_test_hook;
use codex_login::CodexAuth;
use codex_protocol::protocol::CompactedItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::InitialHistory;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::ResumedHistory;
use codex_protocol::protocol::RolloutItem;
use codex_protocol::protocol::RolloutLine;
use codex_protocol::user_input::UserInput;
use codex_thread_store::InMemoryThreadStore;
use codex_thread_store::LoadThreadHistoryParams;
use codex_thread_store::ThreadStore;
use core_test_support::hooks::trust_discovered_hooks;
use core_test_support::responses::ResponseMock;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_completed_with_tokens;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_response_sequence;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::sse_failed;
use core_test_support::responses::sse_response;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::TestCodexBuilder;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

const ACTIVE_MODEL: &str = "gpt-5.4";
const COMPACT_MODEL: &str = "deepseek/deepseek-v4-flash";
const COMPACT_PROMPT: &str = "Summarize the conversation as durable text state.";
const FAILED_TURN_MODEL: &str = "gpt-5.3-codex";
const IMAGE_URL: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";

fn in_memory_atomic_compact_builder(
    store_id: &'static str,
    commit_hook: CompactCommitTestHook,
) -> TestCodexBuilder {
    test_codex()
        .with_runtime_options(with_compact_commit_test_hook(
            ThreadManagerRuntimeOptions::default(),
            commit_hook,
        ))
        .with_model(ACTIVE_MODEL)
        .with_model_info_override(ACTIVE_MODEL, |model_info| {
            model_info.supports_incremental_requests = true;
        })
        .with_config(move |config| {
            config.experimental_thread_store = ThreadStoreConfig::InMemory {
                id: store_id.to_string(),
            };
            config.compact_model = Some(COMPACT_MODEL.to_string());
            config.compact_prompt = Some(COMPACT_PROMPT.to_string());
            config.model_provider.name = "Catalyst-compatible test provider".to_string();
        })
}

async fn only_atomic_compaction_checkpoint(
    test: &TestCodex,
    thread_store: &InMemoryThreadStore,
) -> CompactedItem {
    let history = ThreadStore::load_history(
        thread_store,
        LoadThreadHistoryParams {
            thread_id: test.session_configured.thread_id,
            include_archived: true,
        },
    )
    .await
    .expect("load durable thread history");
    let checkpoint_positions = history
        .items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| match item {
            RolloutItem::Compacted(compacted) if compacted.checkpoint.is_some() => Some(index),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        checkpoint_positions.len(),
        1,
        "compaction transaction must be one durable checkpoint record"
    );
    let checkpoint_index = checkpoint_positions[0];
    assert!(
        history.items.iter().skip(checkpoint_index + 1).all(|item| {
            !matches!(
                item,
                RolloutItem::TurnContext(_)
                    | RolloutItem::WorldState(_)
                    | RolloutItem::EventMsg(EventMsg::TokenCount(_))
            )
        }),
        "checkpoint state must not leak into trailing companion records"
    );
    let RolloutItem::Compacted(compacted) = &history.items[checkpoint_index] else {
        unreachable!("checkpoint position only records compacted items")
    };
    compacted.clone()
}

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

fn replacement_history_from_rollout(path: &Path) -> Vec<Value> {
    let rollout = std::fs::read_to_string(path).expect("read rollout");
    rollout
        .lines()
        .filter_map(|line| serde_json::from_str::<RolloutLine>(line).ok())
        .filter_map(|line| match line.item {
            RolloutItem::Compacted(compacted) => compacted.replacement_history,
            _ => None,
        })
        .next_back()
        .expect("compacted rollout replacement history")
        .into_iter()
        .map(|item| {
            model_visible_value(serde_json::to_value(item).expect("serialize replacement item"))
        })
        .collect()
}

fn model_visible_value(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(model_visible_value).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter(|(key, _)| key != "internal_chat_message_metadata_passthrough")
                .map(|(key, value)| (key, model_visible_value(value)))
                .collect(),
        ),
        other => other,
    }
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

async fn assert_ambiguous_session_quarantined_then_reload_succeeds(
    test: &TestCodex,
    thread_store: &InMemoryThreadStore,
    response_mock: &ResponseMock,
) {
    let thread_id = test.session_configured.thread_id;
    let durable_history = ThreadStore::load_history(
        thread_store,
        LoadThreadHistoryParams {
            thread_id,
            include_archived: true,
        },
    )
    .await
    .expect("load durable history before quarantined follow-up");
    let calls_before_quarantined_follow_up = thread_store.calls().await;
    let flush_error = test
        .codex
        .flush_rollout()
        .await
        .expect_err("quarantined session must reject persistence barriers");
    assert!(flush_error.to_string().contains("quarantined"));
    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "FOLLOW_UP_AFTER_AMBIGUOUS_COMPACT".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await
        .expect("queue follow-up after ambiguous compact");
    let quarantine_event = wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::Error(_) | EventMsg::TurnStarted(_))
    })
    .await;
    let EventMsg::Error(quarantine_error) = quarantine_event else {
        panic!("quarantined session started another inference turn: {quarantine_event:?}")
    };
    assert!(
        quarantine_error.message.contains("quarantined"),
        "same-session use should explain the quarantine: {}",
        quarantine_error.message
    );
    assert!(quarantine_error.message.contains("restart or reload"));

    assert_eq!(
        response_mock.requests().len(),
        2,
        "quarantined session must not issue another inference request"
    );
    assert_eq!(
        thread_store.calls().await.append_items,
        calls_before_quarantined_follow_up.append_items,
        "quarantined session must not attempt another rollout append"
    );
    let history_after_quarantined_follow_up = ThreadStore::load_history(
        thread_store,
        LoadThreadHistoryParams {
            thread_id,
            include_archived: true,
        },
    )
    .await
    .expect("load durable history after quarantined follow-up");
    assert_eq!(
        serde_json::to_value(&history_after_quarantined_follow_up.items)
            .expect("serialize history after quarantine"),
        serde_json::to_value(&durable_history.items).expect("serialize durable history"),
        "quarantined session must not mutate durable rollout history"
    );

    test.codex
        .shutdown_and_wait()
        .await
        .expect("shutdown quarantined session");
    test.thread_manager.remove_thread(&thread_id).await;
    let resumed = test
        .thread_manager
        .resume_thread_with_history(
            test.config.clone(),
            InitialHistory::Resumed(ResumedHistory {
                conversation_id: thread_id,
                history: Arc::new(durable_history.items),
                rollout_path: None,
            }),
            auth_manager_from_auth(CodexAuth::from_api_key("dummy")),
            /*parent_trace*/ None,
            /*supports_openai_form_elicitation*/ false,
        )
        .await
        .expect("reload thread from durable checkpoint");
    resumed
        .thread
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "FOLLOW_UP_AFTER_RELOAD".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await
        .expect("submit follow-up after reload");
    wait_for_event(&resumed.thread, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert_eq!(response_mock.requests().len(), 3);
    let follow_up = request_body_containing_user_text(response_mock, "FOLLOW_UP_AFTER_RELOAD");
    assert!(
        follow_up.get("previous_response_id").is_none(),
        "reload must not reuse the stale pre-compact response id"
    );
    assert!(follow_up.to_string().contains("AMBIGUOUS_COMPACT_SUMMARY"));
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupt_blocked_before_compact_item_started_returns_gracefully() {
    skip_if_no_network!();

    let server = start_mock_server().await;
    let response_mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-before-blocked-item-started"),
                ev_assistant_message("first-message", "FIRST_REPLY"),
                ev_completed("resp-before-blocked-item-started"),
            ]),
            sse(vec![
                ev_response_created("resp-after-blocked-item-started"),
                ev_assistant_message("follow-up-message", "FOLLOW_UP_REPLY"),
                ev_completed("resp-after-blocked-item-started"),
            ]),
        ],
    )
    .await;
    let commit_hook = CompactCommitTestHook::new();
    let mut builder = test_codex()
        .with_runtime_options(with_compact_commit_test_hook(
            ThreadManagerRuntimeOptions::default(),
            commit_hook.clone(),
        ))
        .with_model(ACTIVE_MODEL)
        .with_model_info_override(FAILED_TURN_MODEL, |model_info| {
            model_info.comp_hash = Some("blocked-item-started-hash".to_string());
        })
        .with_model_info_override(ACTIVE_MODEL, |model_info| {
            model_info.comp_hash = Some("active-turn-hash".to_string());
            model_info.supports_incremental_requests = true;
        })
        .with_config(|config| {
            config.compact_model = Some(COMPACT_MODEL.to_string());
            config.compact_prompt = Some(COMPACT_PROMPT.to_string());
            config.model_provider.name = "Catalyst-compatible test provider".to_string();
        });
    let test = builder
        .build_with_auto_env(&server)
        .await
        .expect("build codex");

    test.submit_turn("FIRST_USER_TURN")
        .await
        .expect("submit first user turn");
    commit_hook.pause_item_started_once();
    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "TURN_BLOCKED_BEFORE_COMPACT_ITEM_STARTED".to_string(),
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
        .expect("submit turn blocked before compact ItemStarted");
    tokio::time::timeout(
        Duration::from_secs(5),
        commit_hook.wait_until_item_started_paused(),
    )
    .await
    .expect("compact should pause before ItemStarted");

    test.codex
        .submit(Op::Interrupt)
        .await
        .expect("interrupt blocked compact turn");
    tokio::time::timeout(
        Duration::from_secs(5),
        commit_hook.wait_until_item_started_cancelled(),
    )
    .await
    .expect("ItemStarted wait should observe cancellation before forced task abort");
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnAborted(_))
    })
    .await;

    test.submit_turn("FOLLOW_UP_AFTER_BLOCKED_ITEM_STARTED")
        .await
        .expect("submit follow-up after blocked ItemStarted");

    assert_eq!(response_mock.requests().len(), 2);
    let follow_up =
        request_body_containing_user_text(&response_mock, "FOLLOW_UP_AFTER_BLOCKED_ITEM_STARTED");
    assert_eq!(
        follow_up["previous_response_id"].as_str(),
        Some("resp-before-blocked-item-started")
    );
    assert!(!follow_up.to_string().contains(SUMMARY_PREFIX));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compact_output_then_terminal_error_preserves_history_and_baseline() {
    skip_if_no_network!();

    let server = start_mock_server().await;
    let response_mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-before-output-error"),
                ev_assistant_message("first-message", "FIRST_REPLY"),
                ev_completed("resp-before-output-error"),
            ]),
            sse(vec![
                ev_assistant_message("failed-compact-message", "FAILED_COMPACT_OUTPUT"),
                serde_json::json!({
                    "type": "response.failed",
                    "response": {
                        "id": "failed-compact-response",
                        "error": {
                            "code": "server_error",
                            "message": "compact provider failed after output"
                        }
                    }
                }),
            ]),
            sse(vec![
                ev_response_created("resp-after-output-error"),
                ev_assistant_message("follow-up-message", "FOLLOW_UP_REPLY"),
                ev_completed("resp-after-output-error"),
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
        .submit(Op::Compact)
        .await
        .expect("submit compact operation");
    wait_for_event(&test.codex, |event| matches!(event, EventMsg::Error(_))).await;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    test.submit_turn("FOLLOW_UP_AFTER_OUTPUT_ERROR")
        .await
        .expect("submit follow-up after output error");

    assert_eq!(response_mock.requests().len(), 3);
    let follow_up_body =
        request_body_containing_user_text(&response_mock, "FOLLOW_UP_AFTER_OUTPUT_ERROR");
    assert_eq!(
        follow_up_body["previous_response_id"].as_str(),
        Some("resp-before-output-error")
    );
    let follow_up_body = follow_up_body.to_string();
    assert!(!follow_up_body.contains("FAILED_COMPACT_OUTPUT"));
    assert!(!follow_up_body.contains(SUMMARY_PREFIX));
    let rollout = std::fs::read_to_string(test.codex.rollout_path().expect("rollout path"))
        .expect("read rollout");
    assert!(!rollout.contains("FAILED_COMPACT_OUTPUT"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compact_completed_without_assistant_preserves_history_and_baseline() {
    skip_if_no_network!();

    let server = start_mock_server().await;
    let response_mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-before-empty-compact"),
                ev_assistant_message("first-message", "FIRST_REPLY"),
                ev_completed("resp-before-empty-compact"),
            ]),
            sse(vec![
                ev_response_created("empty-compact-response"),
                ev_completed("empty-compact-response"),
            ]),
            sse(vec![
                ev_response_created("resp-after-empty-compact"),
                ev_assistant_message("follow-up-message", "FOLLOW_UP_REPLY"),
                ev_completed("resp-after-empty-compact"),
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
        .submit(Op::Compact)
        .await
        .expect("submit compact operation");
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    test.submit_turn("FOLLOW_UP_AFTER_EMPTY_COMPACT")
        .await
        .expect("submit follow-up after empty compact");

    assert_eq!(response_mock.requests().len(), 3);
    let follow_up_body =
        request_body_containing_user_text(&response_mock, "FOLLOW_UP_AFTER_EMPTY_COMPACT");
    assert_eq!(
        follow_up_body["previous_response_id"].as_str(),
        Some("resp-before-empty-compact")
    );
    assert!(!follow_up_body.to_string().contains(SUMMARY_PREFIX));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupt_in_flight_compact_preserves_history_and_incremental_baseline() {
    skip_if_no_network!();

    let server = start_mock_server().await;
    let response_mock = mount_response_sequence(
        &server,
        vec![
            sse_response(sse(vec![
                ev_response_created("resp-before-compact-interrupt"),
                ev_assistant_message("first-message", "FIRST_REPLY"),
                ev_completed_with_tokens(
                    "resp-before-compact-interrupt",
                    /*total_tokens*/ 500,
                ),
            ])),
            sse_response(sse(vec![
                ev_assistant_message("compact-message", "INTERRUPTED_COMPACT_OUTPUT"),
                ev_completed("interrupted-compact-response"),
            ]))
            .set_delay(Duration::from_secs(30)),
            sse_response(sse(vec![
                ev_response_created("resp-after-compact-interrupt"),
                ev_assistant_message("follow-up-message", "FOLLOW_UP_REPLY"),
                ev_completed("resp-after-compact-interrupt"),
            ])),
        ],
    )
    .await;

    let mut builder = test_codex()
        .with_model(ACTIVE_MODEL)
        .with_model_info_override(FAILED_TURN_MODEL, |model_info| {
            model_info.comp_hash = Some("interrupting-turn-hash".to_string());
        })
        .with_model_info_override(ACTIVE_MODEL, |model_info| {
            model_info.comp_hash = Some("active-turn-hash".to_string());
            model_info.supports_incremental_requests = true;
        })
        .with_config(|config| {
            config.compact_model = Some(COMPACT_MODEL.to_string());
            config.compact_prompt = Some(COMPACT_PROMPT.to_string());
            config.model_provider.name = "Catalyst-compatible test provider".to_string();
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
                text: "TURN_THAT_TRIGGERS_INTERRUPTIBLE_COMPACT".to_string(),
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
        .expect("submit turn that triggers compact");
    tokio::time::timeout(Duration::from_secs(5), async {
        while response_mock.requests().len() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("compact request should reach the provider");

    test.codex
        .submit(Op::Interrupt)
        .await
        .expect("interrupt compact turn");
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnAborted(_))
    })
    .await;

    test.submit_turn("FOLLOW_UP_AFTER_COMPACT_INTERRUPT")
        .await
        .expect("submit follow-up after compact interrupt");

    assert_eq!(response_mock.requests().len(), 3);
    let follow_up_body =
        request_body_containing_user_text(&response_mock, "FOLLOW_UP_AFTER_COMPACT_INTERRUPT");
    assert_eq!(
        follow_up_body["previous_response_id"].as_str(),
        Some("resp-before-compact-interrupt")
    );
    let follow_up_body = follow_up_body.to_string();
    assert!(!follow_up_body.contains("INTERRUPTED_COMPACT_OUTPUT"));
    assert!(!follow_up_body.contains(SUMMARY_PREFIX));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compact_append_failure_preserves_live_state_and_baseline() {
    skip_if_no_network!();

    const STORE_ID: &str = "compact-append-failure-preserves-live-state";
    InMemoryThreadStore::remove_id(STORE_ID);
    let thread_store = InMemoryThreadStore::for_id(STORE_ID);
    let server = start_mock_server().await;
    let response_mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-before-append-failure"),
                ev_assistant_message("first-message", "FIRST_REPLY"),
                ev_completed("resp-before-append-failure"),
            ]),
            sse(vec![
                ev_assistant_message("compact-message", "UNCOMMITTED_COMPACT_SUMMARY"),
                ev_completed("compact-before-append-failure"),
            ]),
            sse(vec![
                ev_response_created("resp-after-append-failure"),
                ev_assistant_message("follow-up-message", "FOLLOW_UP_REPLY"),
                ev_completed("resp-after-append-failure"),
            ]),
        ],
    )
    .await;
    let commit_hook = CompactCommitTestHook::new();
    let mut builder = test_codex()
        .with_runtime_options(with_compact_commit_test_hook(
            ThreadManagerRuntimeOptions::default(),
            commit_hook.clone(),
        ))
        .with_model(ACTIVE_MODEL)
        .with_model_info_override(ACTIVE_MODEL, |model_info| {
            model_info.supports_incremental_requests = true;
        })
        .with_config(|config| {
            config.experimental_thread_store = ThreadStoreConfig::InMemory {
                id: STORE_ID.to_string(),
            };
            config.compact_model = Some(COMPACT_MODEL.to_string());
            config.compact_prompt = Some(COMPACT_PROMPT.to_string());
            config.model_provider.name = "Catalyst-compatible test provider".to_string();
        });
    let test = builder
        .build_with_auto_env(&server)
        .await
        .expect("build codex");

    test.submit_turn("FIRST_USER_TURN")
        .await
        .expect("submit first user turn");
    test.codex
        .submit(Op::Compact)
        .await
        .expect("submit compact operation");
    tokio::time::timeout(
        Duration::from_secs(5),
        commit_hook.wait_until_commit_paused(),
    )
    .await
    .expect("compact commit should reach the deterministic pause");
    thread_store
        .fail_next_append("injected compact transaction append failure")
        .await;
    commit_hook.release_commit();

    let terminal = wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::Error(_) | EventMsg::ItemCompleted(_))
    })
    .await;
    assert!(
        matches!(terminal, EventMsg::Error(_)),
        "append failure must surface instead of completing compaction: {terminal:?}"
    );
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    test.submit_turn("FOLLOW_UP_AFTER_APPEND_FAILURE")
        .await
        .expect("submit follow-up after append failure");

    assert_eq!(response_mock.requests().len(), 3);
    let follow_up_body =
        request_body_containing_user_text(&response_mock, "FOLLOW_UP_AFTER_APPEND_FAILURE");
    assert_eq!(
        follow_up_body["previous_response_id"].as_str(),
        Some("resp-before-append-failure")
    );
    let follow_up_body = follow_up_body.to_string();
    assert!(!follow_up_body.contains("UNCOMMITTED_COMPACT_SUMMARY"));
    assert!(!follow_up_body.contains(SUMMARY_PREFIX));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compact_post_record_append_error_reconciles_as_committed() {
    skip_if_no_network!();

    const STORE_ID: &str = "compact-post-record-error-reconciles";
    InMemoryThreadStore::remove_id(STORE_ID);
    let thread_store = InMemoryThreadStore::for_id(STORE_ID);
    let server = start_mock_server().await;
    let response_mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-before-post-record-error"),
                ev_assistant_message("first-message", "FIRST_REPLY"),
                ev_completed_with_tokens("resp-before-post-record-error", 5_000),
            ]),
            sse(vec![
                ev_assistant_message("compact-message", "RECONCILED_COMPACT_SUMMARY"),
                ev_completed_with_tokens("compact-post-record-error", 100),
            ]),
            sse(vec![
                ev_response_created("resp-after-post-record-error"),
                ev_assistant_message("follow-up-message", "FOLLOW_UP_REPLY"),
                ev_completed("resp-after-post-record-error"),
            ]),
        ],
    )
    .await;
    let commit_hook = CompactCommitTestHook::new();
    let mut builder = in_memory_atomic_compact_builder(STORE_ID, commit_hook.clone());
    let test = builder
        .build_with_auto_env(&server)
        .await
        .expect("build codex");

    test.submit_turn("FIRST_USER_TURN")
        .await
        .expect("submit first user turn");
    test.codex
        .submit(Op::Compact)
        .await
        .expect("submit compact operation");
    tokio::time::timeout(
        Duration::from_secs(5),
        commit_hook.wait_until_commit_paused(),
    )
    .await
    .expect("compact commit should reach the deterministic pause");
    thread_store
        .fail_next_append_after_items(1, "injected error after durable checkpoint record")
        .await;
    commit_hook.release_commit();

    let terminal = wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::Error(_) | EventMsg::ItemCompleted(_))
    })
    .await;
    assert!(
        matches!(terminal, EventMsg::ItemCompleted(_)),
        "a durable checkpoint discovered by reconciliation must commit: {terminal:?}"
    );
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let compacted = only_atomic_compaction_checkpoint(&test, &thread_store).await;
    let checkpoint = compacted.checkpoint.expect("nested checkpoint state");
    assert!(!checkpoint.checkpoint_id.is_empty());
    let api_token_info = checkpoint
        .api_token_count
        .info
        .expect("API checkpoint token usage");
    assert_eq!(api_token_info.total_token_usage.total_tokens, 5_100);
    assert_eq!(api_token_info.last_token_usage.total_tokens, 100);
    assert_eq!(
        checkpoint
            .final_token_count
            .info
            .expect("final checkpoint token usage")
            .total_token_usage
            .total_tokens,
        5_100
    );

    test.submit_turn("FOLLOW_UP_AFTER_RECONCILED_COMPACT")
        .await
        .expect("submit follow-up after reconciled compact");

    assert_eq!(response_mock.requests().len(), 3);
    let follow_up =
        request_body_containing_user_text(&response_mock, "FOLLOW_UP_AFTER_RECONCILED_COMPACT");
    assert!(follow_up.get("previous_response_id").is_none());
    assert!(follow_up.to_string().contains("RECONCILED_COMPACT_SUMMARY"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compact_metadata_projection_failure_is_best_effort() {
    skip_if_no_network!();

    const STORE_ID: &str = "compact-metadata-projection-failure";
    InMemoryThreadStore::remove_id(STORE_ID);
    let thread_store = InMemoryThreadStore::for_id(STORE_ID);
    let server = start_mock_server().await;
    let response_mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-before-metadata-failure"),
                ev_assistant_message("first-message", "FIRST_REPLY"),
                ev_completed("resp-before-metadata-failure"),
            ]),
            sse(vec![
                ev_assistant_message("compact-message", "METADATA_FAILURE_COMPACT_SUMMARY"),
                ev_completed("compact-metadata-failure"),
            ]),
            sse(vec![
                ev_response_created("resp-after-metadata-failure"),
                ev_assistant_message("follow-up-message", "FOLLOW_UP_REPLY"),
                ev_completed("resp-after-metadata-failure"),
            ]),
        ],
    )
    .await;
    let commit_hook = CompactCommitTestHook::new();
    let mut builder = in_memory_atomic_compact_builder(STORE_ID, commit_hook.clone());
    let test = builder
        .build_with_auto_env(&server)
        .await
        .expect("build codex");

    test.submit_turn("FIRST_USER_TURN")
        .await
        .expect("submit first user turn");
    test.codex
        .submit(Op::Compact)
        .await
        .expect("submit compact operation");
    tokio::time::timeout(
        Duration::from_secs(5),
        commit_hook.wait_until_commit_paused(),
    )
    .await
    .expect("compact commit should reach the deterministic pause");
    thread_store
        .fail_next_metadata_update("injected metadata projection failure")
        .await;
    commit_hook.release_commit();

    let terminal = wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::Error(_) | EventMsg::ItemCompleted(_))
    })
    .await;
    assert!(
        matches!(terminal, EventMsg::ItemCompleted(_)),
        "metadata projection must not turn a durable commit into failure: {terminal:?}"
    );
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    only_atomic_compaction_checkpoint(&test, &thread_store).await;
    test.submit_turn("FOLLOW_UP_AFTER_METADATA_FAILURE")
        .await
        .expect("submit follow-up after metadata failure");

    assert_eq!(response_mock.requests().len(), 3);
    let follow_up =
        request_body_containing_user_text(&response_mock, "FOLLOW_UP_AFTER_METADATA_FAILURE");
    assert!(follow_up.get("previous_response_id").is_none());
    assert!(
        follow_up
            .to_string()
            .contains("METADATA_FAILURE_COMPACT_SUMMARY")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ambiguous_compact_persistence_quarantines_until_reload() {
    skip_if_no_network!();

    const STORE_ID: &str = "ambiguous-compact-persistence";
    InMemoryThreadStore::remove_id(STORE_ID);
    let thread_store = InMemoryThreadStore::for_id(STORE_ID);
    let server = start_mock_server().await;
    let response_mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-before-ambiguous-commit"),
                ev_assistant_message("first-message", "FIRST_REPLY"),
                ev_completed("resp-before-ambiguous-commit"),
            ]),
            sse(vec![
                ev_assistant_message("compact-message", "AMBIGUOUS_COMPACT_SUMMARY"),
                ev_completed("compact-ambiguous-commit"),
            ]),
            sse(vec![
                ev_response_created("resp-after-ambiguous-commit"),
                ev_assistant_message("follow-up-message", "FOLLOW_UP_REPLY"),
                ev_completed("resp-after-ambiguous-commit"),
            ]),
        ],
    )
    .await;
    let commit_hook = CompactCommitTestHook::new();
    let mut builder = in_memory_atomic_compact_builder(STORE_ID, commit_hook.clone());
    let test = builder
        .build_with_auto_env(&server)
        .await
        .expect("build codex");

    test.submit_turn("FIRST_USER_TURN")
        .await
        .expect("submit first user turn");
    test.codex
        .submit(Op::Compact)
        .await
        .expect("submit compact operation");
    tokio::time::timeout(
        Duration::from_secs(5),
        commit_hook.wait_until_commit_paused(),
    )
    .await
    .expect("compact commit should reach the deterministic pause");
    let loads_before = thread_store.calls().await.load_history;
    thread_store
        .fail_next_append_after_items(1, "injected uncertain durable append")
        .await;
    thread_store
        .fail_next_history_loads(3, "injected reconciliation read failure")
        .await;
    commit_hook.release_commit();

    let terminal = wait_for_event(&test.codex, |event| matches!(event, EventMsg::Error(_))).await;
    let EventMsg::Error(error) = terminal else {
        unreachable!("event filter guarantees an error")
    };
    assert!(
        error.message.contains("persistence outcome is uncertain"),
        "ambiguous durability needs a distinct fatal error: {}",
        error.message
    );
    assert!(error.message.contains("restart"));
    assert_eq!(
        thread_store.calls().await.load_history - loads_before,
        3,
        "reconciliation attempts must be bounded"
    );
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    only_atomic_compaction_checkpoint(&test, &thread_store).await;
    Box::pin(assert_ambiguous_session_quarantined_then_reload_succeeds(
        &test,
        thread_store.as_ref(),
        &response_mock,
    ))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inline_compact_append_failure_restores_incremental_baseline_once() {
    skip_if_no_network!();

    const STORE_ID: &str = "inline-compact-append-failure-restores-baseline";
    InMemoryThreadStore::remove_id(STORE_ID);
    let thread_store = InMemoryThreadStore::for_id(STORE_ID);
    let server = start_mock_server().await;
    let response_mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-before-inline-append-failure"),
                ev_assistant_message("first-message", "FIRST_REPLY"),
                ev_completed("resp-before-inline-append-failure"),
            ]),
            sse(vec![
                ev_assistant_message("compact-message", "UNCOMMITTED_INLINE_SUMMARY"),
                ev_completed("compact-before-inline-append-failure"),
            ]),
            sse(vec![
                ev_response_created("resp-after-inline-append-failure"),
                ev_assistant_message("follow-up-message", "FOLLOW_UP_REPLY"),
                ev_completed("resp-after-inline-append-failure"),
            ]),
            sse(vec![
                ev_response_created("resp-after-second-follow-up"),
                ev_assistant_message("second-follow-up-message", "SECOND_FOLLOW_UP_REPLY"),
                ev_completed("resp-after-second-follow-up"),
            ]),
        ],
    )
    .await;
    let commit_hook = CompactCommitTestHook::new();
    let mut builder = test_codex()
        .with_runtime_options(with_compact_commit_test_hook(
            ThreadManagerRuntimeOptions::default(),
            commit_hook.clone(),
        ))
        .with_model(ACTIVE_MODEL)
        .with_model_info_override(FAILED_TURN_MODEL, |model_info| {
            model_info.comp_hash = Some("failed-turn-hash".to_string());
        })
        .with_model_info_override(ACTIVE_MODEL, |model_info| {
            model_info.comp_hash = Some("active-turn-hash".to_string());
            model_info.supports_incremental_requests = true;
        })
        .with_config(|config| {
            config.experimental_thread_store = ThreadStoreConfig::InMemory {
                id: STORE_ID.to_string(),
            };
            config.compact_model = Some(COMPACT_MODEL.to_string());
            config.compact_prompt = Some(COMPACT_PROMPT.to_string());
            config.model_provider.name = "Catalyst-compatible test provider".to_string();
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
                text: "TURN_THAT_TRIGGERS_INLINE_APPEND_FAILURE".to_string(),
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
        .expect("submit turn that triggers inline compact");
    tokio::time::timeout(
        Duration::from_secs(5),
        commit_hook.wait_until_commit_paused(),
    )
    .await
    .expect("inline compact commit should reach the deterministic pause");
    thread_store
        .fail_next_append("injected inline compact transaction append failure")
        .await;
    commit_hook.release_commit();
    wait_for_event(&test.codex, |event| matches!(event, EventMsg::Error(_))).await;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    test.submit_turn("FOLLOW_UP_AFTER_INLINE_APPEND_FAILURE")
        .await
        .expect("submit follow-up after inline append failure");
    test.submit_turn("SECOND_FOLLOW_UP_AFTER_INLINE_APPEND_FAILURE")
        .await
        .expect("submit second follow-up after inline append failure");

    assert_eq!(response_mock.requests().len(), 4);
    let first_follow_up =
        request_body_containing_user_text(&response_mock, "FOLLOW_UP_AFTER_INLINE_APPEND_FAILURE");
    assert_eq!(
        first_follow_up["previous_response_id"].as_str(),
        Some("resp-before-inline-append-failure")
    );
    assert!(!first_follow_up.to_string().contains(SUMMARY_PREFIX));
    let second_follow_up = request_body_containing_user_text(
        &response_mock,
        "SECOND_FOLLOW_UP_AFTER_INLINE_APPEND_FAILURE",
    );
    assert_eq!(
        second_follow_up["previous_response_id"].as_str(),
        Some("resp-after-inline-append-failure")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inline_compact_join_error_restores_incremental_baseline_once() {
    skip_if_no_network!();

    let server = start_mock_server().await;
    let response_mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-before-inline-join-error"),
                ev_assistant_message("first-message", "FIRST_REPLY"),
                ev_completed("resp-before-inline-join-error"),
            ]),
            sse(vec![
                ev_assistant_message("compact-message", "UNCOMMITTED_JOIN_ERROR_SUMMARY"),
                ev_completed("compact-before-inline-join-error"),
            ]),
            sse(vec![
                ev_response_created("resp-after-inline-join-error"),
                ev_assistant_message("follow-up-message", "FOLLOW_UP_REPLY"),
                ev_completed("resp-after-inline-join-error"),
            ]),
            sse(vec![
                ev_response_created("resp-after-join-error-second-follow-up"),
                ev_assistant_message("second-follow-up-message", "SECOND_FOLLOW_UP_REPLY"),
                ev_completed("resp-after-join-error-second-follow-up"),
            ]),
        ],
    )
    .await;
    let commit_hook = CompactCommitTestHook::new();
    let mut builder = test_codex()
        .with_runtime_options(with_compact_commit_test_hook(
            ThreadManagerRuntimeOptions::default(),
            commit_hook.clone(),
        ))
        .with_model(ACTIVE_MODEL)
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
                text: "TURN_THAT_TRIGGERS_INLINE_JOIN_ERROR".to_string(),
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
        .expect("submit turn that triggers inline compact");
    tokio::time::timeout(
        Duration::from_secs(5),
        commit_hook.wait_until_commit_paused(),
    )
    .await
    .expect("inline compact commit should reach the deterministic pause");
    commit_hook.panic_commit_once();
    commit_hook.release_commit();
    wait_for_event(&test.codex, |event| matches!(event, EventMsg::Error(_))).await;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    test.submit_turn("FOLLOW_UP_AFTER_INLINE_JOIN_ERROR")
        .await
        .expect("submit follow-up after inline join error");
    test.submit_turn("SECOND_FOLLOW_UP_AFTER_INLINE_JOIN_ERROR")
        .await
        .expect("submit second follow-up after inline join error");

    assert_eq!(response_mock.requests().len(), 4);
    let first_follow_up =
        request_body_containing_user_text(&response_mock, "FOLLOW_UP_AFTER_INLINE_JOIN_ERROR");
    assert_eq!(
        first_follow_up["previous_response_id"].as_str(),
        Some("resp-before-inline-join-error")
    );
    assert!(!first_follow_up.to_string().contains(SUMMARY_PREFIX));
    let second_follow_up = request_body_containing_user_text(
        &response_mock,
        "SECOND_FOLLOW_UP_AFTER_INLINE_JOIN_ERROR",
    );
    assert_eq!(
        second_follow_up["previous_response_id"].as_str(),
        Some("resp-after-inline-join-error")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compact_commit_survives_forced_parent_abort_and_cold_resume_matches() {
    skip_if_no_network!();

    let server = start_mock_server().await;
    let response_mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-before-commit-abort"),
                ev_assistant_message("first-message", "FIRST_REPLY"),
                ev_completed_with_tokens("resp-before-commit-abort", /*total_tokens*/ 5_000),
            ]),
            sse(vec![
                ev_assistant_message("compact-message", "COMMITTED_COMPACT_SUMMARY"),
                ev_completed_with_tokens("committed-compact-response", /*total_tokens*/ 100),
            ]),
            sse(vec![
                ev_response_created("resp-after-commit-abort"),
                ev_assistant_message("live-follow-up-message", "LIVE_FOLLOW_UP_REPLY"),
                ev_completed("resp-after-commit-abort"),
            ]),
            sse(vec![
                ev_response_created("resp-after-cold-resume"),
                ev_assistant_message("resumed-follow-up-message", "RESUMED_FOLLOW_UP_REPLY"),
                ev_completed("resp-after-cold-resume"),
            ]),
        ],
    )
    .await;
    let home = Arc::new(TempDir::new().expect("temp codex home"));
    let commit_hook = CompactCommitTestHook::new();
    let mut builder = test_codex()
        .with_home(Arc::clone(&home))
        .with_runtime_options(with_compact_commit_test_hook(
            ThreadManagerRuntimeOptions::default(),
            commit_hook.clone(),
        ))
        .with_model(ACTIVE_MODEL)
        .with_model_info_override(ACTIVE_MODEL, |model_info| {
            model_info.supports_incremental_requests = true;
        })
        .with_config(|config| {
            config.compact_model = Some(COMPACT_MODEL.to_string());
            config.compact_prompt = Some(COMPACT_PROMPT.to_string());
            config.model_provider.name = "Catalyst-compatible test provider".to_string();
            config.model_auto_compact_token_limit = Some(4_000);
        });
    let test = builder
        .build_with_auto_env(&server)
        .await
        .expect("build codex");

    test.submit_turn("FIRST_USER_TURN")
        .await
        .expect("submit first user turn");
    test.codex
        .submit(Op::Compact)
        .await
        .expect("submit compact operation");
    tokio::time::timeout(
        Duration::from_secs(5),
        commit_hook.wait_until_commit_paused(),
    )
    .await
    .expect("compact commit should reach the deterministic pause");

    test.codex
        .submit(Op::Interrupt)
        .await
        .expect("interrupt compact commit parent");
    tokio::time::timeout(
        Duration::from_secs(5),
        commit_hook.wait_until_parent_wait_dropped(),
    )
    .await
    .expect("compact parent should be forcibly aborted while commit is paused");
    commit_hook.release_commit();
    tokio::time::timeout(
        Duration::from_secs(5),
        commit_hook.wait_until_commit_completed(),
    )
    .await
    .expect("shielded compact commit should finish after its parent is aborted");
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnAborted(_))
    })
    .await;

    let compacted_token_info = test
        .codex
        .token_usage_info()
        .await
        .expect("live token usage after shielded compact commit");
    assert_eq!(compacted_token_info.total_token_usage.total_tokens, 5_100);
    assert!(
        compacted_token_info.last_token_usage.total_tokens < 4_000,
        "replacement history must be below the configured compaction threshold: {compacted_token_info:?}"
    );

    test.submit_turn("LIVE_FOLLOW_UP_AFTER_COMMIT_ABORT")
        .await
        .expect("submit live follow-up");
    let live_body =
        request_body_containing_user_text(&response_mock, "LIVE_FOLLOW_UP_AFTER_COMMIT_ABORT");
    assert_eq!(response_mock.requests().len(), 3);
    assert!(live_body.get("previous_response_id").is_none());
    assert!(live_body.to_string().contains("COMMITTED_COMPACT_SUMMARY"));
    let live_token_info = test
        .codex
        .token_usage_info()
        .await
        .expect("live token usage before cold resume");
    assert_eq!(live_token_info.total_token_usage.total_tokens, 5_100);

    test.codex.flush_rollout().await.expect("flush rollout");
    let rollout_path = test.codex.rollout_path().expect("rollout path");
    let replacement_history = replacement_history_from_rollout(&rollout_path);
    let live_input = live_body["input"].as_array().expect("live input array");
    assert_eq!(
        live_input.get(..replacement_history.len()),
        Some(replacement_history.as_slice()),
        "live history should begin with the committed replacement"
    );

    test.codex
        .submit(Op::Shutdown)
        .await
        .expect("shut down live thread");
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::ShutdownComplete)
    })
    .await;
    let resumed_cwd = test.config.cwd.clone();
    let mut resume_builder = test_codex()
        .with_home(Arc::clone(&home))
        .with_model(ACTIVE_MODEL)
        .with_model_info_override(ACTIVE_MODEL, |model_info| {
            model_info.supports_incremental_requests = true;
        })
        .with_config(move |config| {
            config.cwd = resumed_cwd;
            config.compact_model = Some(COMPACT_MODEL.to_string());
            config.compact_prompt = Some(COMPACT_PROMPT.to_string());
            config.model_provider.name = "Catalyst-compatible test provider".to_string();
            config.model_auto_compact_token_limit = Some(4_000);
        });
    let resumed = resume_builder
        .resume(&server, Arc::clone(&home), rollout_path)
        .await
        .expect("cold resume compacted thread");
    let resumed_token_info = resumed
        .codex
        .token_usage_info()
        .await
        .expect("cold-resumed token usage after shielded compact commit");
    assert_eq!(resumed_token_info, live_token_info);
    resumed
        .submit_turn("COLD_RESUME_FOLLOW_UP_AFTER_COMMIT_ABORT")
        .await
        .expect("submit cold-resume follow-up");

    assert_eq!(response_mock.requests().len(), 4);
    let resumed_body = request_body_containing_user_text(
        &response_mock,
        "COLD_RESUME_FOLLOW_UP_AFTER_COMMIT_ABORT",
    );
    assert!(resumed_body.get("previous_response_id").is_none());
    let resumed_input = resumed_body["input"]
        .as_array()
        .expect("resumed input array");
    assert_eq!(
        resumed_input.get(..replacement_history.len()),
        Some(replacement_history.as_slice()),
        "cold resume should begin with the persisted replacement"
    );
    assert_eq!(
        resumed_input.get(..live_input.len()),
        Some(live_input.as_slice()),
        "cold resume should replay the live post-commit structured prefix"
    );
}

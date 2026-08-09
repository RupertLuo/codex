use codex_core::InjectIfRunningRejectionReason;
use codex_core::SteerInputError;
use codex_core::ThreadManagerRuntimeOptions;
use codex_core::TryStartTurnIfIdleRejectionReason;
use codex_core::compact::SUMMARY_PREFIX;
use codex_core::config::ThreadStoreConfig;
use codex_core::test_support::CompactCommitTestHook;
use codex_core::test_support::RealtimeStartTestHook;
use codex_core::test_support::auth_manager_from_auth;
use codex_core::test_support::clear_reference_context_item_for_direct_mutation_test;
use codex_core::test_support::conversation_history_for_test;
use codex_core::test_support::direct_mutation_test_snapshot;
use codex_core::test_support::inject_no_new_turn_for_test;
use codex_core::test_support::with_compact_commit_test_hook;
use codex_core::test_support::with_realtime_start_test_hook;
use codex_login::CodexAuth;
use codex_protocol::AgentPath;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::AgentMessageEvent;
use codex_protocol::protocol::CompactedItem;
use codex_protocol::protocol::ConversationStartParams;
use codex_protocol::protocol::ConversationTextParams;
use codex_protocol::protocol::ConversationTextRole;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::InitialHistory;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::RealtimeConversationRealtimeEvent;
use codex_protocol::protocol::RealtimeEvent;
use codex_protocol::protocol::RealtimeOutputModality;
use codex_protocol::protocol::ResumedHistory;
use codex_protocol::protocol::RolloutItem;
use codex_protocol::protocol::RolloutLine;
use codex_protocol::user_input::UserInput;
use codex_thread_store::InMemoryThreadStore;
use codex_thread_store::LoadThreadHistoryParams;
use codex_thread_store::ThreadStore;
use codex_thread_store::ThreadTitleGenerator;
use codex_thread_store::ThreadTitleRequest;
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
use core_test_support::responses::start_websocket_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::TestCodexBuilder;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::Semaphore;

const ACTIVE_MODEL: &str = "gpt-5.4";
const COMPACT_MODEL: &str = "deepseek/deepseek-v4-flash";
const COMPACT_PROMPT: &str = "Summarize the conversation as durable text state.";
const FAILED_TURN_MODEL: &str = "gpt-5.3-codex";
const IMAGE_URL: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";

#[derive(Debug)]
struct BlockingTitleGenerator {
    started: Semaphore,
    release: Semaphore,
    returned: Semaphore,
}

impl BlockingTitleGenerator {
    fn new() -> Self {
        Self {
            started: Semaphore::new(0),
            release: Semaphore::new(0),
            returned: Semaphore::new(0),
        }
    }

    async fn wait_until_started(&self) {
        self.started
            .acquire()
            .await
            .expect("title generator started semaphore should remain open")
            .forget();
    }

    fn release(&self) {
        self.release.add_permits(1);
    }

    async fn wait_until_returned(&self) {
        self.returned
            .acquire()
            .await
            .expect("title generator returned semaphore should remain open")
            .forget();
    }
}

impl ThreadTitleGenerator for BlockingTitleGenerator {
    fn generate_title<'a>(
        &'a self,
        _request: ThreadTitleRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>> {
        Box::pin(async move {
            self.started.add_permits(1);
            self.release
                .acquire()
                .await
                .expect("title generator release semaphore should remain open")
                .forget();
            self.returned.add_permits(1);
            Some("TITLE_MUST_NOT_APPLY_AFTER_QUARANTINE".to_string())
        })
    }
}

fn injected_assistant_item(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: "assistant".to_string(),
        content: vec![ContentItem::OutputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

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

fn in_memory_atomic_compact_realtime_builder(
    store_id: &'static str,
    commit_hook: CompactCommitTestHook,
    realtime_hook: RealtimeStartTestHook,
    realtime_base_url: String,
) -> TestCodexBuilder {
    let runtime_options = with_realtime_start_test_hook(
        with_compact_commit_test_hook(ThreadManagerRuntimeOptions::default(), commit_hook),
        realtime_hook,
    );
    test_codex()
        .with_runtime_options(runtime_options)
        .with_auth(CodexAuth::from_api_key("dummy"))
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
            config.experimental_realtime_ws_base_url = Some(realtime_base_url);
        })
}

fn realtime_start_params() -> ConversationStartParams {
    ConversationStartParams {
        client_managed_handoffs: false,
        codex_responses_as_items: false,
        codex_response_item_prefix: None,
        codex_response_handoff_prefix: None,
        model: None,
        output_modality: RealtimeOutputModality::Audio,
        include_startup_context: false,
        prompt: Some(Some("backend prompt".to_string())),
        realtime_session_id: None,
        transport: None,
        version: None,
        voice: None,
    }
}

async fn start_realtime_race_server() -> core_test_support::responses::WebSocketTestServer {
    start_websocket_server(vec![vec![
        vec![json!({
            "type": "session.updated",
            "session": { "id": "sess_atomicity_race", "instructions": "backend prompt" }
        })],
        vec![],
        vec![],
        vec![],
    ]])
    .await
}

async fn start_realtime_natural_close_server() -> core_test_support::responses::WebSocketTestServer
{
    start_websocket_server(vec![vec![
        vec![json!({
            "type": "session.updated",
            "session": { "id": "sess_natural_close_race", "instructions": "backend prompt" }
        })],
        vec![],
    ]])
    .await
}

fn realtime_closed_count(items: &[RolloutItem]) -> usize {
    items
        .iter()
        .filter(|item| {
            matches!(
                item,
                RolloutItem::EventMsg(EventMsg::RealtimeConversationClosed(_))
            )
        })
        .count()
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

async fn wait_for_ambiguous_compact_owner_to_settle(test: &TestCodex) {
    let compact_error = tokio::time::timeout(
        Duration::from_secs(5),
        wait_for_event(&test.codex, |event| {
            matches!(
                event,
                EventMsg::Error(error)
                    if error.message.contains("persistence outcome is uncertain")
            )
        }),
    )
    .await
    .expect("ambiguous compact should report its fatal persistence error");
    assert!(
        matches!(compact_error, EventMsg::Error(_)),
        "event filter guarantees the compact fatal error"
    );

    // TurnComplete is delivered just before the task boundary clears active_turn. Wait on the
    // actual state transition so snapshots cannot attribute the compact owner's normal cleanup to
    // the operation deliberately paused before the lifecycle gate.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if !direct_mutation_test_snapshot(&test.codex)
                .await
                .has_active_turn
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("ambiguous compact owner should clear its active turn");

    let flush_error = test
        .codex
        .flush_rollout()
        .await
        .expect_err("ambiguous compact should quarantine the session");
    assert!(flush_error.to_string().contains("quarantined"));
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
async fn user_input_waiting_before_start_gate_cannot_mutate_after_quarantine() {
    skip_if_no_network!();

    const STORE_ID: &str = "ambiguous-compact-user-input-before-start-gate";
    InMemoryThreadStore::remove_id(STORE_ID);
    let thread_store = InMemoryThreadStore::for_id(STORE_ID);
    let server = start_mock_server().await;
    let response_mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-before-user-input-race"),
                ev_assistant_message("first-message", "FIRST_REPLY"),
                ev_completed("resp-before-user-input-race"),
            ]),
            sse(vec![
                ev_assistant_message("compact-message", "UNCOMMITTED_RACE_SUMMARY"),
                ev_completed("compact-user-input-race"),
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
    .expect("compact commit should pause before lifecycle ownership");

    commit_hook.pause_task_start_before_gate_once();
    let racing_turn_id = test
        .codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "USER_INPUT_WAITING_BEFORE_START_GATE".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await
        .expect("submit racing user input");
    tokio::time::timeout(
        Duration::from_secs(5),
        commit_hook.wait_until_task_start_before_gate_paused(),
    )
    .await
    .expect("user input should pause immediately before the lifecycle gate");

    thread_store
        .fail_next_append("injected checkpoint failure before durable write")
        .await;
    thread_store
        .fail_next_history_loads(3, "injected reconciliation read failure")
        .await;
    commit_hook.release_commit();
    wait_for_ambiguous_compact_owner_to_settle(&test).await;

    let state_after_quarantine = direct_mutation_test_snapshot(&test.codex).await;
    let calls_after_quarantine = thread_store.calls().await;
    let requests_after_quarantine = response_mock.requests().len();
    commit_hook.release_task_start_before_gate();

    let terminal = tokio::time::timeout(
        Duration::from_secs(5),
        wait_for_event(&test.codex, |event| {
            matches!(event, EventMsg::Error(_))
                || matches!(
                    event,
                    EventMsg::TurnStarted(started) if started.turn_id == racing_turn_id
                )
        }),
    )
    .await
    .expect("resumed user input should terminate explicitly");
    assert!(
        matches!(terminal, EventMsg::Error(ref error) if error.message.contains("quarantined")),
        "quarantine must win before any turn starts: {terminal:?}"
    );
    assert_eq!(
        direct_mutation_test_snapshot(&test.codex).await,
        state_after_quarantine,
        "resumed user input must not reserve a turn or queue input"
    );
    assert_eq!(
        thread_store.calls().await,
        calls_after_quarantine,
        "resumed user input must not append after quarantine"
    );
    assert_eq!(
        response_mock.requests().len(),
        requests_after_quarantine,
        "resumed user input must not start an HTTP model request"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_steer_waiting_before_gate_cannot_revive_start_first_quarantine_cleanup() {
    skip_if_no_network!();

    const STORE_ID: &str = "ambiguous-compact-public-steer-before-gate";
    InMemoryThreadStore::remove_id(STORE_ID);
    let thread_store = InMemoryThreadStore::for_id(STORE_ID);
    let server = start_mock_server().await;
    let response_mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-before-public-steer-race"),
                ev_assistant_message("first-message", "FIRST_REPLY"),
                ev_completed("resp-before-public-steer-race"),
            ]),
            sse(vec![
                ev_assistant_message("compact-message", "UNCOMMITTED_STEER_SUMMARY"),
                ev_completed("compact-public-steer-race"),
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
    .expect("compact commit should pause before lifecycle ownership");

    test.codex
        .submit(Op::Interrupt)
        .await
        .expect("interrupt compact parent while detached commit remains paused");
    tokio::time::timeout(
        Duration::from_secs(5),
        commit_hook.wait_until_parent_wait_dropped(),
    )
    .await
    .expect("compact parent should drop while detached commit remains paused");
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnAborted(_))
    })
    .await;

    let active_turn_id = test
        .codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "START_WINS_BEFORE_AMBIGUOUS_QUARANTINE".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await
        .expect("submit start-first user input");
    tokio::time::timeout(
        Duration::from_secs(5),
        wait_for_event(&test.codex, |event| {
            matches!(event, EventMsg::TurnStarted(started) if started.turn_id == active_turn_id)
        }),
    )
    .await
    .expect("new regular turn should start before quarantine");

    commit_hook.pause_task_start_before_gate_once();
    let codex = Arc::clone(&test.codex);
    let expected_turn_id = active_turn_id.clone();
    let steer_task = tokio::spawn(async move {
        codex
            .steer_input(
                vec![UserInput::Text {
                    text: "STEER_WAITING_BEFORE_GATE".to_string(),
                    text_elements: Vec::new(),
                }],
                Default::default(),
                Some(expected_turn_id.as_str()),
                None,
                None,
            )
            .await
    });
    tokio::time::timeout(
        Duration::from_secs(5),
        commit_hook.wait_until_task_start_before_gate_paused(),
    )
    .await
    .expect("public steer should pause before lifecycle ownership");

    thread_store
        .fail_next_append("injected checkpoint failure before durable write")
        .await;
    thread_store
        .fail_next_history_loads(3, "injected reconciliation read failure")
        .await;
    commit_hook.release_commit();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match test.codex.flush_rollout().await {
                Err(err) if err.to_string().contains("quarantined") => break,
                _ => tokio::task::yield_now().await,
            }
        }
    })
    .await
    .expect("ambiguous checkpoint should quarantine and clean the active task");

    let state_after_quarantine = direct_mutation_test_snapshot(&test.codex).await;
    assert!(
        !state_after_quarantine.has_active_turn && !state_after_quarantine.has_pending_input,
        "start-first quarantine must raw-cancel active and pending turn state: {state_after_quarantine:?}"
    );
    let calls_after_quarantine = thread_store.calls().await;
    let requests_after_quarantine = response_mock.requests().len();
    commit_hook.release_task_start_before_gate();
    let steer_error = steer_task
        .await
        .expect("public steer task should not panic")
        .expect_err("public steer must reject after quarantine");
    assert!(
        matches!(
            steer_error,
            SteerInputError::PersistenceQuarantined { ref message }
                if message.contains("quarantined")
        ),
        "public steer should explain the lifecycle quarantine: {steer_error:?}"
    );
    assert_eq!(
        direct_mutation_test_snapshot(&test.codex).await,
        state_after_quarantine,
        "rejected steer must not revive or queue work"
    );
    assert_eq!(thread_store.calls().await, calls_after_quarantine);
    assert_eq!(response_mock.requests().len(), requests_after_quarantine);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trigger_mailbox_waiting_before_gate_cannot_enqueue_or_start_after_quarantine() {
    skip_if_no_network!();

    const STORE_ID: &str = "ambiguous-compact-trigger-mailbox-before-gate";
    InMemoryThreadStore::remove_id(STORE_ID);
    let thread_store = InMemoryThreadStore::for_id(STORE_ID);
    let server = start_mock_server().await;
    let response_mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-before-trigger-mailbox-race"),
                ev_assistant_message("first-message", "FIRST_REPLY"),
                ev_completed("resp-before-trigger-mailbox-race"),
            ]),
            sse(vec![
                ev_assistant_message("compact-message", "UNCOMMITTED_MAILBOX_SUMMARY"),
                ev_completed("compact-trigger-mailbox-race"),
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
    .expect("compact commit should pause before lifecycle ownership");

    commit_hook.pause_task_start_before_gate_once();
    test.codex
        .submit(Op::InterAgentCommunication {
            communication: codex_protocol::protocol::InterAgentCommunication::new(
                AgentPath::try_from("/root/worker").expect("valid worker path"),
                AgentPath::root(),
                Vec::new(),
                "TRIGGER_MAIL_WAITING_BEFORE_GATE".to_string(),
                /*trigger_turn*/ true,
            ),
        })
        .await
        .expect("submit trigger-turn mailbox mail");
    tokio::time::timeout(
        Duration::from_secs(5),
        commit_hook.wait_until_task_start_before_gate_paused(),
    )
    .await
    .expect("trigger mailbox handler should pause before lifecycle ownership");

    thread_store
        .fail_next_append("injected checkpoint failure before durable write")
        .await;
    thread_store
        .fail_next_history_loads(3, "injected reconciliation read failure")
        .await;
    commit_hook.release_commit();
    wait_for_ambiguous_compact_owner_to_settle(&test).await;

    let state_after_quarantine = direct_mutation_test_snapshot(&test.codex).await;
    let calls_after_quarantine = thread_store.calls().await;
    let requests_after_quarantine = response_mock.requests().len();
    commit_hook.release_task_start_before_gate();
    let terminal = tokio::time::timeout(
        Duration::from_secs(5),
        wait_for_event(&test.codex, |event| matches!(event, EventMsg::Error(_))),
    )
    .await
    .expect("mailbox rejection should be delivered explicitly");
    assert!(
        matches!(terminal, EventMsg::Error(ref error) if error.message.contains("quarantined"))
    );
    assert_eq!(
        direct_mutation_test_snapshot(&test.codex).await,
        state_after_quarantine,
        "rejected trigger mail must not enqueue or reserve a turn"
    );
    assert_eq!(thread_store.calls().await, calls_after_quarantine);
    assert_eq!(response_mock.requests().len(), requests_after_quarantine);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_history_append_failure_leaves_live_and_cold_history_unchanged() {
    skip_if_no_network!();

    const STORE_ID: &str = "ordinary-history-append-failure-durable-first";
    InMemoryThreadStore::remove_id(STORE_ID);
    let thread_store = InMemoryThreadStore::for_id(STORE_ID);
    let server = start_mock_server().await;
    mount_sse_sequence(
        &server,
        vec![sse(vec![
            ev_response_created("resp-before-history-append-failure"),
            ev_assistant_message("first-message", "FIRST_REPLY"),
            ev_completed("resp-before-history-append-failure"),
        ])],
    )
    .await;
    let mut builder = test_codex().with_model(ACTIVE_MODEL).with_config(|config| {
        config.experimental_thread_store = ThreadStoreConfig::InMemory {
            id: STORE_ID.to_string(),
        };
    });
    let test = builder
        .build_with_auto_env(&server)
        .await
        .expect("build codex");
    test.submit_turn("FIRST_USER_TURN")
        .await
        .expect("submit first user turn");

    let live_before = conversation_history_for_test(&test.codex).await;
    let durable_before = ThreadStore::load_history(
        thread_store.as_ref(),
        LoadThreadHistoryParams {
            thread_id: test.session_configured.thread_id,
            include_archived: true,
        },
    )
    .await
    .expect("load durable history before append failure");
    thread_store
        .fail_next_append("injected ordinary history append failure")
        .await;
    let _ = test
        .codex
        .inject_response_items(vec![injected_assistant_item(
            "MUST_NOT_EXIST_AFTER_APPEND_FAILURE",
        )])
        .await;

    assert_eq!(
        conversation_history_for_test(&test.codex).await,
        live_before,
        "failed durable append must not install live conversation history"
    );
    let durable_after = ThreadStore::load_history(
        thread_store.as_ref(),
        LoadThreadHistoryParams {
            thread_id: test.session_configured.thread_id,
            include_archived: true,
        },
    )
    .await
    .expect("load durable history after append failure");
    assert_eq!(
        serde_json::to_value(&durable_after.items).expect("serialize durable history after"),
        serde_json::to_value(&durable_before.items).expect("serialize durable history before"),
        "failed history transaction must not leave raw-only durable companions"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inject_no_active_turn_waiting_before_gate_matches_cold_history_after_quarantine() {
    skip_if_no_network!();

    const STORE_ID: &str = "inject-no-active-turn-before-history-gate";
    InMemoryThreadStore::remove_id(STORE_ID);
    let thread_store = InMemoryThreadStore::for_id(STORE_ID);
    let server = start_mock_server().await;
    mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-before-inject-no-active-race"),
                ev_assistant_message("first-message", "FIRST_REPLY"),
                ev_completed("resp-before-inject-no-active-race"),
            ]),
            sse(vec![
                ev_assistant_message("compact-message", "UNCOMMITTED_INJECT_SUMMARY"),
                ev_completed("compact-inject-no-active-race"),
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
    .expect("compact commit should pause before lifecycle ownership");
    test.codex
        .submit(Op::Interrupt)
        .await
        .expect("interrupt compact parent");
    tokio::time::timeout(
        Duration::from_secs(5),
        commit_hook.wait_until_parent_wait_dropped(),
    )
    .await
    .expect("compact parent should drop while detached commit remains paused");
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnAborted(_))
    })
    .await;

    commit_hook.pause_task_start_before_gate_once();
    let codex = Arc::clone(&test.codex);
    let inject_task = tokio::spawn(async move {
        inject_no_new_turn_for_test(
            &codex,
            vec![injected_assistant_item(
                "INJECT_MUST_NOT_EXIST_AFTER_QUARANTINE",
            )],
        )
        .await;
    });
    tokio::time::timeout(
        Duration::from_secs(5),
        commit_hook.wait_until_task_start_before_gate_paused(),
    )
    .await
    .expect("NoActiveTurn injection should pause before lifecycle ownership");

    thread_store
        .fail_next_append("injected checkpoint failure before durable write")
        .await;
    thread_store
        .fail_next_history_loads(3, "injected reconciliation read failure")
        .await;
    commit_hook.release_commit();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match test.codex.flush_rollout().await {
                Err(err) if err.to_string().contains("quarantined") => break,
                _ => tokio::task::yield_now().await,
            }
        }
    })
    .await
    .expect("not-durable ambiguous checkpoint should quarantine");
    let live_after_quarantine = conversation_history_for_test(&test.codex).await;
    let durable_history = ThreadStore::load_history(
        thread_store.as_ref(),
        LoadThreadHistoryParams {
            thread_id: test.session_configured.thread_id,
            include_archived: true,
        },
    )
    .await
    .expect("load durable history after quarantine");
    commit_hook.release_task_start_before_gate();
    inject_task.await.expect("injection task should not panic");
    assert_eq!(
        conversation_history_for_test(&test.codex).await,
        live_after_quarantine,
        "rejected NoActiveTurn injection must leave live history unchanged"
    );

    let thread_id = test.session_configured.thread_id;
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
            None,
            false,
        )
        .await
        .expect("cold resume from durable history");
    assert_eq!(
        conversation_history_for_test(&resumed.thread).await,
        live_after_quarantine,
        "quarantined live history must equal a true cold reconstruction"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detached_title_generator_cannot_read_or_update_metadata_after_quarantine() {
    skip_if_no_network!();

    const STORE_ID: &str = "detached-title-generator-quarantine-gate";
    InMemoryThreadStore::remove_id(STORE_ID);
    let thread_store = InMemoryThreadStore::for_id(STORE_ID);
    let server = start_mock_server().await;
    mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-before-title-race"),
                ev_assistant_message("first-message", "FIRST_REPLY"),
                ev_completed("resp-before-title-race"),
            ]),
            sse(vec![
                ev_assistant_message("compact-message", "UNCOMMITTED_TITLE_SUMMARY"),
                ev_completed("compact-title-race"),
            ]),
        ],
    )
    .await;
    let commit_hook = CompactCommitTestHook::new();
    let title_generator = Arc::new(BlockingTitleGenerator::new());
    let runtime_options =
        with_compact_commit_test_hook(ThreadManagerRuntimeOptions::default(), commit_hook.clone())
            .with_title_generator(title_generator.clone());
    let mut builder = test_codex()
        .with_runtime_options(runtime_options)
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
    tokio::time::timeout(Duration::from_secs(5), title_generator.wait_until_started())
        .await
        .expect("real title generator should start after the first assistant turn");

    test.codex
        .submit(Op::Compact)
        .await
        .expect("submit compact operation");
    tokio::time::timeout(
        Duration::from_secs(5),
        commit_hook.wait_until_commit_paused(),
    )
    .await
    .expect("compact commit should pause before lifecycle ownership");
    thread_store
        .fail_next_append("injected checkpoint failure before durable write")
        .await;
    thread_store
        .fail_next_history_loads(3, "injected reconciliation read failure")
        .await;
    commit_hook.release_commit();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match test.codex.flush_rollout().await {
                Err(err) if err.to_string().contains("quarantined") => break,
                _ => tokio::task::yield_now().await,
            }
        }
    })
    .await
    .expect("ambiguous checkpoint should quarantine before title generation resumes");

    let calls_after_quarantine = thread_store.calls().await;
    title_generator.release();
    tokio::time::timeout(
        Duration::from_secs(5),
        title_generator.wait_until_returned(),
    )
    .await
    .expect("blocked title generator should return after release");
    tokio::time::timeout(Duration::from_millis(250), async {
        loop {
            let calls = thread_store.calls().await;
            if calls.read_thread != calls_after_quarantine.read_thread
                || calls.update_thread_metadata != calls_after_quarantine.update_thread_metadata
            {
                panic!(
                    "detached title task crossed quarantine: before={calls_after_quarantine:?}, after={calls:?}"
                );
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect_err("counter observation window should end without metadata access");
    let calls_after_release = thread_store.calls().await;
    assert_eq!(
        calls_after_release.read_thread, calls_after_quarantine.read_thread,
        "quarantined title task must not read thread metadata"
    );
    assert_eq!(
        calls_after_release.update_thread_metadata, calls_after_quarantine.update_thread_metadata,
        "quarantined title task must not update thread metadata"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ambiguous_compaction_linearizes_after_in_flight_append_and_blocks_later_appends() {
    skip_if_no_network!();

    const STORE_ID: &str = "ambiguous-compact-blocked-append-race";
    const ORDINARY_APPEND: &str = "ORDINARY_APPEND_ALREADY_IN_FLIGHT";
    InMemoryThreadStore::remove_id(STORE_ID);
    let thread_store = InMemoryThreadStore::for_id(STORE_ID);
    let server = start_mock_server().await;
    mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-before-blocked-append-race"),
                ev_assistant_message("first-message", "FIRST_REPLY"),
                ev_completed("resp-before-blocked-append-race"),
            ]),
            sse(vec![
                ev_assistant_message("compact-message", "RACE_COMPACT_SUMMARY"),
                ev_completed("compact-blocked-append-race"),
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

    let append_blocker = thread_store.block_next_append().await;
    let codex = Arc::clone(&test.codex);
    let append_task = tokio::spawn(async move {
        codex
            .append_rollout_items(&[RolloutItem::EventMsg(EventMsg::AgentMessage(
                AgentMessageEvent {
                    message: ORDINARY_APPEND.to_string(),
                    phase: None,
                    memory_citation: None,
                },
            ))])
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), append_blocker.wait_until_blocked())
        .await
        .expect("ordinary append should pause inside the backing store");

    // These controls remain unclaimed by the already-entered ordinary append and therefore force
    // the following checkpoint append into the Ambiguous outcome.
    thread_store
        .fail_next_append_after_items(1, "injected uncertain durable checkpoint")
        .await;
    thread_store
        .fail_next_history_loads(3, "injected reconciliation read failure")
        .await;
    commit_hook.release_commit();

    // The old TOCTOU implementation can finish Ambiguous quarantine while the ordinary append is
    // still blocked. The lifecycle gate makes this timeout: compaction must wait for the side
    // effect that linearized first. Preserve either observed event so cleanup and the final durable
    // ordering assertion run on both RED and GREEN implementations.
    let premature_error = tokio::time::timeout(
        Duration::from_millis(250),
        wait_for_event(&test.codex, |event| matches!(event, EventMsg::Error(_))),
    )
    .await
    .ok();

    append_blocker.release();
    append_task
        .await
        .expect("ordinary append task should not panic")
        .expect("ordinary append that linearized first should complete");
    let error_event = match premature_error {
        Some(event) => event,
        None => wait_for_event(&test.codex, |event| matches!(event, EventMsg::Error(_))).await,
    };
    let EventMsg::Error(error) = error_event else {
        unreachable!("event filter guarantees an error")
    };
    assert!(error.message.contains("persistence outcome is uncertain"));
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let history = ThreadStore::load_history(
        thread_store.as_ref(),
        LoadThreadHistoryParams {
            thread_id: test.session_configured.thread_id,
            include_archived: true,
        },
    )
    .await
    .expect("load durable history after the race");
    let ordinary_index = history
        .items
        .iter()
        .position(|item| {
            matches!(
                item,
                RolloutItem::EventMsg(EventMsg::AgentMessage(event))
                    if event.message == ORDINARY_APPEND
            )
        })
        .expect("ordinary append should be durable");
    let checkpoint_index = history
        .items
        .iter()
        .position(|item| {
            matches!(
                item,
                RolloutItem::Compacted(compacted) if compacted.checkpoint.is_some()
            )
        })
        .expect("uncertain checkpoint should be durable");
    assert!(
        ordinary_index < checkpoint_index,
        "no ordinary append may land after the checkpoint whose outcome quarantined the session"
    );

    let calls_before_rejected_append = thread_store.calls().await.append_items;
    let rejected = test
        .codex
        .append_rollout_items(&[RolloutItem::EventMsg(EventMsg::AgentMessage(
            AgentMessageEvent {
                message: "APPEND_AFTER_QUARANTINE".to_string(),
                phase: None,
                memory_citation: None,
            },
        ))])
        .await
        .expect_err("quarantined session must reject later append entrypoints");
    assert!(rejected.to_string().contains("quarantined"));
    assert_eq!(
        thread_store.calls().await.append_items,
        calls_before_rejected_append,
        "rejected append must not reach the backing store"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn realtime_prepared_before_quarantine_cannot_connect_after_ambiguous_checkpoint() {
    skip_if_no_network!();

    const STORE_ID: &str = "ambiguous-compact-realtime-before-gate";
    InMemoryThreadStore::remove_id(STORE_ID);
    let thread_store = InMemoryThreadStore::for_id(STORE_ID);
    let response_server = start_mock_server().await;
    let realtime_server = start_realtime_race_server().await;
    mount_sse_sequence(
        &response_server,
        vec![
            sse(vec![
                ev_response_created("resp-before-realtime-race"),
                ev_assistant_message("first-message", "FIRST_REPLY"),
                ev_completed("resp-before-realtime-race"),
            ]),
            sse(vec![
                ev_assistant_message("compact-message", "REALTIME_RACE_SUMMARY"),
                ev_completed("compact-realtime-race"),
            ]),
        ],
    )
    .await;
    let commit_hook = CompactCommitTestHook::new();
    let realtime_hook = RealtimeStartTestHook::new();
    realtime_hook.pause_before_gate_once();
    let mut builder = in_memory_atomic_compact_realtime_builder(
        STORE_ID,
        commit_hook.clone(),
        realtime_hook.clone(),
        realtime_server.uri().to_string(),
    );
    let test = builder
        .build_with_auto_env(&response_server)
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
    .expect("compact commit should pause before lifecycle ownership");
    test.codex
        .submit(Op::RealtimeConversationStart(realtime_start_params()))
        .await
        .expect("submit realtime start");
    tokio::time::timeout(
        Duration::from_secs(5),
        realtime_hook.wait_until_before_gate_paused(),
    )
    .await
    .expect("realtime start should pause after prepare and before lifecycle gate");

    thread_store
        .fail_next_append_after_items(1, "injected uncertain durable checkpoint")
        .await;
    thread_store
        .fail_next_history_loads(3, "injected reconciliation read failure")
        .await;
    commit_hook.release_commit();
    let compact_error =
        wait_for_event(&test.codex, |event| matches!(event, EventMsg::Error(_))).await;
    let EventMsg::Error(compact_error) = compact_error else {
        unreachable!("event filter guarantees an error")
    };
    assert!(
        compact_error
            .message
            .contains("persistence outcome is uncertain")
    );

    realtime_hook.release_before_gate();
    let realtime_terminal = tokio::time::timeout(
        Duration::from_secs(5),
        wait_for_event(&test.codex, |event| {
            matches!(
                event,
                EventMsg::RealtimeConversationStarted(_)
                    | EventMsg::RealtimeConversationRealtime(RealtimeConversationRealtimeEvent {
                        payload: RealtimeEvent::Error(_),
                    })
            )
        }),
    )
    .await
    .expect("realtime start should terminate explicitly after quarantine");
    assert!(
        matches!(
            realtime_terminal,
            EventMsg::RealtimeConversationRealtime(RealtimeConversationRealtimeEvent {
                payload: RealtimeEvent::Error(ref message),
            }) if message.contains("quarantined")
        ),
        "quarantine must reject realtime before connect: {realtime_terminal:?}"
    );
    assert_eq!(
        realtime_server.handshakes().len(),
        0,
        "a prepared realtime start must not connect after quarantine linearizes"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn realtime_that_linearizes_first_is_closed_by_later_ambiguous_checkpoint() {
    skip_if_no_network!();

    const STORE_ID: &str = "ambiguous-compact-realtime-after-gate";
    InMemoryThreadStore::remove_id(STORE_ID);
    let thread_store = InMemoryThreadStore::for_id(STORE_ID);
    let response_server = start_mock_server().await;
    let realtime_server = start_realtime_race_server().await;
    mount_sse_sequence(
        &response_server,
        vec![
            sse(vec![
                ev_response_created("resp-before-realtime-first"),
                ev_assistant_message("first-message", "FIRST_REPLY"),
                ev_completed("resp-before-realtime-first"),
            ]),
            sse(vec![
                ev_assistant_message("compact-message", "REALTIME_FIRST_SUMMARY"),
                ev_completed("compact-realtime-first"),
            ]),
        ],
    )
    .await;
    let commit_hook = CompactCommitTestHook::new();
    let realtime_hook = RealtimeStartTestHook::new();
    realtime_hook.pause_after_gate_once();
    let mut builder = in_memory_atomic_compact_realtime_builder(
        STORE_ID,
        commit_hook.clone(),
        realtime_hook.clone(),
        realtime_server.uri().to_string(),
    );
    let test = builder
        .build_with_auto_env(&response_server)
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
    .expect("compact commit should pause before lifecycle ownership");
    test.codex
        .submit(Op::RealtimeConversationStart(realtime_start_params()))
        .await
        .expect("submit realtime start");
    tokio::time::timeout(
        Duration::from_secs(5),
        realtime_hook.wait_until_after_gate_paused(),
    )
    .await
    .expect("realtime should pause after lifecycle linearization and before connect");

    thread_store
        .fail_next_append_after_items(1, "injected uncertain durable checkpoint")
        .await;
    thread_store
        .fail_next_history_loads(3, "injected reconciliation read failure")
        .await;
    commit_hook.release_commit();
    let premature_compact_error = tokio::time::timeout(
        Duration::from_millis(250),
        wait_for_event(&test.codex, |event| matches!(event, EventMsg::Error(_))),
    )
    .await
    .ok();

    realtime_hook.release_after_gate();
    tokio::time::timeout(
        Duration::from_secs(5),
        wait_for_event(&test.codex, |event| {
            matches!(event, EventMsg::RealtimeConversationStarted(_))
        }),
    )
    .await
    .expect("the realtime start that linearized first should connect");
    let mut compact_error = premature_compact_error.and_then(|event| match event {
        EventMsg::Error(error) => Some(error),
        _ => None,
    });
    let mut saw_realtime_closed = false;
    tokio::time::timeout(Duration::from_secs(5), async {
        while compact_error.is_none() || !saw_realtime_closed {
            let event = test
                .codex
                .next_event()
                .await
                .expect("event stream should remain open");
            match event.msg {
                EventMsg::Error(error) => compact_error = Some(error),
                EventMsg::RealtimeConversationClosed(_) => saw_realtime_closed = true,
                _ => {}
            }
        }
    })
    .await
    .expect("quarantine must report the compact failure and close realtime");
    let compact_error = compact_error.expect("compact failure should be observed");
    assert!(
        compact_error
            .message
            .contains("persistence outcome is uncertain")
    );
    assert_eq!(
        realtime_server.handshakes().len(),
        1,
        "the first-linearized realtime start should connect exactly once before quarantine closes it"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn realtime_quarantine_first_close_has_one_live_and_durable_winner() {
    skip_if_no_network!();

    const STORE_ID: &str = "ambiguous-compact-realtime-quarantine-close-winner";
    InMemoryThreadStore::remove_id(STORE_ID);
    let thread_store = InMemoryThreadStore::for_id(STORE_ID);
    let response_server = start_mock_server().await;
    let realtime_server = start_realtime_race_server().await;
    mount_sse_sequence(
        &response_server,
        vec![
            sse(vec![
                ev_response_created("resp-before-quarantine-close-race"),
                ev_assistant_message("first-message", "FIRST_REPLY"),
                ev_completed("resp-before-quarantine-close-race"),
            ]),
            sse(vec![
                ev_assistant_message("compact-message", "QUARANTINE_CLOSE_RACE_SUMMARY"),
                ev_completed("compact-quarantine-close-race"),
            ]),
        ],
    )
    .await;
    let commit_hook = CompactCommitTestHook::new();
    let realtime_hook = RealtimeStartTestHook::new();
    let mut builder = in_memory_atomic_compact_realtime_builder(
        STORE_ID,
        commit_hook.clone(),
        realtime_hook.clone(),
        realtime_server.uri().to_string(),
    );
    let test = builder
        .build_with_auto_env(&response_server)
        .await
        .expect("build codex");

    test.submit_turn("FIRST_USER_TURN")
        .await
        .expect("submit first user turn");
    test.codex
        .submit(Op::RealtimeConversationStart(realtime_start_params()))
        .await
        .expect("start realtime conversation");
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::RealtimeConversationStarted(_))
    })
    .await;
    test.codex
        .submit(Op::Compact)
        .await
        .expect("submit compact operation");
    tokio::time::timeout(
        Duration::from_secs(5),
        commit_hook.wait_until_commit_paused(),
    )
    .await
    .expect("compact commit should pause before lifecycle ownership");

    realtime_hook.pause_close_before_gate_once();
    test.codex
        .submit(Op::RealtimeConversationClose)
        .await
        .expect("submit explicit realtime close");
    tokio::time::timeout(
        Duration::from_secs(5),
        realtime_hook.wait_until_close_before_gate_paused(),
    )
    .await
    .expect("explicit close should pause before lifecycle ownership");

    thread_store
        .fail_next_append("injected checkpoint failure before durable write")
        .await;
    thread_store
        .fail_next_history_loads(3, "injected reconciliation read failure")
        .await;
    commit_hook.release_commit();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match test.codex.flush_rollout().await {
                Err(err) if err.to_string().contains("quarantined") => break,
                _ => tokio::task::yield_now().await,
            }
        }
    })
    .await
    .expect("ambiguous checkpoint should quarantine the session");
    let first_closed = wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::RealtimeConversationClosed(_))
    })
    .await;
    assert!(matches!(
        first_closed,
        EventMsg::RealtimeConversationClosed(ref event)
            if event.reason.as_deref() == Some("persistence_quarantine")
    ));

    realtime_hook.release_close_before_gate();
    let duplicate_closed = tokio::time::timeout(
        Duration::from_millis(250),
        wait_for_event(&test.codex, |event| {
            matches!(event, EventMsg::RealtimeConversationClosed(_))
        }),
    )
    .await;
    assert!(
        duplicate_closed.is_err(),
        "quarantine and explicit close must not both deliver Closed: {duplicate_closed:?}"
    );

    let durable_history = ThreadStore::load_history(
        thread_store.as_ref(),
        LoadThreadHistoryParams {
            thread_id: test.session_configured.thread_id,
            include_archived: true,
        },
    )
    .await
    .expect("load durable history after close race");
    assert_eq!(realtime_closed_count(&durable_history.items), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn realtime_explicit_first_close_has_one_live_and_durable_winner() {
    skip_if_no_network!();

    const STORE_ID: &str = "ambiguous-compact-realtime-explicit-close-winner";
    InMemoryThreadStore::remove_id(STORE_ID);
    let thread_store = InMemoryThreadStore::for_id(STORE_ID);
    let response_server = start_mock_server().await;
    let realtime_server = start_realtime_race_server().await;
    mount_sse_sequence(
        &response_server,
        vec![
            sse(vec![
                ev_response_created("resp-before-explicit-close-race"),
                ev_assistant_message("first-message", "FIRST_REPLY"),
                ev_completed("resp-before-explicit-close-race"),
            ]),
            sse(vec![
                ev_assistant_message("compact-message", "EXPLICIT_CLOSE_RACE_SUMMARY"),
                ev_completed("compact-explicit-close-race"),
            ]),
        ],
    )
    .await;
    let commit_hook = CompactCommitTestHook::new();
    let realtime_hook = RealtimeStartTestHook::new();
    let mut builder = in_memory_atomic_compact_realtime_builder(
        STORE_ID,
        commit_hook.clone(),
        realtime_hook,
        realtime_server.uri().to_string(),
    );
    let test = builder
        .build_with_auto_env(&response_server)
        .await
        .expect("build codex");

    test.submit_turn("FIRST_USER_TURN")
        .await
        .expect("submit first user turn");
    test.codex
        .submit(Op::RealtimeConversationStart(realtime_start_params()))
        .await
        .expect("start realtime conversation");
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::RealtimeConversationStarted(_))
    })
    .await;
    test.codex
        .submit(Op::Compact)
        .await
        .expect("submit compact operation");
    tokio::time::timeout(
        Duration::from_secs(5),
        commit_hook.wait_until_commit_paused(),
    )
    .await
    .expect("compact commit should pause before lifecycle ownership");

    test.codex
        .submit(Op::RealtimeConversationClose)
        .await
        .expect("submit explicit realtime close");
    let first_closed = wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::RealtimeConversationClosed(_))
    })
    .await;
    assert!(matches!(
        first_closed,
        EventMsg::RealtimeConversationClosed(ref event)
            if event.reason.as_deref() == Some("requested")
    ));

    thread_store
        .fail_next_append("injected checkpoint failure before durable write")
        .await;
    thread_store
        .fail_next_history_loads(3, "injected reconciliation read failure")
        .await;
    commit_hook.release_commit();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match test.codex.flush_rollout().await {
                Err(err) if err.to_string().contains("quarantined") => break,
                _ => tokio::task::yield_now().await,
            }
        }
    })
    .await
    .expect("ambiguous checkpoint should quarantine after explicit close");

    let duplicate_closed = tokio::time::timeout(
        Duration::from_millis(250),
        wait_for_event(&test.codex, |event| {
            matches!(event, EventMsg::RealtimeConversationClosed(_))
        }),
    )
    .await;
    assert!(
        duplicate_closed.is_err(),
        "later quarantine must not deliver another Closed: {duplicate_closed:?}"
    );
    let durable_history = ThreadStore::load_history(
        thread_store.as_ref(),
        LoadThreadHistoryParams {
            thread_id: test.session_configured.thread_id,
            include_archived: true,
        },
    )
    .await
    .expect("load durable history after explicit-first close race");
    assert_eq!(realtime_closed_count(&durable_history.items), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn realtime_natural_and_explicit_close_share_one_durable_winner() {
    skip_if_no_network!();

    const STORE_ID: &str = "realtime-natural-explicit-close-winner";
    InMemoryThreadStore::remove_id(STORE_ID);
    let thread_store = InMemoryThreadStore::for_id(STORE_ID);
    let response_server = start_mock_server().await;
    let realtime_server = start_realtime_natural_close_server().await;
    let commit_hook = CompactCommitTestHook::new();
    let realtime_hook = RealtimeStartTestHook::new();
    realtime_hook.pause_close_after_claim_once();
    let mut builder = in_memory_atomic_compact_realtime_builder(
        STORE_ID,
        commit_hook,
        realtime_hook.clone(),
        realtime_server.uri().to_string(),
    );
    let test = builder
        .build_with_auto_env(&response_server)
        .await
        .expect("build codex");

    test.codex
        .submit(Op::RealtimeConversationStart(realtime_start_params()))
        .await
        .expect("start realtime conversation");
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::RealtimeConversationStarted(_))
    })
    .await;
    wait_for_event(&test.codex, |event| {
        matches!(
            event,
            EventMsg::RealtimeConversationRealtime(RealtimeConversationRealtimeEvent {
                payload: RealtimeEvent::SessionUpdated { .. },
            })
        )
    })
    .await;
    test.codex
        .submit(Op::RealtimeConversationText(ConversationTextParams {
            text: "trigger natural transport close".to_string(),
            role: ConversationTextRole::User,
        }))
        .await
        .expect("send final realtime request");
    tokio::time::timeout(
        Duration::from_secs(5),
        realtime_hook.wait_until_close_after_claim_paused(),
    )
    .await
    .expect("natural close should pause after claiming the active conversation");

    test.codex
        .submit(Op::RealtimeConversationClose)
        .await
        .expect("race explicit close against natural close");
    realtime_hook.release_close_after_claim();

    let first_closed = wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::RealtimeConversationClosed(_))
    })
    .await;
    assert!(matches!(
        first_closed,
        EventMsg::RealtimeConversationClosed(ref event)
            if event.reason.as_deref() == Some("transport_closed")
                || event.reason.as_deref() == Some("requested")
    ));
    let duplicate_closed = tokio::time::timeout(
        Duration::from_millis(250),
        wait_for_event(&test.codex, |event| {
            matches!(event, EventMsg::RealtimeConversationClosed(_))
        }),
    )
    .await;
    assert!(
        duplicate_closed.is_err(),
        "natural and explicit close must not both deliver Closed: {duplicate_closed:?}"
    );

    let durable_history = ThreadStore::load_history(
        thread_store.as_ref(),
        LoadThreadHistoryParams {
            thread_id: test.session_configured.thread_id,
            include_archived: true,
        },
    )
    .await
    .expect("load durable history after natural close race");
    assert_eq!(realtime_closed_count(&durable_history.items), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn realtime_closed_append_failure_is_not_delivered_and_is_retryable() {
    skip_if_no_network!();

    const STORE_ID: &str = "realtime-close-append-failure-retry";
    InMemoryThreadStore::remove_id(STORE_ID);
    let thread_store = InMemoryThreadStore::for_id(STORE_ID);
    let response_server = start_mock_server().await;
    let realtime_server = start_realtime_race_server().await;
    let commit_hook = CompactCommitTestHook::new();
    let realtime_hook = RealtimeStartTestHook::new();
    let mut builder = in_memory_atomic_compact_realtime_builder(
        STORE_ID,
        commit_hook,
        realtime_hook,
        realtime_server.uri().to_string(),
    );
    let test = builder
        .build_with_auto_env(&response_server)
        .await
        .expect("build codex");

    test.codex
        .submit(Op::RealtimeConversationStart(realtime_start_params()))
        .await
        .expect("start realtime conversation");
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::RealtimeConversationStarted(_))
    })
    .await;
    wait_for_event(&test.codex, |event| {
        matches!(
            event,
            EventMsg::RealtimeConversationRealtime(RealtimeConversationRealtimeEvent {
                payload: RealtimeEvent::SessionUpdated { .. },
            })
        )
    })
    .await;

    let appends_before_close = thread_store.calls().await.append_items;
    thread_store
        .fail_next_append("injected Closed append failure")
        .await;
    test.codex
        .submit(Op::RealtimeConversationClose)
        .await
        .expect("submit first explicit close");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if thread_store.calls().await.append_items > appends_before_close {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("first Closed append should reach the store and fail");

    let premature_closed = tokio::time::timeout(
        Duration::from_millis(250),
        wait_for_event(&test.codex, |event| {
            matches!(event, EventMsg::RealtimeConversationClosed(_))
        }),
    )
    .await;
    assert!(
        premature_closed.is_err(),
        "a failed Closed append must not produce a live-only event: {premature_closed:?}"
    );
    let history_after_failure = ThreadStore::load_history(
        thread_store.as_ref(),
        LoadThreadHistoryParams {
            thread_id: test.session_configured.thread_id,
            include_archived: true,
        },
    )
    .await
    .expect("load history after failed close");
    assert_eq!(realtime_closed_count(&history_after_failure.items), 0);

    test.codex
        .submit(Op::RealtimeConversationClose)
        .await
        .expect("retry explicit close");
    let closed = wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::RealtimeConversationClosed(_))
    })
    .await;
    assert!(matches!(
        closed,
        EventMsg::RealtimeConversationClosed(ref event)
            if event.reason.as_deref() == Some("requested")
    ));
    let history_after_retry = ThreadStore::load_history(
        thread_store.as_ref(),
        LoadThreadHistoryParams {
            thread_id: test.session_configured.thread_id,
            include_archived: true,
        },
    )
    .await
    .expect("load history after retried close");
    assert_eq!(realtime_closed_count(&history_after_retry.items), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_injection_apis_reject_quarantine_before_mutation_or_side_effect() {
    skip_if_no_network!();

    const STORE_ID: &str = "ambiguous-compact-direct-injection-gate";
    InMemoryThreadStore::remove_id(STORE_ID);
    let thread_store = InMemoryThreadStore::for_id(STORE_ID);
    let server = start_mock_server().await;
    let response_mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-before-direct-injection"),
                ev_assistant_message("first-message", "FIRST_REPLY"),
                ev_completed("resp-before-direct-injection"),
            ]),
            sse(vec![
                ev_assistant_message("compact-message", "DIRECT_INJECTION_SUMMARY"),
                ev_completed("compact-direct-injection"),
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
    .expect("compact commit should pause before lifecycle ownership");
    thread_store
        .fail_next_append_after_items(1, "injected uncertain durable checkpoint")
        .await;
    thread_store
        .fail_next_history_loads(3, "injected reconciliation read failure")
        .await;
    commit_hook.release_commit();
    let compact_error =
        wait_for_event(&test.codex, |event| matches!(event, EventMsg::Error(_))).await;
    let EventMsg::Error(compact_error) = compact_error else {
        unreachable!("event filter guarantees an error")
    };
    assert!(
        compact_error
            .message
            .contains("persistence outcome is uncertain")
    );
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_) | EventMsg::TurnAborted(_))
    })
    .await;

    clear_reference_context_item_for_direct_mutation_test(&test.codex).await;
    let state_before = direct_mutation_test_snapshot(&test.codex).await;
    let calls_before = thread_store.calls().await;
    let requests_before = response_mock.requests().len();

    let inject_response_error = test
        .codex
        .inject_response_items(vec![injected_assistant_item(
            "HISTORY_ONLY_AFTER_QUARANTINE",
        )])
        .await
        .expect_err("history-only injection must reject quarantine");
    assert!(inject_response_error.to_string().contains("quarantined"));

    let inject_running_error = test
        .codex
        .inject_if_running(vec![injected_assistant_item("RUNNING_AFTER_QUARANTINE")])
        .await
        .expect_err("active-turn injection must reject quarantine explicitly");
    assert_eq!(
        inject_running_error.reason(),
        InjectIfRunningRejectionReason::PersistenceQuarantined
    );

    let idle_start_error = test
        .codex
        .try_start_turn_if_idle(vec![injected_assistant_item("IDLE_AFTER_QUARANTINE")])
        .await
        .expect_err("idle start must reject quarantine explicitly");
    assert_eq!(
        idle_start_error.reason(),
        TryStartTurnIfIdleRejectionReason::PersistenceQuarantined
    );

    assert_eq!(
        direct_mutation_test_snapshot(&test.codex).await,
        state_before,
        "history, queue, and active-turn reservation must remain unchanged"
    );
    assert_eq!(
        thread_store.calls().await,
        calls_before,
        "rejected direct APIs must not reach the thread store"
    );
    assert_eq!(
        response_mock.requests().len(),
        requests_before,
        "rejected direct APIs must not start an HTTP model request"
    );
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

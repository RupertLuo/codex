use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use serde_json::json;
use wiremock::Mock;
use wiremock::Respond;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path_regex;

/// End-to-end regression test for the cache reset seen after an interrupted agent turn.
///
/// This drives the real Codex session loop through a Responses API mock, starts a real built-in
/// shell tool, interrupts it, and submits the next user turn. The final assertion is made on the
/// second HTTP request: losing `previous_response_id` forces providers such as Qwen to rebuild the
/// full context and report zero cached input tokens.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupt_preserves_completed_http_incremental_baseline() {
    let call_id = "call-incremental-interrupt";
    let args = json!({
        "command": "sleep 60",
        "timeout_ms": 60_000
    })
    .to_string();
    let first_body = sse(vec![
        ev_response_created("resp-before-interrupt"),
        ev_function_call(call_id, "shell_command", &args),
        ev_completed("resp-before-interrupt"),
    ]);
    let follow_up_body = sse(vec![
        ev_response_created("resp-after-interrupt"),
        ev_completed("resp-after-interrupt"),
    ]);

    let server = start_mock_server().await;
    mount_incremental_sse_sequence(&server, vec![first_body, follow_up_body]).await;
    let fixture = test_codex()
        .with_config(|config| {
            config.model_provider.name = "Qwen-compatible test provider".to_string();
        })
        .with_model_info_override("gpt-5.4", |model_info| {
            model_info.supports_incremental_requests = true;
        })
        .build_with_auto_env(&server)
        .await
        .expect("Qwen-compatible test session should start");
    let codex = Arc::clone(&fixture.codex);

    submit_text(&codex, "start incremental interrupt").await;
    wait_for_event(&codex, |event| {
        matches!(event, EventMsg::ExecCommandBegin(_))
    })
    .await;
    codex
        .submit(Op::Interrupt)
        .await
        .expect("interrupt should be accepted");
    wait_for_event(&codex, |event| matches!(event, EventMsg::TurnAborted(_))).await;

    submit_text(&codex, "follow up after interrupt").await;
    wait_for_event(&codex, |event| matches!(event, EventMsg::TurnComplete(_))).await;

    let requests = server.received_requests().await.unwrap_or_default();
    let requests = requests
        .iter()
        .filter(|request| request.url.path().ends_with("/responses"))
        .collect::<Vec<_>>();
    assert_eq!(requests.len(), 2, "expected two calls to the responses API");
    let follow_up_body: serde_json::Value =
        serde_json::from_slice(&requests[1].body).expect("follow-up request should be JSON");
    assert_eq!(
        follow_up_body["previous_response_id"].as_str(),
        Some("resp-before-interrupt"),
        "the follow-up after an interrupt should continue the completed provider response"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completed_response_checkpoint_continues_within_and_across_turns() {
    let args = json!({ "command": "echo ok" }).to_string();
    let first_body = sse(vec![
        ev_response_created("resp-tool-call"),
        ev_function_call("call-quick", "shell_command", &args),
        ev_completed("resp-tool-call"),
    ]);
    let tool_follow_up_body = sse(vec![
        ev_response_created("resp-tool-finished"),
        ev_completed("resp-tool-finished"),
    ]);
    let next_turn_body = sse(vec![
        ev_response_created("resp-next-turn"),
        ev_completed("resp-next-turn"),
    ]);

    let server = start_mock_server().await;
    mount_incremental_sse_sequence(
        &server,
        vec![first_body, tool_follow_up_body, next_turn_body],
    )
    .await;
    let fixture = test_codex()
        .with_config(|config| {
            config.model_provider.name = "Qwen-compatible test provider".to_string();
        })
        .with_model_info_override("gpt-5.4", |model_info| {
            model_info.supports_incremental_requests = true;
        })
        .build_with_auto_env(&server)
        .await
        .expect("Qwen-compatible test session should start");
    let codex = Arc::clone(&fixture.codex);

    submit_text(&codex, "run a quick tool").await;
    wait_for_event(&codex, |event| matches!(event, EventMsg::TurnComplete(_))).await;
    submit_text(&codex, "next turn").await;
    wait_for_event(&codex, |event| matches!(event, EventMsg::TurnComplete(_))).await;

    let requests = server.received_requests().await.unwrap_or_default();
    let bodies = requests
        .iter()
        .filter(|request| request.url.path().ends_with("/responses"))
        .map(|request| {
            serde_json::from_slice::<serde_json::Value>(&request.body)
                .expect("request should be JSON")
        })
        .collect::<Vec<_>>();
    assert_eq!(bodies.len(), 3, "expected three calls to the responses API");
    assert_eq!(
        bodies[1]["previous_response_id"].as_str(),
        Some("resp-tool-call"),
        "tool follow-up should continue the response that requested the tool"
    );
    assert_eq!(
        bodies[2]["previous_response_id"].as_str(),
        Some("resp-tool-finished"),
        "next user turn should continue the final response from the previous turn"
    );
}

async fn submit_text(codex: &codex_core::CodexThread, text: &str) {
    codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: text.to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await
        .expect("user input should be accepted");
}

async fn mount_incremental_sse_sequence(server: &wiremock::MockServer, bodies: Vec<String>) {
    struct SequenceResponder {
        calls: AtomicUsize,
        bodies: Vec<String>,
    }

    impl Respond for SequenceResponder {
        fn respond(&self, _request: &wiremock::Request) -> ResponseTemplate {
            let index = self.calls.fetch_add(1, Ordering::SeqCst);
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(self.bodies[index].clone())
        }
    }

    let expected_calls = bodies.len() as u64;
    Mock::given(method("POST"))
        .and(path_regex(".*/responses$"))
        .respond_with(SequenceResponder {
            calls: AtomicUsize::new(0),
            bodies,
        })
        .expect(expected_calls)
        .mount(server)
        .await;
}

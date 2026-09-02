use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use codex_core::ModelRuntimePolicy;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;
use wiremock::Mock;
use wiremock::Respond;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path_regex;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_policy_reuses_previous_response_id_across_http_turns() {
    let server = start_mock_server().await;
    mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-first"),
                ev_completed("resp-first"),
            ]),
            sse(vec![
                ev_response_created("resp-second"),
                ev_completed("resp-second"),
            ]),
        ],
    )
    .await;
    let fixture = test_codex()
        .with_model("gpt-5.4")
        .with_model_runtime_policy(
            "gpt-5.4",
            ModelRuntimePolicy {
                supports_http_incremental_requests: true,
                ..Default::default()
            },
        )
        .build_with_auto_env(&server)
        .await
        .expect("incremental HTTP test session should start");
    fixture.submit_turn("first turn").await.unwrap();
    fixture.submit_turn("second turn").await.unwrap();

    let requests = server.received_requests().await.unwrap_or_default();
    let bodies = requests
        .iter()
        .filter(|request| request.url.path().ends_with("/responses"))
        .map(|request| serde_json::from_slice::<serde_json::Value>(&request.body).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(bodies.len(), 2);
    assert_eq!(
        bodies[1]["previous_response_id"].as_str(),
        Some("resp-first")
    );
    let second_input = bodies[1]["input"].as_array().unwrap();
    assert!(
        second_input
            .iter()
            .any(|item| item.to_string().contains("second turn"))
    );
    assert!(
        !second_input
            .iter()
            .any(|item| item.to_string().contains("first turn"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_policy_resends_full_history_when_previous_response_expires() {
    let server = start_mock_server().await;
    mount_response_sequence(
        &server,
        vec![
            sse_response("resp-first"),
            ResponseTemplate::new(404)
                .insert_header("content-type", "application/json")
                .set_body_string(r#"{"code":"previous_response_not_found"}"#),
            sse_response("resp-second"),
        ],
    )
    .await;
    let fixture = test_codex()
        .with_model("gpt-5.4")
        .with_model_runtime_policy(
            "gpt-5.4",
            ModelRuntimePolicy {
                supports_http_incremental_requests: true,
                ..Default::default()
            },
        )
        .build_with_auto_env(&server)
        .await
        .expect("incremental HTTP test session should start");
    fixture.submit_turn("first turn").await.unwrap();
    fixture.submit_turn("second turn").await.unwrap();

    let requests = server.received_requests().await.unwrap_or_default();
    let bodies = requests
        .iter()
        .filter(|request| request.url.path().ends_with("/responses"))
        .map(|request| serde_json::from_slice::<serde_json::Value>(&request.body).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(bodies.len(), 3);
    assert_eq!(
        bodies[1]["previous_response_id"].as_str(),
        Some("resp-first")
    );
    assert_eq!(bodies[2].get("previous_response_id"), None);
    let full_retry = bodies[2]["input"].to_string();
    assert!(full_retry.contains("first turn"));
    assert!(full_retry.contains("second turn"));
}

async fn mount_sse_sequence(server: &wiremock::MockServer, bodies: Vec<String>) {
    mount_response_sequence(
        server,
        bodies
            .into_iter()
            .map(|body| {
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(body)
            })
            .collect(),
    )
    .await;
}

fn sse_response(response_id: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(sse(vec![
            ev_response_created(response_id),
            ev_completed(response_id),
        ]))
}

async fn mount_response_sequence(server: &wiremock::MockServer, responses: Vec<ResponseTemplate>) {
    struct SequenceResponder {
        calls: AtomicUsize,
        responses: Vec<ResponseTemplate>,
    }

    impl Respond for SequenceResponder {
        fn respond(&self, _request: &wiremock::Request) -> ResponseTemplate {
            let index = self.calls.fetch_add(1, Ordering::SeqCst);
            self.responses[index].clone()
        }
    }

    let expected_calls = responses.len() as u64;
    Mock::given(method("POST"))
        .and(path_regex(".*/responses$"))
        .respond_with(SequenceResponder {
            calls: AtomicUsize::new(0),
            responses,
        })
        .expect(expected_calls)
        .mount(server)
        .await;
}

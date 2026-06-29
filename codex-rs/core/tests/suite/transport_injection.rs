use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Result;
use codex_api::HttpTransportHandle;
use codex_api::Request;
use codex_api::Response;
use codex_api::StreamResponse;
use codex_api::TransportError;
use codex_core::ThreadManagerRuntimeOptions;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::sse;
use core_test_support::test_codex::test_codex;
use http::HeaderMap;
use http::StatusCode;
use pretty_assertions::assert_eq;
use tokio_util::bytes::Bytes;
use wiremock::MockServer;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn injected_transport_completes_native_agent_turn() -> Result<()> {
    let request_bodies = Arc::new(Mutex::new(Vec::new()));
    let transport = HttpTransportHandle::new(
        |_request: Request| async {
            Err::<Response, TransportError>(TransportError::Build(
                "execute should not be called".to_string(),
            ))
        },
        {
            let request_bodies = Arc::clone(&request_bodies);
            move |request: Request| {
                let request_bodies = Arc::clone(&request_bodies);
                async move {
                    let prepared = request
                        .prepare_body_for_send()
                        .map_err(TransportError::Build)?;
                    request_bodies
                        .lock()
                        .expect("request body lock")
                        .push(prepared.body_bytes());

                    let body = sse(vec![
                        ev_response_created("resp-injected"),
                        ev_assistant_message("msg-injected", "injected transport completed"),
                        ev_completed("resp-injected"),
                    ]);
                    Ok(StreamResponse {
                        status: StatusCode::OK,
                        headers: HeaderMap::new(),
                        bytes: Box::pin(futures::stream::iter([Ok(Bytes::from(body))])),
                    })
                }
            }
        },
    );
    let runtime_options = ThreadManagerRuntimeOptions::default().with_http_transport(transport);

    let server = MockServer::start().await;
    let test = test_codex()
        .with_runtime_options(runtime_options)
        .with_config(|config| {
            config.model_provider.supports_websockets = false;
        })
        .build(&server)
        .await?;

    test.submit_turn("hello through injected transport").await?;

    let bodies = request_bodies.lock().expect("request body lock");
    assert_eq!(bodies.len(), 1);
    let request: serde_json::Value = serde_json::from_slice(&bodies[0])?;
    assert!(
        request
            .to_string()
            .contains("hello through injected transport")
    );
    Ok(())
}

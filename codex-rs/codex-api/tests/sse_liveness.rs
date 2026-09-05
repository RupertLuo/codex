#![allow(clippy::expect_used)]
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use bytes::Bytes;
use codex_api::ApiError;
use codex_api::AuthProvider;
use codex_api::Compression;
use codex_api::Provider;
use codex_api::ResponseEvent;
use codex_api::ResponseStream;
use codex_api::ResponsesClient;
use codex_api::RetryConfig;
use codex_client::HttpTransport;
use codex_client::Request;
use codex_client::Response;
use codex_client::StreamResponse;
use codex_client::TransportError;
use futures::StreamExt;
use http::HeaderMap;
use http::StatusCode;
use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::timeout;

type Chunk = Result<Bytes, TransportError>;
const IDLE: Duration = Duration::from_millis(50);

#[derive(Clone)]
struct OpenTransport(Arc<Mutex<Option<mpsc::Receiver<Chunk>>>>);

impl HttpTransport for OpenTransport {
    async fn execute(&self, _: Request) -> Result<Response, TransportError> {
        panic!("streaming requests only");
    }

    async fn stream(&self, _: Request) -> Result<StreamResponse, TransportError> {
        let receiver = self.0.lock().expect("lock").take();
        let Some(receiver) = receiver else {
            return std::future::pending().await;
        };
        let bytes = futures::stream::unfold(receiver, |mut receiver| async move {
            receiver.recv().await.map(|chunk| (chunk, receiver))
        });
        Ok(StreamResponse {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            bytes: Box::pin(bytes),
        })
    }
}

struct NoAuth;

impl AuthProvider for NoAuth {
    fn add_auth_headers(&self, _: &mut HeaderMap) {}
}

async fn open_stream() -> (ResponseStream, mpsc::Sender<Chunk>) {
    let (sender, receiver) = mpsc::channel(16);
    let client = client(OpenTransport(Arc::new(Mutex::new(Some(receiver)))));
    let stream = client
        .stream(
            json!({}),
            HeaderMap::new(),
            Compression::None,
            /*turn_state*/ None,
        )
        .await
        .expect("open stream");
    (stream, sender)
}

fn client(transport: OpenTransport) -> ResponsesClient<OpenTransport> {
    ResponsesClient::new(
        transport,
        Provider {
            name: "fixture".into(),
            base_url: "http://unused.local/v1".into(),
            query_params: None,
            headers: HeaderMap::new(),
            retry: RetryConfig {
                max_attempts: 1,
                base_delay: Duration::from_millis(1),
                retry_429: false,
                retry_5xx: false,
                retry_transport: false,
            },
            stream_idle_timeout: IDLE,
        },
        Arc::new(NoAuth),
    )
}

async fn next_response(stream: &mut ResponseStream) -> Option<Result<ResponseEvent, ApiError>> {
    loop {
        match stream.next().await {
            Some(Ok(ResponseEvent::RateLimits(_))) => continue,
            event => return event,
        }
    }
}

async fn assert_noise_times_out(chunk: &'static str) {
    let (mut stream, sender) = open_stream().await;
    let producer = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(10)).await;
            if sender
                .send(Ok(Bytes::from_static(chunk.as_bytes())))
                .await
                .is_err()
            {
                break;
            }
        }
    });
    let result = timeout(IDLE * 2, next_response(&mut stream)).await;
    producer.abort();
    assert!(
        matches!(result, Ok(Some(Err(ApiError::Stream(_))))),
        "{result:?}"
    );
    assert!(stream.next().await.is_none());
}

#[tokio::test(start_paused = true)]
async fn silence_times_out() {
    assert_noise_times_out("").await;
}

#[tokio::test(start_paused = true)]
async fn missing_response_headers_time_out() {
    let client = client(OpenTransport(Arc::new(Mutex::new(None))));
    let result = timeout(
        IDLE * 2,
        client.stream(
            json!({}),
            HeaderMap::new(),
            Compression::None,
            /*turn_state*/ None,
        ),
    )
    .await;
    assert!(matches!(
        result,
        Ok(Err(ApiError::Transport(TransportError::Timeout)))
    ));
}

#[tokio::test(start_paused = true)]
async fn comments_do_not_extend_deadline() {
    assert_noise_times_out(": ping\n\n").await;
}

#[tokio::test(start_paused = true)]
async fn malformed_data_does_not_extend_deadline() {
    assert_noise_times_out("data: ping\n\n").await;
}

#[tokio::test(start_paused = true)]
async fn unknown_events_do_not_extend_deadline() {
    assert_noise_times_out("data: {\"type\":\"ping\"}\n\n").await;
}

#[tokio::test(start_paused = true)]
async fn status_events_do_not_extend_deadline() {
    assert_noise_times_out("data: {\"type\":\"response.in_progress\"}\n\n").await;
}

#[tokio::test(start_paused = true)]
async fn empty_argument_deltas_do_not_extend_deadline() {
    assert_noise_times_out(
        "data: {\"type\":\"response.function_call_arguments.delta\",\"delta\":\"\"}\n\n",
    )
    .await;
}

#[tokio::test(start_paused = true)]
async fn terminal_errors_surface_before_eof_or_idle_timeout() {
    for (event, expected) in [
        (
            json!({"type": "response.failed", "response": {"error": {"code": "context_length_exceeded"}}}),
            "context window exceeded",
        ),
        (
            json!({"type": "response.failed", "response": {"error": {"code": "server_error", "message": "fixture failure"}}}),
            "fixture failure",
        ),
        (
            json!({"type": "response.incomplete", "response": {"incomplete_details": {"reason": "max_output_tokens"}}}),
            "max_output_tokens",
        ),
        (
            json!({"type": "response.completed", "response": {"id": 42}}),
            "failed to parse ResponseCompleted",
        ),
    ] {
        let (mut stream, sender) = open_stream().await;
        sender
            .send(Ok(Bytes::from(format!("data: {event}\n\n"))))
            .await
            .expect("send");
        let result = timeout(Duration::from_millis(1), next_response(&mut stream))
            .await
            .expect("terminal error must not wait for EOF")
            .expect("error event");
        assert!(
            result
                .as_ref()
                .is_err_and(|error| error.to_string().contains(expected)),
            "{result:?}"
        );
        assert!(stream.next().await.is_none());
    }
}

#[tokio::test(start_paused = true)]
async fn actual_output_and_tool_arguments_extend_deadline() {
    for kind in [
        "response.output_text.delta",
        "response.reasoning_text.delta",
        "response.reasoning_summary_text.delta",
        "response.custom_tool_call_input.delta",
        "response.function_call_arguments.delta",
    ] {
        let (mut stream, sender) = open_stream().await;
        let consumer = tokio::spawn(async move {
            while let Some(event) = stream.next().await {
                if matches!(event, Ok(ResponseEvent::Completed { .. }) | Err(_)) {
                    return event;
                }
            }
            panic!("missing completion");
        });
        // Four idle intervals of real output, including argument deltas that
        // are intentionally not projected into ResponseEvent.
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(10)).await;
            let event = json!({"type": kind, "delta": "x", "item_id": "tool", "summary_index": 0, "content_index": 0});
            sender
                .send(Ok(Bytes::from(format!("data: {event}\n\n"))))
                .await
                .expect("live stream");
        }
        sender
            .send(Ok(Bytes::from_static(
                b"data: {\"type\":\"response.completed\",\"response\":{\"id\":\"done\"}}\n\n",
            )))
            .await
            .expect("complete");
        assert!(matches!(
            consumer.await.expect("consumer"),
            Ok(ResponseEvent::Completed { .. })
        ));
    }
}

#[tokio::test(start_paused = true)]
async fn dropping_consumer_releases_pending_transport_immediately() {
    let (stream, sender) = open_stream().await;
    tokio::task::yield_now().await;
    drop(stream);
    timeout(Duration::from_millis(1), sender.closed())
        .await
        .expect("cancel must release HTTP stream without waiting for idle timeout");
}

#[tokio::test(start_paused = true)]
async fn eof_without_completion_is_an_error() {
    let (mut stream, sender) = open_stream().await;
    drop(sender);
    assert!(
        matches!(next_response(&mut stream).await, Some(Err(ApiError::Stream(message))) if message.contains("before response.completed"))
    );
}

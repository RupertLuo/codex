use super::ConnectionSessionState;
use super::MessageProcessor;
use super::message_processor_tracing_tests::build_test_config;
use super::message_processor_tracing_tests::build_test_processor;
use crate::outgoing_message::ConnectionId;
use crate::outgoing_message::OutgoingEnvelope;
use crate::rpc_extension::AppServerRpcContext;
use crate::rpc_extension::AppServerRpcExtension;
use crate::rpc_extension::AppServerRpcFuture;
use crate::rpc_extension::AppServerRpcRegistry;
use crate::rpc_extension::AppServerRpcTransportContext;
use crate::transport::AppServerTransport;
use codex_app_server_protocol::JSONRPCErrorError;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use tempfile::TempDir;
use tokio::sync::mpsc;

#[derive(Debug, Default)]
struct RecordingExtension {
    calls: AtomicUsize,
}

impl AppServerRpcExtension for RecordingExtension {
    fn methods(&self) -> &'static [&'static str] {
        &["catalyst/echo", "catalyst/reject"]
    }

    fn handle<'a>(
        &'a self,
        context: AppServerRpcContext,
        method: &'a str,
        params: Option<Value>,
    ) -> AppServerRpcFuture<'a> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if method == "catalyst/reject" {
                return Err(JSONRPCErrorError {
                    code: -32042,
                    message: "Extension rejected request".to_string(),
                    data: Some(json!({"reason": "fixture"})),
                });
            }
            let AppServerRpcTransportContext::LoopbackWebSocket { authenticated } =
                context.transport
            else {
                panic!("extension must receive the supplied transport context");
            };
            Ok(json!({"method": method, "params": params, "authenticated": authenticated}))
        })
    }
}

async fn request(
    processor: &Arc<MessageProcessor>,
    session: &Arc<ConnectionSessionState>,
    outgoing: &mut mpsc::Receiver<OutgoingEnvelope>,
    context: AppServerRpcTransportContext,
    request: Value,
) -> Value {
    let request_id = request["id"].clone();
    processor
        .process_request(
            ConnectionId(7),
            serde_json::from_value(request).unwrap(),
            &AppServerTransport::Stdio,
            Arc::clone(session),
            AppServerRpcContext::new(context),
        )
        .await;
    loop {
        let envelope = tokio::time::timeout(std::time::Duration::from_secs(5), outgoing.recv())
            .await
            .expect("response timeout")
            .expect("response channel closed");
        if let OutgoingEnvelope::ToConnection {
            connection_id: ConnectionId(7),
            message,
            ..
        } = envelope
        {
            let message = serde_json::to_value(message).unwrap();
            if message["id"] == request_id {
                return message;
            }
        }
    }
}

#[tokio::test]
async fn extension_dispatch_requires_initialize_and_preserves_context_and_errors()
-> anyhow::Result<()> {
    let codex_home = TempDir::new()?;
    // No model request is made: this test stops at the public JSON-RPC dispatch boundary.
    let config = Arc::new(build_test_config(codex_home.path(), "http://127.0.0.1:1").await?);
    let (mut processor, mut outgoing) = build_test_processor(config).await;
    let extension = Arc::new(RecordingExtension::default());
    Arc::get_mut(&mut processor)
        .expect("unshared test processor")
        .rpc_registry = Arc::new(AppServerRpcRegistry::new(vec![extension.clone()])?);
    let session = Arc::new(ConnectionSessionState::new());
    let unauthenticated = AppServerRpcTransportContext::LoopbackWebSocket {
        authenticated: false,
    };
    let authenticated = AppServerRpcTransportContext::LoopbackWebSocket {
        authenticated: true,
    };

    assert_eq!(
        request(
            &processor,
            &session,
            &mut outgoing,
            unauthenticated,
            json!({"id":1,"method":"catalyst/echo","params":{"value":7}})
        )
        .await,
        json!({"id":1,"error":{"code":-32600,"message":"Not initialized"}})
    );
    assert_eq!(extension.calls.load(Ordering::SeqCst), 0);

    let initialized = request(&processor, &session, &mut outgoing, unauthenticated,
        json!({"id":2,"method":"initialize","params":{"clientInfo":{"name":"rpc-isolation-test","version":"1"}}})).await;
    assert!(initialized.get("result").is_some(), "{initialized}");
    assert!(session.initialized());

    assert_eq!(
        request(
            &processor,
            &session,
            &mut outgoing,
            unauthenticated,
            json!({"id":3,"method":"catalyst/missing"})
        )
        .await,
        json!({"id":3,"error":{"code":-32601,"message":"Method not found: catalyst/missing"}})
    );
    assert_eq!(extension.calls.load(Ordering::SeqCst), 0);

    assert_eq!(
        request(
            &processor,
            &session,
            &mut outgoing,
            unauthenticated,
            json!({"id":4,"method":"catalyst/echo","params":{"value":7}})
        )
        .await,
        json!({"id":4,"result":{"method":"catalyst/echo","params":{"value":7},"authenticated":false}})
    );
    assert_eq!(
        request(
            &processor,
            &session,
            &mut outgoing,
            authenticated,
            json!({"id":5,"method":"catalyst/echo"})
        )
        .await,
        json!({"id":5,"result":{"method":"catalyst/echo","params":null,"authenticated":true}})
    );
    assert_eq!(
        request(
            &processor,
            &session,
            &mut outgoing,
            authenticated,
            json!({"id":6,"method":"catalyst/reject"})
        )
        .await,
        json!({"id":6,"error":{"code":-32042,"message":"Extension rejected request","data":{"reason":"fixture"}}})
    );
    assert_eq!(extension.calls.load(Ordering::SeqCst), 3);
    processor.shutdown_threads().await;
    processor.drain_background_tasks().await;
    Ok(())
}

#[derive(Debug, Default)]
struct SlowBackgroundExtension {
    started: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl AppServerRpcExtension for SlowBackgroundExtension {
    fn methods(&self) -> &'static [&'static str] {
        &["catalyst/test-background"]
    }
    fn concurrent_methods(&self) -> &'static [&'static str] {
        self.methods()
    }
    fn handle<'a>(
        &'a self,
        _context: AppServerRpcContext,
        _method: &'a str,
        _params: Option<Value>,
    ) -> AppServerRpcFuture<'a> {
        Box::pin(async move {
            self.started.notify_one();
            self.release.notified().await;
            Ok(json!({"done":true}))
        })
    }
}

#[tokio::test]
async fn slow_background_extension_does_not_block_following_native_reads() -> anyhow::Result<()> {
    let codex_home = TempDir::new()?;
    let config = Arc::new(build_test_config(codex_home.path(), "http://127.0.0.1:1").await?);
    let (mut processor, mut outgoing) = build_test_processor(config).await;
    let extension = Arc::new(SlowBackgroundExtension::default());
    Arc::get_mut(&mut processor).unwrap().rpc_registry =
        Arc::new(AppServerRpcRegistry::new(vec![extension.clone()])?);
    let session = Arc::new(ConnectionSessionState::default());
    let context = AppServerRpcTransportContext::Stdio;
    let initialized = request(&processor, &session, &mut outgoing, context,
        json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"background-dispatch-test","version":"1"}}})).await;
    assert!(initialized.get("result").is_some());
    // lib.rs awaits each process_request before receiving the next message.
    tokio::time::timeout(
        std::time::Duration::from_millis(250),
        processor.process_request(
            ConnectionId(7),
            serde_json::from_value(json!({"id":2,"method":"catalyst/test-background"}))?,
            &AppServerTransport::Stdio,
            session.clone(),
            AppServerRpcContext::new(context),
        ),
    )
    .await
    .expect("background inference blocked the transport receive loop");
    extension.started.notified().await;
    let native = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        request(
            &processor,
            &session,
            &mut outgoing,
            context,
            json!({"id":3,"method":"thread/list","params":{}}),
        ),
    )
    .await
    .expect("thread/list must respond while inference is still pending");
    assert!(native.get("result").is_some(), "{native}");
    extension.release.notify_one();
    processor.shutdown_threads().await;
    processor.drain_background_tasks().await;
    Ok(())
}

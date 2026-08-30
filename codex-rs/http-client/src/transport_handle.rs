use std::fmt;
use std::future::Future;
use std::sync::Arc;

use futures::future::BoxFuture;

use crate::Request;
use crate::Response;
use crate::StreamResponse;
use crate::TransportError;
use crate::transport::HttpTransport;

type ExecuteFn =
    dyn Fn(Request) -> BoxFuture<'static, Result<Response, TransportError>> + Send + Sync;
type StreamFn =
    dyn Fn(Request) -> BoxFuture<'static, Result<StreamResponse, TransportError>> + Send + Sync;

/// A cloneable, type-erased HTTP transport for callers that need to inject request behavior.
///
/// The handle owns the closures and keeps the transport boundary in `codex-http-client`; callers
/// can pass it across crate boundaries without exposing a concrete HTTP client implementation.
#[derive(Clone)]
pub struct HttpTransportHandle {
    execute: Arc<ExecuteFn>,
    stream: Arc<StreamFn>,
}

impl fmt::Debug for HttpTransportHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpTransportHandle")
            .finish_non_exhaustive()
    }
}

impl HttpTransportHandle {
    /// Builds a handle from asynchronous request and streaming callbacks.
    pub fn new<Execute, ExecuteFuture, Stream, StreamFuture>(
        execute: Execute,
        stream: Stream,
    ) -> Self
    where
        Execute: Fn(Request) -> ExecuteFuture + Send + Sync + 'static,
        ExecuteFuture: Future<Output = Result<Response, TransportError>> + Send + 'static,
        Stream: Fn(Request) -> StreamFuture + Send + Sync + 'static,
        StreamFuture: Future<Output = Result<StreamResponse, TransportError>> + Send + 'static,
    {
        Self {
            execute: Arc::new(move |request| Box::pin(execute(request))),
            stream: Arc::new(move |request| Box::pin(stream(request))),
        }
    }

    /// Erases a concrete [`HttpTransport`] while retaining shared ownership and cloning.
    pub fn from_transport<T>(transport: T) -> Self
    where
        T: HttpTransport + 'static,
    {
        let transport = Arc::new(transport);
        let execute_transport = Arc::clone(&transport);

        Self::new(
            move |request| {
                let transport = Arc::clone(&execute_transport);
                async move { transport.execute(request).await }
            },
            move |request| {
                let transport = Arc::clone(&transport);
                async move { transport.stream(request).await }
            },
        )
    }
}

impl HttpTransport for HttpTransportHandle {
    async fn execute(&self, request: Request) -> Result<Response, TransportError> {
        (self.execute)(request).await
    }

    async fn stream(&self, request: Request) -> Result<StreamResponse, TransportError> {
        (self.stream)(request).await
    }
}

#[cfg(test)]
#[path = "transport_handle_tests.rs"]
mod tests;

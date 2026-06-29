use std::future::Future;
use std::sync::Arc;

use futures::future::BoxFuture;

use crate::HttpTransport;
use crate::Request;
use crate::Response;
use crate::StreamResponse;
use crate::TransportError;

type ExecuteFn =
    dyn Fn(Request) -> BoxFuture<'static, Result<Response, TransportError>> + Send + Sync;

type StreamFn =
    dyn Fn(Request) -> BoxFuture<'static, Result<StreamResponse, TransportError>> + Send + Sync;

#[derive(Clone)]
pub struct HttpTransportHandle {
    execute: Arc<ExecuteFn>,
    stream: Arc<StreamFn>,
}

impl HttpTransportHandle {
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

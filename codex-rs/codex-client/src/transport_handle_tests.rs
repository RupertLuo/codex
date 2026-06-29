use bytes::Bytes;
use http::HeaderMap;
use http::Method;
use http::StatusCode;

use super::HttpTransportHandle;
use crate::HttpTransport;
use crate::Request;
use crate::Response;
use crate::StreamResponse;
use crate::TransportError;

#[test]
fn handle_can_be_constructed_and_cloned() {
    let handle = HttpTransportHandle::new(
        |_request: Request| async {
            Err::<Response, TransportError>(TransportError::Build("execute not called".to_string()))
        },
        |_request: Request| async {
            Err::<StreamResponse, TransportError>(TransportError::Build(
                "stream not called".to_string(),
            ))
        },
    );

    let _cloned = handle.clone();
}

#[tokio::test]
async fn delegates_execute_request() {
    let handle = HttpTransportHandle::new(
        |request: Request| async move {
            Ok(Response {
                status: StatusCode::OK,
                headers: HeaderMap::new(),
                body: Bytes::from(request.url),
            })
        },
        |_request: Request| async {
            Err::<StreamResponse, TransportError>(TransportError::Build(
                "stream not called".to_string(),
            ))
        },
    );

    let response = handle
        .execute(Request::new(
            Method::POST,
            "https://example.com/execute".to_string(),
        ))
        .await
        .expect("execute should succeed");

    assert_eq!(
        response.body,
        Bytes::from_static(b"https://example.com/execute")
    );
}

#[tokio::test]
async fn delegates_stream_request() {
    let handle = HttpTransportHandle::new(
        |_request: Request| async {
            Err::<Response, TransportError>(TransportError::Build("execute not called".to_string()))
        },
        |_request: Request| async {
            Ok(StreamResponse {
                status: StatusCode::ACCEPTED,
                headers: HeaderMap::new(),
                bytes: Box::pin(futures::stream::empty()),
            })
        },
    );

    let response = handle
        .stream(Request::new(
            Method::GET,
            "https://example.com/stream".to_string(),
        ))
        .await
        .expect("stream should succeed");

    assert_eq!(response.status, StatusCode::ACCEPTED);
}

struct EchoTransport;

impl HttpTransport for EchoTransport {
    async fn execute(&self, request: Request) -> Result<Response, TransportError> {
        Ok(Response {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: Bytes::from(request.url),
        })
    }

    async fn stream(&self, _request: Request) -> Result<StreamResponse, TransportError> {
        Err(TransportError::Build("stream not called".to_string()))
    }
}

#[tokio::test]
async fn from_transport_delegates_execute_request() {
    let handle = HttpTransportHandle::from_transport(EchoTransport);

    let response = handle
        .execute(Request::new(
            Method::POST,
            "https://example.com/from-transport".to_string(),
        ))
        .await
        .expect("execute should succeed");

    assert_eq!(
        response.body,
        Bytes::from_static(b"https://example.com/from-transport")
    );
}

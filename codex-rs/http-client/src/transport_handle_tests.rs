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
fn handle_is_cloneable() {
    let handle = HttpTransportHandle::new(
        |_request: Request| async {
            Err::<Response, TransportError>(TransportError::Build("unused".to_string()))
        },
        |_request: Request| async {
            Err::<StreamResponse, TransportError>(TransportError::Build("unused".to_string()))
        },
    );

    let _clone = handle;
}

#[tokio::test]
async fn delegates_execute() {
    let handle = HttpTransportHandle::new(
        |request: Request| async move {
            Ok(Response {
                status: StatusCode::OK,
                headers: HeaderMap::new(),
                body: Bytes::from(request.url),
            })
        },
        |_request: Request| async {
            Err::<StreamResponse, TransportError>(TransportError::Build("unused".to_string()))
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
async fn delegates_stream() {
    let handle = HttpTransportHandle::new(
        |_request: Request| async {
            Err::<Response, TransportError>(TransportError::Build("unused".to_string()))
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
        Err(TransportError::Build("unused".to_string()))
    }
}

#[tokio::test]
async fn erases_concrete_transport() {
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

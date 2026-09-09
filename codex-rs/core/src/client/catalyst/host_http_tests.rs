use super::is_unknown_previous_response;
use codex_api::ApiError;
use codex_api::TransportError;

#[test]
fn recognizes_qwen_unknown_previous_response_error() {
    let error = ApiError::Transport(TransportError::Http {
        status: http::StatusCode::BAD_REQUEST,
        url: None,
        headers: None,
        body: Some(
            r#"{"code":"InvalidParameter","message":"Not found previous_response_id"}"#.to_string(),
        ),
    });

    assert!(is_unknown_previous_response(&error));
}

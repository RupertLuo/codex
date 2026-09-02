use super::AGENT_FINAL_MESSAGE_PREFIX;
use super::HANDOFF_STREAM_TRUNCATION_MARKER;
use super::RealtimeHandoffState;
use super::RealtimeInputTaskExit;
use super::RealtimeOutbound;
use super::RealtimePendingOutbound;
use super::RealtimeSessionKind;
use super::RealtimeStreamedItem;
use super::classify_realtime_input_error;
use super::classify_realtime_input_error_with_pending;
use super::realtime_delegation_from_handoff;
use super::realtime_request_headers;
use super::realtime_text_from_handoff_request;
use super::wrap_realtime_delegation_input;
use crate::context::RealtimeDelegationSource;
use async_channel::bounded;
use codex_api::ApiError;
use codex_api::RealtimeEventParser;
use codex_protocol::models::MessagePhase;
use codex_protocol::protocol::CodexResponseHandoffMode;
use codex_protocol::protocol::ConversationTextParams;
use codex_protocol::protocol::ConversationTextRole;
use codex_protocol::protocol::RealtimeHandoffRequested;
use codex_protocol::protocol::RealtimeTranscriptEntry;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

#[test]
fn prefers_handoff_input_transcript_over_active_transcript() {
    let handoff = RealtimeHandoffRequested {
        handoff_id: "handoff_1".to_string(),
        item_id: "item_1".to_string(),
        input_transcript: "ignored".to_string(),
        active_transcript: vec![
            RealtimeTranscriptEntry {
                role: "user".to_string(),
                text: "hello".to_string(),
            },
            RealtimeTranscriptEntry {
                role: "assistant".to_string(),
                text: "hi there".to_string(),
            },
        ],
    };
    assert_eq!(
        realtime_text_from_handoff_request(&handoff),
        Some("ignored".to_string())
    );
}

#[test]
fn extracts_text_from_handoff_request_active_transcript_if_input_missing() {
    let handoff = RealtimeHandoffRequested {
        handoff_id: "handoff_1".to_string(),
        item_id: "item_1".to_string(),
        input_transcript: String::new(),
        active_transcript: vec![RealtimeTranscriptEntry {
            role: "user".to_string(),
            text: "hello".to_string(),
        }],
    };
    assert_eq!(
        realtime_text_from_handoff_request(&handoff),
        Some("user: hello".to_string())
    );
}

#[test]
fn wraps_handoff_with_transcript_delta() {
    let handoff = RealtimeHandoffRequested {
        handoff_id: "handoff_1".to_string(),
        item_id: "item_1".to_string(),
        input_transcript: "delegate this".to_string(),
        active_transcript: vec![
            RealtimeTranscriptEntry {
                role: "user".to_string(),
                text: "hello".to_string(),
            },
            RealtimeTranscriptEntry {
                role: "assistant".to_string(),
                text: "hi there".to_string(),
            },
        ],
    };
    assert_eq!(
        realtime_delegation_from_handoff(&handoff),
        Some(
            "<realtime_delegation>\n  <input>delegate this</input>\n  <transcript_delta>user: hello\nassistant: hi there</transcript_delta>\n</realtime_delegation>"
                .to_string()
        )
    );
}

#[test]
fn extracts_text_from_handoff_request_input_transcript_if_messages_missing() {
    let handoff = RealtimeHandoffRequested {
        handoff_id: "handoff_1".to_string(),
        item_id: "item_1".to_string(),
        input_transcript: "ignored".to_string(),
        active_transcript: vec![],
    };
    assert_eq!(
        realtime_text_from_handoff_request(&handoff),
        Some("ignored".to_string())
    );
}

#[test]
fn ignores_empty_handoff_request_input_transcript() {
    let handoff = RealtimeHandoffRequested {
        handoff_id: "handoff_1".to_string(),
        item_id: "item_1".to_string(),
        input_transcript: String::new(),
        active_transcript: vec![],
    };
    assert_eq!(realtime_text_from_handoff_request(&handoff), None);
}

#[test]
fn wraps_realtime_delegation_input() {
    assert_eq!(
        wrap_realtime_delegation_input(
            "hello",
            /*transcript_delta*/ None,
            RealtimeDelegationSource::Handoff,
        ),
        "<realtime_delegation>\n  <input>hello</input>\n</realtime_delegation>"
    );
}

#[test]
fn wraps_realtime_delegation_input_with_xml_escaping() {
    assert_eq!(
        wrap_realtime_delegation_input(
            "use a < b && c > d",
            Some("saw <that>"),
            RealtimeDelegationSource::Handoff,
        ),
        "<realtime_delegation>\n  <input>use a &lt; b &amp;&amp; c &gt; d</input>\n  <transcript_delta>saw &lt;that&gt;</transcript_delta>\n</realtime_delegation>"
    );
}

#[test]
fn wraps_realtime_delegation_input_with_xml_escaping_without_transcript() {
    assert_eq!(
        wrap_realtime_delegation_input(
            "use a < b && c > d",
            /*transcript_delta*/ None,
            RealtimeDelegationSource::Handoff,
        ),
        "<realtime_delegation>\n  <input>use a &lt; b &amp;&amp; c &gt; d</input>\n</realtime_delegation>"
    );
}

#[test]
fn bounds_realtime_delegation_fields_and_keeps_latest_transcript() {
    let input = format!("start{}input-end", "x".repeat(8 * 1024));
    let transcript = format!("transcript-start{}latest", "y".repeat(8 * 1024));
    let rendered = wrap_realtime_delegation_input(
        &input,
        Some(&transcript),
        RealtimeDelegationSource::Handoff,
    );

    assert!(rendered.len() < 9 * 1024);
    assert!(rendered.contains("<input>start"));
    assert!(!rendered.contains("input-end"));
    assert!(!rendered.contains("transcript-start"));
    assert!(rendered.contains("latest</transcript_delta>"));
}

#[test]
fn classifies_outbound_api_failures_as_transport_loss() {
    for pending_outbound in [
        RealtimePendingOutbound::Text(ConversationTextParams {
            text: "retry me".to_string(),
            role: ConversationTextRole::User,
        }),
        RealtimePendingOutbound::Handoff(RealtimeOutbound::StandaloneHandoff {
            text: "retry this handoff".to_string(),
            phase: Some(MessagePhase::FinalAnswer),
        }),
    ] {
        let exit = classify_realtime_input_error_with_pending(
            ApiError::Stream("failed to send realtime request".to_string()).into(),
            Some(Box::new(pending_outbound.clone())),
        );
        let RealtimeInputTaskExit::TransportLost {
            err: ApiError::Stream(_),
            pending_outbound: Some(actual_pending_outbound),
        } = exit
        else {
            panic!("outbound API failure should preserve pending output for reconnect");
        };
        assert_eq!(*actual_pending_outbound, pending_outbound);
    }

    assert!(matches!(
        classify_realtime_input_error(anyhow::anyhow!("input channel closed")),
        RealtimeInputTaskExit::Terminal
    ));
}

#[tokio::test]
async fn clears_active_handoff_explicitly() {
    let (tx, _rx) = bounded(1);
    let state = RealtimeHandoffState {
        output_tx: tx,
        last_output: Arc::new(Mutex::new(None)),
        stream: Arc::new(Mutex::new(Default::default())),
        client_managed_handoffs: false,
        codex_responses_as_items: false,
        codex_response_item_prefix: None,
        codex_response_handoff_mode: CodexResponseHandoffMode::Thinking,
        codex_response_handoff_channel_prefixes: Arc::new(BTreeMap::new()),
        session_kind: RealtimeSessionKind::V1,
        event_parser: RealtimeEventParser::V1,
    };

    state.stream.lock().await.active_handoff = Some("handoff_1".to_string());
    assert_eq!(
        state.stream.lock().await.active_handoff.clone(),
        Some("handoff_1".to_string())
    );

    state.stream.lock().await.active_handoff = None;
    assert_eq!(state.stream.lock().await.active_handoff.clone(), None);
}

#[test]
fn streamed_handoff_preserves_a_bounded_final_tail() {
    let mut item = RealtimeStreamedItem {
        handoff_id: "handoff_1".to_string(),
        phase: Some(MessagePhase::FinalAnswer),
        bem_channel_parser: None,
        prefix_final_message: true,
        sent_bytes: 0,
        buffered_text: String::new(),
        tail_text: String::new(),
        truncated: false,
        last_flush_at: Instant::now(),
        flush_scheduled: false,
    };
    item.push_text(&format!("HEAD{}TAIL", "x".repeat(/*n*/ 5_000)));

    let first = item
        .drain_stream_chunk()
        .expect("oversized output should retain a streamable head");
    let final_chunk = item
        .drain_final_chunk()
        .expect("oversized output should retain a final tail");
    let output = format!("{first}{final_chunk}");

    assert!(output.len() <= 4_000);
    assert!(output.starts_with(&format!("{AGENT_FINAL_MESSAGE_PREFIX}HEAD")));
    assert!(output.contains(HANDOFF_STREAM_TRUNCATION_MARKER));
    assert!(output.ends_with("TAIL"));
}

#[test]
fn streamed_v3_handoff_omits_the_final_message_prefix() {
    let mut item = RealtimeStreamedItem {
        handoff_id: "handoff_1".to_string(),
        phase: Some(MessagePhase::FinalAnswer),
        bem_channel_parser: None,
        prefix_final_message: false,
        sent_bytes: 0,
        buffered_text: String::new(),
        tail_text: String::new(),
        truncated: false,
        last_flush_at: Instant::now(),
        flush_scheduled: false,
    };
    item.push_text("done");

    assert_eq!(item.drain_final_chunk(), Some("done".to_string()));
}

#[test]
fn uses_quicksilver_alpha_header_for_realtime_v1() {
    let headers = realtime_request_headers(
        Some("session_1"),
        Some("sk-test"),
        RealtimeEventParser::V1,
        "codex_work_desktop",
    )
    .expect("headers")
    .expect("headers");

    assert_eq!(
        headers
            .get("openai-alpha")
            .and_then(|value| value.to_str().ok()),
        Some("quicksilver=v1")
    );
}

#[test]
fn omits_quicksilver_alpha_header_for_realtime_v2() {
    let headers = realtime_request_headers(
        Some("session_1"),
        Some("sk-test"),
        RealtimeEventParser::RealtimeV2,
        "codex_work_desktop",
    )
    .expect("headers")
    .expect("headers");

    assert!(headers.get("openai-alpha").is_none());
}

#[test]
fn uses_frameless_alpha_header_for_realtime_v3() {
    let headers = realtime_request_headers(
        Some("session_1"),
        Some("sk-test"),
        RealtimeEventParser::FramelessBidi,
        "codex_work_desktop",
    )
    .expect("headers")
    .expect("headers");

    assert_eq!(
        headers
            .get("openai-alpha")
            .and_then(|value| value.to_str().ok()),
        Some("quicksilver=v2")
    );
}

#[test]
fn realtime_headers_include_only_non_default_originator() {
    let default_originator = codex_login::default_client::originator();
    for (originator, expected_header) in [
        ("codex_work_desktop", Some("codex_work_desktop")),
        (default_originator.value.as_str(), None),
    ] {
        let headers = realtime_request_headers(
            Some("session_1"),
            Some("sk-test"),
            RealtimeEventParser::RealtimeV2,
            originator,
        )
        .expect("headers")
        .expect("headers");

        assert_eq!(
            headers
                .get("originator")
                .and_then(|value| value.to_str().ok()),
            expected_header
        );
    }
}

// ---------------------------------------------------------------------------
// Claim / close state-machine tests
// ---------------------------------------------------------------------------

use super::ManagedConversationState;
use super::RealtimeCloseTarget;
use super::RealtimeConversationEnd;
use super::RealtimeConversationManager;
use std::sync::Arc;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn claim_close_from_active_transitions_to_closing() {
    let mgr = RealtimeConversationManager::new();
    let test_conv = mgr.set_active_for_test("sub-1", RealtimeSessionKind::V1);

    let claim = mgr
        .claim_close(
            RealtimeCloseTarget::Current,
            None,
            RealtimeConversationEnd::Requested,
        )
        .await;
    let claim = claim.expect("claim should succeed from Active state");

    assert_eq!(claim.end, RealtimeConversationEnd::Requested);
    assert!(
        claim.conversation.is_some(),
        "first claim gets the ConversationState"
    );
    assert_eq!(claim.sub_id, "sub-1");
    assert!(!claim.quarantine_policy);

    // Manager is now in Closing state.
    assert!(mgr.close_pending_for_test().await);
    // realtime_active is still true (not stopped by us).
    assert!(test_conv.realtime_active.load(Ordering::Relaxed));
}

#[tokio::test]
async fn claim_close_from_none_returns_none() {
    let mgr = RealtimeConversationManager::new();
    let claim = mgr
        .claim_close(
            RealtimeCloseTarget::Current,
            None,
            RealtimeConversationEnd::Requested,
        )
        .await;
    assert!(claim.is_none());
}

#[tokio::test]
async fn claim_close_while_closing_in_progress_returns_none() {
    let mgr = RealtimeConversationManager::new();
    let _test_conv = mgr.set_active_for_test("sub-1", RealtimeSessionKind::V1);

    let _first = mgr
        .claim_close(
            RealtimeCloseTarget::Current,
            None,
            RealtimeConversationEnd::Requested,
        )
        .await
        .expect("first claim succeeds");

    let second = mgr
        .claim_close(
            RealtimeCloseTarget::Current,
            None,
            RealtimeConversationEnd::Requested,
        )
        .await;
    assert!(second.is_none(), "cannot re-claim while in_progress");
}

#[tokio::test]
async fn complete_close_clears_state() {
    let mgr = RealtimeConversationManager::new();
    let _test_conv = mgr.set_active_for_test("sub-1", RealtimeSessionKind::V1);

    let claim = mgr
        .claim_close(
            RealtimeCloseTarget::Current,
            None,
            RealtimeConversationEnd::Requested,
        )
        .await
        .expect("claim succeeds");

    mgr.complete_close(claim.token).await;

    assert!(!mgr.close_pending_for_test().await);
    assert!(mgr.running_state().await.is_none());
}

#[tokio::test]
async fn complete_close_wrong_token_is_noop() {
    let mgr = RealtimeConversationManager::new();
    let _test_conv = mgr.set_active_for_test("sub-1", RealtimeSessionKind::V1);

    let claim = mgr
        .claim_close(
            RealtimeCloseTarget::Current,
            None,
            RealtimeConversationEnd::Requested,
        )
        .await
        .expect("claim succeeds");

    mgr.complete_close(claim.token + 999).await;

    // State should still be Closing because the token didn't match.
    assert!(mgr.close_pending_for_test().await);
}

#[tokio::test]
async fn release_close_for_retry_allows_reclaim() {
    let mgr = RealtimeConversationManager::new();
    let _test_conv = mgr.set_active_for_test("sub-1", RealtimeSessionKind::V1);

    let claim = mgr
        .claim_close(
            RealtimeCloseTarget::Current,
            None,
            RealtimeConversationEnd::Requested,
        )
        .await
        .expect("claim succeeds");
    let token = claim.token;

    mgr.release_close_for_retry(token).await;

    let reclaim = mgr
        .claim_close(
            RealtimeCloseTarget::Current,
            None,
            RealtimeConversationEnd::Requested,
        )
        .await
        .expect("reclaim should succeed after release");
    assert!(
        reclaim.conversation.is_none(),
        "reclaim has no ConversationState"
    );
    assert_eq!(reclaim.token, token, "same token reused");
}

#[tokio::test]
async fn claim_close_expected_target_rejects_mismatch() {
    let mgr = RealtimeConversationManager::new();
    let _test_conv = mgr.set_active_for_test("sub-1", RealtimeSessionKind::V1);

    let wrong_active = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let claim = mgr
        .claim_close(
            RealtimeCloseTarget::Expected(&wrong_active),
            None,
            RealtimeConversationEnd::TransportClosed,
        )
        .await;
    assert!(claim.is_none(), "mismatched target should not claim");
}

#[tokio::test]
async fn quarantine_policy_set_for_persistence_quarantine() {
    let mgr = RealtimeConversationManager::new();
    let _test_conv = mgr.set_active_for_test("sub-1", RealtimeSessionKind::V1);

    let claim = mgr
        .claim_close(
            RealtimeCloseTarget::Current,
            None,
            RealtimeConversationEnd::PersistenceQuarantine,
        )
        .await
        .expect("claim succeeds");
    assert!(claim.quarantine_policy);
}

#[tokio::test]
async fn running_state_returns_none_when_closing() {
    let mgr = RealtimeConversationManager::new();
    let _test_conv = mgr.set_active_for_test("sub-1", RealtimeSessionKind::V1);

    // Active → should have running state.
    assert!(mgr.running_state().await.is_some());

    let _claim = mgr
        .claim_close(
            RealtimeCloseTarget::Current,
            None,
            RealtimeConversationEnd::Requested,
        )
        .await;

    // Closing → running_state should be None.
    assert!(mgr.running_state().await.is_none());
}

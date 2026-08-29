use super::RealtimeHandoffState;
use super::RealtimeSessionKind;
use super::realtime_delegation_from_handoff;
use super::realtime_request_headers;
use super::realtime_text_from_handoff_request;
use super::wrap_realtime_delegation_input;
use async_channel::bounded;
use codex_config::config_toml::RealtimeWsVersion;
use codex_protocol::protocol::RealtimeHandoffRequested;
use codex_protocol::protocol::RealtimeTranscriptEntry;
use pretty_assertions::assert_eq;

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
        wrap_realtime_delegation_input("hello", /*transcript_delta*/ None),
        "<realtime_delegation>\n  <input>hello</input>\n</realtime_delegation>"
    );
}

#[test]
fn wraps_realtime_delegation_input_with_xml_escaping() {
    assert_eq!(
        wrap_realtime_delegation_input("use a < b && c > d", Some("saw <that>")),
        "<realtime_delegation>\n  <input>use a &lt; b &amp;&amp; c &gt; d</input>\n  <transcript_delta>saw &lt;that&gt;</transcript_delta>\n</realtime_delegation>"
    );
}

#[test]
fn wraps_realtime_delegation_input_with_xml_escaping_without_transcript() {
    assert_eq!(
        wrap_realtime_delegation_input("use a < b && c > d", /*transcript_delta*/ None),
        "<realtime_delegation>\n  <input>use a &lt; b &amp;&amp; c &gt; d</input>\n</realtime_delegation>"
    );
}

#[tokio::test]
async fn clears_active_handoff_explicitly() {
    let (tx, _rx) = bounded(1);
    let state = RealtimeHandoffState::new(
        tx,
        /*client_managed_handoffs*/ false,
        /*codex_responses_as_items*/ false,
        /*codex_response_item_prefix*/ None,
        /*codex_response_handoff_prefix*/ None,
        RealtimeSessionKind::V1,
    );

    *state.active_handoff.lock().await = Some("handoff_1".to_string());
    assert_eq!(
        state.active_handoff.lock().await.clone(),
        Some("handoff_1".to_string())
    );

    *state.active_handoff.lock().await = None;
    assert_eq!(state.active_handoff.lock().await.clone(), None);
}

#[test]
fn uses_quicksilver_alpha_header_for_realtime_v1() {
    let headers = realtime_request_headers(
        Some("session_1"),
        Some("sk-test"),
        RealtimeWsVersion::V1,
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
        RealtimeWsVersion::V2,
        "codex_work_desktop",
    )
    .expect("headers")
    .expect("headers");

    assert!(headers.get("openai-alpha").is_none());
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
            RealtimeWsVersion::V2,
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

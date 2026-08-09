use super::*;
use crate::session::tests::build_world_state_from_turn_context;
use crate::session::tests::make_session_and_context;
use codex_protocol::AgentPath;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ReasoningItemReasoningSummary;
use codex_protocol::protocol::CompactedItem;
use codex_protocol::protocol::CompactionCheckpoint;
use codex_protocol::protocol::InterAgentCommunication;
use codex_protocol::protocol::RateLimitSnapshot;
use codex_protocol::protocol::ThreadRolledBackEvent;
use codex_protocol::protocol::TokenCountEvent;
use codex_protocol::protocol::TokenUsage;
use codex_protocol::protocol::TokenUsageInfo;
use codex_protocol::protocol::TurnCompleteEvent;
use codex_protocol::protocol::TurnStartedEvent;
use codex_protocol::protocol::UserMessageEvent;
use codex_protocol::protocol::WorldStateItem;
use pretty_assertions::assert_eq;
use std::sync::Arc;

fn user_msg(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![ContentItem::OutputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

fn assistant_msg(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: "assistant".to_string(),
        content: vec![ContentItem::OutputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

fn developer_msg(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: "developer".to_string(),
        content: vec![ContentItem::InputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

fn inter_agent_msg(text: &str, trigger_turn: bool) -> ResponseItem {
    let communication = InterAgentCommunication::new(
        AgentPath::root(),
        AgentPath::try_from("/root/worker").expect("agent path"),
        Vec::new(),
        text.to_string(),
        trigger_turn,
    );
    communication.to_response_input_item().into()
}

fn inter_agent_communication(text: &str, trigger_turn: bool) -> RolloutItem {
    RolloutItem::InterAgentCommunication(InterAgentCommunication::new(
        AgentPath::root(),
        AgentPath::try_from("/root/worker").expect("agent path"),
        Vec::new(),
        text.to_string(),
        trigger_turn,
    ))
}

fn turn_started(turn_id: &str) -> RolloutItem {
    RolloutItem::EventMsg(EventMsg::TurnStarted(TurnStartedEvent {
        turn_id: turn_id.to_string(),
        trace_id: None,
        started_at: None,
        model_context_window: None,
        collaboration_mode_kind: Default::default(),
    }))
}

fn turn_completed(turn_id: &str) -> RolloutItem {
    RolloutItem::EventMsg(EventMsg::TurnComplete(TurnCompleteEvent {
        turn_id: turn_id.to_string(),
        last_agent_message: None,
        completed_at: None,
        duration_ms: None,
        time_to_first_token_ms: None,
    }))
}

#[test]
fn truncates_rollout_after_terminal_canonical_turn_id() {
    let rollout = vec![
        turn_started("turn-1"),
        turn_completed("turn-1"),
        turn_started("turn-2"),
        turn_completed("turn-2"),
        turn_started("turn-3"),
        turn_completed("turn-3"),
    ];

    let truncated =
        truncate_rollout_after_turn_id(&rollout, "turn-2").expect("truncate through turn-2");

    assert_eq!(
        serde_json::to_value(&truncated).unwrap(),
        serde_json::to_value(&rollout[..4]).unwrap()
    );
}

#[test]
fn truncate_rollout_after_turn_id_rejects_rolled_back_turn() {
    let rollout = vec![
        turn_started("turn-1"),
        turn_completed("turn-1"),
        turn_started("turn-2"),
        turn_completed("turn-2"),
        RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
            num_turns: 1,
        })),
        turn_started("turn-3"),
        turn_completed("turn-3"),
    ];

    let err = truncate_rollout_after_turn_id(&rollout, "turn-2")
        .expect_err("rolled-back turn should not be a fork anchor");

    assert!(matches!(
        err,
        CodexErr::InvalidRequest(message)
            if message == "lastTurnId 'turn-2' was not found in the source thread"
    ));
}

#[test]
fn truncate_rollout_after_turn_id_rejects_synthetic_legacy_turn_id() {
    let rollout = vec![RolloutItem::EventMsg(EventMsg::UserMessage(
        UserMessageEvent {
            message: "legacy".to_string(),
            ..Default::default()
        },
    ))];

    let err = truncate_rollout_after_turn_id(&rollout, "rollout-0")
        .expect_err("synthetic turn should not be a fork anchor");

    assert!(matches!(
        err,
        CodexErr::InvalidRequest(message)
            if message
                == "lastTurnId 'rollout-0' is not a persisted canonical turn in the source thread"
    ));
}

#[test]
fn truncate_rollout_after_turn_id_rejects_in_progress_turn() {
    let rollout = vec![turn_started("turn-1")];

    let err = truncate_rollout_after_turn_id(&rollout, "turn-1")
        .expect_err("in-progress turn should not be a fork anchor");

    assert!(matches!(
        err,
        CodexErr::InvalidRequest(message)
            if message == "lastTurnId 'turn-1' identifies an in-progress turn"
    ));
}

#[test]
fn truncates_rollout_from_start_before_nth_user_only() {
    let items = [
        user_msg("u1"),
        assistant_msg("a1"),
        assistant_msg("a2"),
        user_msg("u2"),
        assistant_msg("a3"),
        ResponseItem::Reasoning {
            id: Some("r1".to_string()),
            summary: vec![ReasoningItemReasoningSummary::SummaryText {
                text: "s".to_string(),
            }],
            content: None,
            encrypted_content: None,
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::FunctionCall {
            id: None,
            call_id: "c1".to_string(),
            name: "tool".to_string(),
            namespace: None,
            arguments: "{}".to_string(),
            internal_chat_message_metadata_passthrough: None,
        },
        assistant_msg("a4"),
    ];

    let rollout: Vec<RolloutItem> = items
        .iter()
        .cloned()
        .map(RolloutItem::ResponseItem)
        .collect();

    let truncated =
        truncate_rollout_before_nth_user_message_from_start(&rollout, /*n_from_start*/ 1);
    let expected = vec![
        RolloutItem::ResponseItem(items[0].clone()),
        RolloutItem::ResponseItem(items[1].clone()),
        RolloutItem::ResponseItem(items[2].clone()),
    ];
    assert_eq!(
        serde_json::to_value(&truncated).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );

    let truncated2 =
        truncate_rollout_before_nth_user_message_from_start(&rollout, /*n_from_start*/ 2);
    assert_eq!(
        serde_json::to_value(&truncated2).unwrap(),
        serde_json::to_value(&rollout).unwrap()
    );
}

#[test]
fn truncation_max_keeps_full_rollout() {
    let rollout = vec![
        RolloutItem::ResponseItem(user_msg("u1")),
        RolloutItem::ResponseItem(assistant_msg("a1")),
        RolloutItem::ResponseItem(user_msg("u2")),
    ];

    let truncated = truncate_rollout_before_nth_user_message_from_start(&rollout, usize::MAX);

    assert_eq!(
        serde_json::to_value(&truncated).unwrap(),
        serde_json::to_value(&rollout).unwrap()
    );
}

#[test]
fn truncates_rollout_from_start_applies_thread_rollback_markers() {
    let rollout_items = vec![
        RolloutItem::ResponseItem(user_msg("u1")),
        RolloutItem::ResponseItem(assistant_msg("a1")),
        RolloutItem::ResponseItem(user_msg("u2")),
        RolloutItem::ResponseItem(assistant_msg("a2")),
        RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
            num_turns: 1,
        })),
        RolloutItem::ResponseItem(user_msg("u3")),
        RolloutItem::ResponseItem(assistant_msg("a3")),
        RolloutItem::ResponseItem(user_msg("u4")),
        RolloutItem::ResponseItem(assistant_msg("a4")),
    ];

    // Effective user history after applying rollback(1) is: u1, u3, u4.
    // So n_from_start=2 should cut before u4 (not u3).
    let truncated = truncate_rollout_before_nth_user_message_from_start(
        &rollout_items,
        /*n_from_start*/ 2,
    );
    let expected = rollout_items[..7].to_vec();
    assert_eq!(
        serde_json::to_value(&truncated).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
}

#[tokio::test]
async fn ignores_session_prefix_messages_when_truncating_rollout_from_start() {
    let (session, turn_context) = make_session_and_context().await;
    let turn_context = Arc::new(turn_context);
    let world_state = build_world_state_from_turn_context(&session, &turn_context).await;
    let mut items = session
        .build_initial_context_with_world_state(&turn_context, &world_state)
        .await;
    items.push(user_msg("feature request"));
    items.push(assistant_msg("ack"));
    items.push(user_msg("second question"));
    items.push(assistant_msg("answer"));

    let rollout_items: Vec<RolloutItem> = items
        .iter()
        .cloned()
        .map(RolloutItem::ResponseItem)
        .collect();

    let truncated = truncate_rollout_before_nth_user_message_from_start(
        &rollout_items,
        /*n_from_start*/ 1,
    );
    let expected: Vec<RolloutItem> = vec![
        RolloutItem::ResponseItem(items[0].clone()),
        RolloutItem::ResponseItem(items[1].clone()),
        RolloutItem::ResponseItem(items[2].clone()),
        RolloutItem::ResponseItem(items[3].clone()),
    ];

    assert_eq!(
        serde_json::to_value(&truncated).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
}

#[test]
fn truncates_rollout_to_last_n_fork_turns_counts_trigger_turn_messages() {
    let rollout = vec![
        RolloutItem::ResponseItem(user_msg("u1")),
        RolloutItem::ResponseItem(assistant_msg("a1")),
        RolloutItem::ResponseItem(inter_agent_msg(
            "queued message",
            /*trigger_turn*/ false,
        )),
        RolloutItem::ResponseItem(assistant_msg("a2")),
        RolloutItem::ResponseItem(inter_agent_msg(
            "triggered task",
            /*trigger_turn*/ true,
        )),
        RolloutItem::ResponseItem(assistant_msg("a3")),
        RolloutItem::ResponseItem(user_msg("u2")),
        RolloutItem::ResponseItem(assistant_msg("a4")),
    ];

    let truncated = truncate_rollout_to_last_n_fork_turns(&rollout, /*n_from_end*/ 2);
    let expected = rollout[4..].to_vec();

    assert_eq!(
        serde_json::to_value(&truncated).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
}

#[test]
fn fork_turn_positions_use_inter_agent_delivery_metadata() {
    let rollout = vec![
        RolloutItem::ResponseItem(user_msg("user task")),
        inter_agent_communication("queued during user turn", /*trigger_turn*/ false),
        RolloutItem::ResponseItem(assistant_msg("first answer")),
        inter_agent_communication("follow-up task", /*trigger_turn*/ true),
        RolloutItem::ResponseItem(assistant_msg("second answer")),
        RolloutItem::ResponseItem(user_msg("next user task")),
    ];

    assert_eq!(fork_turn_positions_in_rollout(&rollout), vec![0, 3, 5]);
}

#[test]
fn fork_turn_positions_use_canonical_agent_messages_and_delivery_metadata() {
    let queued = InterAgentCommunication::new(
        AgentPath::root(),
        AgentPath::try_from("/root/worker").expect("agent path"),
        Vec::new(),
        "queued during user turn".to_string(),
        /*trigger_turn*/ false,
    );
    let triggered = InterAgentCommunication::new(
        AgentPath::root(),
        AgentPath::try_from("/root/worker").expect("agent path"),
        Vec::new(),
        "follow-up task".to_string(),
        /*trigger_turn*/ true,
    );
    let mut rollout = vec![
        RolloutItem::ResponseItem(user_msg("user task")),
        RolloutItem::InterAgentCommunicationMetadata {
            trigger_turn: false,
        },
        RolloutItem::ResponseItem(queued.to_model_input_item()),
        RolloutItem::ResponseItem(assistant_msg("first answer")),
        RolloutItem::InterAgentCommunicationMetadata { trigger_turn: true },
        RolloutItem::ResponseItem(triggered.to_model_input_item()),
        RolloutItem::ResponseItem(assistant_msg("second answer")),
        RolloutItem::ResponseItem(user_msg("next user task")),
    ];

    assert_eq!(fork_turn_positions_in_rollout(&rollout), vec![0, 4, 7]);

    rollout.insert(
        7,
        RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
            num_turns: 1,
        })),
    );
    assert_eq!(fork_turn_positions_in_rollout(&rollout), vec![0, 8]);
}

#[test]
fn truncates_rollout_to_last_n_fork_turns_drops_startup_prefix_even_when_under_limit() {
    let rollout = vec![
        RolloutItem::ResponseItem(developer_msg("startup developer context")),
        RolloutItem::ResponseItem(user_msg("current task")),
        RolloutItem::ResponseItem(assistant_msg("answer")),
    ];

    let truncated = truncate_rollout_to_last_n_fork_turns(&rollout, /*n_from_end*/ 2);
    let expected = rollout[1..].to_vec();

    assert_eq!(
        serde_json::to_value(&truncated).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
}

#[test]
fn truncates_rollout_to_last_n_fork_turns_applies_thread_rollback_markers() {
    let rollout = vec![
        RolloutItem::ResponseItem(user_msg("u1")),
        RolloutItem::ResponseItem(assistant_msg("a1")),
        RolloutItem::ResponseItem(inter_agent_msg(
            "triggered task",
            /*trigger_turn*/ true,
        )),
        RolloutItem::ResponseItem(assistant_msg("a2")),
        RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
            num_turns: 1,
        })),
        RolloutItem::ResponseItem(user_msg("u2")),
        RolloutItem::ResponseItem(assistant_msg("a3")),
    ];

    let truncated = truncate_rollout_to_last_n_fork_turns(&rollout, /*n_from_end*/ 2);

    assert_eq!(
        serde_json::to_value(&truncated).unwrap(),
        serde_json::to_value(&rollout).unwrap()
    );
}

#[test]
fn fork_turn_positions_ignore_zero_turn_rollback_markers() {
    let rollout = vec![
        RolloutItem::ResponseItem(user_msg("u1")),
        RolloutItem::ResponseItem(inter_agent_msg(
            "triggered task",
            /*trigger_turn*/ true,
        )),
        RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
            num_turns: 0,
        })),
        RolloutItem::ResponseItem(user_msg("u2")),
    ];

    assert_eq!(fork_turn_positions_in_rollout(&rollout), vec![0, 1, 3]);
}

#[test]
fn truncates_rollout_to_last_n_fork_turns_discards_trigger_boundaries_in_rolled_back_suffix() {
    let rollout = vec![
        RolloutItem::ResponseItem(user_msg("u1")),
        RolloutItem::ResponseItem(user_msg("u2")),
        RolloutItem::ResponseItem(inter_agent_msg(
            "triggered task",
            /*trigger_turn*/ true,
        )),
        RolloutItem::ResponseItem(assistant_msg("a1")),
        RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
            num_turns: 1,
        })),
        RolloutItem::ResponseItem(user_msg("u3")),
        RolloutItem::ResponseItem(assistant_msg("a2")),
    ];

    let truncated = truncate_rollout_to_last_n_fork_turns(&rollout, /*n_from_end*/ 2);

    let expected = rollout[1..].to_vec();

    assert_eq!(
        serde_json::to_value(&truncated).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
}

#[test]
fn truncates_rollout_to_last_n_fork_turns_discards_rolled_back_assistant_instruction_turns() {
    let rollout = vec![
        RolloutItem::ResponseItem(user_msg("u1")),
        RolloutItem::ResponseItem(assistant_msg("a1")),
        RolloutItem::ResponseItem(inter_agent_msg(
            "triggered task 1",
            /*trigger_turn*/ true,
        )),
        RolloutItem::ResponseItem(assistant_msg("a2")),
        RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
            num_turns: 1,
        })),
        RolloutItem::ResponseItem(inter_agent_msg(
            "triggered task 2",
            /*trigger_turn*/ true,
        )),
        RolloutItem::ResponseItem(assistant_msg("a3")),
    ];

    let truncated = truncate_rollout_to_last_n_fork_turns(&rollout, /*n_from_end*/ 1);
    let expected = rollout[5..].to_vec();

    assert_eq!(
        serde_json::to_value(&truncated).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
}

#[test]
fn truncates_rollout_to_last_n_fork_turns_keeps_full_rollout_when_n_is_large() {
    let rollout = vec![
        RolloutItem::ResponseItem(user_msg("u1")),
        RolloutItem::ResponseItem(assistant_msg("a1")),
        RolloutItem::ResponseItem(inter_agent_msg(
            "triggered task",
            /*trigger_turn*/ true,
        )),
        RolloutItem::ResponseItem(assistant_msg("a2")),
    ];

    let truncated = truncate_rollout_to_last_n_fork_turns(&rollout, /*n_from_end*/ 10);

    assert_eq!(
        serde_json::to_value(&truncated).unwrap(),
        serde_json::to_value(&rollout).unwrap()
    );
}

#[tokio::test]
async fn last_n_fork_sanitizes_parent_checkpoint_recovery_state() {
    let (session, turn_context) = make_session_and_context().await;
    let turn_context = Arc::new(turn_context);
    let parent_reference_context = turn_context.to_turn_context_item();
    let parent_world_state = build_world_state_from_turn_context(&session, &turn_context).await;
    let parent_token_info = TokenUsageInfo {
        total_token_usage: TokenUsage {
            total_tokens: 42_000,
            ..TokenUsage::default()
        },
        last_token_usage: TokenUsage {
            total_tokens: 2_000,
            ..TokenUsage::default()
        },
        model_context_window: Some(64_000),
    };
    let rate_limits = RateLimitSnapshot {
        limit_id: Some("account-limit".to_string()),
        limit_name: None,
        primary: None,
        secondary: None,
        credits: None,
        individual_limit: None,
        plan_type: None,
        rate_limit_reached_type: None,
    };
    let parent_first_window_id = uuid::Uuid::now_v7().to_string();
    let parent_previous_window_id = uuid::Uuid::now_v7().to_string();
    let parent_window_id = uuid::Uuid::now_v7().to_string();
    let checkpoint = CompactedItem {
        message: "retained compact summary".to_string(),
        replacement_history: Some(vec![
            user_msg("retained parent turn"),
            assistant_msg("retained compact summary"),
        ]),
        window_number: Some(7),
        first_window_id: Some(parent_first_window_id.clone()),
        previous_window_id: Some(parent_previous_window_id.clone()),
        window_id: Some(parent_window_id.clone()),
        checkpoint: Some(CompactionCheckpoint {
            checkpoint_id: "parent-checkpoint".to_string(),
            reference_context_item: Some(parent_reference_context),
            world_state: Some(WorldStateItem::full(
                parent_world_state.snapshot().into_value(),
            )),
            api_token_count: TokenCountEvent {
                info: Some(parent_token_info.clone()),
                rate_limits: Some(rate_limits.clone()),
            },
            final_token_count: TokenCountEvent {
                info: Some(parent_token_info.clone()),
                rate_limits: Some(rate_limits.clone()),
            },
            server_reasoning_included: true,
        }),
    };
    let rollout = vec![
        RolloutItem::ResponseItem(user_msg("retained parent turn")),
        RolloutItem::Compacted(checkpoint),
        RolloutItem::EventMsg(EventMsg::TokenCount(TokenCountEvent {
            info: Some(parent_token_info),
            rate_limits: Some(rate_limits.clone()),
        })),
        RolloutItem::ResponseItem(assistant_msg("retained answer")),
    ];

    let truncated = truncate_rollout_to_last_n_fork_turns(&rollout, /*n_from_end*/ 1);
    let compacted = truncated
        .iter()
        .find_map(|item| match item {
            RolloutItem::Compacted(compacted) => Some(compacted),
            _ => None,
        })
        .expect("retained compact checkpoint");
    assert_eq!(compacted.window_number, Some(1));
    let child_first_window_id = uuid::Uuid::parse_str(
        compacted
            .first_window_id
            .as_deref()
            .expect("child first window id"),
    )
    .expect("child first window id should be a UUID");
    let child_previous_window_id = uuid::Uuid::parse_str(
        compacted
            .previous_window_id
            .as_deref()
            .expect("child previous window id"),
    )
    .expect("child previous window id should be a UUID");
    let child_window_id =
        uuid::Uuid::parse_str(compacted.window_id.as_deref().expect("child window id"))
            .expect("child window id should be a UUID");
    assert_eq!(child_first_window_id.get_version_num(), 7);
    assert_eq!(child_previous_window_id, child_first_window_id);
    assert_eq!(child_window_id.get_version_num(), 7);
    assert_ne!(child_window_id, child_first_window_id);
    assert_ne!(child_first_window_id.to_string(), parent_first_window_id);
    assert_ne!(
        child_previous_window_id.to_string(),
        parent_previous_window_id
    );
    assert_ne!(child_window_id.to_string(), parent_window_id);
    let checkpoint = compacted.checkpoint.as_ref().expect("nested checkpoint");
    assert_eq!(checkpoint.reference_context_item, None);
    assert_eq!(checkpoint.world_state, None);
    assert_eq!(checkpoint.api_token_count.info, None);
    assert_eq!(checkpoint.final_token_count.info, None);
    assert_eq!(
        checkpoint.final_token_count.rate_limits,
        Some(rate_limits),
        "account-scoped rate limits remain useful across a child fork"
    );
    assert!(!checkpoint.server_reasoning_included);
    assert!(truncated.iter().all(|item| {
        !matches!(
            item,
            RolloutItem::TurnContext(_)
                | RolloutItem::WorldState(_)
                | RolloutItem::EventMsg(EventMsg::TokenCount(TokenCountEvent {
                    info: Some(_),
                    ..
                }))
        )
    }));
}

#[test]
fn last_n_fork_rewrites_multiple_compactions_as_one_child_local_chain() {
    let parent_first = uuid::Uuid::now_v7().to_string();
    let parent_window_one = uuid::Uuid::now_v7().to_string();
    let parent_window_two = uuid::Uuid::now_v7().to_string();
    let compacted = |number, previous: &str, window: &str| {
        RolloutItem::Compacted(CompactedItem {
            message: format!("summary {number}"),
            replacement_history: None,
            window_number: Some(number),
            first_window_id: Some(parent_first.clone()),
            previous_window_id: Some(previous.to_string()),
            window_id: Some(window.to_string()),
            checkpoint: None,
        })
    };
    let rollout = vec![
        RolloutItem::ResponseItem(user_msg("turn one")),
        compacted(8, &parent_first, &parent_window_one),
        RolloutItem::ResponseItem(user_msg("turn two")),
        compacted(9, &parent_window_one, &parent_window_two),
    ];

    let truncated = truncate_rollout_to_last_n_fork_turns(&rollout, /*n_from_end*/ 2);
    let windows = truncated
        .iter()
        .filter_map(|item| match item {
            RolloutItem::Compacted(compacted) => Some(compacted),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(windows.len(), 2);
    assert_eq!(windows[0].window_number, Some(1));
    assert_eq!(windows[1].window_number, Some(2));
    assert_eq!(windows[0].first_window_id, windows[1].first_window_id);
    assert_eq!(windows[0].previous_window_id, windows[0].first_window_id);
    assert_eq!(windows[1].previous_window_id, windows[0].window_id);
    assert_ne!(
        windows[0].first_window_id.as_deref(),
        Some(parent_first.as_str())
    );
    assert_ne!(
        windows[0].window_id.as_deref(),
        Some(parent_window_one.as_str())
    );
    assert_ne!(
        windows[1].window_id.as_deref(),
        Some(parent_window_two.as_str())
    );
}

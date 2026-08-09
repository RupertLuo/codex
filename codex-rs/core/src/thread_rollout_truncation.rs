//! Helpers for truncating rollouts based on "user turn" boundaries.
//!
//! In core, "user turns" are detected by scanning `ResponseItem::Message` items and
//! interpreting them via `event_mapping::parse_turn_item(...)`.

use crate::context_manager::is_user_turn_boundary;
use crate::event_mapping;
use codex_app_server_protocol::TurnStatus;
use codex_app_server_protocol::build_turns_from_rollout_items;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::items::TurnItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::InitialHistory;
use codex_protocol::protocol::InterAgentCommunication;
use codex_protocol::protocol::RolloutItem;
use codex_protocol::protocol::RolloutItemFlattener;
use codex_protocol::protocol::RolloutTransaction;
use uuid::Uuid;

pub(crate) fn initial_history_has_prior_user_turns(conversation_history: &InitialHistory) -> bool {
    conversation_history.scan_rollout_items(rollout_item_is_user_turn_boundary)
}

fn rollout_item_is_user_turn_boundary(item: &RolloutItem) -> bool {
    match item {
        RolloutItem::ResponseItem(item) => is_user_turn_boundary(item),
        RolloutItem::InterAgentCommunication(_) => true,
        _ => false,
    }
}

/// Return the indices of user message boundaries in a rollout.
///
/// A user message boundary is a `RolloutItem::ResponseItem(ResponseItem::Message { .. })`
/// whose parsed turn item is `TurnItem::UserMessage`.
///
/// Rollouts can contain `ThreadRolledBack` markers. Those markers indicate that the
/// last N user turns were removed from the effective thread history; we apply them here so
/// indexing uses the post-rollback history rather than the raw stream.
pub(crate) fn user_message_positions_in_rollout(items: &[RolloutItem]) -> Vec<usize> {
    let mut user_positions = Vec::new();
    for (idx, item) in items.iter().enumerate() {
        match item {
            RolloutItem::ResponseItem(item @ ResponseItem::Message { .. })
                if matches!(
                    event_mapping::parse_turn_item(item),
                    Some(TurnItem::UserMessage(_))
                ) =>
            {
                user_positions.push(idx);
            }
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(rollback)) => {
                let num_turns = usize::try_from(rollback.num_turns).unwrap_or(usize::MAX);
                let new_len = user_positions.len().saturating_sub(num_turns);
                user_positions.truncate(new_len);
            }
            _ => {}
        }
    }
    user_positions
}

/// Return the indices of fork-turn boundaries in a rollout.
///
/// A fork-turn boundary is either:
/// - a real user message boundary, or
/// - an inter-agent communication whose `trigger_turn` is `true`, or
/// - a legacy assistant inter-agent envelope with the same flag.
///
/// Like `user_message_positions_in_rollout`, this applies `ThreadRolledBack` markers so indexing
/// reflects the effective post-rollback history. Rollback counts instruction turns, so a rollback
/// removes the stale suffix starting at the earliest rolled-back instruction-turn boundary instead
/// of simply truncating the mixed fork-boundary list.
pub(crate) fn fork_turn_positions_in_rollout(items: &[RolloutItem]) -> Vec<usize> {
    let mut rollback_turn_positions = Vec::new();
    let mut fork_turn_positions = Vec::new();
    for (idx, item) in items.iter().enumerate() {
        match item {
            RolloutItem::ResponseItem(item) => {
                let has_delivery_metadata = matches!(item, ResponseItem::AgentMessage { .. })
                    && idx.checked_sub(1).is_some_and(|previous_idx| {
                        matches!(
                            items.get(previous_idx),
                            Some(RolloutItem::InterAgentCommunicationMetadata { .. })
                        )
                    });
                if is_user_turn_boundary(item) && !has_delivery_metadata {
                    rollback_turn_positions.push(idx);
                }
                if is_real_user_message_boundary(item) || is_trigger_turn_boundary(item) {
                    fork_turn_positions.push(idx);
                }
            }
            RolloutItem::InterAgentCommunication(communication) => {
                rollback_turn_positions.push(idx);
                if communication.trigger_turn {
                    fork_turn_positions.push(idx);
                }
            }
            RolloutItem::InterAgentCommunicationMetadata { trigger_turn } => {
                rollback_turn_positions.push(idx);
                if *trigger_turn {
                    fork_turn_positions.push(idx);
                }
            }
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(rollback)) => {
                let num_turns = usize::try_from(rollback.num_turns).unwrap_or(usize::MAX);
                if num_turns == 0 {
                    continue;
                }
                let Some(rollback_start_idx) = rollback_turn_positions
                    .len()
                    .checked_sub(num_turns)
                    .map(|rollback_start| rollback_turn_positions[rollback_start])
                    .or_else(|| rollback_turn_positions.first().copied())
                else {
                    continue;
                };
                let new_rollback_len = rollback_turn_positions.len().saturating_sub(num_turns);
                rollback_turn_positions.truncate(new_rollback_len);
                fork_turn_positions.retain(|position| *position < rollback_start_idx);
            }
            _ => {}
        }
    }
    fork_turn_positions
}

/// Return a prefix of `items` obtained by cutting strictly before the nth user message.
///
/// The boundary index is 0-based from the start of `items` (so `n_from_start = 0` returns
/// a prefix that excludes the first user message and everything after it).
///
/// If `n_from_start` is `usize::MAX`, this returns the full rollout (no truncation).
/// If fewer than or equal to `n_from_start` user messages exist, this returns the full
/// rollout unchanged.
pub(crate) fn truncate_rollout_before_nth_user_message_from_start(
    items: &[RolloutItem],
    n_from_start: usize,
) -> Vec<RolloutItem> {
    if n_from_start == usize::MAX {
        return items.to_vec();
    }

    let user_positions = user_message_positions_in_rollout(items);

    // If fewer than or equal to n user messages exist, keep the full rollout.
    if user_positions.len() <= n_from_start {
        return items.to_vec();
    }

    // Cut strictly before the nth user message (do not keep the nth itself).
    let cut_idx = user_positions[n_from_start];
    items[..cut_idx].to_vec()
}

/// Return a rollout prefix ending after the requested persisted terminal turn.
///
/// The turn must still be present in the effective post-rollback history and
/// must have an explicit persisted TurnStarted boundary. Synthetic IDs
/// generated while projecting legacy rollouts are intentionally unsupported
/// because they do not provide a stable raw rollout boundary for a fork.
pub fn truncate_rollout_after_turn_id(
    items: &[RolloutItem],
    last_turn_id: &str,
) -> CodexResult<Vec<RolloutItem>> {
    let mut flattener = RolloutItemFlattener::default();
    let mut logical_items = Vec::new();
    let mut physical_logical_spans = Vec::with_capacity(items.len());
    for item in items {
        let start = logical_items.len();
        logical_items.extend(
            flattener
                .flatten(std::slice::from_ref(item))
                .map_err(|err| {
                    CodexErr::Fatal(format!(
                        "cannot truncate corrupt rollout transaction history: {err}"
                    ))
                })?
                .into_iter()
                .cloned(),
        );
        physical_logical_spans.push(start..logical_items.len());
    }
    let logical_items = logical_items;
    let logical_slice = logical_items.as_slice();
    let turns = build_turns_from_rollout_items(logical_slice);
    let turn = turns
        .iter()
        .find(|turn| turn.id == last_turn_id)
        .ok_or_else(|| {
            CodexErr::InvalidRequest(format!(
                "lastTurnId '{last_turn_id}' was not found in the source thread"
            ))
        })?;

    let target_start_index = logical_slice
        .iter()
        .position(|item| {
            matches!(
                item,
                RolloutItem::EventMsg(EventMsg::TurnStarted(event))
                    if event.turn_id == last_turn_id
            )
        })
        .ok_or_else(|| {
            CodexErr::InvalidRequest(format!(
                "lastTurnId '{last_turn_id}' is not a persisted canonical turn in the source thread"
            ))
        })?;

    if matches!(turn.status, TurnStatus::InProgress) {
        return Err(CodexErr::InvalidRequest(format!(
            "lastTurnId '{last_turn_id}' identifies an in-progress turn"
        )));
    }

    let cut_index = logical_slice
        .iter()
        .enumerate()
        .skip(target_start_index.saturating_add(1))
        .find_map(|(index, item)| {
            matches!(item, RolloutItem::EventMsg(EventMsg::TurnStarted(_))).then_some(index)
        })
        .unwrap_or(logical_slice.len());

    if cut_index == logical_slice.len() {
        return Ok(items.to_vec());
    }
    let physical_cut_index = physical_logical_spans
        .iter()
        .position(|span| span.end > cut_index)
        .expect("a logical cut before the end must belong to a physical record");
    let span = &physical_logical_spans[physical_cut_index];
    if cut_index == span.start {
        return Ok(items[..physical_cut_index].to_vec());
    }

    let RolloutItem::Transaction(_) = &items[physical_cut_index] else {
        unreachable!("one physical legacy record contributes exactly one logical item");
    };
    let retained_transaction_items = logical_slice[span.start..cut_index].to_vec();
    let mut truncated = items[..physical_cut_index].to_vec();
    truncated.push(RolloutItem::Transaction(RolloutTransaction {
        // A stable transaction ID denotes one immutable payload. Splitting a parent envelope for
        // a fork creates a new atomic record, so it must receive a fresh identity.
        transaction_id: Uuid::now_v7().to_string(),
        items: retained_transaction_items,
    }));
    Ok(truncated)
}

/// Return a suffix of `items` that keeps the last `n_from_end` fork turns.
///
/// If fewer than or equal to `n_from_end` fork turns exist, this keeps from the first fork-turn
/// boundary and still drops pre-turn startup context.
pub(crate) fn truncate_rollout_to_last_n_fork_turns(
    items: &[RolloutItem],
    n_from_end: usize,
) -> Vec<RolloutItem> {
    if n_from_end == 0 {
        return Vec::new();
    }

    let fork_turn_positions = fork_turn_positions_in_rollout(items);
    let Some(keep_idx) = fork_turn_positions
        .len()
        .checked_sub(n_from_end)
        .map(|position| fork_turn_positions[position])
        .or_else(|| fork_turn_positions.first().copied())
    else {
        return Vec::new();
    };

    let mut forked_items = items[keep_idx..].to_vec();
    forked_items.retain_mut(sanitize_last_n_fork_item);
    rewrite_last_n_fork_window_identity(&mut forked_items);
    forked_items
}

/// Remove thread-local recovery state from rollout items retained for a bounded child fork.
///
/// Replacement history and checkpoint IDs remain part of the child's inherited transcript.
/// Context/world-state baselines, cumulative usage, and reasoning accounting belong to the parent
/// thread and must be rebuilt by the child. Rate-limit snapshots are intentionally retained because
/// they describe the account/provider rather than a thread. Compaction-window identities are
/// rewritten into a fresh child-local chain after sanitization.
fn sanitize_last_n_fork_item(item: &mut RolloutItem) -> bool {
    match item {
        RolloutItem::TurnContext(_) | RolloutItem::WorldState(_) => false,
        RolloutItem::EventMsg(EventMsg::TokenCount(token_count)) => {
            token_count.info = None;
            token_count.rate_limits.is_some()
        }
        RolloutItem::Compacted(compacted) => {
            if let Some(checkpoint) = compacted.checkpoint.as_mut() {
                checkpoint.reference_context_item = None;
                checkpoint.world_state = None;
                checkpoint.api_token_count.info = None;
                checkpoint.final_token_count.info = None;
                checkpoint.server_reasoning_included = false;
            }
            true
        }
        _ => true,
    }
}

fn rewrite_last_n_fork_window_identity(items: &mut [RolloutItem]) {
    let first_window_id = Uuid::now_v7();
    let mut previous_window_id = first_window_id;
    let mut window_number = 0_u64;

    for item in items {
        let RolloutItem::Compacted(compacted) = item else {
            continue;
        };
        window_number = window_number.saturating_add(1);
        let window_id = Uuid::now_v7();
        compacted.window_number = Some(window_number);
        compacted.first_window_id = Some(first_window_id.to_string());
        compacted.previous_window_id = Some(previous_window_id.to_string());
        compacted.window_id = Some(window_id.to_string());
        previous_window_id = window_id;
    }
}

fn is_real_user_message_boundary(item: &ResponseItem) -> bool {
    matches!(
        event_mapping::parse_turn_item(item),
        Some(TurnItem::UserMessage(_))
    )
}

fn is_trigger_turn_boundary(item: &ResponseItem) -> bool {
    let ResponseItem::Message { role, content, .. } = item else {
        return false;
    };

    role == "assistant"
        && InterAgentCommunication::from_message_content(content)
            .is_some_and(|communication| communication.trigger_turn)
}

#[cfg(test)]
#[path = "thread_rollout_truncation_tests.rs"]
mod tests;

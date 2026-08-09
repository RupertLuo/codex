use crate::protocol::EventMsg;
use crate::protocol::RolloutItem;
use crate::protocol::RolloutTransaction;
use codex_protocol::models::ResponseItem;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RolloutTransactionBuildError {
    EmptyTransactionId,
    EmptyTransaction,
    NestedTransaction,
    TooManyItems { max_items: usize },
}

impl std::fmt::Display for RolloutTransactionBuildError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyTransactionId => formatter.write_str("rollout transaction ID is empty"),
            Self::EmptyTransaction => {
                formatter.write_str("rollout transaction has no durable items")
            }
            Self::NestedTransaction => {
                formatter.write_str("nested rollout transactions cannot be written")
            }
            Self::TooManyItems { max_items } => {
                write!(
                    formatter,
                    "rollout transaction exceeds maximum item count {max_items}"
                )
            }
        }
    }
}

impl std::error::Error for RolloutTransactionBuildError {}

/// Applies rollout persistence policy before constructing one canonical transaction envelope.
pub fn build_rollout_transaction(
    transaction_id: String,
    items: &[RolloutItem],
) -> Result<RolloutItem, RolloutTransactionBuildError> {
    if transaction_id.trim().is_empty() {
        return Err(RolloutTransactionBuildError::EmptyTransactionId);
    }
    if items
        .iter()
        .any(|item| matches!(item, RolloutItem::Transaction(_)))
    {
        return Err(RolloutTransactionBuildError::NestedTransaction);
    }
    // The envelope itself consumes one traversal slot when read.
    let max_items = codex_protocol::protocol::DEFAULT_MAX_EXPANDED_ROLLOUT_ITEMS.saturating_sub(1);
    let mut persisted_items = Vec::new();
    for item in items
        .iter()
        .filter(|item| should_persist_transaction_item(item))
    {
        if persisted_items.len() == max_items {
            return Err(RolloutTransactionBuildError::TooManyItems { max_items });
        }
        persisted_items.push(item.clone());
    }
    if persisted_items.is_empty() {
        return Err(RolloutTransactionBuildError::EmptyTransaction);
    }
    Ok(RolloutItem::Transaction(RolloutTransaction {
        transaction_id,
        items: persisted_items,
    }))
}

/// Whether a rollout `item` should be persisted in rollout files.
pub fn is_persisted_rollout_item(item: &RolloutItem) -> bool {
    match item {
        RolloutItem::ResponseItem(item) => should_persist_response_item(item),
        RolloutItem::InterAgentCommunication(_)
        | RolloutItem::InterAgentCommunicationMetadata { .. } => true,
        RolloutItem::EventMsg(ev) => should_persist_event_msg(ev),
        // Persist Codex executive markers so we can analyze flows (e.g., compaction, API turns).
        RolloutItem::Compacted(_)
        | RolloutItem::TurnContext(_)
        | RolloutItem::WorldState(_)
        | RolloutItem::SessionMeta(_) => true,
        RolloutItem::Transaction(transaction) => is_canonical_rollout_transaction(transaction),
    }
}

fn is_canonical_rollout_transaction(transaction: &RolloutTransaction) -> bool {
    !transaction.transaction_id.trim().is_empty()
        && !transaction.items.is_empty()
        && transaction.items.len() < codex_protocol::protocol::DEFAULT_MAX_EXPANDED_ROLLOUT_ITEMS
        && transaction.items.iter().all(|item| {
            !matches!(item, RolloutItem::Transaction(_)) && should_persist_transaction_item(item)
        })
}

fn should_persist_transaction_item(item: &RolloutItem) -> bool {
    match item {
        // A transaction retains the paired raw event so durable history and live delivery expose
        // the same logical unit. Standalone raw events remain filtered by the ordinary policy.
        RolloutItem::EventMsg(EventMsg::RawResponseItem(event)) => {
            should_persist_response_item(&event.item)
        }
        RolloutItem::SessionMeta(_)
        | RolloutItem::ResponseItem(_)
        | RolloutItem::InterAgentCommunication(_)
        | RolloutItem::InterAgentCommunicationMetadata { .. }
        | RolloutItem::Compacted(_)
        | RolloutItem::TurnContext(_)
        | RolloutItem::WorldState(_)
        | RolloutItem::Transaction(_)
        | RolloutItem::EventMsg(_) => is_persisted_rollout_item(item),
    }
}

/// Return the canonical rollout items that should be persisted for a live append.
pub fn persisted_rollout_items(items: &[RolloutItem]) -> Vec<RolloutItem> {
    let mut persisted = Vec::new();
    for item in items {
        if is_persisted_rollout_item(item) {
            persisted.push(item.clone());
        }
    }
    persisted
}

/// Whether a `ResponseItem` should be persisted in rollout files.
#[inline]
pub fn should_persist_response_item(item: &ResponseItem) -> bool {
    match item {
        ResponseItem::Message { .. }
        | ResponseItem::AgentMessage { .. }
        | ResponseItem::Reasoning { .. }
        | ResponseItem::LocalShellCall { .. }
        | ResponseItem::FunctionCall { .. }
        | ResponseItem::ToolSearchCall { .. }
        | ResponseItem::FunctionCallOutput { .. }
        | ResponseItem::ToolSearchOutput { .. }
        | ResponseItem::CustomToolCall { .. }
        | ResponseItem::CustomToolCallOutput { .. }
        | ResponseItem::WebSearchCall { .. }
        | ResponseItem::ImageGenerationCall { .. }
        | ResponseItem::Compaction { .. }
        | ResponseItem::ContextCompaction { .. } => true,
        ResponseItem::AdditionalTools { .. }
        | ResponseItem::CompactionTrigger { .. }
        | ResponseItem::Other => false,
    }
}

/// Whether a `ResponseItem` should be persisted for the memories.
#[inline]
pub fn should_persist_response_item_for_memories(item: &ResponseItem) -> bool {
    match item {
        ResponseItem::Message { role, .. } => role != "developer",
        ResponseItem::AgentMessage { .. }
        | ResponseItem::LocalShellCall { .. }
        | ResponseItem::FunctionCall { .. }
        | ResponseItem::ToolSearchCall { .. }
        | ResponseItem::FunctionCallOutput { .. }
        | ResponseItem::ToolSearchOutput { .. }
        | ResponseItem::CustomToolCall { .. }
        | ResponseItem::CustomToolCallOutput { .. }
        | ResponseItem::WebSearchCall { .. } => true,
        ResponseItem::AdditionalTools { .. }
        | ResponseItem::Reasoning { .. }
        | ResponseItem::ImageGenerationCall { .. }
        | ResponseItem::Compaction { .. }
        | ResponseItem::CompactionTrigger { .. }
        | ResponseItem::ContextCompaction { .. }
        | ResponseItem::Other => false,
    }
}

/// Whether an `EventMsg` should be persisted in rollout files.
#[inline]
pub fn should_persist_event_msg(ev: &EventMsg) -> bool {
    match ev {
        EventMsg::UserMessage(_)
        | EventMsg::AgentMessage(_)
        | EventMsg::AgentReasoning(_)
        | EventMsg::AgentReasoningRawContent(_)
        | EventMsg::PatchApplyEnd(_)
        | EventMsg::TokenCount(_)
        | EventMsg::ThreadGoalUpdated(_)
        | EventMsg::ContextCompacted(_)
        | EventMsg::EnteredReviewMode(_)
        | EventMsg::ExitedReviewMode(_)
        | EventMsg::McpToolCallEnd(_)
        | EventMsg::ThreadRolledBack(_)
        | EventMsg::TurnAborted(_)
        | EventMsg::TurnStarted(_)
        | EventMsg::TurnComplete(_)
        | EventMsg::WebSearchEnd(_)
        | EventMsg::ImageGenerationEnd(_)
        | EventMsg::RealtimeConversationClosed(_)
        | EventMsg::SubAgentActivity(_) => true,
        EventMsg::ItemCompleted(event) => {
            // These items have no equivalent raw ResponseItem or legacy event,
            // so persist their completion for replay without retaining every
            // item lifecycle event.
            matches!(
                event.item,
                codex_protocol::items::TurnItem::Plan(_)
                    | codex_protocol::items::TurnItem::Sleep(_)
            )
        }
        EventMsg::Error(_)
        | EventMsg::GuardianAssessment(_)
        | EventMsg::ExecCommandEnd(_)
        | EventMsg::ViewImageToolCall(_)
        | EventMsg::CollabAgentSpawnEnd(_)
        | EventMsg::CollabAgentInteractionEnd(_)
        | EventMsg::CollabWaitingEnd(_)
        | EventMsg::CollabCloseEnd(_)
        | EventMsg::CollabResumeEnd(_)
        | EventMsg::DynamicToolCallRequest(_)
        | EventMsg::DynamicToolCallResponse(_)
        | EventMsg::Warning(_)
        | EventMsg::GuardianWarning(_)
        | EventMsg::RealtimeConversationStarted(_)
        | EventMsg::RealtimeConversationSdp(_)
        | EventMsg::RealtimeConversationRealtime(_)
        | EventMsg::SafetyBuffering(_)
        | EventMsg::ModelReroute(_)
        | EventMsg::ModelVerification(_)
        | EventMsg::TurnModerationMetadata(_)
        | EventMsg::AgentReasoningSectionBreak(_)
        | EventMsg::RawResponseItem(_)
        | EventMsg::SessionConfigured(_)
        | EventMsg::ThreadSettingsApplied(_)
        | EventMsg::McpToolCallBegin(_)
        | EventMsg::ExecCommandBegin(_)
        | EventMsg::TerminalInteraction(_)
        | EventMsg::ExecCommandOutputDelta(_)
        | EventMsg::ExecApprovalRequest(_)
        | EventMsg::RequestPermissions(_)
        | EventMsg::RequestUserInput(_)
        | EventMsg::ElicitationRequest(_)
        | EventMsg::ApplyPatchApprovalRequest(_)
        | EventMsg::StreamError(_)
        | EventMsg::PatchApplyBegin(_)
        | EventMsg::PatchApplyUpdated(_)
        | EventMsg::TurnDiff(_)
        | EventMsg::RealtimeConversationListVoicesResponse(_)
        | EventMsg::McpStartupUpdate(_)
        | EventMsg::McpStartupComplete(_)
        | EventMsg::WebSearchBegin(_)
        | EventMsg::PlanUpdate(_)
        | EventMsg::ShutdownComplete
        | EventMsg::DeprecationNotice(_)
        | EventMsg::ItemStarted(_)
        | EventMsg::HookStarted(_)
        | EventMsg::HookCompleted(_)
        | EventMsg::AgentMessageContentDelta(_)
        | EventMsg::PlanDelta(_)
        | EventMsg::ReasoningContentDelta(_)
        | EventMsg::ReasoningRawContentDelta(_)
        | EventMsg::ImageGenerationBegin(_)
        | EventMsg::CollabAgentSpawnBegin(_)
        | EventMsg::CollabAgentInteractionBegin(_)
        | EventMsg::CollabWaitingBegin(_)
        | EventMsg::CollabCloseBegin(_)
        | EventMsg::CollabResumeBegin(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::protocol::RealtimeConversationClosedEvent;
    use codex_protocol::protocol::RolloutTransaction;
    use codex_protocol::protocol::WarningEvent;
    use codex_protocol::protocol::WorldStateItem;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn world_state(label: &str) -> RolloutItem {
        RolloutItem::WorldState(WorldStateItem::full(json!({ "label": label })))
    }

    #[test]
    fn transaction_builder_filters_before_constructing_one_canonical_envelope() {
        let transaction = build_rollout_transaction(
            "txn-1".to_string(),
            &[
                RolloutItem::EventMsg(EventMsg::Warning(WarningEvent {
                    message: "not durable".to_string(),
                })),
                world_state("durable"),
            ],
        )
        .expect("one durable item should produce a transaction");

        let RolloutItem::Transaction(transaction) = transaction else {
            panic!("expected transaction envelope");
        };
        assert_eq!(transaction.transaction_id, "txn-1");
        assert_eq!(transaction.items.len(), 1);
        assert!(matches!(
            transaction.items.as_slice(),
            [RolloutItem::WorldState(_)]
        ));
        assert!(is_persisted_rollout_item(&RolloutItem::Transaction(
            transaction
        )));
    }

    #[test]
    fn transaction_builder_rejects_empty_id_empty_payload_and_nested_transaction() {
        assert_eq!(
            build_rollout_transaction(String::new(), &[world_state("item")]).unwrap_err(),
            RolloutTransactionBuildError::EmptyTransactionId
        );
        assert_eq!(
            build_rollout_transaction(
                "empty".to_string(),
                &[RolloutItem::EventMsg(EventMsg::Warning(WarningEvent {
                    message: "filtered".to_string(),
                }))],
            )
            .unwrap_err(),
            RolloutTransactionBuildError::EmptyTransaction
        );
        assert_eq!(
            build_rollout_transaction(
                "outer".to_string(),
                &[RolloutItem::Transaction(RolloutTransaction {
                    transaction_id: "inner".to_string(),
                    items: vec![world_state("nested")],
                })],
            )
            .unwrap_err(),
            RolloutTransactionBuildError::NestedTransaction
        );
    }

    #[test]
    fn transaction_builder_rejects_payload_over_reader_expansion_limit() {
        let max_items =
            codex_protocol::protocol::DEFAULT_MAX_EXPANDED_ROLLOUT_ITEMS.saturating_sub(1);
        let items = vec![world_state("bounded"); max_items + 1];

        assert_eq!(
            build_rollout_transaction("too-large".to_string(), &items).unwrap_err(),
            RolloutTransactionBuildError::TooManyItems { max_items }
        );
    }

    #[test]
    fn persistence_policy_rejects_noncanonical_transaction_payloads() {
        let cases = [
            RolloutTransaction {
                transaction_id: String::new(),
                items: vec![world_state("empty-id")],
            },
            RolloutTransaction {
                transaction_id: "empty".to_string(),
                items: Vec::new(),
            },
            RolloutTransaction {
                transaction_id: "filtered-child".to_string(),
                items: vec![RolloutItem::EventMsg(EventMsg::Warning(WarningEvent {
                    message: "not durable".to_string(),
                }))],
            },
            RolloutTransaction {
                transaction_id: "nested".to_string(),
                items: vec![RolloutItem::Transaction(RolloutTransaction {
                    transaction_id: "inner".to_string(),
                    items: vec![world_state("nested")],
                })],
            },
        ];

        assert_eq!(
            cases
                .into_iter()
                .map(|transaction| {
                    is_persisted_rollout_item(&RolloutItem::Transaction(transaction))
                })
                .collect::<Vec<_>>(),
            vec![false, false, false, false]
        );
    }

    #[test]
    fn realtime_closed_is_durable_lifecycle_state() {
        assert!(should_persist_event_msg(
            &EventMsg::RealtimeConversationClosed(RealtimeConversationClosedEvent {
                reason: Some("requested".to_string()),
            })
        ));
    }
}

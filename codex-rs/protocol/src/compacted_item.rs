use crate::models::ResponseItem;
use crate::protocol::CompactedItem;
use serde::Deserialize;

// Before `window_number` was introduced, the numeric window number was serialized as
// `window_id`. Accept that shape so existing rollouts remain resumable.
impl<'de> Deserialize<'de> for CompactedItem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let serialized = SerializedCompactedItem::deserialize(deserializer)?;
        let mut window_number = serialized.window_number;
        let window_id = match serialized.window_id {
            Some(SerializedWindowId::Id(window_id)) => Some(window_id),
            Some(SerializedWindowId::LegacyWindowNumber(legacy_window_number)) => {
                window_number.get_or_insert(legacy_window_number);
                None
            }
            None => None,
        };
        Ok(Self {
            message: serialized.message,
            replacement_history: serialized.replacement_history,
            window_number,
            first_window_id: serialized.first_window_id,
            previous_window_id: serialized.previous_window_id,
            window_id,
            checkpoint: serialized.checkpoint,
        })
    }
}

#[derive(Deserialize)]
struct SerializedCompactedItem {
    message: String,
    #[serde(default)]
    replacement_history: Option<Vec<ResponseItem>>,
    #[serde(default)]
    window_number: Option<u64>,
    #[serde(default)]
    first_window_id: Option<String>,
    #[serde(default)]
    previous_window_id: Option<String>,
    #[serde(default)]
    window_id: Option<SerializedWindowId>,
    #[serde(default)]
    checkpoint: Option<crate::protocol::CompactionCheckpoint>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SerializedWindowId {
    Id(String),
    LegacyWindowNumber(u64),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::CompactionCheckpoint;
    use crate::protocol::TokenCountEvent;
    use crate::protocol::TokenUsage;
    use crate::protocol::TokenUsageInfo;
    use crate::protocol::WorldStateItem;
    use anyhow::Result;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn serializes_window_number_and_id() -> Result<()> {
        let item = CompactedItem {
            message: "summary".to_string(),
            replacement_history: None,
            window_number: Some(3),
            first_window_id: Some("019b3f6e-0000-7000-8000-000000000001".to_string()),
            previous_window_id: Some("019b3f6e-0000-7000-8000-000000000002".to_string()),
            window_id: Some("019b3f6e-7a10-7cc3-8b6e-1d09e2f7a001".to_string()),
            checkpoint: None,
        };

        assert_eq!(
            serde_json::to_value(item)?,
            json!({
                "message": "summary",
                "window_number": 3,
                "first_window_id": "019b3f6e-0000-7000-8000-000000000001",
                "previous_window_id": "019b3f6e-0000-7000-8000-000000000002",
                "window_id": "019b3f6e-7a10-7cc3-8b6e-1d09e2f7a001",
            })
        );
        Ok(())
    }

    #[test]
    fn migrates_legacy_numeric_window_id() -> Result<()> {
        let item = serde_json::from_value::<CompactedItem>(json!({
            "message": "summary",
            "window_id": 3,
        }))?;

        assert_eq!(
            item,
            CompactedItem {
                message: "summary".to_string(),
                replacement_history: None,
                window_number: Some(3),
                first_window_id: None,
                previous_window_id: None,
                window_id: None,
                checkpoint: None,
            }
        );
        Ok(())
    }

    #[test]
    fn round_trips_atomic_compaction_checkpoint() -> Result<()> {
        let api_token_info = TokenUsageInfo {
            total_token_usage: TokenUsage {
                total_tokens: 110,
                ..TokenUsage::default()
            },
            last_token_usage: TokenUsage {
                total_tokens: 10,
                ..TokenUsage::default()
            },
            model_context_window: Some(4_096),
        };
        let final_token_info = TokenUsageInfo {
            last_token_usage: TokenUsage {
                total_tokens: 42,
                ..TokenUsage::default()
            },
            ..api_token_info.clone()
        };
        let item = CompactedItem {
            message: "summary".to_string(),
            replacement_history: Some(Vec::new()),
            window_number: Some(3),
            first_window_id: Some("019b3f6e-0000-7000-8000-000000000001".to_string()),
            previous_window_id: Some("019b3f6e-0000-7000-8000-000000000002".to_string()),
            window_id: Some("019b3f6e-7a10-7cc3-8b6e-1d09e2f7a001".to_string()),
            checkpoint: Some(CompactionCheckpoint {
                checkpoint_id: "compact-checkpoint-1".to_string(),
                reference_context_item: None,
                world_state: Some(WorldStateItem::full(json!({"cwd": "/tmp/project"}))),
                api_token_count: TokenCountEvent {
                    info: Some(api_token_info),
                    rate_limits: None,
                },
                final_token_count: TokenCountEvent {
                    info: Some(final_token_info),
                    rate_limits: None,
                },
                server_reasoning_included: true,
            }),
        };

        let value = serde_json::to_value(&item)?;
        assert_eq!(serde_json::from_value::<CompactedItem>(value)?, item);
        Ok(())
    }
}

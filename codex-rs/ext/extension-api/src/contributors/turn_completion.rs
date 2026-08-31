use crate::ExtensionData;
use crate::ExtensionFuture;

/// Hard byte limit for one model-visible turn-completion contribution.
pub const TURN_COMPLETION_CONTRIBUTION_MAX_BYTES: usize = 16 * 1024;

/// One bounded contextual-user contribution supplied at model quiescence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnCompletionContribution {
    text: String,
}

impl TurnCompletionContribution {
    pub fn new(text: impl Into<String>) -> Result<Self, TurnCompletionContributionError> {
        let text = text.into();
        if text.len() > TURN_COMPLETION_CONTRIBUTION_MAX_BYTES {
            return Err(TurnCompletionContributionError {
                actual_bytes: text.len(),
            });
        }
        Ok(Self { text })
    }

    pub fn as_str(&self) -> &str {
        self.text.as_str()
    }

    pub fn into_inner(self) -> String {
        self.text
    }

    pub fn is_within_limit(&self) -> bool {
        self.text.len() <= TURN_COMPLETION_CONTRIBUTION_MAX_BYTES
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TurnCompletionContributionError {
    actual_bytes: usize,
}

impl TurnCompletionContributionError {
    pub fn actual_bytes(&self) -> usize {
        self.actual_bytes
    }

    pub fn max_bytes(&self) -> usize {
        TURN_COMPLETION_CONTRIBUTION_MAX_BYTES
    }
}

impl std::fmt::Display for TurnCompletionContributionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "turn-completion contribution is {} bytes; maximum is {} bytes",
            self.actual_bytes, TURN_COMPLETION_CONTRIBUTION_MAX_BYTES
        )
    }
}

impl std::error::Error for TurnCompletionContributionError {}

/// Stable inputs exposed when the model would otherwise finish a turn.
pub struct TurnCompletionInput<'a> {
    pub turn_id: &'a str,
    pub last_agent_message: Option<&'a str>,
    pub session_store: &'a ExtensionData,
    pub thread_store: &'a ExtensionData,
    pub turn_store: &'a ExtensionData,
}

/// Extension contribution that can keep a turn alive with host-owned follow-up input.
///
/// Contributors run only after the model has no more tool work and before the
/// turn is finalized. Returning no contribution allows the turn to complete.
pub trait TurnCompletionContributor: Send + Sync {
    fn contribute<'a>(
        &'a self,
        input: TurnCompletionInput<'a>,
    ) -> ExtensionFuture<'a, Option<TurnCompletionContribution>>;
}

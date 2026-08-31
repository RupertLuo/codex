use crate::ExtensionData;
use crate::ExtensionFuture;
use crate::PromptFragment;

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
/// turn is finalized. Returning no fragments allows the turn to complete.
pub trait TurnCompletionContributor: Send + Sync {
    fn contribute<'a>(
        &'a self,
        input: TurnCompletionInput<'a>,
    ) -> ExtensionFuture<'a, Vec<PromptFragment>>;
}

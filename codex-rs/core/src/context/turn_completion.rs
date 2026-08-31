use codex_extension_api::TurnCompletionContribution;

use super::ContextualUserFragment;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TurnCompletion {
    text: String,
}

impl TurnCompletion {
    pub(crate) fn new(contribution: TurnCompletionContribution) -> Option<Self> {
        contribution.is_within_limit().then(|| Self {
            text: contribution.into_inner(),
        })
    }
}

impl ContextualUserFragment for TurnCompletion {
    fn role(&self) -> &'static str {
        "user"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<turn_completion>", "</turn_completion>")
    }

    fn body(&self) -> String {
        format!("\n{}\n", self.text)
    }
}

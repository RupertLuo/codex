use std::future::Future;
use std::pin::Pin;

/// Opening exchange handed to a [`ThreadTitleGenerator`].
#[derive(Clone, Debug)]
pub struct ThreadTitleRequest {
    /// The first user message text, already stripped of protocol prefixes.
    pub first_user_message: String,
    /// The first assistant answer, when one was available at dispatch time.
    pub first_assistant_message: Option<String>,
}

/// Host-supplied, best-effort generator that turns the opening exchange of a
/// thread into a short human title.
///
/// The store invokes this off the turn hot path after the first assistant turn
/// completes. Implementations must never block the caller: returning `None`
/// (or panicking, which is caught by the spawned task boundary) simply leaves
/// the existing rule-based title in place.
pub trait ThreadTitleGenerator: Send + Sync + std::fmt::Debug {
    /// Produce a concise title for the supplied opening exchange, or `None` when
    /// no better title could be generated.
    fn generate_title<'a>(
        &'a self,
        request: ThreadTitleRequest,
    ) -> Pin<Box<dyn Future<Output = Option<String>> + Send + 'a>>;
}

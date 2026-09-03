use std::future::Future;
use std::pin::Pin;

/// Opening exchange handed to a [`ThreadTitleGenerator`].
#[derive(Clone, Debug)]
pub struct ThreadTitleRequest {
    pub first_user_message: String,
    pub first_assistant_message: Option<String>,
}

/// Host-supplied, best-effort generator for concise thread titles.
pub trait ThreadTitleGenerator: Send + Sync + std::fmt::Debug {
    fn generate_title<'a>(
        &'a self,
        request: ThreadTitleRequest,
    ) -> Pin<Box<dyn Future<Output = Option<String>> + Send + 'a>>;
}

/// Owned capability authorizing one detached thread-metadata mutation sequence.
pub trait ThreadMetadataMutationPermit: Send {}

pub type ThreadMetadataMutationPermitFuture<'a> =
    Pin<Box<dyn Future<Output = Option<Box<dyn ThreadMetadataMutationPermit>>> + Send + 'a>>;

/// Host-owned lifecycle boundary for detached thread-metadata mutations.
pub trait ThreadMetadataMutationGate: Send + Sync + std::fmt::Debug {
    fn acquire<'a>(&'a self) -> ThreadMetadataMutationPermitFuture<'a>;

    fn title_updated(&self, _title: String) {}
}

//! CATALYST: Host model readiness and credential interaction contracts; implementations belong to Runtime.

use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;

// Compatibility export: existing TUI callers keep the same underlying type.
pub use codex_utils_sensitive_string::SensitiveString as SensitiveInput;

#[path = "credentials.rs"]
mod credentials;

pub use credentials::CredentialEntry;
pub use credentials::CredentialGroup;
pub use credentials::CredentialMutation;
pub use credentials::CredentialStatus;

pub type ModelRuntimeFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OnboardingProvider {
    pub id: String,
    pub display_name: String,
    pub credential: CredentialEntry,
    pub model_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelReadiness {
    Ready,
    MissingCredential(CredentialEntry),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelRuntimeError {
    pub code: String,
    pub message: String,
    pub action: Option<String>,
}

impl ModelRuntimeError {
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        action: Option<String>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            action,
        }
    }
}

impl fmt::Display for ModelRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)?;
        if let Some(action) = &self.action {
            write!(formatter, " {action}")?;
        }
        Ok(())
    }
}

impl Error for ModelRuntimeError {}

/// Host adapter for model readiness and credential interactions.
/// Implementations delegate product policy and storage to Runtime services; TUI owns event flow.
pub trait TuiModelRuntime: fmt::Debug + Send + Sync {
    fn list_onboarding_providers(
        &self,
    ) -> ModelRuntimeFuture<Result<Vec<OnboardingProvider>, ModelRuntimeError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn list_credentials(
        &self,
    ) -> ModelRuntimeFuture<Result<Vec<CredentialEntry>, ModelRuntimeError>>;

    fn model_readiness(
        &self,
        model_id: String,
    ) -> ModelRuntimeFuture<Result<ModelReadiness, ModelRuntimeError>>;

    fn store_credential(
        &self,
        credential_id: String,
        value: SensitiveInput,
    ) -> ModelRuntimeFuture<Result<CredentialMutation, ModelRuntimeError>>;

    fn revalidate_credential(
        &self,
        credential_id: String,
    ) -> ModelRuntimeFuture<Result<CredentialMutation, ModelRuntimeError>>;

    fn delete_credential(
        &self,
        credential_id: String,
    ) -> ModelRuntimeFuture<Result<(), ModelRuntimeError>>;
}

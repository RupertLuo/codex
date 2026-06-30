use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use zeroize::Zeroize;

pub type ModelRuntimeFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CredentialStatus {
    EnvironmentOverride,
    Verified,
    Unverified,
    Missing,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum CredentialGroup {
    #[default]
    ModelProviders,
    SearchServices,
}

impl CredentialGroup {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::ModelProviders => "Model Providers",
            Self::SearchServices => "Search Services",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialEntry {
    pub id: String,
    pub display_name: String,
    pub environment_variable: String,
    pub status: CredentialStatus,
    pub group: CredentialGroup,
}

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
pub enum CredentialMutation {
    Verified,
    SavedUnverified { warning: String },
}

pub struct SensitiveInput(String);

impl SensitiveInput {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SensitiveInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SensitiveInput([REDACTED])")
    }
}

impl Drop for SensitiveInput {
    fn drop(&mut self) {
        self.0.zeroize();
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_input_debug_is_redacted() {
        let input = SensitiveInput::new("provider-secret".to_string());

        assert_eq!(format!("{input:?}"), "SensitiveInput([REDACTED])");
        assert_eq!(input.expose_secret(), "provider-secret");
    }

    #[test]
    fn runtime_error_display_uses_the_user_facing_message() {
        let error = ModelRuntimeError::new(
            "credential_missing",
            "A credential is required for this model.",
            Some("Open /credentials to add one.".to_string()),
        );

        assert_eq!(
            error.to_string(),
            "A credential is required for this model. Open /credentials to add one."
        );
    }

    #[test]
    fn onboarding_provider_contract_keeps_credential_and_model_ids_together() {
        let provider = OnboardingProvider {
            id: "example".to_string(),
            display_name: "Example Provider".to_string(),
            credential: CredentialEntry {
                id: "example".to_string(),
                display_name: "Example Provider".to_string(),
                environment_variable: "EXAMPLE_PROVIDER_API_KEY".to_string(),
                status: CredentialStatus::Missing,
                group: CredentialGroup::ModelProviders,
            },
            model_ids: vec!["example/model-pro".to_string()],
        };

        assert_eq!(provider.credential.id, provider.id);
        assert_eq!(provider.model_ids, ["example/model-pro"]);
    }
}

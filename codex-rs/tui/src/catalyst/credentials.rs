//! CATALYST: credential presentation values exchanged with the host adapter.
//! Secrets and credential persistence belong to the foundation and Runtime layers.

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
pub enum CredentialMutation {
    Verified,
    SavedUnverified { warning: String },
}

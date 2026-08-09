//! Test-only helpers exposed for cross-crate integration tests.
//!
//! Production code should not depend on this module. Synchronization hooks and direct state
//! helpers are available only to unit tests or builds that explicitly enable `test-support`.

use std::path::PathBuf;
use std::sync::Arc;

use codex_exec_server::EnvironmentManager;
use codex_extension_api::LoadUserInstructionsFuture;
use codex_extension_api::LoadedUserInstructions;
use codex_extension_api::UserInstructionsProvider;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_model_provider::create_model_provider;
use codex_model_provider_info::ModelProviderInfo;
use codex_models_manager::bundled_models_response;
use codex_models_manager::collaboration_mode_presets;
use codex_models_manager::manager::SharedModelsManager;
use codex_models_manager::test_support::construct_model_info_offline_for_tests;
use codex_models_manager::test_support::get_model_offline_for_tests;
use codex_protocol::ThreadId;
use codex_protocol::config_types::CollaborationModeMask;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::protocol::SessionSource;
use once_cell::sync::Lazy;

#[cfg(any(test, feature = "test-support"))]
use crate::CodexThread;
use crate::ThreadManager;
#[cfg(any(test, feature = "test-support"))]
use crate::ThreadManagerRuntimeOptions;
use crate::config::Config;
use crate::responses_metadata::CodexResponsesMetadata;
use crate::responses_metadata::CodexResponsesRequestKind;
use crate::responses_metadata::subagent_header_value;
use crate::responses_metadata::subagent_metadata_kind;
use crate::thread_manager;
use crate::unified_exec;

/// Instance-scoped synchronization for compaction integration tests.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct CompactCommitTestHook(crate::compact::CompactCommitTestHook);

#[cfg(any(test, feature = "test-support"))]
impl CompactCommitTestHook {
    pub fn new() -> Self {
        Self(crate::compact::CompactCommitTestHook::new())
    }

    pub async fn wait_until_commit_paused(&self) {
        self.0.wait_until_commit_paused().await;
    }

    pub fn release_commit(&self) {
        self.0.release_commit();
    }

    pub fn panic_commit_once(&self) {
        self.0.panic_commit_once();
    }

    pub fn pause_item_started_once(&self) {
        self.0.pause_item_started_once();
    }

    pub async fn wait_until_item_started_paused(&self) {
        self.0.wait_until_item_started_paused().await;
    }

    pub async fn wait_until_item_started_cancelled(&self) {
        self.0.wait_until_item_started_cancelled().await;
    }

    pub async fn wait_until_parent_wait_dropped(&self) {
        self.0.wait_until_parent_wait_dropped().await;
    }

    pub async fn wait_until_commit_completed(&self) {
        self.0.wait_until_commit_completed().await;
    }

    pub fn pause_task_start_before_gate_once(&self) {
        self.0.pause_task_start_before_gate_once();
    }

    pub async fn wait_until_task_start_before_gate_paused(&self) {
        self.0.wait_until_task_start_before_gate_paused().await;
    }

    pub fn release_task_start_before_gate(&self) {
        self.0.release_task_start_before_gate();
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Default for CompactCommitTestHook {
    fn default() -> Self {
        Self::new()
    }
}

/// Install the compaction synchronization hook without adding a production runtime API.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn with_compact_commit_test_hook(
    options: ThreadManagerRuntimeOptions,
    hook: CompactCommitTestHook,
) -> ThreadManagerRuntimeOptions {
    options.with_compact_commit_test_hook_for_tests(hook.0)
}

/// Instance-scoped synchronization for realtime lifecycle integration tests.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct RealtimeStartTestHook(crate::realtime_conversation::RealtimeStartTestHook);

#[cfg(any(test, feature = "test-support"))]
impl RealtimeStartTestHook {
    pub fn new() -> Self {
        Self(crate::realtime_conversation::RealtimeStartTestHook::new())
    }

    pub fn pause_before_gate_once(&self) {
        self.0.pause_before_gate_once();
    }

    pub async fn wait_until_before_gate_paused(&self) {
        self.0.wait_until_before_gate_paused().await;
    }

    pub fn release_before_gate(&self) {
        self.0.release_before_gate();
    }

    pub fn pause_after_gate_once(&self) {
        self.0.pause_after_gate_once();
    }

    pub async fn wait_until_after_gate_paused(&self) {
        self.0.wait_until_after_gate_paused().await;
    }

    pub fn release_after_gate(&self) {
        self.0.release_after_gate();
    }

    pub fn pause_close_before_gate_once(&self) {
        self.0.pause_close_before_gate_once();
    }

    pub async fn wait_until_close_before_gate_paused(&self) {
        self.0.wait_until_close_before_gate_paused().await;
    }

    pub fn release_close_before_gate(&self) {
        self.0.release_close_before_gate();
    }

    pub fn pause_close_after_claim_once(&self) {
        self.0.pause_close_after_claim_once();
    }

    pub async fn wait_until_close_after_claim_paused(&self) {
        self.0.wait_until_close_after_claim_paused().await;
    }

    pub fn release_close_after_claim(&self) {
        self.0.release_close_after_claim();
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Default for RealtimeStartTestHook {
    fn default() -> Self {
        Self::new()
    }
}

/// Install the realtime synchronization hook without adding a production runtime API.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn with_realtime_start_test_hook(
    options: ThreadManagerRuntimeOptions,
    hook: RealtimeStartTestHook,
) -> ThreadManagerRuntimeOptions {
    options.with_realtime_start_test_hook_for_tests(hook.0)
}

/// Snapshot of the state direct injection APIs must leave untouched on rejection.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectMutationTestSnapshot {
    pub history_len: usize,
    pub has_active_turn: bool,
    pub has_pending_input: bool,
}

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub async fn direct_mutation_test_snapshot(thread: &CodexThread) -> DirectMutationTestSnapshot {
    let (history_len, has_active_turn, has_pending_input) =
        thread.direct_mutation_test_snapshot().await;
    DirectMutationTestSnapshot {
        history_len,
        has_active_turn,
        has_pending_input,
    }
}

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub async fn conversation_history_for_test(
    thread: &CodexThread,
) -> Vec<codex_protocol::models::ResponseItem> {
    thread.conversation_history_for_test().await
}

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub async fn inject_no_new_turn_for_test(
    thread: &CodexThread,
    items: Vec<codex_protocol::models::ResponseItem>,
) {
    thread.inject_no_new_turn_for_test(items).await;
}

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub async fn clear_reference_context_item_for_direct_mutation_test(thread: &CodexThread) {
    thread
        .clear_reference_context_item_for_direct_mutation_test()
        .await;
}

static TEST_MODEL_PRESETS: Lazy<Vec<ModelPreset>> = Lazy::new(|| {
    let mut response = bundled_models_response()
        .unwrap_or_else(|err| panic!("bundled models.json should parse: {err}"));
    response.models.sort_by_key(|model| model.priority);
    let mut presets: Vec<ModelPreset> = response.models.into_iter().map(Into::into).collect();
    ModelPreset::mark_default_by_picker_visibility(&mut presets);
    presets
});

/// Test-only provider that supplies no user instructions.
#[derive(Debug, Default)]
pub struct EmptyUserInstructionsProvider;

impl UserInstructionsProvider for EmptyUserInstructionsProvider {
    fn load_user_instructions(&self) -> LoadUserInstructionsFuture<'_> {
        Box::pin(async { LoadedUserInstructions::default() })
    }
}

pub fn set_thread_manager_test_mode(enabled: bool) {
    thread_manager::set_thread_manager_test_mode_for_tests(enabled);
}

pub fn set_deterministic_process_ids(enabled: bool) {
    unified_exec::set_deterministic_process_ids_for_tests(enabled);
}

pub fn auth_manager_from_auth(auth: CodexAuth) -> Arc<AuthManager> {
    AuthManager::from_auth_for_testing(auth)
}

pub fn auth_manager_from_auth_with_home(auth: CodexAuth, codex_home: PathBuf) -> Arc<AuthManager> {
    AuthManager::from_auth_for_testing_with_home(auth, codex_home)
}

pub fn thread_manager_with_models_provider(
    auth: CodexAuth,
    provider: ModelProviderInfo,
) -> ThreadManager {
    ThreadManager::with_models_provider_for_tests(auth, provider)
}

pub fn thread_manager_with_models_provider_and_home(
    auth: CodexAuth,
    provider: ModelProviderInfo,
    codex_home: PathBuf,
    environment_manager: Arc<EnvironmentManager>,
) -> ThreadManager {
    ThreadManager::with_models_provider_and_home_for_tests(
        auth,
        provider,
        codex_home,
        environment_manager,
    )
}

pub fn thread_manager_with_models_provider_home_and_state(
    auth: CodexAuth,
    provider: ModelProviderInfo,
    codex_home: PathBuf,
    environment_manager: Arc<EnvironmentManager>,
    state_db: Option<crate::StateDbHandle>,
) -> ThreadManager {
    ThreadManager::with_models_provider_home_and_state_for_tests(
        auth,
        provider,
        codex_home,
        environment_manager,
        state_db,
    )
}

pub async fn start_thread_with_user_shell_override(
    thread_manager: &ThreadManager,
    config: Config,
    user_shell_override: crate::shell::Shell,
    supports_openai_form_elicitation: bool,
) -> codex_protocol::error::Result<crate::NewThread> {
    thread_manager
        .start_thread_with_user_shell_override_for_tests(
            config,
            user_shell_override,
            supports_openai_form_elicitation,
        )
        .await
}

pub async fn resume_thread_from_rollout_with_user_shell_override(
    thread_manager: &ThreadManager,
    config: Config,
    rollout_path: PathBuf,
    auth_manager: Arc<AuthManager>,
    user_shell_override: crate::shell::Shell,
    supports_openai_form_elicitation: bool,
) -> codex_protocol::error::Result<crate::NewThread> {
    thread_manager
        .resume_thread_from_rollout_with_user_shell_override_for_tests(
            config,
            rollout_path,
            auth_manager,
            user_shell_override,
            supports_openai_form_elicitation,
        )
        .await
}

pub fn models_manager_with_provider(
    codex_home: PathBuf,
    auth_manager: Arc<AuthManager>,
    provider: ModelProviderInfo,
) -> SharedModelsManager {
    let provider = create_model_provider(provider, Some(auth_manager));
    provider.models_manager(codex_home, /*config_model_catalog*/ None)
}

pub fn get_model_offline(model: Option<&str>) -> String {
    get_model_offline_for_tests(model)
}

pub fn construct_model_info_offline(model: &str, config: &Config) -> ModelInfo {
    construct_model_info_offline_for_tests(model, &config.to_models_manager_config())
}

#[derive(Clone, Copy)]
pub enum TestCodexResponsesRequestKind {
    Turn,
    Prewarm,
    WebsocketConnection,
}

#[allow(clippy::too_many_arguments)]
pub fn responses_metadata(
    installation_id: &str,
    session_id: &str,
    thread_id: &str,
    turn_id: Option<&str>,
    window_id: String,
    session_source: &SessionSource,
    parent_thread_id: Option<ThreadId>,
    request_kind: TestCodexResponsesRequestKind,
) -> CodexResponsesMetadata {
    let request_kind = match request_kind {
        TestCodexResponsesRequestKind::Turn => Some(CodexResponsesRequestKind::Turn),
        TestCodexResponsesRequestKind::Prewarm => Some(CodexResponsesRequestKind::Prewarm),
        TestCodexResponsesRequestKind::WebsocketConnection => None,
    };
    CodexResponsesMetadata {
        turn_id: request_kind.and(turn_id.map(ToString::to_string)),
        request_kind,
        parent_thread_id,
        subagent_header: subagent_header_value(session_source),
        subagent_kind: request_kind.and_then(|_| subagent_metadata_kind(session_source)),
        ..CodexResponsesMetadata::new(
            installation_id.to_string(),
            session_id.to_string(),
            thread_id.to_string(),
            window_id,
        )
    }
}

pub fn all_model_presets() -> &'static Vec<ModelPreset> {
    &TEST_MODEL_PRESETS
}

pub fn builtin_collaboration_mode_presets() -> Vec<CollaborationModeMask> {
    collaboration_mode_presets::builtin_collaboration_mode_presets()
}

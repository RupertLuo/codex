use std::sync::Arc;

use codex_exec_server::EnvironmentManager;
use codex_exec_server::ExecServerRuntimePaths;
use codex_extension_api::UserInstructionsProvider;
use codex_login::AuthManager;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::SessionSource;
use codex_protocol::user_input::UserInput;
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::resolve_installation_id;
use crate::session::TurnInput;
use crate::session::session::Session;
use crate::session::turn::build_prompt;
use crate::session::turn::build_skills_and_plugins;
use crate::session::turn::built_tools;
use crate::state_db_bridge::StateDbHandle;
use crate::thread_manager::ThreadManager;
use crate::thread_manager::ThreadManagerRuntimeOptions;
use crate::thread_manager::thread_store_from_config;
use codex_extension_api::ExtensionRegistryBuilder;

/// Everything a single debug turn would send to the model: the `instructions`
/// field (base instructions) plus the model-visible `input` list.
#[doc(hidden)]
#[derive(Debug, Clone, serde::Serialize)]
pub struct PromptDebugSnapshot {
    pub instructions: String,
    pub input: Vec<ResponseItem>,
}

/// Build the model-visible `input` list for a single debug turn.
#[doc(hidden)]
pub async fn build_prompt_input(
    config: Config,
    input: Vec<UserInput>,
    state_db: Option<StateDbHandle>,
    user_instructions_provider: Arc<dyn UserInstructionsProvider>,
) -> CodexResult<Vec<ResponseItem>> {
    build_prompt_snapshot_with_runtime_options(
        config,
        input,
        state_db,
        user_instructions_provider,
        ThreadManagerRuntimeOptions::default(),
    )
    .await
    .map(|snapshot| snapshot.input)
}

/// Build both halves of a single debug turn's request, reproducing a host's
/// runtime options (required base instructions, model catalog, runtime
/// extensions) so the dump matches what that host would actually send.
#[doc(hidden)]
pub async fn build_prompt_snapshot_with_runtime_options(
    mut config: Config,
    input: Vec<UserInput>,
    state_db: Option<StateDbHandle>,
    user_instructions_provider: Arc<dyn UserInstructionsProvider>,
    runtime_options: ThreadManagerRuntimeOptions,
) -> CodexResult<PromptDebugSnapshot> {
    config.ephemeral = true;

    let auth_manager =
        AuthManager::shared_from_config(&config, /*enable_codex_api_key_env*/ false).await;

    let local_runtime_paths = ExecServerRuntimePaths::from_optional_paths(
        config.codex_self_exe.clone(),
        config.codex_linux_sandbox_exe.clone(),
    )?;

    let thread_store = thread_store_from_config(&config, state_db.clone());
    let installation_id = resolve_installation_id(&config.codex_home).await?;
    let mut extensions_builder = ExtensionRegistryBuilder::<Config>::new();
    for extension in runtime_options.runtime_extensions() {
        extension.install(&mut extensions_builder);
    }
    let thread_manager = ThreadManager::new_with_runtime_options(
        &config,
        Arc::clone(&auth_manager),
        SessionSource::Exec,
        Arc::new(
            EnvironmentManager::from_codex_home(
                config.codex_home.clone(),
                Some(local_runtime_paths),
            )
            .await
            .map_err(|err| CodexErr::Fatal(err.to_string()))?,
        ),
        Arc::new(extensions_builder.build()),
        user_instructions_provider,
        /*analytics_events_client*/ None,
        thread_store,
        crate::local_agent_graph_store_from_state_db(state_db.as_ref()),
        installation_id,
        /*attestation_provider*/ None,
        /*external_time_provider*/ None,
        runtime_options,
    );
    let thread = thread_manager.start_thread(config).await?;

    let output = build_prompt_input_from_session(&thread.thread.codex.session, input).await;
    let shutdown = thread.thread.shutdown_and_wait().await;
    let _removed = thread_manager.remove_thread(&thread.thread_id).await;

    shutdown?;
    output
}

pub(crate) async fn build_prompt_input_from_session(
    sess: &Arc<Session>,
    input: Vec<UserInput>,
) -> CodexResult<PromptDebugSnapshot> {
    let turn_context = sess.new_default_turn().await;
    // Prompt debugging builds a standalone request without entering run_turn.
    let step_context = sess.capture_step_context(Arc::clone(&turn_context)).await;
    sess.record_context_updates_and_set_reference_context_item(step_context.as_ref())
        .await;

    // Mirror run_turn's ordering: user message first, then the skill, plugin and
    // extension turn-input fragments that turn derives from it.
    let cancellation_token = CancellationToken::new();
    let turn_input = vec![TurnInput::UserInput {
        content: input.clone(),
        client_id: None,
    }];
    let injection_items = build_skills_and_plugins(
        sess,
        step_context.as_ref(),
        &turn_input,
        &cancellation_token,
    )
    .await
    .map(|(items, _connectors)| items)
    .unwrap_or_default();

    if !input.is_empty() {
        let response_item = sess.response_item_from_user_input(input);
        sess.record_conversation_items(turn_context.as_ref(), std::slice::from_ref(&response_item))
            .await;
    }
    for response_item in injection_items {
        sess.record_conversation_items(turn_context.as_ref(), std::slice::from_ref(&response_item))
            .await;
    }

    let prompt_input = sess
        .clone_history()
        .await
        .for_prompt(&turn_context.model_info.input_modalities);
    let router = built_tools(sess, step_context.as_ref(), &cancellation_token).await?;
    let base_instructions = sess.get_base_instructions().await;
    let prompt = build_prompt(
        prompt_input,
        router.as_ref(),
        turn_context.as_ref(),
        base_instructions,
    );

    Ok(PromptDebugSnapshot {
        instructions: prompt.base_instructions.text,
        input: prompt.input,
    })
}

use std::sync::Arc;
use std::sync::Weak;
use std::time::Duration;

use codex_analytics::AnalyticsEventsClient;
use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::ThreadGoal;
use codex_app_server_protocol::ThreadGoalUpdatedNotification;
use codex_app_server_protocol::ThreadQueueChangedNotification;
use codex_app_server_protocol::WarningNotification;
use codex_core::AgentSpawnerRuntimeExtensionFactory;
use codex_core::NativeAgentFuture;
use codex_core::NativeAgentRuntime;
use codex_core::NativeAgentSpawn;
use codex_core::NativeAgentSpawnRequest;
use codex_core::NativeAgentSpawner;
use codex_core::NewThread;
use codex_core::StartThreadOptions;
use codex_core::ThreadManager;
use codex_core::config::Config;
use codex_exec_server::EnvironmentManager;
use codex_extension_api::AgentSpawnFuture;
use codex_extension_api::AgentSpawner;
use codex_extension_api::ExtensionEventSink;
use codex_extension_api::ExtensionRegistry;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::ExtensionWarning;
use codex_extension_api::InternalSessionSpawnFuture;
use codex_extension_api::InternalSessionSpawner;
use codex_extension_api::RuntimeExtension;
use codex_goal_extension::GoalExtensionConfig;
use codex_goal_extension::GoalService;
use codex_http_client::HttpClientFactory;
use codex_login::AuthManager;
use codex_protocol::ThreadId;
use codex_protocol::error::CodexErr;
use codex_protocol::protocol::Event;
use codex_protocol::protocol::EventMsg;
use codex_queue_extension::QueuedItemService;
use codex_rollout::state_db::StateDbHandle;

use crate::outgoing_message::OutgoingMessageSender;
use crate::outgoing_message::ThreadScopedOutgoingMessageSender;
use crate::thread_state::ThreadListenerCommand;
use crate::thread_state::ThreadStateManager;

pub(crate) struct ThreadExtensionDependencies {
    pub(crate) event_sink: Arc<dyn ExtensionEventSink>,
    pub(crate) auth_manager: Arc<AuthManager>,
    pub(crate) state_db: Option<StateDbHandle>,
    pub(crate) analytics_events_client: AnalyticsEventsClient,
    pub(crate) thread_manager: Weak<ThreadManager>,
    pub(crate) goal_service: Arc<GoalService>,
    pub(crate) environment_manager: Arc<EnvironmentManager>,
    pub(crate) executor_skill_provider: Arc<dyn codex_skills_extension::SkillProvider>,
    pub(crate) git_attribution_base_url: String,
    pub(crate) http_client_factory: HttpClientFactory,
    /// Process-scoped queue shared by idle dispatch and app-server requests.
    pub(crate) queue_service: Option<Arc<QueuedItemService>>,
}

pub(crate) fn thread_extensions<S>(
    guardian_agent_spawner: S,
    native_agent_spawner: Arc<NativeAgentSpawner>,
    dependencies: ThreadExtensionDependencies,
    runtime_extensions: &[Arc<dyn RuntimeExtension<Config>>],
    agent_spawner_runtime_extension_factories: &[Arc<dyn AgentSpawnerRuntimeExtensionFactory>],
) -> Arc<ExtensionRegistry<Config>>
where
    S: AgentSpawner<StartThreadOptions, Spawned = NewThread, Error = CodexErr> + 'static,
{
    let ThreadExtensionDependencies {
        event_sink,
        auth_manager,
        state_db,
        analytics_events_client,
        thread_manager,
        goal_service,
        environment_manager,
        executor_skill_provider,
        git_attribution_base_url,
        http_client_factory,
        queue_service,
    } = dependencies;
    let mut builder = ExtensionRegistryBuilder::<Config>::with_event_sink(Arc::clone(&event_sink));
    if let Some(queue_service) = queue_service {
        codex_queue_extension::install(&mut builder, queue_service);
    }
    codex_history_notes_extension::install(&mut builder, auth_manager.clone());
    if let Some(state_db) = state_db {
        codex_goal_extension::install_with_backend(
            &mut builder,
            state_db,
            analytics_events_client,
            codex_otel::global(),
            thread_manager.clone(),
            goal_service,
            |config: &Config| GoalExtensionConfig {
                enabled: config.features.enabled(codex_features::Feature::Goals),
                max_goal_token_budget: config.max_goal_token_budget,
            },
        );
    }
    codex_git_attribution::install(
        &mut builder,
        auth_manager.clone(),
        git_attribution_base_url,
        http_client_factory,
    );
    codex_guardian_v2::install(
        &mut builder,
        guardian_agent_spawner,
        internal_session_spawner(thread_manager.clone()),
        auth_manager.clone(),
        thread_manager,
    );
    codex_memories_extension::install(&mut builder, codex_otel::global());
    codex_mcp_extension::install(&mut builder);
    codex_mcp_extension::install_executor_plugins(&mut builder, environment_manager);
    codex_web_search_extension::install(&mut builder, auth_manager.clone());
    codex_image_generation_extension::install(&mut builder, auth_manager, |config: &Config| {
        Some(config.codex_home.clone())
    });
    let skill_providers = codex_skills_extension::SkillProviders::new()
        .with_executor_provider(executor_skill_provider)
        .with_orchestrator_provider(Arc::new(
            codex_skills_extension::OrchestratorSkillProvider::new(),
        ))
        .with_host_provider(Arc::new(codex_skills_extension::HostSkillProvider::new()));
    codex_skills_extension::install_with_providers_and_metrics(
        &mut builder,
        skill_providers,
        codex_otel::global(),
        |config: &Config| codex_skills_extension::SkillsExtensionConfig {
            include_instructions: config.include_skill_instructions,
            max_context_tokens: config.skill_max_context_tokens,
            bundled_skills_enabled: config.bundled_skills_enabled(),
            orchestrator_skills_enabled: config.orchestrator_skills_enabled,
            shadow_selection_enabled: config
                .features
                .enabled(codex_features::Feature::SkillSearch),
        },
    );
    install_runtime_extensions(&mut builder, runtime_extensions);
    install_agent_spawner_runtime_extensions(
        &mut builder,
        agent_spawner_runtime_extension_factories,
        native_agent_spawner,
    );
    Arc::new(builder.build())
}

fn install_runtime_extensions(
    builder: &mut ExtensionRegistryBuilder<Config>,
    runtime_extensions: &[Arc<dyn RuntimeExtension<Config>>],
) {
    for extension in runtime_extensions {
        extension.install(builder);
    }
}

fn install_agent_spawner_runtime_extensions(
    builder: &mut ExtensionRegistryBuilder<Config>,
    factories: &[Arc<dyn AgentSpawnerRuntimeExtensionFactory>],
    spawner: Arc<NativeAgentSpawner>,
) {
    for factory in factories {
        factory.create(Arc::clone(&spawner)).install(builder);
    }
}

pub(crate) fn app_server_extension_event_sink(
    outgoing: Arc<OutgoingMessageSender>,
    thread_state_manager: ThreadStateManager,
) -> Arc<dyn ExtensionEventSink> {
    Arc::new(AppServerExtensionEventSink {
        outgoing,
        thread_state_manager,
    })
}

pub(crate) async fn send_thread_warning(
    outgoing: &Arc<OutgoingMessageSender>,
    thread_state_manager: &ThreadStateManager,
    thread_id: ThreadId,
    message: String,
) {
    let subscribed_connection_ids = thread_state_manager
        .subscribed_connection_ids(thread_id)
        .await;
    let thread_outgoing = ThreadScopedOutgoingMessageSender::new(
        Arc::clone(outgoing),
        subscribed_connection_ids,
        thread_id,
    );
    thread_outgoing
        .send_server_notification(ServerNotification::Warning(WarningNotification {
            thread_id: Some(thread_id.to_string()),
            message,
        }))
        .await;
}

struct AppServerExtensionEventSink {
    outgoing: Arc<OutgoingMessageSender>,
    thread_state_manager: ThreadStateManager,
}

const MAX_EXTENSION_WARNING_BYTES: usize = 256;
const EXTENSION_WARNING_SUBSCRIBER_TIMEOUT: Duration = Duration::from_secs(10);

impl ExtensionEventSink for AppServerExtensionEventSink {
    fn emit(&self, event: Event) {
        match event.msg {
            EventMsg::ThreadQueueChanged(queue_event) => {
                let thread_id = queue_event.thread_id;
                if let Some(listener_command_tx) = self
                    .thread_state_manager
                    .current_listener_command_tx(thread_id)
                {
                    let command = ThreadListenerCommand::EmitThreadQueueChanged;
                    if listener_command_tx.send(command).is_ok() {
                        return;
                    }
                    tracing::warn!(
                        "failed to enqueue extension queue update for {thread_id}: listener command channel is closed"
                    );
                }
                let outgoing = Arc::clone(&self.outgoing);
                let thread_state_manager = self.thread_state_manager.clone();
                tokio::spawn(async move {
                    let subscribed_connection_ids = thread_state_manager
                        .subscribed_connection_ids(thread_id)
                        .await;
                    let outgoing = ThreadScopedOutgoingMessageSender::new(
                        outgoing,
                        subscribed_connection_ids,
                        thread_id,
                    );
                    outgoing
                        .send_server_notification(ServerNotification::ThreadQueueChanged(
                            ThreadQueueChangedNotification {
                                thread_id: thread_id.to_string(),
                            },
                        ))
                        .await;
                });
            }
            EventMsg::ThreadGoalUpdated(thread_goal_event) => {
                let thread_id = thread_goal_event.thread_id;
                let turn_id = thread_goal_event.turn_id;
                let goal: ThreadGoal = thread_goal_event.goal.into();
                if let Some(listener_command_tx) = self
                    .thread_state_manager
                    .current_listener_command_tx(thread_id)
                {
                    let command = ThreadListenerCommand::EmitThreadGoalUpdated {
                        turn_id: turn_id.clone(),
                        goal: goal.clone(),
                    };
                    if listener_command_tx.send(command).is_ok() {
                        return;
                    }
                    tracing::warn!(
                        "failed to enqueue extension goal update for {thread_id}: listener command channel is closed"
                    );
                }
                let outgoing = Arc::clone(&self.outgoing);
                tokio::spawn(async move {
                    outgoing
                        .send_server_notification(ServerNotification::ThreadGoalUpdated(
                            ThreadGoalUpdatedNotification {
                                thread_id: thread_id.to_string(),
                                turn_id,
                                goal,
                            },
                        ))
                        .await;
                });
            }
            msg => {
                tracing::debug!(event_id = %event.id, ?msg, "dropping unsupported extension event");
            }
        }
    }

    fn emit_warning(&self, warning: ExtensionWarning) {
        let ExtensionWarning {
            thread_id,
            turn_id: _,
            message,
        } = warning;
        let Ok(thread_id) = ThreadId::from_string(&thread_id) else {
            tracing::warn!(
                %thread_id,
                "dropping extension warning with invalid thread id"
            );
            return;
        };
        let mut message = message;
        if message.len() > MAX_EXTENSION_WARNING_BYTES {
            let mut truncate_at = MAX_EXTENSION_WARNING_BYTES;
            while !message.is_char_boundary(truncate_at) {
                truncate_at -= 1;
            }
            message.truncate(truncate_at);
        }
        if let Some(listener_command_tx) = self
            .thread_state_manager
            .current_listener_command_tx(thread_id)
        {
            let command = ThreadListenerCommand::EmitWarning {
                message: message.clone(),
            };
            if listener_command_tx.send(command).is_ok() {
                return;
            }
            tracing::warn!(
                "failed to enqueue extension warning for {thread_id}: listener command channel is closed"
            );
        }
        let outgoing = Arc::clone(&self.outgoing);
        let thread_state_manager = self.thread_state_manager.clone();
        tokio::spawn(async move {
            if tokio::time::timeout(
                EXTENSION_WARNING_SUBSCRIBER_TIMEOUT,
                thread_state_manager.wait_for_thread_subscriber(thread_id),
            )
            .await
            .is_err()
            {
                tracing::warn!(
                    %thread_id,
                    timeout_secs = EXTENSION_WARNING_SUBSCRIBER_TIMEOUT.as_secs(),
                    "dropping extension warning after waiting for a thread subscriber"
                );
                return;
            }
            send_thread_warning(&outgoing, &thread_state_manager, thread_id, message).await;
        });
    }
}

pub(crate) fn guardian_agent_spawner(
    thread_manager: Weak<ThreadManager>,
) -> impl AgentSpawner<StartThreadOptions, Spawned = NewThread, Error = CodexErr> {
    move |forked_from_thread_id: ThreadId,
          options: StartThreadOptions|
          -> AgentSpawnFuture<'static, NewThread, CodexErr> {
        let thread_manager = thread_manager.clone();
        Box::pin(async move {
            let thread_manager = thread_manager.upgrade().ok_or_else(|| {
                CodexErr::UnsupportedOperation("thread manager dropped".to_string())
            })?;
            thread_manager
                .spawn_subagent(forked_from_thread_id, options)
                .await
        })
    }
}

fn internal_session_spawner(
    thread_manager: Weak<ThreadManager>,
) -> impl InternalSessionSpawner<StartThreadOptions, Spawned = NewThread, Error = CodexErr> {
    move |parent_thread_id: ThreadId,
          options: StartThreadOptions|
          -> InternalSessionSpawnFuture<'static, NewThread, CodexErr> {
        let thread_manager = thread_manager.clone();
        Box::pin(async move {
            let thread_manager = thread_manager.upgrade().ok_or_else(|| {
                CodexErr::UnsupportedOperation("thread manager dropped".to_string())
            })?;
            thread_manager
                .spawn_internal_session(parent_thread_id, options)
                .await
        })
    }
}

pub(crate) fn native_agent_spawner(thread_manager: Weak<ThreadManager>) -> impl NativeAgentRuntime {
    AppServerNativeAgentRuntime { thread_manager }
}

struct AppServerNativeAgentRuntime {
    thread_manager: Weak<ThreadManager>,
}

impl AgentSpawner<NativeAgentSpawnRequest> for AppServerNativeAgentRuntime {
    type Spawned = NativeAgentSpawn;
    type Error = CodexErr;

    fn spawn_subagent<'a>(
        &'a self,
        forked_from_thread_id: ThreadId,
        request: NativeAgentSpawnRequest,
    ) -> AgentSpawnFuture<'a, NativeAgentSpawn, CodexErr> {
        let thread_manager = self.thread_manager.clone();
        Box::pin(async move {
            let thread_manager = upgrade_thread_manager(thread_manager)?;
            thread_manager
                .spawn_native_agent(forked_from_thread_id, request)
                .await
        })
    }
}

impl NativeAgentRuntime for AppServerNativeAgentRuntime {
    fn agent_status<'a>(
        &'a self,
        thread_id: ThreadId,
    ) -> NativeAgentFuture<'a, codex_protocol::protocol::AgentStatus> {
        let thread_manager = self.thread_manager.clone();
        Box::pin(async move {
            let thread_manager = upgrade_thread_manager(thread_manager)?;
            Ok(thread_manager.native_agent_status(thread_id).await)
        })
    }

    fn interrupt_agent<'a>(
        &'a self,
        parent_thread_id: ThreadId,
        child_thread_id: ThreadId,
    ) -> NativeAgentFuture<'a, ()> {
        let thread_manager = self.thread_manager.clone();
        Box::pin(async move {
            let thread_manager = upgrade_thread_manager(thread_manager)?;
            thread_manager
                .interrupt_native_agent(parent_thread_id, child_thread_id)
                .await
        })
    }
}

fn upgrade_thread_manager(
    thread_manager: Weak<ThreadManager>,
) -> Result<Arc<ThreadManager>, CodexErr> {
    thread_manager
        .upgrade()
        .ok_or_else(|| CodexErr::UnsupportedOperation("thread manager dropped".to_string()))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use codex_extension_api::ExtensionData;
    use codex_extension_api::RuntimeExtension;
    use codex_extension_api::ToolCall;
    use codex_extension_api::ToolContributor;
    use codex_extension_api::ToolExecutor;
    use codex_protocol::protocol::ThreadGoal as CoreThreadGoal;
    use codex_protocol::protocol::ThreadGoalStatus;
    use codex_protocol::protocol::ThreadGoalUpdatedEvent;
    use pretty_assertions::assert_eq;
    use tokio::sync::mpsc;
    use tokio::time::timeout;

    use crate::outgoing_message::ConnectionId;
    use crate::outgoing_message::OutgoingEnvelope;
    use crate::outgoing_message::OutgoingMessage;
    use crate::thread_state::ConnectionCapabilities;

    use super::*;

    #[derive(Debug)]
    struct ProbeRuntimeExtension;

    struct ProbeToolContributor;

    #[derive(Debug)]
    struct ProbeAgentSpawnerExtensionFactory;

    struct RejectingNativeAgentRuntime;

    impl AgentSpawner<NativeAgentSpawnRequest> for RejectingNativeAgentRuntime {
        type Spawned = NativeAgentSpawn;
        type Error = CodexErr;

        fn spawn_subagent<'a>(
            &'a self,
            _thread_id: ThreadId,
            _request: NativeAgentSpawnRequest,
        ) -> AgentSpawnFuture<'a, NativeAgentSpawn, CodexErr> {
            Box::pin(async {
                Err(CodexErr::UnsupportedOperation(
                    "test native agent runtime".to_string(),
                ))
            })
        }
    }

    impl NativeAgentRuntime for RejectingNativeAgentRuntime {
        fn agent_status<'a>(
            &'a self,
            _thread_id: ThreadId,
        ) -> NativeAgentFuture<'a, codex_protocol::protocol::AgentStatus> {
            Box::pin(async {
                Err(CodexErr::UnsupportedOperation(
                    "test native agent runtime".to_string(),
                ))
            })
        }

        fn interrupt_agent<'a>(
            &'a self,
            _parent_thread_id: ThreadId,
            _child_thread_id: ThreadId,
        ) -> NativeAgentFuture<'a, ()> {
            Box::pin(async {
                Err(CodexErr::UnsupportedOperation(
                    "test native agent runtime".to_string(),
                ))
            })
        }
    }

    impl AgentSpawnerRuntimeExtensionFactory for ProbeAgentSpawnerExtensionFactory {
        fn create(&self, _spawner: Arc<NativeAgentSpawner>) -> Arc<dyn RuntimeExtension<Config>> {
            Arc::new(ProbeRuntimeExtension)
        }
    }

    impl ToolContributor for ProbeToolContributor {
        fn tools(
            &self,
            _session_store: &ExtensionData,
            _thread_store: &ExtensionData,
        ) -> Vec<Arc<dyn for<'call> ToolExecutor<ToolCall<'call>>>> {
            Vec::new()
        }
    }

    impl RuntimeExtension<Config> for ProbeRuntimeExtension {
        fn install(&self, builder: &mut ExtensionRegistryBuilder<Config>) {
            builder.tool_contributor(Arc::new(ProbeToolContributor));
        }
    }

    #[test]
    fn runtime_extensions_install_tool_contributors() {
        let mut empty_builder = ExtensionRegistryBuilder::<Config>::new();
        install_runtime_extensions(&mut empty_builder, &[]);
        assert!(empty_builder.build().tool_contributors().is_empty());

        let mut builder = ExtensionRegistryBuilder::<Config>::new();
        let extensions: Vec<Arc<dyn RuntimeExtension<Config>>> =
            vec![Arc::new(ProbeRuntimeExtension)];
        install_runtime_extensions(&mut builder, &extensions);

        assert_eq!(builder.build().tool_contributors().len(), 1);
    }

    #[test]
    fn agent_spawner_factories_install_runtime_extensions() {
        let factory: Arc<dyn AgentSpawnerRuntimeExtensionFactory> =
            Arc::new(ProbeAgentSpawnerExtensionFactory);
        let spawner: Arc<NativeAgentSpawner> = Arc::new(RejectingNativeAgentRuntime);
        let mut builder = ExtensionRegistryBuilder::<Config>::new();

        install_agent_spawner_runtime_extensions(&mut builder, &[factory], spawner);

        assert_eq!(builder.build().tool_contributors().len(), 1);
    }

    #[tokio::test]
    async fn native_agent_runtime_exposes_status_and_interrupt_capabilities() {
        let runtime = native_agent_spawner(Weak::<ThreadManager>::new());
        let thread_id = ThreadId::new();

        let status_error = runtime
            .agent_status(thread_id)
            .await
            .expect_err("a dropped manager cannot report native agent status");
        let interrupt_error = runtime
            .interrupt_agent(thread_id, ThreadId::new())
            .await
            .expect_err("a dropped manager cannot interrupt a native agent");

        assert!(matches!(status_error, CodexErr::UnsupportedOperation(_)));
        assert!(matches!(interrupt_error, CodexErr::UnsupportedOperation(_)));
    }

    #[tokio::test]
    async fn app_server_event_sink_uses_listener_fifo_for_goal_updates_warnings_and_clears() {
        let (outgoing_tx, _outgoing_rx) = mpsc::channel(4);
        let outgoing = Arc::new(OutgoingMessageSender::new(
            outgoing_tx,
            AnalyticsEventsClient::disabled(),
        ));
        let thread_state_manager = ThreadStateManager::new();
        let thread_id = ThreadId::default();
        let (listener_command_tx, mut listener_command_rx) = mpsc::unbounded_channel();
        thread_state_manager.register_listener_command_tx(thread_id, listener_command_tx.clone());
        let sink = app_server_extension_event_sink(outgoing, thread_state_manager);

        sink.emit(thread_goal_updated_event(thread_id, "turn-1"));
        sink.emit_warning(ExtensionWarning {
            thread_id: thread_id.to_string(),
            turn_id: Some("turn-warning".to_string()),
            message: "catalog was shortened".to_string(),
        });
        sink.emit(thread_goal_updated_event(thread_id, "turn-2"));
        listener_command_tx
            .send(ThreadListenerCommand::EmitThreadGoalCleared)
            .expect("listener command channel should be open");

        let mut observed = Vec::new();
        for _ in 0..4 {
            let command = timeout(Duration::from_secs(1), listener_command_rx.recv())
                .await
                .expect("timed out waiting for listener command")
                .expect("listener command channel closed unexpectedly");
            match command {
                ThreadListenerCommand::EmitThreadGoalUpdated { turn_id, .. } => {
                    observed.push(turn_id.expect("extension goal updates should include turn ids"));
                }
                ThreadListenerCommand::EmitWarning { message } => observed.push(message),
                ThreadListenerCommand::EmitThreadGoalCleared => {
                    observed.push("cleared".to_string())
                }
                _ => panic!("unexpected listener command"),
            }
        }

        assert_eq!(
            vec![
                "turn-1".to_string(),
                "catalog was shortened".to_string(),
                "turn-2".to_string(),
                "cleared".to_string()
            ],
            observed
        );
    }

    #[tokio::test]
    async fn app_server_event_sink_truncates_warning_before_listener_enqueue() {
        let (outgoing_tx, _outgoing_rx) = mpsc::channel(4);
        let outgoing = Arc::new(OutgoingMessageSender::new(
            outgoing_tx,
            AnalyticsEventsClient::disabled(),
        ));
        let thread_state_manager = ThreadStateManager::new();
        let thread_id = ThreadId::default();
        let (listener_command_tx, mut listener_command_rx) = mpsc::unbounded_channel();
        thread_state_manager.register_listener_command_tx(thread_id, listener_command_tx);
        let sink = app_server_extension_event_sink(outgoing, thread_state_manager);

        sink.emit_warning(ExtensionWarning {
            thread_id: thread_id.to_string(),
            turn_id: Some("turn-warning".to_string()),
            message: "🙂".repeat(65),
        });

        let command = timeout(Duration::from_secs(1), listener_command_rx.recv())
            .await
            .expect("timed out waiting for listener command")
            .expect("listener command channel closed unexpectedly");
        let ThreadListenerCommand::EmitWarning { message } = command else {
            panic!("expected warning listener command");
        };
        assert_eq!(message, "🙂".repeat(64));
    }

    #[tokio::test]
    async fn app_server_event_sink_targets_subscriber_without_listener() {
        let (outgoing_tx, mut outgoing_rx) = mpsc::channel(4);
        let outgoing = Arc::new(OutgoingMessageSender::new(
            outgoing_tx,
            AnalyticsEventsClient::disabled(),
        ));
        let thread_id = ThreadId::new();
        let subscribed_connection = ConnectionId(1);
        let unrelated_connection = ConnectionId(2);
        let thread_state_manager = ThreadStateManager::new();
        for connection_id in [subscribed_connection, unrelated_connection] {
            thread_state_manager
                .connection_initialized(connection_id, ConnectionCapabilities::default())
                .await;
        }
        thread_state_manager
            .try_ensure_connection_subscribed(
                thread_id,
                subscribed_connection,
                /*experimental_raw_events*/ false,
            )
            .await
            .expect("connection should be subscribed");
        let sink = app_server_extension_event_sink(outgoing, thread_state_manager);

        sink.emit_warning(ExtensionWarning {
            thread_id: thread_id.to_string(),
            turn_id: Some("turn-1".to_string()),
            message: "catalog was shortened".to_string(),
        });

        let envelope = timeout(Duration::from_secs(1), outgoing_rx.recv())
            .await
            .expect("timed out waiting for warning notification")
            .expect("outgoing channel closed unexpectedly");
        let OutgoingEnvelope::ToConnection {
            connection_id,
            message,
            write_complete_tx: _,
        } = envelope
        else {
            panic!("expected connection-targeted warning notification");
        };
        assert_eq!(connection_id, subscribed_connection);
        let OutgoingMessage::AppServerNotification(envelope) = message else {
            panic!("expected app-server warning notification");
        };
        let ServerNotification::Warning(notification) = envelope.notification else {
            panic!("expected warning notification");
        };
        assert_eq!(
            notification,
            WarningNotification {
                thread_id: Some(thread_id.to_string()),
                message: "catalog was shortened".to_string(),
            }
        );
        assert!(outgoing_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn app_server_event_sink_waits_for_subscriber_without_listener() {
        let (outgoing_tx, mut outgoing_rx) = mpsc::channel(4);
        let outgoing = Arc::new(OutgoingMessageSender::new(
            outgoing_tx,
            AnalyticsEventsClient::disabled(),
        ));
        let thread_id = ThreadId::new();
        let subscribed_connection = ConnectionId(1);
        let thread_state_manager = ThreadStateManager::new();
        thread_state_manager
            .connection_initialized(subscribed_connection, ConnectionCapabilities::default())
            .await;
        let sink = app_server_extension_event_sink(outgoing, thread_state_manager.clone());

        sink.emit_warning(ExtensionWarning {
            thread_id: thread_id.to_string(),
            turn_id: Some("turn-1".to_string()),
            message: "catalog was shortened".to_string(),
        });
        tokio::task::yield_now().await;
        thread_state_manager
            .try_ensure_connection_subscribed(
                thread_id,
                subscribed_connection,
                /*experimental_raw_events*/ false,
            )
            .await
            .expect("connection should be subscribed");

        let envelope = timeout(Duration::from_secs(1), outgoing_rx.recv())
            .await
            .expect("timed out waiting for warning notification")
            .expect("outgoing channel closed unexpectedly");
        let OutgoingEnvelope::ToConnection {
            connection_id,
            message,
            write_complete_tx: _,
        } = envelope
        else {
            panic!("expected connection-targeted warning notification");
        };
        assert_eq!(connection_id, subscribed_connection);
        let OutgoingMessage::AppServerNotification(envelope) = message else {
            panic!("expected app-server warning notification");
        };
        let ServerNotification::Warning(notification) = envelope.notification else {
            panic!("expected warning notification");
        };
        assert_eq!(
            notification,
            WarningNotification {
                thread_id: Some(thread_id.to_string()),
                message: "catalog was shortened".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn app_server_event_sink_targets_subscriber_after_listener_closes() {
        let (outgoing_tx, mut outgoing_rx) = mpsc::channel(4);
        let outgoing = Arc::new(OutgoingMessageSender::new(
            outgoing_tx,
            AnalyticsEventsClient::disabled(),
        ));
        let thread_id = ThreadId::new();
        let subscribed_connection = ConnectionId(1);
        let thread_state_manager = ThreadStateManager::new();
        thread_state_manager
            .connection_initialized(subscribed_connection, ConnectionCapabilities::default())
            .await;
        thread_state_manager
            .try_ensure_connection_subscribed(
                thread_id,
                subscribed_connection,
                /*experimental_raw_events*/ false,
            )
            .await
            .expect("connection should be subscribed");
        let (listener_command_tx, listener_command_rx) = mpsc::unbounded_channel();
        drop(listener_command_rx);
        thread_state_manager.register_listener_command_tx(thread_id, listener_command_tx);
        let sink = app_server_extension_event_sink(outgoing, thread_state_manager);

        sink.emit_warning(ExtensionWarning {
            thread_id: thread_id.to_string(),
            turn_id: Some("turn-1".to_string()),
            message: "catalog was shortened".to_string(),
        });

        let envelope = timeout(Duration::from_secs(1), outgoing_rx.recv())
            .await
            .expect("timed out waiting for warning notification")
            .expect("outgoing channel closed unexpectedly");
        let OutgoingEnvelope::ToConnection {
            connection_id,
            message,
            write_complete_tx: _,
        } = envelope
        else {
            panic!("expected connection-targeted warning notification");
        };
        assert_eq!(connection_id, subscribed_connection);
        let OutgoingMessage::AppServerNotification(envelope) = message else {
            panic!("expected app-server warning notification");
        };
        let ServerNotification::Warning(notification) = envelope.notification else {
            panic!("expected warning notification");
        };
        assert_eq!(
            notification,
            WarningNotification {
                thread_id: Some(thread_id.to_string()),
                message: "catalog was shortened".to_string(),
            }
        );
        assert!(outgoing_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn app_server_event_sink_drops_warning_with_invalid_thread_id() {
        let (outgoing_tx, mut outgoing_rx) = mpsc::channel(4);
        let outgoing = Arc::new(OutgoingMessageSender::new(
            outgoing_tx,
            AnalyticsEventsClient::disabled(),
        ));
        let sink = app_server_extension_event_sink(outgoing, ThreadStateManager::new());

        sink.emit_warning(ExtensionWarning {
            thread_id: "not-a-thread-id".to_string(),
            turn_id: Some("turn-1".to_string()),
            message: "catalog was shortened".to_string(),
        });

        assert!(outgoing_rx.try_recv().is_err());
    }

    fn thread_goal_updated_event(thread_id: ThreadId, turn_id: &str) -> Event {
        Event {
            id: turn_id.to_string(),
            msg: EventMsg::ThreadGoalUpdated(ThreadGoalUpdatedEvent {
                thread_id,
                turn_id: Some(turn_id.to_string()),
                goal: CoreThreadGoal {
                    thread_id,
                    objective: "wire extension events".to_string(),
                    status: ThreadGoalStatus::Active,
                    token_budget: Some(123),
                    tokens_used: 45,
                    time_used_seconds: 6,
                    created_at: 7,
                    updated_at: 8,
                },
            }),
        }
    }
}

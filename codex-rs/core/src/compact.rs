use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::Prompt;
use crate::client::HttpIncrementalSession;
use crate::client::ModelClientSession;
use crate::client_common::ResponseEvent;
use crate::context::world_state::WorldState;
use crate::hook_runtime::PostCompactHookOutcome;
use crate::hook_runtime::PreCompactHookOutcome;
use crate::hook_runtime::run_post_compact_hooks;
use crate::hook_runtime::run_pre_compact_hooks;
use crate::responses_metadata::CodexResponsesMetadata;
use crate::responses_metadata::CodexResponsesRequestKind;
use crate::responses_metadata::CompactionTurnMetadata;
#[cfg(test)]
use crate::session::PreviousTurnSettings;
use crate::session::session::Session;
use crate::session::turn::get_last_assistant_message_from_turn;
use crate::session::turn_context::TurnContext;
use crate::state::AutoCompactWindowIds;
use crate::util::backoff;
use codex_analytics::CodexCompactionEvent;
use codex_analytics::CompactionImplementation;
use codex_analytics::CompactionPhase;
use codex_analytics::CompactionReason;
use codex_analytics::CompactionStatus;
use codex_analytics::CompactionStrategy;
use codex_analytics::CompactionTrigger;
use codex_analytics::now_unix_seconds;
use codex_async_utils::OrCancelExt;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::items::ContextCompactionItem;
use codex_protocol::items::TurnItem;
use codex_protocol::models::ContentItem;
use codex_protocol::models::InternalChatMessageMetadataPassthrough;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::CompactedItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::RateLimitSnapshot;
use codex_protocol::protocol::TokenUsage;
use codex_protocol::protocol::TurnStartedEvent;
use codex_protocol::protocol::WarningEvent;
use codex_protocol::user_input::UserInput;
use codex_rollout_trace::InferenceTraceContext;
use codex_utils_output_truncation::TruncationPolicy;
use codex_utils_output_truncation::approx_token_count;
use codex_utils_output_truncation::truncate_text;
use futures::prelude::*;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::error;

use codex_model_provider_info::ModelProviderInfo;

pub use codex_prompts::SUMMARIZATION_PROMPT;
pub use codex_prompts::SUMMARY_PREFIX;
const COMPACT_USER_MESSAGE_MAX_TOKENS: usize = 20_000;

/// Move-only ownership of the incremental HTTP baseline across the shielded commit task.
///
/// The active turn cannot clone this state because it contains a oneshot receiver. A shared,
/// take-once slot lets either the child restore it on a normal commit error or the parent restore
/// it after a [`tokio::task::JoinError`], while success consumes it exactly once.
#[derive(Clone, Debug, Default)]
pub(crate) struct CompactCommitBaseline {
    inner: Arc<std::sync::Mutex<Option<HttpIncrementalSession>>>,
}

impl CompactCommitBaseline {
    fn take_from_active(active_client_session: Option<&mut ModelClientSession>) -> Self {
        Self {
            inner: Arc::new(std::sync::Mutex::new(
                active_client_session.map(ModelClientSession::take_incremental_baseline),
            )),
        }
    }

    pub(crate) fn take(&self) -> Option<HttpIncrementalSession> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    async fn restore_to_session(&self, sess: &Session) {
        if let Some(baseline) = self.take() {
            sess.store_http_incremental_baseline(baseline).await;
        }
    }
}

/// Instance-scoped synchronization used by compaction commit integration tests.
#[derive(Clone, Debug)]
pub(crate) struct CompactCommitTestHook {
    inner: Arc<CompactCommitTestHookInner>,
}

#[derive(Debug)]
struct CompactCommitTestHookInner {
    commit_paused: Semaphore,
    release_commit: Semaphore,
    parent_wait_dropped: Semaphore,
    commit_completed: Semaphore,
    panic_commit: AtomicBool,
    pause_item_started: AtomicBool,
    item_started_paused: Semaphore,
    release_item_started: Semaphore,
    item_started_cancelled: Semaphore,
    pause_task_start_before_gate: AtomicBool,
    task_start_before_gate_paused: Semaphore,
    release_task_start_before_gate: Semaphore,
}

impl CompactCommitTestHook {
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(CompactCommitTestHookInner {
                commit_paused: Semaphore::new(0),
                release_commit: Semaphore::new(0),
                parent_wait_dropped: Semaphore::new(0),
                commit_completed: Semaphore::new(0),
                panic_commit: AtomicBool::new(false),
                pause_item_started: AtomicBool::new(false),
                item_started_paused: Semaphore::new(0),
                release_item_started: Semaphore::new(0),
                item_started_cancelled: Semaphore::new(0),
                pause_task_start_before_gate: AtomicBool::new(false),
                task_start_before_gate_paused: Semaphore::new(0),
                release_task_start_before_gate: Semaphore::new(0),
            }),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) async fn wait_until_commit_paused(&self) {
        let Ok(permit) = self.inner.commit_paused.acquire().await else {
            return;
        };
        permit.forget();
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn release_commit(&self) {
        self.inner.release_commit.add_permits(1);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn panic_commit_once(&self) {
        self.inner.panic_commit.store(true, Ordering::SeqCst);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn pause_item_started_once(&self) {
        self.inner.pause_item_started.store(true, Ordering::SeqCst);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) async fn wait_until_item_started_paused(&self) {
        let Ok(permit) = self.inner.item_started_paused.acquire().await else {
            return;
        };
        permit.forget();
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) async fn wait_until_item_started_cancelled(&self) {
        let Ok(permit) = self.inner.item_started_cancelled.acquire().await else {
            return;
        };
        permit.forget();
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) async fn wait_until_parent_wait_dropped(&self) {
        let Ok(permit) = self.inner.parent_wait_dropped.acquire().await else {
            return;
        };
        permit.forget();
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) async fn wait_until_commit_completed(&self) {
        let Ok(permit) = self.inner.commit_completed.acquire().await else {
            return;
        };
        permit.forget();
    }

    pub(crate) async fn pause_commit(&self) {
        self.inner.commit_paused.add_permits(1);
        let Ok(permit) = self.inner.release_commit.acquire().await else {
            return;
        };
        permit.forget();
    }

    pub(crate) fn parent_wait_guard(&self) -> CompactCommitParentWaitGuard {
        CompactCommitParentWaitGuard {
            hook: self.clone(),
            armed: true,
        }
    }

    pub(crate) fn should_panic_commit(&self) -> bool {
        self.inner.panic_commit.swap(false, Ordering::SeqCst)
    }

    pub(crate) fn notify_commit_completed(&self) {
        self.inner.commit_completed.add_permits(1);
    }

    pub(crate) fn take_item_started_pause(&self) -> bool {
        self.inner.pause_item_started.swap(false, Ordering::SeqCst)
    }

    pub(crate) async fn pause_item_started(&self) {
        self.inner.item_started_paused.add_permits(1);
        let Ok(permit) = self.inner.release_item_started.acquire().await else {
            return;
        };
        permit.forget();
    }

    pub(crate) fn notify_item_started_cancelled(&self) {
        self.inner.item_started_cancelled.add_permits(1);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn pause_task_start_before_gate_once(&self) {
        self.inner
            .pause_task_start_before_gate
            .store(true, Ordering::SeqCst);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) async fn wait_until_task_start_before_gate_paused(&self) {
        let Ok(permit) = self.inner.task_start_before_gate_paused.acquire().await else {
            return;
        };
        permit.forget();
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn release_task_start_before_gate(&self) {
        self.inner.release_task_start_before_gate.add_permits(1);
    }

    pub(crate) async fn pause_task_start_before_gate_if_requested(&self) {
        if !self
            .inner
            .pause_task_start_before_gate
            .swap(false, Ordering::SeqCst)
        {
            return;
        }
        self.inner.task_start_before_gate_paused.add_permits(1);
        let Ok(permit) = self.inner.release_task_start_before_gate.acquire().await else {
            return;
        };
        permit.forget();
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Default for CompactCommitTestHook {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) struct CompactCommitParentWaitGuard {
    hook: CompactCommitTestHook,
    armed: bool,
}

impl CompactCommitParentWaitGuard {
    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CompactCommitParentWaitGuard {
    fn drop(&mut self) {
        if self.armed {
            self.hook.inner.parent_wait_dropped.add_permits(1);
        }
    }
}

/// Controls whether compaction replacement history must include initial context.
///
/// Pre-turn/manual compaction variants use `DoNotInject`: they replace history with a summary and
/// clear `reference_context_item`, so the next regular turn will fully reinject initial context
/// after compaction.
///
/// Mid-turn compaction must use `BeforeLastUserMessage` because the model is trained to see the
/// compaction summary as the last item in history after mid-turn compaction; we therefore inject
/// initial context into the replacement history just above the last real user message.
#[derive(Debug)]
pub(crate) enum InitialContextInjection {
    BeforeLastUserMessage(Arc<WorldState>),
    DoNotInject,
}

pub(crate) async fn build_compaction_initial_context(
    sess: &Session,
    turn_context: &TurnContext,
    initial_context_injection: &InitialContextInjection,
    window_ids: Option<AutoCompactWindowIds>,
) -> (Vec<ResponseItem>, Option<Arc<WorldState>>) {
    // Return the rendered state with its items so history and its baseline stay identical.
    match initial_context_injection {
        InitialContextInjection::BeforeLastUserMessage(world_state) => {
            let items = match window_ids {
                Some(window_ids) => {
                    sess.build_initial_context_with_world_state_for_window(
                        turn_context,
                        world_state.as_ref(),
                        window_ids,
                    )
                    .await
                }
                None => {
                    sess.build_initial_context_with_world_state(turn_context, world_state.as_ref())
                        .await
                }
            };
            (items, Some(Arc::clone(world_state)))
        }
        InitialContextInjection::DoNotInject => (Vec::new(), None),
    }
}

pub(crate) fn should_use_remote_compact_task(provider: &ModelProviderInfo) -> bool {
    provider.supports_remote_compaction()
}

async fn resolve_compact_turn_context(
    sess: &Session,
    turn_context: &Arc<TurnContext>,
) -> Arc<TurnContext> {
    let Some(compact_model) = turn_context.config.compact_model.as_deref() else {
        return Arc::clone(turn_context);
    };
    Arc::new(
        turn_context
            .with_model(compact_model.to_string(), &sess.services.models_manager)
            .await,
    )
}

pub(crate) async fn run_inline_auto_compact_task(
    sess: Arc<Session>,
    turn_context: Arc<TurnContext>,
    active_client_session: &mut ModelClientSession,
    cancellation_token: CancellationToken,
    initial_context_injection: InitialContextInjection,
    reason: CompactionReason,
    phase: CompactionPhase,
) -> CodexResult<()> {
    let prompt = turn_context
        .config
        .compact_prompt
        .as_deref()
        .unwrap_or(SUMMARIZATION_PROMPT)
        .to_string();
    let input = vec![UserInput::Text {
        text: prompt,
        // Compaction prompt is synthesized; no UI element ranges to preserve.
        text_elements: Vec::new(),
    }];

    run_compact_task_inner(
        sess,
        turn_context,
        input,
        Some(active_client_session),
        cancellation_token,
        initial_context_injection,
        CompactionTrigger::Auto,
        reason,
        phase,
    )
    .await?;
    Ok(())
}

pub(crate) async fn run_compact_task(
    sess: Arc<Session>,
    turn_context: Arc<TurnContext>,
    input: Vec<UserInput>,
    cancellation_token: CancellationToken,
) -> CodexResult<()> {
    let started_at = turn_context
        .turn_timing_state
        .started_at_unix_secs()
        .or_cancel(&cancellation_token)
        .await?;
    let start_event = EventMsg::TurnStarted(TurnStartedEvent {
        turn_id: turn_context.sub_id.clone(),
        trace_id: turn_context.trace_id.clone(),
        started_at,
        model_context_window: turn_context.model_context_window(),
        collaboration_mode_kind: turn_context.collaboration_mode.mode,
    });
    sess.send_event(&turn_context, start_event)
        .or_cancel(&cancellation_token)
        .await?;
    run_compact_task_inner(
        sess.clone(),
        turn_context,
        input,
        /*active_client_session*/ None,
        cancellation_token,
        InitialContextInjection::DoNotInject,
        CompactionTrigger::Manual,
        CompactionReason::UserRequested,
        CompactionPhase::StandaloneTurn,
    )
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_compact_task_inner(
    sess: Arc<Session>,
    turn_context: Arc<TurnContext>,
    input: Vec<UserInput>,
    active_client_session: Option<&mut ModelClientSession>,
    cancellation_token: CancellationToken,
    initial_context_injection: InitialContextInjection,
    trigger: CompactionTrigger,
    reason: CompactionReason,
    phase: CompactionPhase,
) -> CodexResult<()> {
    let compact_turn_context = resolve_compact_turn_context(sess.as_ref(), &turn_context)
        .or_cancel(&cancellation_token)
        .await?;
    let compaction_metadata =
        CompactionTurnMetadata::new(trigger, reason, CompactionImplementation::Responses, phase);
    let attempt = CompactionAnalyticsAttempt::begin(
        sess.as_ref(),
        compact_turn_context.as_ref(),
        trigger,
        reason,
        CompactionImplementation::Responses,
        phase,
    )
    .or_cancel(&cancellation_token)
    .await?;
    let pre_compact_outcome = run_pre_compact_hooks(&sess, &turn_context, trigger)
        .or_cancel(&cancellation_token)
        .await?;
    match pre_compact_outcome {
        PreCompactHookOutcome::Continue => {}
        PreCompactHookOutcome::Stopped => {
            let error = CodexErr::TurnAborted;
            attempt
                .track(
                    sess.as_ref(),
                    CompactionStatus::Interrupted,
                    Some(&error),
                    CompactionAnalyticsDetails::default(),
                )
                .or_cancel(&cancellation_token)
                .await?;
            return Err(error);
        }
    }
    let result = run_compact_task_inner_impl(
        Arc::clone(&sess),
        Arc::clone(&turn_context),
        compact_turn_context,
        input,
        active_client_session,
        cancellation_token.clone(),
        initial_context_injection,
        compaction_metadata,
    )
    .await;
    let status = compaction_status_from_result(&result);
    let codex_error = result.as_ref().err();
    if result.is_ok() {
        let post_compact_outcome = run_post_compact_hooks(&sess, &turn_context, trigger)
            .or_cancel(&cancellation_token)
            .await?;
        if let PostCompactHookOutcome::Stopped = post_compact_outcome {
            attempt
                .track(
                    sess.as_ref(),
                    status,
                    codex_error,
                    CompactionAnalyticsDetails::default(),
                )
                .or_cancel(&cancellation_token)
                .await?;
            return Err(CodexErr::TurnAborted);
        }
    }
    attempt
        .track(
            sess.as_ref(),
            status,
            codex_error,
            CompactionAnalyticsDetails::default(),
        )
        .or_cancel(&cancellation_token)
        .await?;
    result.map(|_| ())
}

#[allow(clippy::too_many_arguments)]
async fn run_compact_task_inner_impl(
    sess: Arc<Session>,
    turn_context: Arc<TurnContext>,
    compact_turn_context: Arc<TurnContext>,
    input: Vec<UserInput>,
    active_client_session: Option<&mut ModelClientSession>,
    cancellation_token: CancellationToken,
    initial_context_injection: InitialContextInjection,
    compaction_metadata: CompactionTurnMetadata,
) -> CodexResult<String> {
    if cancellation_token.is_cancelled() {
        return Err(CodexErr::TurnAborted);
    }
    let compaction_item = TurnItem::ContextCompaction(ContextCompactionItem::new());
    let item_started_hook = sess
        .services
        .compact_commit_test_hook
        .as_ref()
        .filter(|hook| hook.take_item_started_pause())
        .cloned();
    let item_started_result = async {
        if let Some(hook) = item_started_hook.as_ref() {
            hook.pause_item_started().await;
        }
        sess.emit_turn_item_started(&turn_context, &compaction_item)
            .await;
    }
    .or_cancel(&cancellation_token)
    .await;
    if item_started_result.is_err()
        && cancellation_token.is_cancelled()
        && let Some(hook) = item_started_hook.as_ref()
    {
        hook.notify_item_started_cancelled();
    }
    item_started_result?;
    let initial_input_for_turn: ResponseInputItem = ResponseInputItem::from(input);

    let mut history = sess.clone_history().or_cancel(&cancellation_token).await?;
    history.replace(crate::compact_input::sanitize_for_compaction(
        history.raw_items(),
    ));
    history.record_items(
        &[initial_input_for_turn.into()],
        compact_turn_context.model_info.truncation_policy.into(),
    );

    let max_retries = compact_turn_context.provider.info().stream_max_retries();
    let mut retries = 0;
    let mut compact_client_session = sess.services.model_client.new_session();
    // Reuse one client session so turn-scoped state (sticky routing, websocket incremental
    // request tracking)
    // survives retries within this compact turn.
    let window_id = sess
        .current_window_id()
        .or_cancel(&cancellation_token)
        .await?;
    let responses_metadata = compact_turn_context
        .turn_metadata_state
        .to_responses_metadata(
            sess.installation_id.clone(),
            window_id,
            CodexResponsesRequestKind::Compaction(compaction_metadata),
        );

    let completed_attempt = loop {
        // Clone is required because of the loop
        let turn_input = history
            .clone()
            .for_prompt(&compact_turn_context.model_info.input_modalities);
        let turn_input_len = turn_input.len();
        let prompt = Prompt {
            input: turn_input,
            base_instructions: sess
                .get_base_instructions()
                .or_cancel(&cancellation_token)
                .await?,
            ..Default::default()
        };
        let attempt_result = drain_to_completed(
            compact_turn_context.as_ref(),
            &mut compact_client_session,
            &responses_metadata,
            &prompt,
            &cancellation_token,
        )
        .await;

        match attempt_result {
            Ok(completed_attempt) => {
                if let Some(token_usage) = completed_attempt.token_usage.as_ref()
                    && let Err(e) = sess.record_rollout_budget_usage(token_usage)
                {
                    sess.track_turn_codex_error(turn_context.as_ref(), &e);
                    let event = EventMsg::Error(e.to_error_event(/*message_prefix*/ None));
                    sess.send_event(&turn_context, event)
                        .or_cancel(&cancellation_token)
                        .await?;
                    return Err(e);
                }
                break completed_attempt;
            }
            Err(err @ (CodexErr::Interrupted | CodexErr::TurnAborted)) => {
                return Err(err);
            }
            Err(e @ CodexErr::SessionBudgetExceeded) => {
                sess.track_turn_codex_error(turn_context.as_ref(), &e);
                let event = EventMsg::Error(e.to_error_event(/*message_prefix*/ None));
                sess.send_event(&turn_context, event)
                    .or_cancel(&cancellation_token)
                    .await?;
                return Err(e);
            }
            Err(e @ CodexErr::ContextWindowExceeded) => {
                if turn_input_len > 1 {
                    // Trim from the beginning to preserve cache (prefix-based) and keep recent messages intact.
                    error!(
                        "Context window exceeded while compacting; removing oldest history item. Error: {e}"
                    );
                    history.remove_first_item();
                    retries = 0;
                    continue;
                }
                sess.set_total_tokens_full(turn_context.as_ref())
                    .or_cancel(&cancellation_token)
                    .await?;
                sess.track_turn_codex_error(turn_context.as_ref(), &e);
                let event = EventMsg::Error(e.to_error_event(/*message_prefix*/ None));
                sess.send_event(&turn_context, event)
                    .or_cancel(&cancellation_token)
                    .await?;
                return Err(e);
            }
            Err(e) => {
                if retries < max_retries {
                    retries += 1;
                    let delay = backoff(retries);
                    sess.notify_stream_error(
                        turn_context.as_ref(),
                        format!("Reconnecting... {retries}/{max_retries}"),
                        e,
                    )
                    .or_cancel(&cancellation_token)
                    .await?;
                    tokio::time::sleep(delay)
                        .or_cancel(&cancellation_token)
                        .await?;
                    continue;
                } else {
                    sess.track_turn_codex_error(turn_context.as_ref(), &e);
                    let event = EventMsg::Error(e.to_error_event(/*message_prefix*/ None));
                    sess.send_event(&turn_context, event)
                        .or_cancel(&cancellation_token)
                        .await?;
                    return Err(e);
                }
            }
        }
    };

    let history_snapshot = sess.clone_history().or_cancel(&cancellation_token).await?;
    let history_items = history_snapshot.raw_items();
    let summary_suffix = completed_attempt.summary_suffix.clone();
    let summary_text = format!("{SUMMARY_PREFIX}\n{summary_suffix}");
    let user_messages = collect_user_messages(history_items);

    let mut new_history = build_compacted_history(Vec::new(), &user_messages, &summary_text);
    if let Some(summary_item) = new_history.last_mut() {
        // This replacement history skips `record_conversation_items`; only the appended summary
        // belongs to this compaction turn.
        summary_item.set_turn_id_if_missing(&turn_context.sub_id);
    }
    let (window_number, window_ids) = sess
        .prepare_auto_compact_window_advance()
        .or_cancel(&cancellation_token)
        .await?;

    let (initial_context, world_state_baseline) = build_compaction_initial_context(
        sess.as_ref(),
        turn_context.as_ref(),
        &initial_context_injection,
        Some(window_ids),
    )
    .or_cancel(&cancellation_token)
    .await?;
    if !initial_context.is_empty() {
        new_history =
            insert_initial_context_before_last_real_user_or_summary(new_history, initial_context);
    }
    let reference_context_item = match initial_context_injection {
        InitialContextInjection::DoNotInject => None,
        InitialContextInjection::BeforeLastUserMessage(_) => {
            Some(turn_context.to_turn_context_item())
        }
    };
    let compacted_item = CompactedItem {
        message: summary_text.clone(),
        replacement_history: None,
        window_number: Some(window_number),
        first_window_id: Some(window_ids.first_window_id.to_string()),
        previous_window_id: window_ids.previous_window_id.map(|id| id.to_string()),
        window_id: Some(window_ids.window_id.to_string()),
        checkpoint: None,
    };
    let prepared_commit = sess.prepare_compaction_commit(
        turn_context.as_ref(),
        new_history,
        reference_context_item,
        world_state_baseline,
        compacted_item,
        window_number,
        window_ids,
        completed_attempt.token_usage.clone(),
        completed_attempt.server_reasoning_included,
        completed_attempt.rate_limits.clone(),
    );
    if cancellation_token.is_cancelled() {
        return Err(CodexErr::TurnAborted);
    }
    let commit_sess = Arc::clone(&sess);
    let commit_turn_context = Arc::clone(&turn_context);
    let commit_baseline = CompactCommitBaseline::take_from_active(active_client_session);
    let child_baseline = commit_baseline.clone();
    let commit_handle = tokio::spawn(async move {
        commit_sess
            .commit_prepared_compaction(prepared_commit, child_baseline, commit_turn_context)
            .await
    });
    let mut parent_wait_guard = sess
        .services
        .compact_commit_test_hook
        .as_ref()
        .map(CompactCommitTestHook::parent_wait_guard);
    let commit_result = commit_handle.await;
    if let Some(guard) = parent_wait_guard.as_mut() {
        guard.disarm();
    }
    let commit_result = match commit_result {
        Ok(result) => result,
        Err(err) => {
            commit_baseline.restore_to_session(sess.as_ref()).await;
            Err(CodexErr::Io(std::io::Error::other(format!(
                "compact commit task failed: {err}"
            ))))
        }
    };
    if let Err(err) = commit_result {
        sess.track_turn_codex_error(turn_context.as_ref(), &err);
        let event = EventMsg::Error(err.to_error_event(/*message_prefix*/ None));
        sess.send_event(&turn_context, event)
            .or_cancel(&cancellation_token)
            .await?;
        return Err(err);
    }
    sess.emit_turn_item_completed(&turn_context, compaction_item)
        .or_cancel(&cancellation_token)
        .await?;
    let warning = EventMsg::Warning(WarningEvent {
        message: "Heads up: Long threads and multiple compactions can cause the model to be less accurate. Start a new thread when possible to keep threads small and targeted.".to_string(),
    });
    sess.send_event(&turn_context, warning)
        .or_cancel(&cancellation_token)
        .await?;
    Ok(summary_suffix)
}

pub(crate) struct CompactionAnalyticsAttempt {
    thread_id: String,
    turn_id: String,
    trigger: CompactionTrigger,
    reason: CompactionReason,
    implementation: CompactionImplementation,
    phase: CompactionPhase,
    active_context_tokens_before: i64,
    started_at: u64,
    start_instant: Instant,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct CompactionAnalyticsDetails {
    pub(crate) active_context_tokens_before: Option<i64>,
    pub(crate) retained_image_count: Option<usize>,
    pub(crate) compaction_summary_tokens: Option<i64>,
    pub(crate) cached_input_tokens: Option<i64>,
}

impl CompactionAnalyticsAttempt {
    pub(crate) async fn begin(
        sess: &Session,
        turn_context: &TurnContext,
        trigger: CompactionTrigger,
        reason: CompactionReason,
        implementation: CompactionImplementation,
        phase: CompactionPhase,
    ) -> Self {
        let active_context_tokens_before = sess.get_total_token_usage().await;
        Self {
            thread_id: sess.thread_id.to_string(),
            turn_id: turn_context.sub_id.clone(),
            trigger,
            reason,
            implementation,
            phase,
            active_context_tokens_before,
            started_at: now_unix_seconds(),
            start_instant: Instant::now(),
        }
    }

    pub(crate) async fn track(
        self,
        sess: &Session,
        status: CompactionStatus,
        codex_error: Option<&CodexErr>,
        details: CompactionAnalyticsDetails,
    ) {
        let CompactionAnalyticsDetails {
            active_context_tokens_before,
            retained_image_count,
            compaction_summary_tokens,
            cached_input_tokens,
        } = details;
        let active_context_tokens_before =
            active_context_tokens_before.unwrap_or(self.active_context_tokens_before);
        let active_context_tokens_after = sess.get_total_token_usage().await;
        sess.services
            .analytics_events_client
            .track_compaction(CodexCompactionEvent {
                thread_id: self.thread_id,
                turn_id: self.turn_id,
                trigger: self.trigger,
                reason: self.reason,
                implementation: self.implementation,
                phase: self.phase,
                strategy: CompactionStrategy::Memento,
                status,
                codex_error_kind: codex_error.map(Into::into),
                codex_error_http_status_code: codex_error
                    .and_then(CodexErr::http_status_code_value),
                active_context_tokens_before,
                active_context_tokens_after,
                retained_image_count,
                compaction_summary_tokens,
                cached_input_tokens,
                started_at: self.started_at,
                completed_at: now_unix_seconds(),
                duration_ms: Some(
                    u64::try_from(self.start_instant.elapsed().as_millis()).unwrap_or(u64::MAX),
                ),
            });
    }
}

pub(crate) fn compaction_status_from_result<T>(result: &CodexResult<T>) -> CompactionStatus {
    match result {
        Ok(_) => CompactionStatus::Completed,
        Err(CodexErr::Interrupted | CodexErr::TurnAborted) => CompactionStatus::Interrupted,
        Err(_) => CompactionStatus::Failed,
    }
}

pub fn content_items_to_text(content: &[ContentItem]) -> Option<String> {
    let mut pieces = Vec::new();
    for item in content {
        match item {
            ContentItem::InputText { text } | ContentItem::OutputText { text } => {
                if !text.is_empty() {
                    pieces.push(text.as_str());
                }
            }
            ContentItem::InputImage { .. } => {}
        }
    }
    if pieces.is_empty() {
        None
    } else {
        Some(pieces.join("\n"))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CompactedUserMessage {
    message: String,
    internal_chat_message_metadata_passthrough: Option<InternalChatMessageMetadataPassthrough>,
}

pub(crate) fn collect_user_messages(items: &[ResponseItem]) -> Vec<CompactedUserMessage> {
    items
        .iter()
        .filter_map(|item| match crate::event_mapping::parse_turn_item(item) {
            Some(TurnItem::UserMessage(user)) => {
                if is_summary_message(&user.message()) {
                    None
                } else {
                    Some(CompactedUserMessage {
                        message: user.message(),
                        internal_chat_message_metadata_passthrough: match item {
                            ResponseItem::Message {
                                internal_chat_message_metadata_passthrough,
                                ..
                            } => internal_chat_message_metadata_passthrough.clone(),
                            _ => None,
                        },
                    })
                }
            }
            _ => None,
        })
        .collect()
}

pub(crate) fn is_summary_message(message: &str) -> bool {
    message.starts_with(format!("{SUMMARY_PREFIX}\n").as_str())
}

/// Inserts canonical initial context into compacted replacement history at the
/// model-expected boundary.
///
/// Placement rules:
/// - Prefer immediately before the last real user message.
/// - If no real user messages remain, insert before the compaction summary so
///   the summary stays last.
/// - If there are no user messages, insert before the last compaction item so
///   that item remains last (remote compaction may return only compaction items).
/// - If there are no user messages or compaction items, append the context.
pub(crate) fn insert_initial_context_before_last_real_user_or_summary(
    mut compacted_history: Vec<ResponseItem>,
    initial_context: Vec<ResponseItem>,
) -> Vec<ResponseItem> {
    let mut last_user_or_summary_index = None;
    let mut last_real_user_index = None;
    for (i, item) in compacted_history.iter().enumerate().rev() {
        let Some(TurnItem::UserMessage(user)) = crate::event_mapping::parse_turn_item(item) else {
            continue;
        };
        // Compaction summaries are encoded as user messages, so track both:
        // the last real user message (preferred insertion point) and the last
        // user-message-like item (fallback summary insertion point).
        last_user_or_summary_index.get_or_insert(i);
        if !is_summary_message(&user.message()) {
            last_real_user_index = Some(i);
            break;
        }
    }
    let last_compaction_index = compacted_history
        .iter()
        .enumerate()
        .rev()
        .find_map(|(i, item)| {
            matches!(
                item,
                ResponseItem::Compaction { .. } | ResponseItem::ContextCompaction { .. }
            )
            .then_some(i)
        });
    let insertion_index = last_real_user_index
        .or(last_user_or_summary_index)
        .or(last_compaction_index);

    // Re-inject canonical context from the current session since we stripped it
    // from the pre-compaction history. Prefer placing it before the last real
    // user message; if there is no real user message left, place it before the
    // summary or compaction item so the compaction item remains last.
    if let Some(insertion_index) = insertion_index {
        compacted_history.splice(insertion_index..insertion_index, initial_context);
    } else {
        compacted_history.extend(initial_context);
    }

    compacted_history
}

pub(crate) fn build_compacted_history(
    initial_context: Vec<ResponseItem>,
    user_messages: &[CompactedUserMessage],
    summary_text: &str,
) -> Vec<ResponseItem> {
    build_compacted_history_with_limit(
        initial_context,
        user_messages,
        summary_text,
        COMPACT_USER_MESSAGE_MAX_TOKENS,
    )
}

fn build_compacted_history_with_limit(
    mut history: Vec<ResponseItem>,
    user_messages: &[CompactedUserMessage],
    summary_text: &str,
    max_tokens: usize,
) -> Vec<ResponseItem> {
    let mut selected_messages: Vec<CompactedUserMessage> = Vec::new();
    if max_tokens > 0 {
        let mut remaining = max_tokens;
        for message in user_messages.iter().rev() {
            if remaining == 0 {
                break;
            }
            let tokens = approx_token_count(&message.message);
            if tokens <= remaining {
                selected_messages.push(message.clone());
                remaining = remaining.saturating_sub(tokens);
            } else {
                let truncated =
                    truncate_text(&message.message, TruncationPolicy::Tokens(remaining));
                selected_messages.push(CompactedUserMessage {
                    message: truncated,
                    internal_chat_message_metadata_passthrough: message
                        .internal_chat_message_metadata_passthrough
                        .clone(),
                });
                break;
            }
        }
        selected_messages.reverse();
    }

    for message in &selected_messages {
        history.push(ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: message.message.clone(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: message
                .internal_chat_message_metadata_passthrough
                .clone(),
        });
    }

    let summary_text = if summary_text.is_empty() {
        "(no summary available)".to_string()
    } else {
        summary_text.to_string()
    };

    history.push(ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputText { text: summary_text }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    });

    history
}

struct CompletedCompactAttempt {
    summary_suffix: String,
    token_usage: Option<TokenUsage>,
    server_reasoning_included: Option<bool>,
    rate_limits: Vec<RateLimitSnapshot>,
}

async fn drain_to_completed(
    compact_turn_context: &TurnContext,
    client_session: &mut ModelClientSession,
    responses_metadata: &CodexResponsesMetadata,
    prompt: &Prompt,
    cancellation_token: &CancellationToken,
) -> CodexResult<CompletedCompactAttempt> {
    let mut stream = client_session
        .stream(
            prompt,
            &compact_turn_context.model_info,
            &compact_turn_context.session_telemetry,
            compact_turn_context.reasoning_effort.clone(),
            compact_turn_context.reasoning_summary,
            compact_turn_context.config.service_tier.clone(),
            responses_metadata,
            // Rollout tracing currently models remote compaction only; local compaction streams
            // are left untraced until the reducer has a first-class local compaction lifecycle.
            &InferenceTraceContext::disabled(),
        )
        .or_cancel(cancellation_token)
        .await??;
    let mut output_items = Vec::new();
    let mut server_reasoning_included = None;
    let mut rate_limits = Vec::new();
    loop {
        let maybe_event = stream.next().or_cancel(cancellation_token).await?;
        let Some(event) = maybe_event else {
            return Err(CodexErr::Stream(
                "stream closed before response.completed".into(),
                None,
            ));
        };
        match event {
            Ok(ResponseEvent::OutputItemDone(item)) => {
                output_items.push(item);
            }
            Ok(ResponseEvent::ServerReasoningIncluded(included)) => {
                server_reasoning_included = Some(included);
            }
            Ok(ResponseEvent::RateLimits(snapshot)) => {
                rate_limits.push(snapshot);
            }
            Ok(ResponseEvent::Completed { token_usage, .. }) => {
                let summary_suffix = get_last_assistant_message_from_turn(&output_items)
                    .filter(|summary| !summary.trim().is_empty())
                    .ok_or_else(|| {
                        CodexErr::Stream(
                            "compact response completed without a non-empty assistant summary"
                                .into(),
                            None,
                        )
                    })?;
                return Ok(CompletedCompactAttempt {
                    summary_suffix,
                    token_usage,
                    server_reasoning_included,
                    rate_limits,
                });
            }
            Ok(_) => continue,
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
#[path = "compact_tests.rs"]
mod tests;

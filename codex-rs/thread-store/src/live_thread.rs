use std::path::PathBuf;
use std::sync::Arc;

use codex_protocol::ThreadId;
use codex_protocol::protocol::CompactedItem;
use codex_protocol::protocol::RolloutItem;
use codex_protocol::protocol::ThreadMemoryMode;
use codex_rollout::RolloutPersistenceBatchMeasurement;
use codex_rollout::RolloutPersistenceTelemetry;
use codex_rollout::measure_and_filter_rollout_items;
use codex_rollout::persisted_rollout_items;
use tokio::sync::Mutex;
use tracing::warn;

use crate::AppendThreadItemsParams;
use crate::CreateThreadParams;
use crate::LoadThreadHistoryParams;
use crate::LocalThreadStore;
use crate::ReadThreadParams;
use crate::ResumeThreadParams;
use crate::StoredThread;
use crate::StoredThreadHistory;
use crate::ThreadMetadataMutationGate;
use crate::ThreadMetadataPatch;
use crate::ThreadStore;
use crate::ThreadStoreResult;
use crate::ThreadTitleGenerator;
use crate::ThreadTitleRequest;
use crate::UpdateThreadMetadataParams;
use crate::thread_metadata_sync::ThreadMetadataSync;

const COMPACTION_CHECKPOINT_RECONCILIATION_ATTEMPTS: usize = 3;

/// Durable outcome of appending one atomic local-compaction checkpoint.
#[derive(Debug)]
pub enum CompactionCheckpointAppendOutcome {
    /// The checkpoint record is durably visible, either from the append result or reconciliation.
    Committed,
    /// Durable history was readable and did not contain the checkpoint ID.
    NotCommitted {
        append_error: crate::ThreadStoreError,
    },
    /// Durable history could not be read after bounded reconciliation attempts.
    ///
    /// Callers must not restore an incremental baseline in this state. Restarting/reloading the
    /// thread converges from whichever complete checkpoint records are actually durable.
    Ambiguous {
        append_error: crate::ThreadStoreError,
        reconciliation_errors: Vec<crate::ThreadStoreError>,
    },
}

/// Handle for an active thread's persistence lifecycle.
///
/// `LiveThread` keeps lifecycle decisions with the caller while delegating storage details to
/// [`ThreadStore`]. Local stores may use a rollout file internally and remote stores may use a
/// service, but session code should only need this handle for the active thread.
#[derive(Clone)]
pub struct LiveThread {
    thread_id: ThreadId,
    thread_store: Arc<dyn ThreadStore>,
    metadata_sync: Arc<Mutex<ThreadMetadataSync>>,
    persistence_telemetry: RolloutPersistenceTelemetry,
    metadata_mutation_gate: Option<Arc<dyn ThreadMetadataMutationGate>>,
}

/// Owns a live thread while session initialization is still fallible.
///
/// If initialization returns early after persistence has been opened, dropping this guard discards
/// the live writer without forcing lazy in-memory state to become durable. Call [`commit`] once the
/// session owns the live thread for normal operation.
pub struct LiveThreadInitGuard {
    live_thread: Option<LiveThread>,
}

impl LiveThreadInitGuard {
    pub fn new(live_thread: Option<LiveThread>) -> Self {
        Self { live_thread }
    }

    pub fn as_ref(&self) -> Option<&LiveThread> {
        self.live_thread.as_ref()
    }

    pub fn commit(&mut self) {
        self.live_thread = None;
    }

    pub async fn discard(&mut self) {
        let Some(live_thread) = self.live_thread.take() else {
            return;
        };
        if let Err(err) = live_thread.discard().await {
            warn!("failed to discard thread persistence for failed session init: {err}");
        }
    }
}

impl Drop for LiveThreadInitGuard {
    fn drop(&mut self) {
        let Some(live_thread) = self.live_thread.take() else {
            return;
        };
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            warn!("failed to discard thread persistence for failed session init: no Tokio runtime");
            return;
        };
        handle.spawn(async move {
            if let Err(err) = live_thread.discard().await {
                warn!("failed to discard thread persistence for failed session init: {err}");
            }
        });
    }
}

impl LiveThread {
    pub async fn create(
        thread_store: Arc<dyn ThreadStore>,
        params: CreateThreadParams,
    ) -> ThreadStoreResult<Self> {
        let thread_id = params.thread_id;
        let metadata_sync = ThreadMetadataSync::for_create(&params).await;
        thread_store.create_thread(params).await?;
        Ok(Self {
            thread_id,
            thread_store,
            metadata_sync: Arc::new(Mutex::new(metadata_sync)),
            persistence_telemetry: RolloutPersistenceTelemetry::new(thread_id),
            metadata_mutation_gate: None,
        })
    }

    pub async fn resume(
        thread_store: Arc<dyn ThreadStore>,
        params: ResumeThreadParams,
    ) -> ThreadStoreResult<Self> {
        let thread_id = params.thread_id;
        let should_load_history = params.history.is_none();
        let include_archived = params.include_archived;
        let mut metadata_sync = ThreadMetadataSync::for_resume(&params);
        thread_store.resume_thread(params).await?;
        if should_load_history {
            match thread_store
                .load_history(LoadThreadHistoryParams {
                    thread_id,
                    include_archived,
                })
                .await
            {
                Ok(history) => metadata_sync.record_resume_history(&history.items),
                Err(err) => {
                    if let Err(discard_err) = thread_store.discard_thread(thread_id).await {
                        warn!(
                            "failed to discard thread persistence after resume history load failed: {discard_err}"
                        );
                    }
                    return Err(err);
                }
            }
        }
        Ok(Self {
            thread_id,
            thread_store,
            metadata_sync: Arc::new(Mutex::new(metadata_sync)),
            persistence_telemetry: RolloutPersistenceTelemetry::new(thread_id),
            metadata_mutation_gate: None,
        })
    }

    /// Installs the host lifecycle gate used by detached title metadata updates.
    pub fn with_metadata_mutation_gate(
        mut self,
        gate: Arc<dyn ThreadMetadataMutationGate>,
    ) -> Self {
        self.metadata_mutation_gate = Some(gate);
        self
    }

    #[tracing::instrument(
        level = "trace",
        skip_all,
        fields(item_count = items.len())
    )]
    pub async fn append_items(&self, items: &[RolloutItem]) -> ThreadStoreResult<()> {
        // Empty appends are intentionally ignored rather than represented as zero-sized batches.
        if items.is_empty() {
            return Ok(());
        }
        let (canonical_items, measurement) = if self.persistence_telemetry.is_enabled() {
            let (canonical_items, measurement) = measure_and_filter_rollout_items(items);
            (canonical_items, Some(measurement))
        } else {
            (persisted_rollout_items(items), None)
        };
        self.thread_store
            .append_items(AppendThreadItemsParams {
                thread_id: self.thread_id,
                items: items.to_vec(),
            })
            .await?;
        self.finish_durable_append(items, &canonical_items, measurement)
            .await;
        Ok(())
    }

    /// Appends one local-compaction checkpoint record and reconciles an ambiguous store error by
    /// scanning durable history for its stable ID.
    ///
    /// Reconciliation retries reads, never the append itself. This avoids creating duplicates at
    /// this layer while still tolerating duplicates produced by a backing writer's own retry.
    pub async fn append_compaction_checkpoint(
        &self,
        compacted_item: &CompactedItem,
    ) -> CompactionCheckpointAppendOutcome {
        let Some(checkpoint_id) = compacted_item
            .checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.checkpoint_id.clone())
        else {
            return CompactionCheckpointAppendOutcome::NotCommitted {
                append_error: crate::ThreadStoreError::InvalidRequest {
                    message: "compaction checkpoint payload is missing".to_string(),
                },
            };
        };
        let item = RolloutItem::Compacted(compacted_item.clone());
        let append_error = match self.append_items(std::slice::from_ref(&item)).await {
            Ok(()) => return CompactionCheckpointAppendOutcome::Committed,
            Err(err) => err,
        };
        let mut reconciliation_errors = Vec::new();
        for attempt in 0..COMPACTION_CHECKPOINT_RECONCILIATION_ATTEMPTS {
            match self.load_history(/*include_archived*/ true).await {
                Ok(history) => {
                    let committed = history.items.iter().any(|item| {
                        matches!(
                            item,
                            RolloutItem::Compacted(compacted)
                                if compacted.checkpoint.as_ref().is_some_and(|checkpoint| {
                                    checkpoint.checkpoint_id == checkpoint_id
                                })
                        )
                    });
                    if committed {
                        let items = std::slice::from_ref(&item);
                        let (canonical_items, measurement) =
                            if self.persistence_telemetry.is_enabled() {
                                let (canonical_items, measurement) =
                                    measure_and_filter_rollout_items(items);
                                (canonical_items, Some(measurement))
                            } else {
                                (persisted_rollout_items(items), None)
                            };
                        self.finish_durable_append(items, &canonical_items, measurement)
                            .await;
                        return CompactionCheckpointAppendOutcome::Committed;
                    }
                    return CompactionCheckpointAppendOutcome::NotCommitted { append_error };
                }
                Err(err) => reconciliation_errors.push(err),
            }
            if attempt + 1 < COMPACTION_CHECKPOINT_RECONCILIATION_ATTEMPTS {
                tokio::task::yield_now().await;
            }
        }
        CompactionCheckpointAppendOutcome::Ambiguous {
            append_error,
            reconciliation_errors,
        }
    }

    async fn finish_durable_append(
        &self,
        items: &[RolloutItem],
        canonical_items: &[RolloutItem],
        measurement: Option<RolloutPersistenceBatchMeasurement>,
    ) {
        if let Some(measurement) = measurement.as_ref() {
            self.persistence_telemetry.record_batch(items, measurement);
        }
        if canonical_items.is_empty() {
            return;
        }
        let update = self
            .metadata_sync
            .lock()
            .await
            .observe_appended_items(canonical_items);
        if let Some(update) = update {
            let result = self
                .thread_store
                .update_thread_metadata(UpdateThreadMetadataParams {
                    thread_id: self.thread_id,
                    patch: update.patch.clone(),
                    include_archived: true,
                })
                .await;
            match result {
                Ok(_) => {
                    self.metadata_sync
                        .lock()
                        .await
                        .mark_pending_update_applied(&update);
                }
                Err(err) => {
                    warn!(
                        thread_id = %self.thread_id,
                        %err,
                        "durable append succeeded but thread metadata projection failed; update remains pending"
                    );
                }
            }
        }
        self.maybe_dispatch_llm_title(items).await;
    }

    /// Best-effort: once the first assistant turn completes, spawn an async task
    /// that upgrades the rule-based title to an LLM-generated one. This never
    /// blocks the append/turn hot path and leaves the rule-based title on any
    /// failure.
    async fn maybe_dispatch_llm_title(&self, items: &[RolloutItem]) {
        let request = {
            let mut metadata_sync = self.metadata_sync.lock().await;
            metadata_sync.take_llm_title_request(items)
        };
        let Some(request) = request else {
            return;
        };
        let Some(generator) = self.thread_store.title_generator() else {
            return;
        };
        spawn_llm_title_task(
            Arc::clone(&self.thread_store),
            generator,
            self.thread_id,
            request,
            self.metadata_mutation_gate.clone(),
        );
    }

    pub async fn persist(&self) -> ThreadStoreResult<()> {
        self.thread_store.persist_thread(self.thread_id).await?;
        self.flush_pending_metadata_update().await
    }

    pub async fn flush(&self) -> ThreadStoreResult<()> {
        self.thread_store.flush_thread(self.thread_id).await?;
        self.flush_pending_metadata_update_for_existing_history()
            .await
    }

    pub async fn shutdown(&self) -> ThreadStoreResult<()> {
        self.flush_pending_metadata_update_for_existing_history()
            .await?;
        self.thread_store.shutdown_thread(self.thread_id).await
    }

    pub async fn discard(&self) -> ThreadStoreResult<()> {
        self.thread_store.discard_thread(self.thread_id).await
    }

    pub async fn load_history(
        &self,
        include_archived: bool,
    ) -> ThreadStoreResult<StoredThreadHistory> {
        self.thread_store
            .load_history(LoadThreadHistoryParams {
                thread_id: self.thread_id,
                include_archived,
            })
            .await
    }

    pub async fn read_thread(
        &self,
        include_archived: bool,
        include_history: bool,
    ) -> ThreadStoreResult<StoredThread> {
        self.thread_store
            .read_thread(ReadThreadParams {
                thread_id: self.thread_id,
                include_archived,
                include_history,
            })
            .await
    }

    pub async fn update_memory_mode(
        &self,
        mode: ThreadMemoryMode,
        include_archived: bool,
    ) -> ThreadStoreResult<()> {
        self.flush_pending_metadata_update().await?;
        self.thread_store
            .update_thread_metadata(UpdateThreadMetadataParams {
                thread_id: self.thread_id,
                patch: ThreadMetadataPatch {
                    memory_mode: Some(mode),
                    ..Default::default()
                },
                include_archived,
            })
            .await?;
        Ok(())
    }

    pub async fn update_metadata(
        &self,
        patch: ThreadMetadataPatch,
        include_archived: bool,
    ) -> ThreadStoreResult<StoredThread> {
        self.flush_pending_metadata_update().await?;
        self.thread_store
            .update_thread_metadata(UpdateThreadMetadataParams {
                thread_id: self.thread_id,
                patch,
                include_archived,
            })
            .await
    }

    /// Returns the live local rollout path for legacy local-only callers.
    ///
    /// Remote stores do not expose rollout files, so they return `Ok(None)`.
    pub async fn local_rollout_path(&self) -> ThreadStoreResult<Option<PathBuf>> {
        let Some(local_store) = self
            .thread_store
            .as_any()
            .downcast_ref::<LocalThreadStore>()
        else {
            return Ok(None);
        };
        local_store
            .live_rollout_path(self.thread_id)
            .await
            .map(Some)
    }

    async fn flush_pending_metadata_update(&self) -> ThreadStoreResult<()> {
        let update = self.metadata_sync.lock().await.take_pending_update();
        self.apply_pending_metadata_update(update).await
    }

    async fn flush_pending_metadata_update_for_existing_history(&self) -> ThreadStoreResult<()> {
        let update = self
            .metadata_sync
            .lock()
            .await
            .take_pending_update_for_existing_history();
        self.apply_pending_metadata_update(update).await
    }

    async fn apply_pending_metadata_update(
        &self,
        update: Option<crate::thread_metadata_sync::PendingThreadMetadataPatch>,
    ) -> ThreadStoreResult<()> {
        let Some(update) = update else {
            return Ok(());
        };
        self.thread_store
            .update_thread_metadata(UpdateThreadMetadataParams {
                thread_id: self.thread_id,
                patch: update.patch.clone(),
                include_archived: true,
            })
            .await?;
        self.metadata_sync
            .lock()
            .await
            .mark_pending_update_applied(&update);
        Ok(())
    }
}

/// Spawns the best-effort LLM title task on the current Tokio runtime.
///
/// The task calls the host generator, then writes the title through the same
/// [`ThreadStore::update_thread_metadata`] path used by rule-based titles and
/// manual renames. It only overwrites when the stored name is still the
/// rule-based first-user-message title, so a manual rename is never clobbered.
fn spawn_llm_title_task(
    thread_store: Arc<dyn ThreadStore>,
    generator: Arc<dyn ThreadTitleGenerator>,
    thread_id: ThreadId,
    request: ThreadTitleRequest,
    metadata_mutation_gate: Option<Arc<dyn ThreadMetadataMutationGate>>,
) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    handle.spawn(async move {
        let guard_title = request.first_user_message.clone();
        let Some(title) = generator
            .generate_title(request)
            .await
            .map(|title| sanitize_title(&title))
            .filter(|title| !title.is_empty())
        else {
            return;
        };
        let _mutation_permit = match metadata_mutation_gate {
            Some(gate) => {
                let Some(permit) = gate.acquire().await else {
                    return;
                };
                Some(permit)
            }
            None => None,
        };
        match thread_store
            .read_thread(ReadThreadParams {
                thread_id,
                include_archived: true,
                include_history: false,
            })
            .await
        {
            // Only replace the auto-derived rule-based title, never a manual
            // rename. Stores that keep the rule-based title equal to the first
            // user message surface it as an *empty* name (it is not a "distinct"
            // title), so `None` is the common auto-derived state; other stores may
            // instead expose the name verbatim. Treat both as still-auto-derived,
            // while a manual rename or an already-applied generated title leaves a
            // different, non-empty name and is left untouched.
            Ok(thread)
                if thread.name.is_none()
                    || thread.name.as_deref() == Some(guard_title.as_str()) =>
            {
                if let Err(err) = thread_store
                    .update_thread_metadata(UpdateThreadMetadataParams {
                        thread_id,
                        patch: ThreadMetadataPatch {
                            title: Some(title),
                            ..Default::default()
                        },
                        include_archived: true,
                    })
                    .await
                {
                    warn!("failed to persist generated thread title for {thread_id}: {err}");
                }
            }
            Ok(_) => {}
            Err(err) => {
                warn!(
                    "failed to read thread before applying generated title for {thread_id}: {err}"
                );
            }
        }
    });
}

/// Normalizes a raw model title into a short, single-line, punctuation-trimmed
/// display title.
fn sanitize_title(raw: &str) -> String {
    let first_line = raw
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim();
    let unquoted = first_line.trim_matches(|c: char| {
        c.is_whitespace() || matches!(c, '"' | '\'' | '「' | '」' | '《' | '》' | '“' | '”')
    });
    let trimmed = unquoted.trim_end_matches(|c: char| {
        matches!(
            c,
            '。' | '.' | '!' | '！' | '?' | '？' | '，' | ',' | '、' | '：' | ':' | ';' | '；'
        )
    });
    trimmed.chars().take(30).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InMemoryThreadStore;
    use crate::ThreadPersistenceMetadata;
    use codex_protocol::models::BaseInstructions;
    use codex_protocol::protocol::AgentMessageEvent;
    use codex_protocol::protocol::CompactedItem;
    use codex_protocol::protocol::CompactionCheckpoint;
    use codex_protocol::protocol::EventMsg;
    use codex_protocol::protocol::SessionSource;
    use codex_protocol::protocol::ThreadHistoryMode;
    use codex_protocol::protocol::TokenCountEvent;
    use codex_protocol::protocol::TokenUsage;
    use codex_protocol::protocol::TokenUsageInfo;
    use codex_protocol::protocol::TurnCompleteEvent;
    use codex_protocol::protocol::UserMessageEvent;
    use pretty_assertions::assert_eq;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering;

    #[derive(Debug)]
    struct StubTitleGenerator;

    impl ThreadTitleGenerator for StubTitleGenerator {
        fn generate_title<'a>(
            &'a self,
            _request: ThreadTitleRequest,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>>
        {
            Box::pin(async { Some("generated title".to_string()) })
        }
    }

    #[derive(Debug)]
    struct TrackingMutationGate {
        allow: bool,
        acquire_attempted: Arc<AtomicBool>,
        permit_held: Arc<AtomicBool>,
        permit_released: Arc<AtomicBool>,
    }

    struct TrackingMutationPermit {
        permit_held: Arc<AtomicBool>,
        permit_released: Arc<AtomicBool>,
    }

    impl Drop for TrackingMutationPermit {
        fn drop(&mut self) {
            self.permit_held.store(false, Ordering::SeqCst);
            self.permit_released.store(true, Ordering::SeqCst);
        }
    }

    impl crate::ThreadMetadataMutationPermit for TrackingMutationPermit {}

    impl ThreadMetadataMutationGate for TrackingMutationGate {
        fn acquire<'a>(&'a self) -> crate::ThreadMetadataMutationPermitFuture<'a> {
            Box::pin(async move {
                self.acquire_attempted.store(true, Ordering::SeqCst);
                if !self.allow {
                    return None;
                }
                self.permit_held.store(true, Ordering::SeqCst);
                Some(Box::new(TrackingMutationPermit {
                    permit_held: Arc::clone(&self.permit_held),
                    permit_released: Arc::clone(&self.permit_released),
                })
                    as Box<dyn crate::ThreadMetadataMutationPermit>)
            })
        }
    }

    struct PermitObservingThreadStore {
        inner: Arc<InMemoryThreadStore>,
        permit_held: Arc<AtomicBool>,
        title_read_while_held: AtomicBool,
        title_update_seen: AtomicBool,
        title_update_while_held: AtomicBool,
    }

    impl PermitObservingThreadStore {
        fn new(permit_held: Arc<AtomicBool>) -> Self {
            Self {
                inner: Arc::new(InMemoryThreadStore::default()),
                permit_held,
                title_read_while_held: AtomicBool::new(false),
                title_update_seen: AtomicBool::new(false),
                title_update_while_held: AtomicBool::new(false),
            }
        }
    }

    impl ThreadStore for PermitObservingThreadStore {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn set_title_generator(&self, generator: Arc<dyn ThreadTitleGenerator>) {
            self.inner.set_title_generator(generator);
        }

        fn title_generator(&self) -> Option<Arc<dyn ThreadTitleGenerator>> {
            self.inner.title_generator()
        }

        fn create_thread(&self, params: CreateThreadParams) -> crate::ThreadStoreFuture<'_, ()> {
            ThreadStore::create_thread(self.inner.as_ref(), params)
        }

        fn resume_thread(&self, params: ResumeThreadParams) -> crate::ThreadStoreFuture<'_, ()> {
            ThreadStore::resume_thread(self.inner.as_ref(), params)
        }

        fn append_items(
            &self,
            params: AppendThreadItemsParams,
        ) -> crate::ThreadStoreFuture<'_, ()> {
            ThreadStore::append_items(self.inner.as_ref(), params)
        }

        fn persist_thread(&self, thread_id: ThreadId) -> crate::ThreadStoreFuture<'_, ()> {
            ThreadStore::persist_thread(self.inner.as_ref(), thread_id)
        }

        fn flush_thread(&self, thread_id: ThreadId) -> crate::ThreadStoreFuture<'_, ()> {
            ThreadStore::flush_thread(self.inner.as_ref(), thread_id)
        }

        fn shutdown_thread(&self, thread_id: ThreadId) -> crate::ThreadStoreFuture<'_, ()> {
            ThreadStore::shutdown_thread(self.inner.as_ref(), thread_id)
        }

        fn discard_thread(&self, thread_id: ThreadId) -> crate::ThreadStoreFuture<'_, ()> {
            ThreadStore::discard_thread(self.inner.as_ref(), thread_id)
        }

        fn load_history(
            &self,
            params: LoadThreadHistoryParams,
        ) -> crate::ThreadStoreFuture<'_, StoredThreadHistory> {
            ThreadStore::load_history(self.inner.as_ref(), params)
        }

        fn read_thread(
            &self,
            params: ReadThreadParams,
        ) -> crate::ThreadStoreFuture<'_, StoredThread> {
            self.title_read_while_held
                .store(self.permit_held.load(Ordering::SeqCst), Ordering::SeqCst);
            ThreadStore::read_thread(self.inner.as_ref(), params)
        }

        fn read_thread_by_rollout_path(
            &self,
            params: crate::ReadThreadByRolloutPathParams,
        ) -> crate::ThreadStoreFuture<'_, StoredThread> {
            ThreadStore::read_thread_by_rollout_path(self.inner.as_ref(), params)
        }

        fn list_threads(
            &self,
            params: crate::ListThreadsParams,
        ) -> crate::ThreadStoreFuture<'_, crate::ThreadPage> {
            ThreadStore::list_threads(self.inner.as_ref(), params)
        }

        fn update_thread_metadata(
            &self,
            params: UpdateThreadMetadataParams,
        ) -> crate::ThreadStoreFuture<'_, StoredThread> {
            if params.patch.title.as_deref() == Some("generated title") {
                self.title_update_seen.store(true, Ordering::SeqCst);
                self.title_update_while_held
                    .store(self.permit_held.load(Ordering::SeqCst), Ordering::SeqCst);
            }
            ThreadStore::update_thread_metadata(self.inner.as_ref(), params)
        }

        fn archive_thread(
            &self,
            params: crate::ArchiveThreadParams,
        ) -> crate::ThreadStoreFuture<'_, ()> {
            ThreadStore::archive_thread(self.inner.as_ref(), params)
        }

        fn unarchive_thread(
            &self,
            params: crate::ArchiveThreadParams,
        ) -> crate::ThreadStoreFuture<'_, StoredThread> {
            ThreadStore::unarchive_thread(self.inner.as_ref(), params)
        }

        fn delete_thread(
            &self,
            params: crate::DeleteThreadParams,
        ) -> crate::ThreadStoreFuture<'_, ()> {
            ThreadStore::delete_thread(self.inner.as_ref(), params)
        }
    }

    fn user_message_item(message: &str) -> RolloutItem {
        RolloutItem::EventMsg(EventMsg::UserMessage(UserMessageEvent {
            client_id: None,
            message: message.to_string(),
            images: None,
            local_images: Vec::new(),
            text_elements: Vec::new(),
            ..Default::default()
        }))
    }

    fn completed_assistant_turn(message: &str) -> Vec<RolloutItem> {
        vec![
            RolloutItem::EventMsg(EventMsg::AgentMessage(AgentMessageEvent {
                message: message.to_string(),
                phase: None,
                memory_citation: None,
            })),
            RolloutItem::EventMsg(EventMsg::TurnComplete(TurnCompleteEvent {
                turn_id: "title-turn".to_string(),
                last_agent_message: Some(message.to_string()),
                completed_at: None,
                duration_ms: None,
                time_to_first_token_ms: None,
            })),
        ]
    }

    fn create_params(thread_id: ThreadId) -> CreateThreadParams {
        CreateThreadParams {
            session_id: thread_id.into(),
            thread_id,
            extra_config: None,
            forked_from_id: None,
            parent_thread_id: None,
            source: SessionSource::Exec,
            thread_source: None,
            originator: "live-thread-checkpoint-test".to_string(),
            base_instructions: BaseInstructions::default(),
            dynamic_tools: Vec::new(),
            selected_capability_roots: Vec::new(),
            multi_agent_version: None,
            history_mode: ThreadHistoryMode::Legacy,
            initial_window_id: uuid::Uuid::now_v7().to_string(),
            metadata: ThreadPersistenceMetadata {
                cwd: None,
                model_provider: "test-provider".to_string(),
                memory_mode: ThreadMemoryMode::Enabled,
            },
        }
    }

    fn checkpoint(checkpoint_id: &str) -> CompactedItem {
        let info = TokenUsageInfo {
            total_token_usage: TokenUsage {
                total_tokens: 100,
                ..TokenUsage::default()
            },
            last_token_usage: TokenUsage {
                total_tokens: 25,
                ..TokenUsage::default()
            },
            model_context_window: Some(4_096),
        };
        let token_count = TokenCountEvent {
            info: Some(info),
            rate_limits: None,
        };
        CompactedItem {
            message: "summary".to_string(),
            replacement_history: Some(Vec::new()),
            window_number: Some(1),
            first_window_id: None,
            previous_window_id: None,
            window_id: None,
            checkpoint: Some(CompactionCheckpoint {
                checkpoint_id: checkpoint_id.to_string(),
                reference_context_item: None,
                world_state: None,
                api_token_count: token_count.clone(),
                final_token_count: token_count,
                server_reasoning_included: true,
            }),
        }
    }

    async fn live_thread() -> (Arc<InMemoryThreadStore>, LiveThread, ThreadId) {
        let thread_id = ThreadId::default();
        let store = Arc::new(InMemoryThreadStore::default());
        let live_thread = LiveThread::create(store.clone(), create_params(thread_id))
            .await
            .expect("create live thread");
        (store, live_thread, thread_id)
    }

    #[tokio::test]
    async fn checkpoint_append_reconciles_present_id_as_committed() {
        let (store, live_thread, thread_id) = live_thread().await;
        let checkpoint = checkpoint("checkpoint-present");
        store
            .fail_next_append_after_items(1, "ambiguous append after durable record")
            .await;

        let outcome = live_thread.append_compaction_checkpoint(&checkpoint).await;

        assert!(matches!(
            outcome,
            CompactionCheckpointAppendOutcome::Committed
        ));
        let history = store
            .load_history(LoadThreadHistoryParams {
                thread_id,
                include_archived: true,
            })
            .await
            .expect("load reconciled history");
        assert_eq!(
            history
                .items
                .iter()
                .filter(|item| matches!(
                    item,
                    RolloutItem::Compacted(item)
                        if item.checkpoint.as_ref().map(|checkpoint| checkpoint.checkpoint_id.as_str())
                            == Some("checkpoint-present")
                ))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn checkpoint_append_reconciles_absent_id_as_not_committed() {
        let (store, live_thread, thread_id) = live_thread().await;
        store
            .fail_next_append("failure before checkpoint write")
            .await;

        let outcome = live_thread
            .append_compaction_checkpoint(&checkpoint("checkpoint-absent"))
            .await;

        assert!(matches!(
            outcome,
            CompactionCheckpointAppendOutcome::NotCommitted { .. }
        ));
        let history = store
            .load_history(LoadThreadHistoryParams {
                thread_id,
                include_archived: true,
            })
            .await
            .expect("load history");
        assert!(history.items.iter().all(|item| !matches!(
            item,
            RolloutItem::Compacted(item) if item.checkpoint.is_some()
        )));
    }

    #[tokio::test]
    async fn checkpoint_append_reports_ambiguous_after_bounded_reconciliation() {
        let (store, live_thread, _thread_id) = live_thread().await;
        store.fail_next_append("ambiguous backing append").await;
        store
            .fail_next_history_loads(10, "history unavailable during reconciliation")
            .await;

        let outcome = live_thread
            .append_compaction_checkpoint(&checkpoint("checkpoint-unknown"))
            .await;

        assert!(matches!(
            outcome,
            CompactionCheckpointAppendOutcome::Ambiguous { .. }
        ));
    }

    #[tokio::test]
    async fn metadata_failure_after_durable_checkpoint_is_retryable_not_append_failure() {
        let (store, live_thread, _thread_id) = live_thread().await;
        store
            .fail_next_metadata_update("metadata projection unavailable")
            .await;

        let outcome = live_thread
            .append_compaction_checkpoint(&checkpoint("checkpoint-metadata"))
            .await;

        assert!(matches!(
            outcome,
            CompactionCheckpointAppendOutcome::Committed
        ));
        assert_eq!(store.calls().await.update_thread_metadata, 1);
        live_thread.persist().await.expect("retry pending metadata");
        assert_eq!(store.calls().await.update_thread_metadata, 2);
    }

    #[tokio::test]
    async fn generated_title_holds_mutation_permit_across_read_and_update() {
        let permit_held = Arc::new(AtomicBool::new(false));
        let acquire_attempted = Arc::new(AtomicBool::new(false));
        let permit_released = Arc::new(AtomicBool::new(false));
        let gate = Arc::new(TrackingMutationGate {
            allow: true,
            acquire_attempted: Arc::clone(&acquire_attempted),
            permit_held: Arc::clone(&permit_held),
            permit_released: Arc::clone(&permit_released),
        });
        let store = Arc::new(PermitObservingThreadStore::new(Arc::clone(&permit_held)));
        store.set_title_generator(Arc::new(StubTitleGenerator));
        let thread_id = ThreadId::default();
        let live_thread = LiveThread::create(store.clone(), create_params(thread_id))
            .await
            .expect("create live thread")
            .with_metadata_mutation_gate(gate);

        live_thread
            .append_items(&[user_message_item("opening question")])
            .await
            .expect("append first user message");
        let updates_before_title = store.inner.calls().await.update_thread_metadata;
        live_thread
            .append_items(&completed_assistant_turn("opening answer"))
            .await
            .expect("append completed assistant turn");

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !store.title_update_seen.load(Ordering::SeqCst)
                || !permit_released.load(Ordering::SeqCst)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("generated title update should complete");

        assert!(acquire_attempted.load(Ordering::SeqCst));
        assert!(store.title_read_while_held.load(Ordering::SeqCst));
        assert!(store.title_update_while_held.load(Ordering::SeqCst));
        assert!(permit_released.load(Ordering::SeqCst));
        assert!(!permit_held.load(Ordering::SeqCst));
        assert!(
            store.inner.calls().await.update_thread_metadata > updates_before_title,
            "the guarded title path must perform an actual metadata update"
        );
    }

    #[tokio::test]
    async fn denied_title_mutation_permit_skips_store_access() {
        let permit_held = Arc::new(AtomicBool::new(false));
        let acquire_attempted = Arc::new(AtomicBool::new(false));
        let permit_released = Arc::new(AtomicBool::new(false));
        let gate = Arc::new(TrackingMutationGate {
            allow: false,
            acquire_attempted: Arc::clone(&acquire_attempted),
            permit_held: Arc::clone(&permit_held),
            permit_released: Arc::clone(&permit_released),
        });
        let store = Arc::new(PermitObservingThreadStore::new(Arc::clone(&permit_held)));
        store.set_title_generator(Arc::new(StubTitleGenerator));
        let thread_id = ThreadId::default();
        let live_thread = LiveThread::create(store.clone(), create_params(thread_id))
            .await
            .expect("create live thread")
            .with_metadata_mutation_gate(gate);

        live_thread
            .append_items(&[user_message_item("opening question")])
            .await
            .expect("append first user message");
        let calls_before_title = store.inner.calls().await;
        live_thread
            .append_items(&completed_assistant_turn("opening answer"))
            .await
            .expect("append completed assistant turn");

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !acquire_attempted.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("detached title task should consult the mutation gate");
        tokio::task::yield_now().await;

        let calls_after_title = store.inner.calls().await;
        assert_eq!(
            calls_after_title.read_thread,
            calls_before_title.read_thread
        );
        assert!(!store.title_update_seen.load(Ordering::SeqCst));
        assert!(!permit_held.load(Ordering::SeqCst));
        assert!(!permit_released.load(Ordering::SeqCst));
    }
}

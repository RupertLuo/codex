use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use codex_protocol::ThreadId;
use codex_protocol::items::TurnItem;
use codex_protocol::items::UserMessageItem;
use codex_protocol::protocol::AgentMessageEvent;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::ItemCompletedEvent;
use codex_protocol::protocol::ThreadHistoryMode;
use codex_protocol::user_input::UserInput;
use codex_rollout::RolloutItem;
use codex_state::StateRuntime;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::Duration;
use tokio::time::timeout;

use super::test_support::test_config;
use super::tests::create_thread_params;
use super::tests::user_message_item;
use crate::ArchiveThreadParams;
use crate::ListThreadsParams;
use crate::LiveThread;
use crate::LocalThreadStore;
use crate::PersistContext;
use crate::ReadThreadParams;
use crate::SortDirection;
use crate::ThreadMetadataMutationGate;
use crate::ThreadMetadataMutationPermit;
use crate::ThreadMetadataMutationPermitFuture;
use crate::ThreadMetadataPatch;
use crate::ThreadSortKey;
use crate::ThreadStore;
use crate::ThreadTitleGenerator;
use crate::ThreadTitleRequest;
use crate::UpdateThreadMetadataParams;

#[derive(Debug)]
struct ControlledTitleGenerator {
    request_tx: Mutex<Option<oneshot::Sender<ThreadTitleRequest>>>,
    release: Arc<Notify>,
    result: Option<String>,
}

impl ThreadTitleGenerator for ControlledTitleGenerator {
    fn generate_title<'a>(
        &'a self,
        request: ThreadTitleRequest,
    ) -> Pin<Box<dyn Future<Output = Option<String>> + Send + 'a>> {
        Box::pin(async move {
            if let Some(request_tx) = self.request_tx.lock().await.take() {
                let _ = request_tx.send(request);
            }
            self.release.notified().await;
            self.result.clone()
        })
    }
}

#[derive(Debug)]
struct TestPermit {
    finished: Arc<Notify>,
}

impl Drop for TestPermit {
    fn drop(&mut self) {
        self.finished.notify_one();
    }
}

impl ThreadMetadataMutationPermit for TestPermit {}

#[derive(Debug)]
struct TestMutationGate {
    allow: bool,
    acquired: Arc<Notify>,
    finished: Arc<Notify>,
    title_tx: mpsc::UnboundedSender<String>,
}

impl ThreadMetadataMutationGate for TestMutationGate {
    fn acquire<'a>(&'a self) -> ThreadMetadataMutationPermitFuture<'a> {
        let allow = self.allow;
        let acquired = Arc::clone(&self.acquired);
        let finished = Arc::clone(&self.finished);
        Box::pin(async move {
            acquired.notify_one();
            if allow {
                Some(Box::new(TestPermit { finished }) as Box<dyn ThreadMetadataMutationPermit>)
            } else {
                finished.notify_one();
                None
            }
        })
    }

    fn title_updated(&self, title: String) {
        let _ = self.title_tx.send(title);
    }
}

fn assistant_message_item(message: &str) -> RolloutItem {
    RolloutItem::EventMsg(codex_protocol::protocol::EventMsg::AgentMessage(
        AgentMessageEvent {
            message: message.to_string(),
            phase: None,
            memory_citation: None,
            delivery: None,
            questions: None,
        },
    ))
}

async fn test_store() -> (TempDir, Arc<StateRuntime>, Arc<LocalThreadStore>) {
    let home = TempDir::new().expect("temp dir");
    let config = test_config(home.path());
    let runtime = StateRuntime::init(
        config.sqlite.clone(),
        config.default_model_provider_id.clone(),
    )
    .await
    .expect("state db should initialize");
    let store = Arc::new(LocalThreadStore::new(config, Some(runtime.clone())));
    (home, runtime, store)
}

#[tokio::test]
async fn llm_title_generation_is_detached_and_notifies_after_persist() {
    let thread_id = ThreadId::default();
    let (_home, runtime, store) = test_store().await;
    let (request_tx, request_rx) = oneshot::channel();
    let release = Arc::new(Notify::new());
    let generator = Arc::new(ControlledTitleGenerator {
        request_tx: Mutex::new(Some(request_tx)),
        release: Arc::clone(&release),
        result: Some("\n\"Generated title.\"\nignored".to_string()),
    });
    let (title_tx, mut title_rx) = mpsc::unbounded_channel();
    let finished = Arc::new(Notify::new());
    let gate = Arc::new(TestMutationGate {
        allow: true,
        acquired: Arc::new(Notify::new()),
        finished: Arc::clone(&finished),
        title_tx,
    });
    store.set_title_generator(generator);
    let live_thread = LiveThread::create(store, create_thread_params(thread_id))
        .await
        .expect("create live thread")
        .with_metadata_mutation_gate(gate);

    live_thread
        .append_items(&[
            user_message_item("first user request"),
            assistant_message_item("first assistant response"),
        ])
        .await
        .expect("append");
    let request = timeout(Duration::from_secs(2), request_rx)
        .await
        .expect("generator should start without blocking append")
        .expect("title request");
    assert_eq!(request.first_user_message, "first user request");
    assert_eq!(
        request.first_assistant_message.as_deref(),
        Some("first assistant response")
    );

    release.notify_one();
    let title = timeout(Duration::from_secs(2), title_rx.recv())
        .await
        .expect("title callback")
        .expect("callback title");
    assert_eq!(title, "Generated title");
    timeout(Duration::from_secs(2), finished.notified())
        .await
        .expect("metadata mutation should finish");

    let metadata = runtime
        .get_thread(thread_id)
        .await
        .expect("metadata read")
        .expect("metadata");
    assert_eq!(metadata.title, "Generated title");
}

#[tokio::test]
async fn paginated_generated_title_survives_read_list_and_unarchive() {
    let thread_id = ThreadId::default();
    let (_home, runtime, store) = test_store().await;
    let mut params = create_thread_params(thread_id);
    params.history_mode = ThreadHistoryMode::Paginated;
    let live_thread = LiveThread::create(store.clone(), params)
        .await
        .expect("create paginated live thread");
    live_thread
        .append_items(&[RolloutItem::EventMsg(EventMsg::ItemCompleted(
            ItemCompletedEvent {
                thread_id,
                turn_id: "turn-1".to_string(),
                item: TurnItem::UserMessage(UserMessageItem {
                    id: "item-1".to_string(),
                    client_id: None,
                    content: vec![UserInput::Text {
                        text: "first user request".to_string(),
                        text_elements: Vec::new(),
                    }],
                }),
                started_at_ms: Some(0),
                completed_at_ms: 1,
            },
        ))])
        .await
        .expect("append");
    live_thread
        .persist(PersistContext::Standard)
        .await
        .expect("persist thread");
    live_thread.shutdown().await.expect("release live writer");
    // Title generation persists this patch; discovery must recover it without its live event.
    store
        .update_thread_metadata(UpdateThreadMetadataParams {
            thread_id,
            patch: ThreadMetadataPatch {
                title: Some("Generated title".to_string()),
                ..Default::default()
            },
            include_archived: false,
        })
        .await
        .expect("persist generated title");

    let metadata = runtime
        .get_thread(thread_id)
        .await
        .expect("metadata read")
        .expect("metadata");
    assert_eq!(metadata.title, "Generated title");
    assert_eq!(metadata.name, None);
    assert_eq!(metadata.preview.as_deref(), Some("first user request"));

    for expected_name in ["Generated title", "Manual title"] {
        if expected_name == "Manual title" {
            store
                .update_thread_metadata(UpdateThreadMetadataParams {
                    thread_id,
                    patch: ThreadMetadataPatch {
                        name: Some(Some(expected_name.to_string())),
                        ..Default::default()
                    },
                    include_archived: false,
                })
                .await
                .expect("explicit name overrides the generated title");
        }
        let thread = store
            .read_thread(ReadThreadParams {
                thread_id,
                include_archived: false,
                include_history: false,
            })
            .await
            .expect("read thread");
        assert_eq!(thread.name.as_deref(), Some(expected_name));
        let page = store
            .list_threads(ListThreadsParams {
                page_size: 10,
                cursor: None,
                sort_key: ThreadSortKey::CreatedAt,
                sort_direction: SortDirection::Desc,
                allowed_sources: Vec::new(),
                model_providers: None,
                cwd_filters: None,
                section: None,
                project_id: None,
                archived: false,
                search_term: None,
                relation_filter: None,
                use_state_db_only: true,
            })
            .await
            .expect("list thread");
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].name.as_deref(), Some(expected_name));
        store
            .archive_thread(ArchiveThreadParams { thread_id })
            .await
            .expect("archive thread");
        let restored = store
            .unarchive_thread(ArchiveThreadParams { thread_id })
            .await
            .expect("unarchive thread");
        assert_eq!(restored.name.as_deref(), Some(expected_name));
    }
}

#[tokio::test]
async fn manual_thread_rename_wins_over_in_flight_llm_title() {
    let thread_id = ThreadId::default();
    let (_home, runtime, store) = test_store().await;
    let (request_tx, request_rx) = oneshot::channel();
    let release = Arc::new(Notify::new());
    let generator = Arc::new(ControlledTitleGenerator {
        request_tx: Mutex::new(Some(request_tx)),
        release: Arc::clone(&release),
        result: Some("LLM generated title".to_string()),
    });
    let (title_tx, mut title_rx) = mpsc::unbounded_channel();
    let finished = Arc::new(Notify::new());
    let gate = Arc::new(TestMutationGate {
        allow: true,
        acquired: Arc::new(Notify::new()),
        finished: Arc::clone(&finished),
        title_tx,
    });
    store.set_title_generator(generator);
    let live_thread = LiveThread::create(store, create_thread_params(thread_id))
        .await
        .expect("create live thread")
        .with_metadata_mutation_gate(gate);

    live_thread
        .append_items(&[
            user_message_item("first user request"),
            assistant_message_item("first assistant response"),
        ])
        .await
        .expect("append");
    timeout(Duration::from_secs(2), request_rx)
        .await
        .expect("generator should start")
        .expect("title request");
    live_thread
        .update_metadata(
            ThreadMetadataPatch {
                name: Some(Some("Manual title".to_string())),
                ..Default::default()
            },
            /*include_archived*/ false,
        )
        .await
        .expect("manual rename");

    release.notify_one();
    timeout(Duration::from_secs(2), finished.notified())
        .await
        .expect("metadata mutation should finish");
    assert!(title_rx.try_recv().is_err());

    let metadata = runtime
        .get_thread(thread_id)
        .await
        .expect("metadata read")
        .expect("metadata");
    assert_eq!(metadata.title, "Manual title");
}

#[tokio::test]
async fn rejected_metadata_mutation_gate_skips_llm_title_and_callback() {
    let thread_id = ThreadId::default();
    let (_home, runtime, store) = test_store().await;
    let (request_tx, request_rx) = oneshot::channel();
    let release = Arc::new(Notify::new());
    let generator = Arc::new(ControlledTitleGenerator {
        request_tx: Mutex::new(Some(request_tx)),
        release: Arc::clone(&release),
        result: Some("Rejected title".to_string()),
    });
    let (title_tx, mut title_rx) = mpsc::unbounded_channel();
    let acquired = Arc::new(Notify::new());
    let finished = Arc::new(Notify::new());
    let gate = Arc::new(TestMutationGate {
        allow: false,
        acquired: Arc::clone(&acquired),
        finished: Arc::clone(&finished),
        title_tx,
    });
    store.set_title_generator(generator);
    let live_thread = LiveThread::create(store, create_thread_params(thread_id))
        .await
        .expect("create live thread")
        .with_metadata_mutation_gate(gate);

    live_thread
        .append_items(&[
            user_message_item("first user request"),
            assistant_message_item("first assistant response"),
        ])
        .await
        .expect("append");
    timeout(Duration::from_secs(2), request_rx)
        .await
        .expect("generator should start")
        .expect("title request");
    release.notify_one();
    timeout(Duration::from_secs(2), acquired.notified())
        .await
        .expect("mutation gate should be consulted");
    timeout(Duration::from_secs(2), finished.notified())
        .await
        .expect("rejected mutation should finish");
    assert!(title_rx.try_recv().is_err());

    let metadata = runtime
        .get_thread(thread_id)
        .await
        .expect("metadata read")
        .expect("metadata");
    assert_eq!(metadata.title, "first user request");
}

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use codex_protocol::ThreadId;
use codex_protocol::protocol::AgentMessageEvent;
use codex_rollout::RolloutItem;
use codex_state::StateRuntime;
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
use crate::LiveThread;
use crate::LocalThreadStore;
use crate::ThreadMetadataMutationGate;
use crate::ThreadMetadataMutationPermit;
use crate::ThreadMetadataMutationPermitFuture;
use crate::ThreadMetadataPatch;
use crate::ThreadStore;
use crate::ThreadTitleGenerator;
use crate::ThreadTitleRequest;

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

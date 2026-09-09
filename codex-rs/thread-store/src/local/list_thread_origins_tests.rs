use std::collections::HashMap;
use std::fs;

use chrono::Utc;
use codex_protocol::ThreadId;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::ThreadSource;
use codex_utils_absolute_path::test_support::PathExt;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use uuid::Uuid;

use super::LocalThreadStore;
use super::test_support::test_config;
use super::test_support::write_archived_session_file;
use super::test_support::write_session_file;
use crate::ListThreadsParams;
use crate::SortDirection;
use crate::ThreadSortKey;
use crate::ThreadStore;

const SOURCES: [Option<&str>; 4] = [
    Some("catalyst:ppt-pro"),
    Some("catalyst:quickstart:industry-report"),
    Some("catalyst:meeting:meeting-summary"),
    None,
];

#[tokio::test]
async fn cold_state_db_listing_preserves_origins_across_pages_and_archive() {
    for archived in [false, true] {
        let home = TempDir::new().unwrap();
        let config = test_config(home.path());
        let runtime = codex_state::StateRuntime::init(
            codex_state::SqliteConfig::new_for_testing(home.path().abs()),
            config.default_model_provider_id.clone(),
        )
        .await
        .unwrap();
        runtime
            .mark_backfill_complete(/*last_watermark*/ None)
            .await
            .unwrap();
        let mut expected = HashMap::new();
        for (index, source) in SOURCES.into_iter().enumerate() {
            let id =
                ThreadId::from_string(&Uuid::from_u128(index as u128 + 1).to_string()).unwrap();
            // Empty history proves that classification comes from persisted metadata.
            let rollout_path = home.path().join(format!("empty-history-{index}.jsonl"));
            fs::write(&rollout_path, "").unwrap();
            let mut metadata = codex_state::ThreadMetadataBuilder::new(
                id,
                rollout_path,
                Utc::now() + chrono::Duration::seconds(index as i64),
                SessionSource::Cli,
            )
            .build(&config.default_model_provider_id);
            metadata.first_user_message = Some("Saved conversation".to_owned());
            metadata.preview = metadata.first_user_message.clone();
            metadata.thread_source = source.map(|source| ThreadSource::Feature(source.to_owned()));
            metadata.archived_at = archived.then(Utc::now);
            expected.insert(id, metadata.thread_source.clone());
            runtime.upsert_thread(&metadata).await.unwrap();
        }
        runtime.close().await;
        drop(runtime);

        let runtime = codex_state::StateRuntime::init(
            codex_state::SqliteConfig::new_for_testing(home.path().abs()),
            config.default_model_provider_id.clone(),
        )
        .await
        .unwrap();
        let store = LocalThreadStore::new(config, Some(runtime.clone()));
        let mut params = list_params();
        params.use_state_db_only = true;
        params.archived = archived;
        params.page_size = 2;
        let mut actual = HashMap::new();
        for page_index in 0..2 {
            let page = store.list_threads(params.clone()).await.unwrap();
            assert_eq!(
                page.items.len(),
                2,
                "archived={archived}, page={page_index}"
            );
            for thread in page.items {
                assert!(thread.history.is_none());
                actual.insert(thread.thread_id, thread.thread_source);
            }
            assert_eq!(page.next_cursor.is_some(), page_index == 0);
            params.cursor = page.next_cursor;
        }
        assert_eq!(actual, expected);
        runtime.close().await;
    }
}

#[tokio::test]
async fn filesystem_listing_preserves_session_origins_without_a_state_db() {
    for archived in [false, true] {
        let home = TempDir::new().unwrap();
        let mut expected = HashMap::new();
        for (index, source) in SOURCES.into_iter().enumerate() {
            let uuid = Uuid::from_u128(index as u128 + 1);
            let path = if archived {
                write_archived_session_file(home.path(), "2025-01-03T12-00-00", uuid)
            } else {
                write_session_file(home.path(), "2025-01-03T12-00-00", uuid)
            }
            .unwrap();
            let content = fs::read_to_string(&path).unwrap();
            let (head, tail) = content.split_once('\n').unwrap();
            let mut head: serde_json::Value = serde_json::from_str(head).unwrap();
            head["payload"]["thread_source"] = serde_json::json!(source);
            fs::write(path, format!("{head}\n{tail}")).unwrap();
            expected.insert(
                ThreadId::from_string(&uuid.to_string()).unwrap(),
                source.map(|source| ThreadSource::Feature(source.to_owned())),
            );
        }
        let store = LocalThreadStore::new(test_config(home.path()), /*state_db*/ None);
        let mut params = list_params();
        params.archived = archived;
        let page = store.list_threads(params).await.unwrap();
        assert_eq!(page.next_cursor, None);
        let actual = page
            .items
            .into_iter()
            .map(|thread| (thread.thread_id, thread.thread_source))
            .collect::<HashMap<_, _>>();
        assert_eq!(actual, expected);
    }
}

fn list_params() -> ListThreadsParams {
    ListThreadsParams {
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
        use_state_db_only: false,
    }
}

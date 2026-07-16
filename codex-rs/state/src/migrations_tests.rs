use std::borrow::Cow;
use std::fs;
use std::path::Path;

use pretty_assertions::assert_eq;
use sqlx::AssertSqlSafe;
use sqlx::Row;
use sqlx::SqlSafeStr;
use sqlx::migrate::Migration;
use sqlx::migrate::Migrator;
use sqlx::sqlite::SqlitePoolOptions;

use super::GOALS_MIGRATOR;
use super::LOGS_MIGRATOR;
use super::MEMORIES_MIGRATOR;
use super::STATE_MIGRATOR;
use super::repair_legacy_crlf_migration_checksums;
use super::repair_legacy_recency_migration_version;

#[test]
fn migration_sources_are_lf_normalized_and_match_embedded_checksums() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let attributes = fs::read_to_string(manifest_dir.join("../../.gitattributes"))
        .expect("workspace .gitattributes should be readable");
    for required_rule in [
        "codex-rs/state/migrations/*.sql text eol=lf",
        "codex-rs/state/*_migrations/*.sql text eol=lf",
    ] {
        assert!(
            attributes.lines().any(|line| line.trim() == required_rule),
            "missing migration line-ending rule: {required_rule}"
        );
    }

    for (directory, migrator) in [
        ("migrations", &STATE_MIGRATOR),
        ("logs_migrations", &LOGS_MIGRATOR),
        ("goals_migrations", &GOALS_MIGRATOR),
        ("memory_migrations", &MEMORIES_MIGRATOR),
    ] {
        for embedded in migrator.migrations.iter() {
            let prefix = format!("{:04}_", embedded.version);
            let path = fs::read_dir(manifest_dir.join(directory))
                .expect("migration directory should be readable")
                .map(|entry| entry.expect("migration entry should be readable").path())
                .find(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".sql"))
                })
                .unwrap_or_else(|| {
                    panic!("missing migration source for version {}", embedded.version)
                });
            let bytes = fs::read(&path).expect("migration source should be readable");
            assert!(
                !bytes.windows(2).any(|window| window == b"\r\n"),
                "migration source must use LF line endings: {}",
                path.display()
            );

            let sql = String::from_utf8(bytes).expect("migration source should be UTF-8");
            let normalized = Migration::new(
                embedded.version,
                embedded.description.clone(),
                embedded.migration_type,
                AssertSqlSafe(sql.replace("\r\n", "\n")).into_sql_str(),
                embedded.no_tx,
            );
            assert_eq!(
                embedded.checksum,
                normalized.checksum,
                "embedded checksum must be derived from LF-normalized SQL: {}",
                path.display()
            );
        }
    }
}

fn migrator_through(version: i64) -> Migrator {
    Migrator {
        migrations: Cow::Owned(
            STATE_MIGRATOR
                .migrations
                .iter()
                .filter(|migration| migration.version <= version)
                .cloned()
                .collect(),
        ),
        ignore_missing: STATE_MIGRATOR.ignore_missing,
        locking: STATE_MIGRATOR.locking,
        table_name: STATE_MIGRATOR.table_name.clone(),
        create_schemas: STATE_MIGRATOR.create_schemas.clone(),
        no_tx: STATE_MIGRATOR.no_tx,
    }
}

fn migrator_with_crlf_line_endings_through(base: &Migrator, version: i64) -> Migrator {
    Migrator {
        migrations: Cow::Owned(
            base.migrations
                .iter()
                .filter(|migration| migration.version <= version)
                .map(|migration| {
                    Migration::new(
                        migration.version,
                        migration.description.clone(),
                        migration.migration_type,
                        AssertSqlSafe(migration.sql.as_str().replace('\n', "\r\n")).into_sql_str(),
                        migration.no_tx,
                    )
                })
                .collect(),
        ),
        ignore_missing: base.ignore_missing,
        locking: base.locking,
        table_name: base.table_name.clone(),
        create_schemas: base.create_schemas.clone(),
        no_tx: base.no_tx,
    }
}

async fn applied_migration_checksums(pool: &sqlx::SqlitePool) -> Vec<(i64, Vec<u8>)> {
    sqlx::query_as("SELECT version, checksum FROM _sqlx_migrations ORDER BY version")
        .fetch_all(pool)
        .await
        .expect("applied migration checksums should load")
}

#[tokio::test]
async fn repairs_crlf_migration_checksums_and_preserves_state_data() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("in-memory database should open");
    migrator_with_crlf_line_endings_through(&STATE_MIGRATOR, /*version*/ 29)
        .run(&pool)
        .await
        .expect("legacy CRLF migrations should apply");
    sqlx::query(
        r#"
INSERT INTO threads (
    id,
    rollout_path,
    created_at,
    updated_at,
    source,
    model_provider,
    cwd,
    title,
    sandbox_policy,
    approval_mode
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind("legacy-thread")
    .bind("/tmp/legacy.jsonl")
    .bind(1_700_000_000_i64)
    .bind(1_700_000_100_i64)
    .bind("cli")
    .bind("openai")
    .bind("/tmp")
    .bind("Legacy thread")
    .bind("read-only")
    .bind("on-request")
    .execute(&pool)
    .await
    .expect("legacy thread should insert");

    repair_legacy_crlf_migration_checksums(&pool, &STATE_MIGRATOR)
        .await
        .expect("legacy CRLF checksums should be repaired");
    STATE_MIGRATOR
        .run(&pool)
        .await
        .expect("current migrations should apply after repair");

    let thread_ids = sqlx::query_scalar::<_, String>("SELECT id FROM threads ORDER BY id")
        .fetch_all(&pool)
        .await
        .expect("threads should load");
    assert_eq!(thread_ids, vec!["legacy-thread"]);
    let expected_checksums = STATE_MIGRATOR
        .migrations
        .iter()
        .map(|migration| (migration.version, migration.checksum.as_ref().to_vec()))
        .collect::<Vec<_>>();
    assert_eq!(applied_migration_checksums(&pool).await, expected_checksums);
}

#[tokio::test]
async fn repairs_crlf_migration_checksums_and_preserves_logs() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("in-memory database should open");
    migrator_with_crlf_line_endings_through(&LOGS_MIGRATOR, i64::MAX)
        .run(&pool)
        .await
        .expect("legacy CRLF log migrations should apply");
    sqlx::query(
        "INSERT INTO logs (ts, ts_nanos, level, target, feedback_log_body) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(1_700_000_000_i64)
    .bind(123_i64)
    .bind("INFO")
    .bind("codex_test")
    .bind("legacy log")
    .execute(&pool)
    .await
    .expect("legacy log should insert");

    repair_legacy_crlf_migration_checksums(&pool, &LOGS_MIGRATOR)
        .await
        .expect("legacy CRLF checksums should be repaired");
    LOGS_MIGRATOR
        .run(&pool)
        .await
        .expect("current log migrations should validate after repair");

    let logs = sqlx::query_as::<_, (String, String)>(
        "SELECT target, feedback_log_body FROM logs ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .expect("logs should load");
    assert_eq!(
        logs,
        vec![("codex_test".to_string(), "legacy log".to_string())]
    );
    let expected_checksums = LOGS_MIGRATOR
        .migrations
        .iter()
        .map(|migration| (migration.version, migration.checksum.as_ref().to_vec()))
        .collect::<Vec<_>>();
    assert_eq!(applied_migration_checksums(&pool).await, expected_checksums);
}

#[tokio::test]
async fn does_not_repair_unrecognized_migration_checksum() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("in-memory database should open");
    migrator_through(/*version*/ 1)
        .run(&pool)
        .await
        .expect("initial migration should apply");
    let unknown_checksum = vec![0x5a; 48];
    sqlx::query("UPDATE _sqlx_migrations SET checksum = ? WHERE version = 1")
        .bind(&unknown_checksum)
        .execute(&pool)
        .await
        .expect("migration checksum should be replaced for the test");

    repair_legacy_crlf_migration_checksums(&pool, &STATE_MIGRATOR)
        .await
        .expect("unrecognized checksum should be left for SQLx to validate");

    assert_eq!(
        applied_migration_checksums(&pool).await,
        vec![(1, unknown_checksum)]
    );
    assert!(STATE_MIGRATOR.run(&pool).await.is_err());
}

#[tokio::test]
async fn recency_migration_backfills_and_seeds_old_binary_inserts() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("in-memory database should open");
    migrator_through(/*version*/ 37)
        .run(&pool)
        .await
        .expect("pre-recency migrations should apply");

    sqlx::query(
        r#"
INSERT INTO threads (
    id,
    rollout_path,
    created_at,
    updated_at,
    created_at_ms,
    updated_at_ms,
    source,
    model_provider,
    cwd,
    title,
    sandbox_policy,
    approval_mode
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind("00000000-0000-0000-0000-000000000001")
    .bind("/tmp/first.jsonl")
    .bind(1_700_000_000_i64)
    .bind(1_700_000_100_i64)
    .bind(1_700_000_000_123_i64)
    .bind(1_700_000_100_456_i64)
    .bind("cli")
    .bind("openai")
    .bind("/tmp")
    .bind("")
    .bind("read-only")
    .bind("on-request")
    .execute(&pool)
    .await
    .expect("legacy row should insert");

    STATE_MIGRATOR
        .run(&pool)
        .await
        .expect("recency migration should apply");

    let backfilled = sqlx::query(
        "SELECT updated_at, updated_at_ms, recency_at, recency_at_ms FROM threads WHERE id = ?",
    )
    .bind("00000000-0000-0000-0000-000000000001")
    .fetch_one(&pool)
    .await
    .expect("backfilled row should load");
    assert_eq!(backfilled.get::<i64, _>("recency_at"), 1_700_000_100);
    assert_eq!(backfilled.get::<i64, _>("recency_at_ms"), 1_700_000_100_456);

    sqlx::query(
        r#"
INSERT INTO threads (
    id,
    rollout_path,
    created_at,
    updated_at,
    created_at_ms,
    updated_at_ms,
    source,
    model_provider,
    cwd,
    title,
    sandbox_policy,
    approval_mode
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind("00000000-0000-0000-0000-000000000002")
    .bind("/tmp/second.jsonl")
    .bind(1_700_000_200_i64)
    .bind(1_700_000_300_i64)
    .bind(1_700_000_200_123_i64)
    .bind(1_700_000_300_456_i64)
    .bind("cli")
    .bind("openai")
    .bind("/tmp")
    .bind("")
    .bind("read-only")
    .bind("on-request")
    .execute(&pool)
    .await
    .expect("old-binary row should insert");

    let seeded = sqlx::query("SELECT recency_at, recency_at_ms FROM threads WHERE id = ?")
        .bind("00000000-0000-0000-0000-000000000002")
        .fetch_one(&pool)
        .await
        .expect("old-binary row should load");
    assert_eq!(seeded.get::<i64, _>("recency_at"), 1_700_000_300);
    assert_eq!(seeded.get::<i64, _>("recency_at_ms"), 1_700_000_300_456);
}

#[tokio::test]
async fn repairs_recency_migration_that_was_applied_as_version_38() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("in-memory database should open");
    migrator_through(/*version*/ 37)
        .run(&pool)
        .await
        .expect("pre-recency migrations should apply");

    let recency_migration = STATE_MIGRATOR
        .migrations
        .iter()
        .find(|migration| migration.version == 39)
        .expect("recency migration should exist");
    let mut legacy_migrations = STATE_MIGRATOR
        .migrations
        .iter()
        .filter(|migration| migration.version <= 37)
        .cloned()
        .collect::<Vec<_>>();
    legacy_migrations.push(Migration::new(
        38,
        recency_migration.description.clone(),
        recency_migration.migration_type,
        recency_migration.sql.clone(),
        recency_migration.no_tx,
    ));
    let legacy_recency_migrator = Migrator::with_migrations(legacy_migrations);
    legacy_recency_migrator
        .run(&pool)
        .await
        .expect("legacy recency migration should apply as version 38");

    repair_legacy_recency_migration_version(&pool, &STATE_MIGRATOR)
        .await
        .expect("legacy migration history should be repaired");
    STATE_MIGRATOR
        .run(&pool)
        .await
        .expect("current migrations should apply after repair");

    let applied = sqlx::query(
        "SELECT version, checksum FROM _sqlx_migrations WHERE version >= 38 ORDER BY version",
    )
    .fetch_all(&pool)
    .await
    .expect("applied migrations should load")
    .into_iter()
    .map(|row| {
        (
            row.get::<i64, _>("version"),
            row.get::<Vec<u8>, _>("checksum"),
        )
    })
    .collect::<Vec<_>>();
    let expected = STATE_MIGRATOR
        .migrations
        .iter()
        .filter(|migration| migration.version >= 38)
        .map(|migration| (migration.version, migration.checksum.to_vec()))
        .collect::<Vec<_>>();
    assert_eq!(applied, expected);
}

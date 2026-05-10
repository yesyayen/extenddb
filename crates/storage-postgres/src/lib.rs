// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0

//! `PostgreSQL` storage backend for extenddb.
//!
//! Implements the `TableEngine` and `DataEngine` traits from `extenddb-storage`
//! using `PostgreSQL` via `sqlx`. All SQL uses parameterized queries exclusively
//! — no dynamic SQL, except for per-DynamoDB-table DDL where table names are
//! validated at the engine layer.

mod admin_store;
mod authorization_store;
mod backup_engine;
mod bootstrapper;
mod catalog_store;
pub mod config;
mod create_table;
mod credential_store;
mod data;
mod delete_table;
pub(crate) mod gsi_queue;
mod management_store;
mod metadata_engine;
mod migrations;
mod pg_util;
mod stream_engine;
mod table_engine;
mod table_helpers;
mod update_table;
mod worker_store;

pub use bootstrapper::PostgresBootstrapper;
pub use catalog_store::PostgresCatalogStore;
pub use config::PostgresStorageConfig;
pub use config::parse_connection_string;
pub use credential_store::DbCredentialStore;

// Auto-register the Postgres backend at compile time
inventory::submit! {
    extenddb_storage::bootstrapper::BackendRegistration {
        name: "postgres",
        factory: |config_path, cli_args| {
            Box::pin(async move {
                let store = PostgresBootstrapper::from_config(&config_path, &cli_args).await?;
                Ok(Box::new(store) as Box<dyn extenddb_storage::bootstrapper::Bootstrapper>)
            })
        }
    }
}

use std::sync::Arc;

use extenddb_core::version::CatalogVersion;
use extenddb_storage::error::StorageError;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

/// Expected catalog version — compiled into the binary (REQ-CAT-006, D-9).
///
/// The tuple is the single source of truth. Use `CATALOG_VERSION.to_string()`
/// wherever a string representation is needed.
pub const CATALOG_VERSION: CatalogVersion = CatalogVersion::new(0, 0, 2);

/// `PostgreSQL` storage backend configuration.
pub struct PostgresConfig {
    pub connection_string: String,
    pub pool_size: u32,
    /// Maximum item size in bytes for post-update validation.
    pub max_item_size_bytes: usize,
}

/// `PostgreSQL` storage backend.
///
/// The engine no longer stores a single `account_id`. Instead, `account_id`
/// is passed per-request through the storage trait methods, enabling
/// multi-account isolation (Phase 12f).
///
/// Uses two connection pools: `pool` for catalog metadata (tables, indexes,
/// settings, accounts, IAM) and `data_pool` for per-DynamoDB-table data
/// (`_ddb_*` tables, GSI tables). This separation allows the catalog and
/// data to live in different PostgreSQL databases (Bug 1, P54).
pub struct PostgresEngine {
    pub(crate) pool: PgPool,
    /// Connection pool for the data database where `_ddb_*` tables live.
    pub(crate) data_pool: PgPool,
    pub(crate) region: String,
    pub(crate) max_item_size_bytes: usize,
    /// F-3: Wakes the control plane poller when a table enters CREATING or
    /// DELETING state, so transitions are processed without polling delay.
    pub(crate) control_plane_notify: Arc<tokio::sync::Notify>,
    /// D-4: Async GSI update queue. `None` until `start_gsi_workers()` is called.
    pub(crate) gsi_queue: Option<Arc<gsi_queue::GsiQueue>>,
    /// P119: Cached GSI default propagation delay (milliseconds). Updated by
    /// background poller every 30s. Avoids per-request DB query on write path.
    pub gsi_default_delay_ms: Arc<std::sync::atomic::AtomicU64>,
}

impl PostgresEngine {
    pub async fn new(config: &PostgresConfig, region: &str) -> Result<Self, StorageError> {
        // P79/P6: Set min_connections to avoid cold-start latency on first requests.
        let min_conns = config.pool_size.min(2);
        let pool = PgPoolOptions::new()
            .max_connections(config.pool_size)
            .min_connections(min_conns)
            .connect(&config.connection_string)
            .await
            .map_err(|e| StorageError::Connection(e.to_string()))?;

        // P54 Bug 1: Read data database connection string from catalog settings.
        // Falls back to the catalog pool if no separate data database is configured.
        let data_pool = match sqlx::query_as::<_, (String,)>(
            "SELECT value FROM settings WHERE key = 'data_database_connection_string'",
        )
        .fetch_optional(&pool)
        .await
        {
            Ok(Some((data_conn,))) if !data_conn.is_empty() => PgPoolOptions::new()
                .max_connections(config.pool_size)
                .min_connections(min_conns)
                .connect(&data_conn)
                .await
                .map_err(|e| {
                    StorageError::Connection(format!("data database connection failed: {e}"))
                })?,
            _ => pool.clone(),
        };

        // P119: Read initial GSI propagation delay from settings table.
        let initial_gsi_delay: u64 = sqlx::query_as::<_, (String,)>(
            "SELECT value FROM settings WHERE key = 'gsi_propagation_delay_ms'",
        )
        .fetch_optional(&pool)
        .await
        .ok()
        .flatten()
        .and_then(|(v,)| v.parse::<u64>().ok())
        .unwrap_or(10);

        Ok(Self {
            pool,
            data_pool,
            region: region.to_owned(),
            max_item_size_bytes: config.max_item_size_bytes,
            control_plane_notify: Arc::new(tokio::sync::Notify::new()),
            gsi_queue: None,
            gsi_default_delay_ms: Arc::new(std::sync::atomic::AtomicU64::new(initial_gsi_delay)),
        })
    }

    /// Start the async GSI worker tasks (D-4).
    ///
    /// Must be called after construction, before serving requests.
    /// Returns `&Self` for chaining.
    pub fn start_gsi_workers(mut self) -> Self {
        self.gsi_queue = Some(gsi_queue::GsiQueue::spawn(self.data_pool.clone()));
        self
    }

    /// Returns a handle to the control plane notify, for use by the
    /// background poller task (F-3).
    pub fn control_plane_notify(&self) -> Arc<tokio::sync::Notify> {
        Arc::clone(&self.control_plane_notify)
    }

    /// Defense-in-depth: validate `account_id` before use in SQL identifiers.
    ///
    /// `account_id` is interpolated into SQL identifiers via `data_table_name()`.
    /// Called by all methods that use `data_table_name()` or `format!`-based DDL.
    /// Reject values that could break quoted identifiers.
    /// See `docs/adr/sql-injection-defense.md`.
    pub(crate) fn validate_account_id(account_id: &str) -> Result<(), StorageError> {
        if account_id.contains('"') || account_id.contains('\0') || !account_id.is_ascii() {
            return Err(StorageError::Internal(
                "account_id contains invalid characters for use in SQL identifiers".to_owned(),
            ));
        }
        Ok(())
    }

    /// Validate catalog version matches the compiled-in expectation (REQ-CAT-007, D-10).
    ///
    /// Reads the version string from the `settings` table and parses it
    /// strictly into a `CatalogVersion`. Rejects malformed strings.
    ///
    /// # Errors
    ///
    /// Returns `StorageError::CatalogNotInitialized` if the catalog tables don't exist.
    /// Returns `StorageError::CatalogVersionMismatch` if the version doesn't match.
    /// Returns `StorageError::Internal` if the stored version string is malformed.
    pub async fn check_catalog_version(&self) -> Result<(), StorageError> {
        // Check table existence via information_schema (robust, not string-matching).
        let exists: (bool,) = sqlx::query_as(
            "SELECT EXISTS(SELECT 1 FROM information_schema.tables WHERE table_name = 'settings' AND table_schema = 'public')",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| StorageError::Connection(e.to_string()))?;

        if !exists.0 {
            return Err(StorageError::CatalogNotInitialized);
        }

        let row: Option<(String,)> =
            sqlx::query_as("SELECT value FROM settings WHERE key = 'catalog_version'")
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| StorageError::Connection(e.to_string()))?;

        let found_str = row.ok_or(StorageError::CatalogNotInitialized)?.0;

        let found = found_str
            .parse::<CatalogVersion>()
            .map_err(|e| StorageError::Internal(e.to_string()))?;

        if found != CATALOG_VERSION {
            return Err(StorageError::CatalogVersionMismatch {
                expected: CATALOG_VERSION.to_string(),
                found: found_str,
            });
        }

        Ok(())
    }

    /// Query the data database name from the catalog for the startup banner (REQ-LOG-001).
    ///
    /// Returns `"(not configured)"` if no data database has been registered.
    ///
    /// # Errors
    ///
    /// Returns `StorageError::Connection` if the query fails.
    pub async fn get_data_database_info(&self) -> Result<String, StorageError> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT value FROM settings WHERE key = 'data_database_name'")
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| StorageError::Connection(e.to_string()))?;

        Ok(row.map_or_else(|| "(not configured)".to_owned(), |(name,)| name))
    }

    /// Returns a reference to the data pool for use by background workers
    /// that operate on `_ddb_*` tables (e.g., TTL cleanup, table size refresh).
    pub fn data_pool(&self) -> &PgPool {
        &self.data_pool
    }
}

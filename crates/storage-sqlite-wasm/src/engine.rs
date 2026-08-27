// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0

//! `SqliteWasmEngine`: the wasm `StorageEngine` implementation.
//!
//! Implements the full 6-subtrait `StorageEngine` surface
//! (`TableEngine + DataEngine + MetadataEngine + StreamEngine + BackupEngine +
//! WorkerStore`, via the blanket impl in `extenddb-storage`) over a `WasmDb`
//! connection. All SQL is ported verbatim from PR #182's `storage-sqlite`.
//!
//! Milestone status (M2a): the 3-op vertical slice
//! (`create_table` / `put_item` / `get_item`, plus `table_key_info`) is
//! implemented; the remaining methods return a clear "not yet ported" error
//! and are filled in per-store across M2b..M2e.

use futures::future::BoxFuture;

use extenddb_core::expression::{Expr, ExpressionMaps, KeyCondition, UpdateAction};
use extenddb_core::types::{
    BackupDescription, BackupDetails, BackupSummary, ContinuousBackupsDescription,
    CreateTableInput, DeleteTableInput, DescribeStreamInput, DescribeTableInput, IndexInfo, Item,
    ListTablesInput, ListTablesOutput, StreamDescription, StreamRecord, TableDescription,
    TableKeyInfo, Tag, TimeToLiveDescription, UpdateTableInput,
};
use extenddb_storage::error::StorageError;
use extenddb_storage::{
    BackupEngine, DataEngine, IdempotencyKey, ItemPairResult, MetadataEngine, QueryResult,
    StreamCapture, StreamEngine, StreamListResult, StreamRecordsResult, TableEngine, TransactGetOp,
    TransactWriteOp, TtlTableInfo, VectorSearch, VectorSearchEngine, VectorSearchResult,
    WorkerStore,
};

use crate::db::WasmDb;

/// Storage engine backed by an in-memory `sqlite-wasm-rs` database.
pub struct SqliteWasmEngine {
    pub(crate) db: WasmDb,
    pub(crate) region: String,
    pub(crate) max_item_size_bytes: usize,
}

impl SqliteWasmEngine {
    /// Open a fresh in-memory engine and apply the catalog schema.
    ///
    /// # Errors
    /// Returns `StorageError::Internal` if the database cannot be opened or the
    /// schema cannot be applied.
    pub fn open_memory(region: impl Into<String>) -> Result<Self, StorageError> {
        let db = WasmDb::open_memory().map_err(StorageError::Internal)?;
        crate::schema::apply_schema(&db).map_err(StorageError::Internal)?;
        Ok(Self {
            db,
            region: region.into(),
            max_item_size_bytes: extenddb_core::limits::LimitsConfig::default().max_item_size_bytes,
        })
    }
}

/// Helper for methods not yet ported to the wasm backend.
fn unsupported<T: Send + 'static>() -> BoxFuture<'static, Result<T, StorageError>> {
    Box::pin(async {
        Err(StorageError::Internal(
            "operation not yet ported to the wasm SQLite backend".to_string(),
        ))
    })
}

impl TableEngine for SqliteWasmEngine {
    fn create_table(
        &self,
        account_id: &str,
        input: CreateTableInput,
    ) -> BoxFuture<'_, Result<TableDescription, StorageError>> {
        let account_id = account_id.to_string();
        Box::pin(async move { self.create_table_impl(&account_id, input) })
    }

    fn delete_table(
        &self,
        account_id: &str,
        input: DeleteTableInput,
    ) -> BoxFuture<'_, Result<TableDescription, StorageError>> {
        let account_id = account_id.to_string();
        Box::pin(async move { self.delete_table_impl(&account_id, input) })
    }

    fn describe_table(
        &self,
        account_id: &str,
        input: DescribeTableInput,
    ) -> BoxFuture<'_, Result<TableDescription, StorageError>> {
        let account_id = account_id.to_string();
        Box::pin(async move { self.describe_table_impl(&account_id, &input.table_name) })
    }

    fn list_tables(
        &self,
        account_id: &str,
        input: ListTablesInput,
    ) -> BoxFuture<'_, Result<ListTablesOutput, StorageError>> {
        let account_id = account_id.to_string();
        Box::pin(async move { self.list_tables_impl(&account_id, input) })
    }

    fn update_table(
        &self,
        _account_id: &str,
        _input: UpdateTableInput,
    ) -> BoxFuture<'_, Result<TableDescription, StorageError>> {
        unsupported()
    }

    fn table_key_info(
        &self,
        account_id: &str,
        table_name: &str,
    ) -> BoxFuture<'_, Result<TableKeyInfo, StorageError>> {
        let account_id = account_id.to_string();
        let table_name = table_name.to_string();
        Box::pin(async move { self.table_key_info_impl(&account_id, &table_name) })
    }

    fn index_info(
        &self,
        _account_id: &str,
        _table_name: &str,
        _index_name: &str,
    ) -> BoxFuture<'_, Result<IndexInfo, StorageError>> {
        unsupported()
    }

    fn index_info_by_table_id(
        &self,
        _table_id: &str,
        _index_name: &str,
    ) -> BoxFuture<'_, Result<IndexInfo, StorageError>> {
        unsupported()
    }
}

impl DataEngine for SqliteWasmEngine {
    fn put_item(
        &self,
        key_info: &TableKeyInfo,
        item: Item,
        return_old: bool,
        condition: Option<&Expr>,
        maps: &ExpressionMaps,
        _stream: Option<&StreamCapture>,
    ) -> BoxFuture<'_, Result<Option<Item>, StorageError>> {
        let key_info = key_info.clone();
        let condition = condition.cloned();
        let maps = maps.clone();
        Box::pin(async move {
            self.put_item_impl(&key_info, &item, return_old, condition.as_ref(), &maps)
        })
    }

    fn get_item(
        &self,
        key_info: &TableKeyInfo,
        key: &Item,
    ) -> BoxFuture<'_, Result<Option<Item>, StorageError>> {
        let key_info = key_info.clone();
        let key = key.clone();
        Box::pin(async move { self.get_item_impl(&key_info, &key) })
    }

    fn delete_item(
        &self,
        key_info: &TableKeyInfo,
        key: &Item,
        return_old: bool,
        condition: Option<&Expr>,
        maps: &ExpressionMaps,
        _stream: Option<&StreamCapture>,
    ) -> BoxFuture<'_, Result<Option<Item>, StorageError>> {
        let key_info = key_info.clone();
        let key = key.clone();
        let condition = condition.cloned();
        let maps = maps.clone();
        Box::pin(async move {
            self.delete_item_impl(&key_info, &key, return_old, condition.as_ref(), &maps)
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn update_item(
        &self,
        key_info: &TableKeyInfo,
        key: &Item,
        actions: &[UpdateAction],
        return_old: bool,
        return_new: bool,
        condition: Option<&Expr>,
        maps: &ExpressionMaps,
        _stream: Option<&StreamCapture>,
    ) -> BoxFuture<'_, ItemPairResult> {
        let key_info = key_info.clone();
        let key = key.clone();
        let actions = actions.to_vec();
        let condition = condition.cloned();
        let maps = maps.clone();
        Box::pin(async move {
            self.update_item_impl(
                &key_info,
                &key,
                &actions,
                return_old,
                return_new,
                condition.as_ref(),
                &maps,
            )
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn query(
        &self,
        key_info: &TableKeyInfo,
        key_condition: &KeyCondition,
        maps: &ExpressionMaps,
        forward: bool,
        limit: Option<i64>,
        exclusive_start_key: Option<&Item>,
        index_name: Option<&str>,
    ) -> BoxFuture<'_, QueryResult> {
        if index_name.is_some() {
            // Secondary-index queries arrive with M2c-index work.
            return unsupported();
        }
        let key_info = key_info.clone();
        let key_condition = key_condition.clone();
        let maps = maps.clone();
        let esk = exclusive_start_key.cloned();
        Box::pin(async move {
            self.query_impl(
                &key_info,
                &key_condition,
                &maps,
                forward,
                limit,
                esk.as_ref(),
            )
        })
    }

    fn scan(
        &self,
        key_info: &TableKeyInfo,
        limit: Option<i64>,
        exclusive_start_key: Option<&Item>,
        _segment: Option<i64>,
        _total_segments: Option<i64>,
        index_name: Option<&str>,
    ) -> BoxFuture<'_, QueryResult> {
        if index_name.is_some() {
            // Secondary index scans arrive with M2c-index work.
            return unsupported();
        }
        let key_info = key_info.clone();
        let esk = exclusive_start_key.cloned();
        Box::pin(async move { self.scan_impl(&key_info, limit, esk.as_ref()) })
    }

    fn transact_get_items(
        &self,
        _ops: &[TransactGetOp<'_>],
    ) -> BoxFuture<'_, Result<Vec<Option<Item>>, StorageError>> {
        unsupported()
    }

    fn as_vector_search(&self) -> Option<&dyn VectorSearchEngine> {
        // Not a capability flag: returning `Some` hands the engine the
        // implementation below, and is what admits vector indexes on
        // CreateTable and routes SearchVectors here.
        Some(self)
    }

    fn transact_write_items(
        &self,
        _ops: &[TransactWriteOp<'_>],
        _token: Option<IdempotencyKey<'_>>,
    ) -> BoxFuture<'_, Result<(), StorageError>> {
        unsupported()
    }

    fn cleanup_expired_idempotency_tokens(
        &self,
        _max_age_seconds: i64,
    ) -> BoxFuture<'_, Result<u64, StorageError>> {
        unsupported()
    }
}

impl VectorSearchEngine for SqliteWasmEngine {
    fn search_vectors(&self, req: VectorSearch<'_>) -> BoxFuture<'_, VectorSearchResult> {
        // Own what the body needs so the future is not tied to the caller's
        // frame, mirroring the native implementation; the body itself is
        // synchronous, like every other method on this backend.
        let table_id = req.key_info.table_id.clone();
        let index_name = req.index_name.to_owned();
        let query_vector = req.query_vector.to_vec();
        let top_k = req.top_k;
        let partition = extenddb_storage::vector_lifecycle::partition_value(req.hash_key);
        let filters: Vec<(String, extenddb_core::types::AttributeValue)> = req
            .filters
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).clone()))
            .collect();
        Box::pin(async move {
            self.search_vectors_impl(
                &table_id,
                &index_name,
                &query_vector,
                top_k,
                &partition?,
                &filters,
            )
        })
    }
}

impl MetadataEngine for SqliteWasmEngine {
    fn describe_ttl(
        &self,
        _account_id: &str,
        _table_name: &str,
    ) -> BoxFuture<'_, Result<TimeToLiveDescription, StorageError>> {
        unsupported()
    }

    fn update_ttl(
        &self,
        _account_id: &str,
        _table_name: &str,
        _attribute_name: &str,
        _enabled: bool,
    ) -> BoxFuture<'_, Result<(), StorageError>> {
        unsupported()
    }

    fn tag_resource(&self, _arn: &str, _tags: &[Tag]) -> BoxFuture<'_, Result<(), StorageError>> {
        unsupported()
    }

    fn untag_resource(
        &self,
        _arn: &str,
        _tag_keys: &[String],
    ) -> BoxFuture<'_, Result<(), StorageError>> {
        unsupported()
    }

    fn list_tags(&self, _arn: &str) -> BoxFuture<'_, Result<Vec<Tag>, StorageError>> {
        unsupported()
    }

    fn tables_with_ttl(
        &self,
        _account_id: &str,
    ) -> BoxFuture<'_, Result<Vec<(String, String)>, StorageError>> {
        unsupported()
    }

    fn all_tables_with_ttl(&self) -> BoxFuture<'_, Result<Vec<TtlTableInfo>, StorageError>> {
        unsupported()
    }

    fn all_tables_with_ttl_index_ready(
        &self,
    ) -> BoxFuture<'_, Result<Vec<TtlTableInfo>, StorageError>> {
        unsupported()
    }

    fn create_ttl_index(
        &self,
        _account_id: &str,
        _table_name: &str,
        _ttl_attribute: &str,
    ) -> BoxFuture<'_, Result<(), StorageError>> {
        unsupported()
    }

    fn drop_ttl_index(
        &self,
        _account_id: &str,
        _table_name: &str,
    ) -> BoxFuture<'_, Result<(), StorageError>> {
        unsupported()
    }

    fn find_expired_items_indexed(
        &self,
        _account_id: &str,
        _table_name: &str,
        _ttl_attribute: &str,
        _limit: usize,
    ) -> BoxFuture<'_, Result<Vec<Item>, StorageError>> {
        unsupported()
    }

    fn refresh_table_size(
        &self,
        _account_id: &str,
        _table_name: &str,
    ) -> BoxFuture<'_, Result<(), StorageError>> {
        unsupported()
    }

    fn list_active_table_names(
        &self,
        _account_id: &str,
    ) -> BoxFuture<'_, Result<Vec<String>, StorageError>> {
        unsupported()
    }

    fn all_active_tables(&self) -> BoxFuture<'_, Result<Vec<(String, String)>, StorageError>> {
        unsupported()
    }
}

impl StreamEngine for SqliteWasmEngine {
    fn write_stream_record(
        &self,
        _account_id: &str,
        _record: &StreamRecord,
        _shard_id: &str,
        _table_name: &str,
    ) -> BoxFuture<'_, Result<(), StorageError>> {
        unsupported()
    }

    fn get_stream_records(
        &self,
        _account_id: &str,
        _shard_id: &str,
        _after_sequence: Option<&str>,
        _limit: i64,
    ) -> BoxFuture<'_, StreamRecordsResult> {
        unsupported()
    }

    fn describe_stream(
        &self,
        _account_id: &str,
        _input: &DescribeStreamInput,
    ) -> BoxFuture<'_, Result<StreamDescription, StorageError>> {
        unsupported()
    }

    fn list_streams(
        &self,
        _account_id: &str,
        _table_name: Option<&str>,
        _limit: i64,
        _exclusive_start_stream_arn: Option<&str>,
    ) -> BoxFuture<'_, StreamListResult> {
        unsupported()
    }

    fn cleanup_expired_stream_records(
        &self,
        _retention_hours: i64,
    ) -> BoxFuture<'_, Result<u64, StorageError>> {
        unsupported()
    }

    fn assign_shard(
        &self,
        _account_id: &str,
        _table_name: &str,
        _partition_key: &str,
    ) -> BoxFuture<'_, Result<String, StorageError>> {
        unsupported()
    }

    fn next_sequence_number(&self, _shard_id: &str) -> BoxFuture<'_, Result<String, StorageError>> {
        unsupported()
    }

    fn validate_shard(
        &self,
        _account_id: &str,
        _stream_arn: &str,
        _shard_id: &str,
    ) -> BoxFuture<'_, Result<(), StorageError>> {
        unsupported()
    }

    fn latest_sequence_number(
        &self,
        _shard_id: &str,
    ) -> BoxFuture<'_, Result<Option<String>, StorageError>> {
        unsupported()
    }
}

impl WorkerStore for SqliteWasmEngine {
    fn process_control_plane_transitions(
        &self,
    ) -> BoxFuture<'_, Result<Vec<(String, &'static str)>, StorageError>> {
        // No background control plane on wasm: tables are created ACTIVE.
        Box::pin(async { Ok(Vec::new()) })
    }
}

impl BackupEngine for SqliteWasmEngine {
    fn create_backup(
        &self,
        _account_id: &str,
        _table_name: &str,
        _backup_name: &str,
    ) -> BoxFuture<'_, Result<BackupDetails, StorageError>> {
        unsupported()
    }

    fn describe_backup(
        &self,
        _account_id: &str,
        _backup_arn: &str,
    ) -> BoxFuture<'_, Result<BackupDescription, StorageError>> {
        unsupported()
    }

    fn list_backups(
        &self,
        _account_id: &str,
        _table_name: Option<&str>,
    ) -> BoxFuture<'_, Result<Vec<BackupSummary>, StorageError>> {
        unsupported()
    }

    fn delete_backup(
        &self,
        _account_id: &str,
        _backup_arn: &str,
    ) -> BoxFuture<'_, Result<BackupDescription, StorageError>> {
        unsupported()
    }

    fn restore_table_from_backup(
        &self,
        _account_id: &str,
        _target_table_name: &str,
        _backup_arn: &str,
    ) -> BoxFuture<'_, Result<TableDescription, StorageError>> {
        unsupported()
    }

    fn describe_continuous_backups(
        &self,
        _account_id: &str,
        _table_name: &str,
    ) -> BoxFuture<'_, Result<ContinuousBackupsDescription, StorageError>> {
        unsupported()
    }

    fn update_continuous_backups(
        &self,
        _account_id: &str,
        _table_name: &str,
        _pitr_enabled: bool,
    ) -> BoxFuture<'_, Result<ContinuousBackupsDescription, StorageError>> {
        unsupported()
    }

    fn restore_table_to_point_in_time(
        &self,
        _account_id: &str,
        _source_table_name: &str,
        _target_table_name: &str,
    ) -> BoxFuture<'_, Result<TableDescription, StorageError>> {
        unsupported()
    }
}

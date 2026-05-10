// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0

//! `StreamEngine` trait implementation for `PostgresEngine`.

use extenddb_core::types::{
    SequenceNumberRange, Shard, StreamDescription, StreamRecord, StreamStatus, StreamSummary,
    StreamViewType,
};
use extenddb_storage::StreamEngine;
use extenddb_storage::error::StorageError;
use extenddb_storage::util::{parse_stream_arn, stream_arn};
use sqlx::PgPool;

use crate::PostgresEngine;

/// Number of fixed shards per stream (hash-based assignment).
const SHARDS_PER_STREAM: u32 = 4;

impl PostgresEngine {
    /// Initialize stream shards for a table and set the stream_label.
    ///
    /// Updates the stream label in the catalog (via the provided catalog
    /// transaction) and creates shard rows in the data database within a
    /// data transaction for atomicity.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Internal`] if any query fails.
    pub(crate) async fn init_stream_shards(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        data_pool: &PgPool,
        account_id: &str,
        table_name: &str,
        table_id: &str,
    ) -> Result<String, StorageError> {
        let label: String = sqlx::query_scalar(
            "UPDATE tables SET stream_label = to_char(NOW(), 'YYYY-MM-DD\"T\"HH24:MI:SS') \
             WHERE account_id = $1 AND table_name = $2 \
             RETURNING stream_label",
        )
        .bind(account_id)
        .bind(table_name)
        .fetch_one(&mut **tx)
        .await
        .map_err(|e| StorageError::Internal(e.to_string()))?;

        // P54 Bug 1: Stream shards live in the data database for atomic
        // writes with stream records and item data. Use a transaction so
        // all shards are created atomically.
        let mut data_tx = data_pool
            .begin()
            .await
            .map_err(|e| StorageError::Internal(e.to_string()))?;

        for i in 0..SHARDS_PER_STREAM {
            let shard_id = format!("shardId-{table_name}-{i:012}");
            let start_seq = format!("{:021}", 0);
            sqlx::query(
                "INSERT INTO stream_shards (shard_id, table_id, starting_sequence_number) \
                 VALUES ($1, $2, $3)",
            )
            .bind(&shard_id)
            .bind(table_id)
            .bind(&start_seq)
            .execute(&mut *data_tx)
            .await
            .map_err(|e| StorageError::Internal(e.to_string()))?;
        }

        data_tx
            .commit()
            .await
            .map_err(|e| StorageError::Internal(e.to_string()))?;

        Ok(label)
    }
}

impl StreamEngine for PostgresEngine {
    async fn write_stream_record(
        &self,
        account_id: &str,
        record: &StreamRecord,
        shard_id: &str,
        table_name: &str,
    ) -> Result<(), StorageError> {
        let record_json =
            serde_json::to_value(record).map_err(|e| StorageError::Internal(e.to_string()))?;

        // Resolve table_id from account_id + table_name for the stream_records FK.
        let table_id: String = sqlx::query_scalar(
            "SELECT table_id FROM tables WHERE account_id = $1 AND table_name = $2",
        )
        .bind(account_id)
        .bind(table_name)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| StorageError::Internal(e.to_string()))?;

        sqlx::query(
            "INSERT INTO stream_records (sequence_number, shard_id, table_id, event_name, record_data) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(&record.dynamodb.sequence_number)
        .bind(shard_id)
        .bind(&table_id)
        .bind(format!("{:?}", record.event_name))
        .bind(&record_json)
        .execute(&self.data_pool)
        .await
        .map_err(|e| StorageError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn get_stream_records(
        &self,
        shard_id: &str,
        after_sequence: Option<&str>,
        limit: i64,
    ) -> Result<(Vec<StreamRecord>, Option<String>), StorageError> {
        let rows: Vec<(serde_json::Value,)> = if let Some(after) = after_sequence {
            sqlx::query_as(
                "SELECT record_data FROM stream_records \
                 WHERE shard_id = $1 AND sequence_number > $2 \
                 ORDER BY sequence_number LIMIT $3",
            )
            .bind(shard_id)
            .bind(after)
            .bind(limit)
            .fetch_all(&self.data_pool)
            .await
            .map_err(|e| StorageError::Internal(e.to_string()))?
        } else {
            sqlx::query_as(
                "SELECT record_data FROM stream_records \
                 WHERE shard_id = $1 \
                 ORDER BY sequence_number LIMIT $2",
            )
            .bind(shard_id)
            .bind(limit)
            .fetch_all(&self.data_pool)
            .await
            .map_err(|e| StorageError::Internal(e.to_string()))?
        };

        let records: Vec<StreamRecord> = rows
            .into_iter()
            .map(|(data,)| {
                serde_json::from_value(data).map_err(|e| StorageError::Internal(e.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let last_seq = records.last().map(|r| r.dynamodb.sequence_number.clone());
        Ok((records, last_seq))
    }

    async fn describe_stream(
        &self,
        account_id: &str,
        input: &extenddb_core::types::DescribeStreamInput,
    ) -> Result<StreamDescription, StorageError> {
        let (table_name, stream_label) = parse_stream_arn(&input.stream_arn)?;

        let row: Option<(serde_json::Value, serde_json::Value, Option<serde_json::Value>, String, String)> =
            sqlx::query_as(
                "SELECT key_schema, attribute_definitions, stream_specification, table_status, table_id \
                 FROM tables WHERE account_id = $1 AND table_name = $2 AND stream_label = $3",
            )
            .bind(account_id)
            .bind(&table_name)
            .bind(&stream_label)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| StorageError::Internal(e.to_string()))?;

        let (ks_json, _ad_json, stream_spec_json, table_status, table_id) =
            row.ok_or_else(|| {
                StorageError::TableNotFound(format!(
                    "Requested resource not found: Stream: {arn} not found.",
                    arn = input.stream_arn
                ))
            })?;

        let key_schema =
            serde_json::from_value(ks_json).map_err(|e| StorageError::Internal(e.to_string()))?;

        let stream_view_type = stream_spec_json
            .and_then(|v| {
                v.get("StreamViewType")
                    .and_then(|sv| serde_json::from_value::<StreamViewType>(sv.clone()).ok())
            })
            .unwrap_or(StreamViewType::KeysOnly);

        let limit = input.limit.unwrap_or(100);
        let shard_rows: Vec<(String, Option<String>, String, Option<String>)> = if let Some(
            ref start,
        ) =
            input.exclusive_start_shard_id
        {
            sqlx::query_as(
                    "SELECT shard_id, parent_shard_id, starting_sequence_number, ending_sequence_number \
                     FROM stream_shards WHERE table_id = $1 AND shard_id > $2 \
                     ORDER BY shard_id LIMIT $3",
                )
                .bind(&table_id)
                .bind(start)
                .bind(limit + 1)
                .fetch_all(&self.data_pool)
                .await
                .map_err(|e| StorageError::Internal(e.to_string()))?
        } else {
            sqlx::query_as(
                    "SELECT shard_id, parent_shard_id, starting_sequence_number, ending_sequence_number \
                     FROM stream_shards WHERE table_id = $1 \
                     ORDER BY shard_id LIMIT $2",
                )
                .bind(&table_id)
                .bind(limit + 1)
                .fetch_all(&self.data_pool)
                .await
                .map_err(|e| StorageError::Internal(e.to_string()))?
        };

        #[allow(clippy::cast_sign_loss)]
        let limit_usize = limit as usize;
        let last_shard = if shard_rows.len() > limit_usize {
            Some(shard_rows[limit_usize - 1].0.clone())
        } else {
            None
        };

        let shards: Vec<Shard> = shard_rows
            .into_iter()
            .take(limit_usize)
            .map(|(id, parent, start, end)| Shard {
                shard_id: id,
                parent_shard_id: parent,
                sequence_number_range: SequenceNumberRange {
                    starting_sequence_number: start,
                    ending_sequence_number: end,
                },
            })
            .collect();

        // Tables in DELETING state have streams in DISABLING status.
        let stream_status = if table_status == "DELETING" {
            StreamStatus::Disabling
        } else {
            StreamStatus::Enabled
        };

        Ok(StreamDescription {
            stream_arn: input.stream_arn.clone(),
            stream_label,
            stream_status,
            stream_view_type,
            table_name,
            key_schema,
            shards,
            last_evaluated_shard_id: last_shard,
        })
    }

    // Stream status for DELETING tables is handled in describe_stream.
    // list_streams still returns streams for all tables (matching real DynamoDB).
    async fn list_streams(
        &self,
        account_id: &str,
        table_name: Option<&str>,
        limit: i64,
        exclusive_start_stream_arn: Option<&str>,
    ) -> Result<(Vec<StreamSummary>, Option<String>), StorageError> {
        let rows: Vec<(String, String, String)> = match (table_name, exclusive_start_stream_arn) {
            (Some(tn), Some(start_arn)) => {
                let (_, start_label) = parse_stream_arn(start_arn)?;
                sqlx::query_as(
                    "SELECT table_name, table_arn, stream_label FROM tables \
                     WHERE account_id = $1 AND stream_label IS NOT NULL AND table_name = $2 AND stream_label > $3 \
                     ORDER BY stream_label LIMIT $4",
                )
                .bind(account_id)
                .bind(tn)
                .bind(&start_label)
                .bind(limit + 1)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| StorageError::Internal(e.to_string()))?
            }
            (Some(tn), None) => sqlx::query_as(
                "SELECT table_name, table_arn, stream_label FROM tables \
                     WHERE account_id = $1 AND stream_label IS NOT NULL AND table_name = $2 \
                     ORDER BY stream_label LIMIT $3",
            )
            .bind(account_id)
            .bind(tn)
            .bind(limit + 1)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StorageError::Internal(e.to_string()))?,
            (None, Some(start_arn)) => {
                let (start_table, start_label) = parse_stream_arn(start_arn)?;
                sqlx::query_as(
                    "SELECT table_name, table_arn, stream_label FROM tables \
                     WHERE account_id = $1 AND stream_label IS NOT NULL \
                       AND (table_name, stream_label) > ($2, $3) \
                     ORDER BY table_name, stream_label LIMIT $4",
                )
                .bind(account_id)
                .bind(&start_table)
                .bind(&start_label)
                .bind(limit + 1)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| StorageError::Internal(e.to_string()))?
            }
            (None, None) => sqlx::query_as(
                "SELECT table_name, table_arn, stream_label FROM tables \
                     WHERE account_id = $1 AND stream_label IS NOT NULL \
                     ORDER BY table_name, stream_label LIMIT $2",
            )
            .bind(account_id)
            .bind(limit + 1)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StorageError::Internal(e.to_string()))?,
        };

        #[allow(clippy::cast_sign_loss)]
        let limit_usize = limit as usize;

        let summaries: Vec<StreamSummary> = rows
            .iter()
            .take(limit_usize)
            .map(|(tn, _table_arn, label)| StreamSummary {
                stream_arn: stream_arn(&self.region, account_id, tn, label),
                stream_label: label.clone(),
                table_name: tn.clone(),
            })
            .collect();

        let last_arn = if rows.len() > limit_usize {
            summaries.last().map(|s| s.stream_arn.clone())
        } else {
            None
        };

        Ok((summaries, last_arn))
    }

    async fn cleanup_expired_stream_records(
        &self,
        retention_hours: i64,
    ) -> Result<u64, StorageError> {
        // Cast i64→integer for PG 15 compat; safe for realistic retention values (<245k years).
        let result = sqlx::query(
            "DELETE FROM stream_records \
             WHERE created_at < NOW() - make_interval(hours => $1::integer)",
        )
        .bind(retention_hours)
        .execute(&self.data_pool)
        .await
        .map_err(|e| StorageError::Internal(e.to_string()))?;
        Ok(result.rows_affected())
    }

    // TODO(architecture): Shard list per table requires an extra SQL round-trip
    // per write when streams are enabled. Caching is prohibited by the No Caching
    // Rule (multiple vddb instances may share one catalog). If profiling shows this
    // is a bottleneck, a cross-instance invalidation design is required first.
    async fn assign_shard(
        &self,
        account_id: &str,
        table_name: &str,
        partition_key: &str,
    ) -> Result<String, StorageError> {
        // P54 Bug 1: Resolve table_id from catalog, then query shards from data pool.
        let table_id: String = sqlx::query_scalar(
            "SELECT table_id FROM tables WHERE account_id = $1 AND table_name = $2",
        )
        .bind(account_id)
        .bind(table_name)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| StorageError::Internal(e.to_string()))?;

        let shards: Vec<(String,)> = sqlx::query_as(
            "SELECT shard_id FROM stream_shards \
             WHERE table_id = $1 \
             ORDER BY shard_id",
        )
        .bind(&table_id)
        .fetch_all(&self.data_pool)
        .await
        .map_err(|e| StorageError::Internal(e.to_string()))?;

        if shards.is_empty() {
            return Err(StorageError::Internal(format!(
                "No stream shards for table {table_name}"
            )));
        }

        let hash = crc32fast::hash(partition_key.as_bytes());
        #[allow(clippy::cast_possible_truncation)]
        let idx = (hash as usize) % shards.len();
        Ok(shards[idx].0.clone())
    }

    // Sequence numbers are global microsecond timestamps, not per-shard
    // sequences. This matches extenddb's single-node design where global ordering
    // is sufficient. The `_shard_id` parameter is accepted for trait
    // compatibility but unused. Real DynamoDB uses per-shard sequences.
    async fn next_sequence_number(&self, _shard_id: &str) -> Result<String, StorageError> {
        let (seq_val,): (i64,) = sqlx::query_as("SELECT nextval('stream_seq')")
            .fetch_one(&self.data_pool)
            .await
            .map_err(|e| StorageError::Internal(e.to_string()))?;
        Ok(format!("{seq_val:021}"))
    }

    async fn validate_shard(
        &self,
        account_id: &str,
        stream_arn: &str,
        shard_id: &str,
    ) -> Result<(), StorageError> {
        let (table_name, stream_label) = parse_stream_arn(stream_arn)?;

        // P54 Bug 1: Resolve table_id from catalog, then check shard in data pool.
        let table_id: Option<String> = sqlx::query_scalar(
            "SELECT table_id FROM tables \
             WHERE account_id = $1 AND table_name = $2 AND stream_label = $3",
        )
        .bind(account_id)
        .bind(&table_name)
        .bind(&stream_label)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| StorageError::Internal(e.to_string()))?;

        let Some(table_id) = table_id else {
            return Err(StorageError::TableNotFound(format!(
                "Requested resource not found: Stream: {stream_arn} not found."
            )));
        };

        let exists: Option<(i32,)> =
            sqlx::query_as("SELECT 1 FROM stream_shards WHERE shard_id = $1 AND table_id = $2")
                .bind(shard_id)
                .bind(&table_id)
                .fetch_optional(&self.data_pool)
                .await
                .map_err(|e| StorageError::Internal(e.to_string()))?;

        if exists.is_none() {
            return Err(StorageError::TableNotFound(format!(
                "Requested resource not found: Stream: {stream_arn} not found."
            )));
        }
        Ok(())
    }

    async fn latest_sequence_number(&self, shard_id: &str) -> Result<Option<String>, StorageError> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT sequence_number FROM stream_records \
             WHERE shard_id = $1 ORDER BY sequence_number DESC LIMIT 1",
        )
        .bind(shard_id)
        .fetch_optional(&self.data_pool)
        .await
        .map_err(|e| StorageError::Internal(e.to_string()))?;
        Ok(row.map(|(s,)| s))
    }
}

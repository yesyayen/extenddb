// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0

//! Vector similarity search: exact scan over one partition.
//!
//! Exact rather than approximate, and that is a measured decision rather than a
//! placeholder. No SQLite vector extension meets this backend's constraints: the
//! static-musl `FROM scratch` build cannot `dlopen` a loadable extension, the one
//! extension with a compatible licence and an in-database index (`sqlite-vec`) is
//! brute force in every stable release anyway, and every option offering a real
//! ANN index either stores it in a sidecar file, forbids transactions, or is not
//! open source. See `docs/adr` for the full elimination.
//!
//! Measured cost on one core, warm cache, row-per-vector layout: roughly 213k to
//! 334k vectors/sec at 256 dimensions, 94k to 103k at 1024, and 39k to 43k at
//! 4096. So a partition stays inside a 10 ms budget up to about 1,000 vectors at
//! 1024 dimensions, and inside 100 ms up to about 10,000. The scan is dominated by
//! getting bytes out of SQLite rather than by the arithmetic, which is why a
//! zero-copy `&[f32]` view of the blob measured no faster than decoding per
//! element.
//!
//! The scoring arithmetic and the top-k ranking live in
//! `extenddb_storage::vector_scoring`, shared with the browser/wasm backend so
//! the two exact-scan implementations cannot drift apart; this module owns only
//! the SQL that feeds them.

use extenddb_core::types::{AttributeValue, DistanceFunction, Item};
use extenddb_storage::error::StorageError;
use extenddb_storage::vector_lifecycle::partition_value;
use extenddb_storage::vector_scoring::{TopK, decode_vector, score};
use extenddb_storage::{
    BoxedFuture, VectorHit, VectorSearch, VectorSearchEngine, VectorSearchOutput,
    VectorSearchResult,
};

use crate::data::vector_table_name;
use crate::store::SqliteEngine;

impl VectorSearchEngine for SqliteEngine {
    fn search_vectors(&self, req: VectorSearch<'_>) -> BoxedFuture<'_, VectorSearchResult> {
        // The request borrows; own what the async body needs so the future is not
        // tied to the caller's frame.
        let table_id = req.key_info.table_id.clone();
        let index_name = req.index_name.to_owned();
        let query_vector = req.query_vector.to_vec();
        let top_k = req.top_k;
        let partition = partition_value(req.hash_key);
        let filters: Vec<(String, AttributeValue)> = req
            .filters
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).clone()))
            .collect();

        Box::pin(async move {
            let partition = partition?;

            // The index definition comes from the catalog rather than from
            // TableKeyInfo, because the cached key info carries dimensions and the
            // search schema but not the distance function, without which a score
            // cannot be computed or ordered.
            let row: Option<(String, i64, String, String)> = sqlx::query_as(
                "SELECT index_id, dimensions, distance_function, vector_attribute \
                 FROM vector_indexes WHERE table_id = ? AND index_name = ?",
            )
            .bind(&table_id)
            .bind(&index_name)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| StorageError::Internal(e.to_string()))?;

            let (index_id, dimensions, distance_raw, vector_attribute_json) =
                row.ok_or_else(|| StorageError::IndexNotFound(index_name.clone()))?;
            // Stored as the serialized `VectorAttribute`, not a bare name, so it is
            // deserialized exactly as the write path does. Treating the column as a
            // plain string yields the key `{"AttributeName":"emb"}`.
            let vector_attribute_name =
                serde_json::from_str::<extenddb_core::types::VectorAttribute>(
                    &vector_attribute_json,
                )
                .map_err(|e| StorageError::Internal(format!("vector_attribute: {e}")))?
                .attribute_name;
            let dimensions = usize::try_from(dimensions).map_err(|_| {
                StorageError::Internal(format!("vector dimensions out of range: {dimensions}"))
            })?;
            let function: DistanceFunction = serde_json::from_str(&format!("\"{distance_raw}\""))
                .map_err(|e| {
                StorageError::Internal(format!("unknown distance function: {e}"))
            })?;

            if query_vector.len() != dimensions {
                // Core validates this against the cached key info, so reaching
                // here means the catalog and the cache disagree.
                return Err(StorageError::Validation(format!(
                    "query vector has {} dimensions, index expects {dimensions}",
                    query_vector.len()
                )));
            }

            let vec_table = vector_table_name(&table_id, &index_id);
            // `nrm` is deliberately not selected. The stored norm is an f32 and cannot
            // be trusted for either the zero test or the cosine denominator, so the
            // scorer recomputes both sides in f64 from the vector it already decoded.
            // The column stays because dropping it would need a data migration for
            // no benefit; see the reasoning on `create_vector_data_table`.
            let sql = format!("SELECT vec, item_data FROM {vec_table} WHERE part = ?");

            let k = usize::try_from(top_k.max(0)).unwrap_or(0);
            let mut top = TopK::new(k, function);

            // Streamed rather than fetched all at once, so a large partition does
            // not allocate proportionally to its size. This is the reason the
            // row-per-vector layout was chosen over a packed blob per partition.
            use futures::TryStreamExt;
            let mut stream = sqlx::query_as::<_, (Vec<u8>, String)>(&sql)
                .bind(&partition)
                .fetch(&self.pool);

            while let Some((blob, item_json)) = stream
                .try_next()
                .await
                .map_err(|e| StorageError::Internal(e.to_string()))?
            {
                let candidate = decode_vector(&blob, dimensions)?;
                let item: Item = serde_json::from_str(&item_json)
                    .map_err(|e| StorageError::Internal(format!("stored item: {e}")))?;

                // Inline-filter attributes are applied here rather than in SQL,
                // because they are item attributes rather than columns. Equality
                // only, which is all the wire surface admits today.
                if !filters.is_empty()
                    && !filters
                        .iter()
                        .all(|(name, expected)| item.get(name) == Some(expected))
                {
                    continue;
                }

                let candidate_score = score(function, &query_vector, &candidate);
                top.offer(candidate_score, item, candidate);
            }

            Ok(VectorSearchOutput {
                hits: top
                    .into_hits()
                    .into_iter()
                    .map(|(score, mut item, components)| {
                        // Reinstated from the stored `f32`s rather than from a
                        // second copy in the payload, so what comes back is the
                        // narrowed value that was actually indexed. The engine drops
                        // it again unless a `ProjectionExpression` names it, and the
                        // billed byte count subtracts it, so putting it here does not
                        // change either the default response or the metric.
                        item.insert(
                            vector_attribute_name.clone(),
                            extenddb_core::validation::vector_item::vector_attribute(&components),
                        );
                        VectorHit { item, score }
                    })
                    .collect(),
                distance_function: function,
            })
        })
    }
}

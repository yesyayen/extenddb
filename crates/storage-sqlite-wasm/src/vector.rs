// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0

//! Vector indexes on the wasm backend: catalog, write-path maintenance, search.
//!
//! The behaviour mirrors `storage-sqlite` with one structural simplification:
//! everything is synchronous and inline. Native maintenance chooses between
//! applying in the caller's transaction and enqueueing on the `gsi_pending`
//! queue; wasm has no worker to drain a queue, so the inline arm is the only
//! arm (the same reasoning that creates tables ACTIVE at birth here). The
//! conformance-critical parts do not diverge: indexability, partition
//! placement, and the stored payload come from `extenddb_storage::vector_lifecycle`,
//! catalog row decoding from `extenddb_storage::vector_catalog`, and the score
//! arithmetic and ranking from `extenddb_storage::vector_scoring`, all shared
//! with the native backend.

use extenddb_core::types::{
    AttributeValue, DistanceFunction, Item, SearchSchemaElementType, TableKeyInfo,
    VectorIndexSpecification,
};
use extenddb_core::validation::vector_components;
use extenddb_storage::error::StorageError;
use extenddb_storage::vector_catalog::VectorIndexCatalogRow;
use extenddb_storage::vector_lifecycle::{
    VectorIndexMeta, item_is_indexable, item_partition, projected_payload,
};
use extenddb_storage::vector_scoring::{TopK, decode_vector, score};
use extenddb_storage::{VectorHit, VectorSearchOutput, VectorSearchResult};

use crate::db::Val;
use crate::engine::SqliteWasmEngine;
use crate::ops::extract_keys;

fn intern<E: std::fmt::Display>(e: E) -> StorageError {
    StorageError::Internal(e.to_string())
}

impl SqliteWasmEngine {
    /// Insert the catalog rows for a `CreateTable`'s vector indexes.
    ///
    /// Mirrors the native insert except for the status: the table this backend
    /// creates is ACTIVE immediately (no control-plane worker), so its indexes
    /// are too, where native writes the table's initial status and lets the
    /// worker flip both together.
    pub(crate) fn insert_vector_catalog_rows(
        &self,
        table_id: &str,
        specs: &[VectorIndexSpecification],
    ) -> Result<(), StorageError> {
        for vi in specs {
            let vec_attr = serde_json::to_string(&vi.vector_attribute).map_err(intern)?;
            // An empty SearchSchema is stored as absent: it means the same as
            // omitting the member, and the service never reports an empty list.
            let search_schema = vi
                .search_schema_for_storage()
                .map(serde_json::to_string)
                .transpose()
                .map_err(intern)?;
            let proj = vi
                .projection
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(intern)?
                .ok_or_else(|| {
                    // Core validation requires Projection, so reaching here means
                    // the request bypassed validation rather than that the caller
                    // omitted it.
                    StorageError::Internal(
                        "vector index reached storage without a projection".to_owned(),
                    )
                })?;
            let distance =
                extenddb_storage::vector_catalog::distance_function_token(vi.distance_function)?;
            let index_id = uuid::Uuid::new_v4().to_string();
            self.db
                .execute(
                    "INSERT INTO vector_indexes \
                     (table_id, index_name, index_id, dimensions, distance_function, \
                      vector_attribute, search_schema, projection, index_status) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'ACTIVE')",
                    &[
                        Val::Text(table_id),
                        Val::Text(&vi.index_name),
                        Val::Text(&index_id),
                        Val::Int(i64::from(vi.dimensions)),
                        Val::Text(&distance),
                        Val::Text(&vec_attr),
                        match &search_schema {
                            Some(s) => Val::Text(s),
                            None => Val::Null,
                        },
                        Val::Text(&proj),
                    ],
                )
                .map_err(StorageError::Internal)?;
        }
        Ok(())
    }

    /// Read a table's vector index catalog rows in the shared decoded shape.
    ///
    /// `backfilling` is `None` unconditionally: this backend has no backfill
    /// (see the schema notes), so the member is never reported, which is also
    /// how the service reports an ACTIVE index.
    pub(crate) fn vector_catalog_rows(
        &self,
        table_id: &str,
    ) -> Result<Vec<VectorIndexCatalogRow>, StorageError> {
        let rows = self
            .db
            .query(
                "SELECT index_name, dimensions, distance_function, vector_attribute, \
                 search_schema, projection, index_status \
                 FROM vector_indexes WHERE table_id = ? ORDER BY index_name",
                &[Val::Text(table_id)],
            )
            .map_err(StorageError::Internal)?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let parse = |i: usize, col: &str| -> Result<serde_json::Value, StorageError> {
                serde_json::from_str(r.text(i).unwrap_or("null"))
                    .map_err(|e| StorageError::Internal(format!("{col}: {e}")))
            };
            out.push(VectorIndexCatalogRow {
                index_name: r.text(0).unwrap_or_default().to_owned(),
                dimensions: r.i64(1).unwrap_or_default(),
                distance_function: r.text(2).unwrap_or_default().to_owned(),
                vector_attribute: parse(3, "vector_attribute")?,
                search_schema: match r.text(4) {
                    Some(_) => Some(parse(4, "search_schema")?),
                    None => None,
                },
                projection: parse(5, "projection")?,
                index_status: r.text(6).unwrap_or_default().to_owned(),
                backfilling: None,
            });
        }
        Ok(out)
    }

    /// Load the vector indexes of a table in the shape the write path needs.
    ///
    /// Mirrors the native `fetch_vector_indexes_for_table`: read from the
    /// catalog rather than the cached `TableKeyInfo`, because the cache carries
    /// the search schema but not the index id, and the id keys the data rows.
    fn fetch_vector_metas(&self, table_id: &str) -> Result<Vec<VectorIndexMeta>, StorageError> {
        let rows = self
            .db
            .query(
                "SELECT index_id, dimensions, vector_attribute, search_schema, projection \
                 FROM vector_indexes WHERE table_id = ?",
                &[Val::Text(table_id)],
            )
            .map_err(StorageError::Internal)?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let attr: extenddb_core::types::VectorAttribute =
                serde_json::from_str(r.text(2).unwrap_or("null"))
                    .map_err(|e| StorageError::Internal(format!("vector_attribute: {e}")))?;
            let (hash_attribute_name, search_schema_attribute_names) = match r.text(3) {
                Some(json) => {
                    let elements: Vec<extenddb_core::types::SearchSchemaElement> =
                        serde_json::from_str(json)
                            .map_err(|e| StorageError::Internal(format!("search_schema: {e}")))?;
                    let hash = elements
                        .iter()
                        .find(|e| e.element_type == SearchSchemaElementType::Hash)
                        .map(|e| e.attribute_name.clone());
                    let all = elements
                        .into_iter()
                        .map(|e| e.attribute_name)
                        .collect::<Vec<_>>();
                    (hash, all)
                }
                None => (None, Vec::new()),
            };
            let projection: extenddb_core::types::Projection =
                serde_json::from_str(r.text(4).unwrap_or("null"))
                    .map_err(|e| StorageError::Internal(format!("vector projection: {e}")))?;
            let dimensions = r.i64(1).unwrap_or_default();
            out.push(VectorIndexMeta {
                index_id: r.text(0).unwrap_or_default().to_owned(),
                dimensions: usize::try_from(dimensions).map_err(|_| {
                    StorageError::Internal(format!("vector dimensions out of range: {dimensions}"))
                })?,
                vector_attribute_name: attr.attribute_name,
                hash_attribute_name,
                search_schema_attribute_names,
                projection,
            });
        }
        Ok(out)
    }

    /// Maintain every vector index on a table for one item write.
    ///
    /// The wasm counterpart of the native `maintain_vector_indexes`, with the
    /// delay branch removed: there is no worker here, so every write applies
    /// inline, which is exactly the native `delay_ms == 0` arm. The
    /// delete-then-insert shape is kept because it is what makes a HASH-attribute
    /// change move the row between partitions rather than duplicate it: the row
    /// is keyed by the base item, and the base key is immutable, so `old` and
    /// `new` name the same row and either serves as the delete key.
    ///
    /// `old_item` and `new_item` follow the native convention: a put supplies
    /// the new image (the old is not needed, the base key is shared), a delete
    /// supplies only the old.
    pub(crate) fn maintain_vector_indexes(
        &self,
        key_info: &TableKeyInfo,
        old_item: Option<&Item>,
        new_item: Option<&Item>,
    ) -> Result<(), StorageError> {
        let metas = self.fetch_vector_metas(&key_info.table_id)?;
        if metas.is_empty() {
            return Ok(());
        }
        let source = old_item.or(new_item).ok_or_else(|| {
            StorageError::Internal("vector maintenance called with no item image".to_owned())
        })?;
        let (hash_val, range_val) = extract_keys(&key_info.base_key_schema, source)?;

        for meta in &metas {
            // Remove any existing row for this base item first, whatever
            // partition it was in.
            self.db
                .execute(
                    "DELETE FROM vector_rows \
                     WHERE table_id = ? AND index_id = ? AND hash_val = ? AND range_val = ?",
                    &[
                        Val::Text(&key_info.table_id),
                        Val::Text(&meta.index_id),
                        Val::Text(&hash_val),
                        Val::Text(&range_val),
                    ],
                )
                .map_err(StorageError::Internal)?;

            let Some(new_item) = new_item else {
                continue; // A delete: removal above is the whole of the work.
            };
            if !item_is_indexable(new_item, meta) {
                continue;
            }
            let value = new_item.get(&meta.vector_attribute_name).ok_or_else(|| {
                StorageError::Internal("indexable check passed but the vector is absent".to_owned())
            })?;
            let components = vector_components(value).ok_or_else(|| {
                // Core validates the write before it reaches storage, so a
                // malformed vector here means validation was bypassed rather
                // than that a caller sent bad input.
                StorageError::Internal(
                    "vector attribute reached storage without passing validation".to_owned(),
                )
            })?;
            if components.len() != meta.dimensions {
                return Err(StorageError::Internal(format!(
                    "vector has {} components, index declares {}",
                    components.len(),
                    meta.dimensions
                )));
            }
            let mut blob = Vec::with_capacity(components.len() * 4);
            for x in &components {
                blob.extend_from_slice(&x.to_le_bytes());
            }
            let part = item_partition(new_item, meta)?;
            let projected = projected_payload(new_item, &key_info.base_key_schema, meta);
            let item_json = serde_json::to_string(&projected)
                .map_err(|e| StorageError::Internal(format!("serialize item: {e}")))?;
            // Plain INSERT, matching the native write path's contract: the row
            // for this base key was just deleted, so a conflict means a broken
            // invariant and must be loud.
            self.db
                .execute(
                    "INSERT INTO vector_rows \
                     (table_id, index_id, hash_val, range_val, part, vec, item) \
                     VALUES (?, ?, ?, ?, ?, ?, ?)",
                    &[
                        Val::Text(&key_info.table_id),
                        Val::Text(&meta.index_id),
                        Val::Text(&hash_val),
                        Val::Text(&range_val),
                        Val::Text(&part),
                        Val::Blob(&blob),
                        Val::Text(&item_json),
                    ],
                )
                .map_err(StorageError::Internal)?;
        }
        Ok(())
    }

    /// Exact top-k scan over one partition of one vector index.
    ///
    /// The synchronous body behind this backend's `VectorSearchEngine`
    /// implementation; the SQL differs from native only in naming the shared
    /// `vector_rows` table instead of a per-index one.
    pub(crate) fn search_vectors_impl(
        &self,
        table_id: &str,
        index_name: &str,
        query_vector: &[f32],
        top_k: i64,
        partition: &str,
        filters: &[(String, AttributeValue)],
    ) -> VectorSearchResult {
        // The index definition comes from the catalog rather than from
        // TableKeyInfo, because the cached key info carries dimensions and the
        // search schema but not the distance function, without which a score
        // cannot be computed or ordered.
        let row = self
            .db
            .query_opt(
                "SELECT index_id, dimensions, distance_function, vector_attribute \
                 FROM vector_indexes WHERE table_id = ? AND index_name = ?",
                &[Val::Text(table_id), Val::Text(index_name)],
            )
            .map_err(StorageError::Internal)?
            .ok_or_else(|| StorageError::IndexNotFound(index_name.to_owned()))?;

        let index_id = row.text(0).unwrap_or_default().to_owned();
        let raw_dimensions = row.i64(1).unwrap_or_default();
        let dimensions = usize::try_from(raw_dimensions).map_err(|_| {
            StorageError::Internal(format!("vector dimensions out of range: {raw_dimensions}"))
        })?;
        let distance_raw = row.text(2).unwrap_or_default();
        let function: DistanceFunction = serde_json::from_str(&format!("\"{distance_raw}\""))
            .map_err(|e| StorageError::Internal(format!("unknown distance function: {e}")))?;
        // Stored as the serialized `VectorAttribute`, not a bare name, so it is
        // deserialized exactly as the write path does.
        let vector_attribute_name = serde_json::from_str::<extenddb_core::types::VectorAttribute>(
            row.text(3).unwrap_or("null"),
        )
        .map_err(|e| StorageError::Internal(format!("vector_attribute: {e}")))?
        .attribute_name;

        if query_vector.len() != dimensions {
            // Core validates this against the cached key info, so reaching here
            // means the catalog and the cache disagree.
            return Err(StorageError::Validation(format!(
                "query vector has {} dimensions, index expects {dimensions}",
                query_vector.len()
            )));
        }

        let k = usize::try_from(top_k.max(0)).unwrap_or(0);
        let mut top = TopK::new(k, function);

        // Fetched in one go rather than streamed: `WasmDb` materialises result
        // rows anyway, and the browser workload is bounded (thousands of rows).
        let rows = self
            .db
            .query(
                "SELECT vec, item FROM vector_rows \
                 WHERE table_id = ? AND index_id = ? AND part = ?",
                &[
                    Val::Text(table_id),
                    Val::Text(&index_id),
                    Val::Text(partition),
                ],
            )
            .map_err(StorageError::Internal)?;

        for r in &rows {
            let blob = r
                .blob(0)
                .ok_or_else(|| StorageError::Internal("vector column is not a blob".to_owned()))?;
            let candidate = decode_vector(blob, dimensions)?;
            let item: Item = serde_json::from_str(r.text(1).unwrap_or("{}"))
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

            let candidate_score = score(function, query_vector, &candidate);
            top.offer(candidate_score, item, candidate);
        }

        Ok(VectorSearchOutput {
            hits: top
                .into_hits()
                .into_iter()
                .map(|(score, mut item, components)| {
                    // Reinstated from the stored `f32`s rather than from a second
                    // copy in the payload, so what comes back is the narrowed
                    // value that was actually indexed. The engine drops it again
                    // unless a `ProjectionExpression` names it.
                    item.insert(
                        vector_attribute_name.clone(),
                        extenddb_core::validation::vector_item::vector_attribute(&components),
                    );
                    VectorHit { item, score }
                })
                .collect(),
            distance_function: function,
        })
    }
}

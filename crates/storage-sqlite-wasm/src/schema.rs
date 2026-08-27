// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0

//! Catalog schema for the wasm SQLite backend (milestone M2).
//!
//! Streamlined subset of PR #182's catalog: enough to round-trip the core data
//! plane through the real engine. Items live in one shared `items` table keyed
//! by `(table_id, hash_val, range_val)`. Tables are created ACTIVE immediately
//! (wasm has no control-plane worker). The full #182 schema (per-table data
//! tables, indexes, streams, tags, order-preserving number keys) is a later
//! expansion.
//!
//! Vector indexes diverge from the native `storage-sqlite` layout in three
//! deliberate, bounded ways:
//!
//! * One shared `vector_rows` table keyed by `(table_id, index_id, base key)`
//!   instead of a data table per index, mirroring how this backend keys every
//!   item in one shared `items` table. The base key is the same canonical-JSON
//!   `(hash_val, range_val)` pair `items` uses, not native's typed
//!   order-preserving columns: vector rows are only ever looked up by exact
//!   base key (displacement on write) or by `part` (search), never range-scanned.
//! * No `backfilling` column. The only creation path here is `CreateTable`
//!   (`UpdateTable` is unsupported on wasm), the table is empty at that point,
//!   and there is no worker to run a backfill, so every index is ACTIVE from
//!   birth and the member would never be reported.
//! * No `nrm` column. Native keeps it only because dropping it would need a
//!   data migration; the shared scorer recomputes norms in f64 and never reads
//!   it. A fresh schema has no migration to avoid.

use crate::db::WasmDb;

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS tables (
  account_id             TEXT NOT NULL,
  table_name             TEXT NOT NULL,
  table_id               TEXT NOT NULL,
  key_schema             TEXT NOT NULL,
  attribute_definitions  TEXT NOT NULL,
  billing_mode           TEXT NOT NULL DEFAULT 'PROVISIONED',
  provisioned_throughput TEXT,
  deletion_protection    INTEGER NOT NULL DEFAULT 0,
  table_status           TEXT NOT NULL DEFAULT 'ACTIVE',
  creation_epoch         INTEGER NOT NULL,
  table_arn              TEXT NOT NULL,
  PRIMARY KEY (account_id, table_name),
  CONSTRAINT tables_table_id_unique UNIQUE (table_id)
);
CREATE TABLE IF NOT EXISTS items (
  table_id  TEXT NOT NULL,
  hash_val  TEXT NOT NULL,
  range_val TEXT NOT NULL DEFAULT '',
  item      TEXT NOT NULL,
  PRIMARY KEY (table_id, hash_val, range_val)
);
CREATE TABLE IF NOT EXISTS vector_indexes (
  table_id          TEXT NOT NULL,
  index_name        TEXT NOT NULL,
  index_id          TEXT NOT NULL,
  dimensions        INTEGER NOT NULL,
  distance_function TEXT NOT NULL,
  vector_attribute  TEXT NOT NULL,
  search_schema     TEXT,
  projection        TEXT NOT NULL,
  index_status      TEXT NOT NULL DEFAULT 'ACTIVE',
  PRIMARY KEY (table_id, index_name)
);
CREATE TABLE IF NOT EXISTS vector_rows (
  table_id  TEXT NOT NULL,
  index_id  TEXT NOT NULL,
  hash_val  TEXT NOT NULL,
  range_val TEXT NOT NULL DEFAULT '',
  part      TEXT NOT NULL,
  vec       BLOB NOT NULL,
  item      TEXT NOT NULL,
  PRIMARY KEY (table_id, index_id, hash_val, range_val)
);
CREATE INDEX IF NOT EXISTS vector_rows_by_part ON vector_rows (table_id, index_id, part);";

/// Apply the catalog schema to a fresh connection.
pub(crate) fn apply_schema(db: &WasmDb) -> Result<(), String> {
    db.exec(SCHEMA_SQL)
}

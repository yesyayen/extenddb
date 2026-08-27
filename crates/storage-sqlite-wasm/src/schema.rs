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
);";

/// Apply the catalog schema to a fresh connection.
pub(crate) fn apply_schema(db: &WasmDb) -> Result<(), String> {
    db.exec(SCHEMA_SQL)
}

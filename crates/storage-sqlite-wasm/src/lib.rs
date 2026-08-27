// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0

//! SQLite storage backend for the browser/WASM target (milestone M2).
//!
//! This is the wasm sibling of `extenddb-storage-sqlite`. It implements the same
//! `StorageEngine` trait surface over the same catalog/data SQL (reused verbatim
//! from PR #182), but swaps the execution layer from `sqlx` (tokio + C
//! `libsqlite3` + a worker thread, none of which target `wasm32-unknown-unknown`)
//! to `sqlite-wasm-rs` (SQLite compiled to wasm, synchronous, in-RAM memory VFS).
//!
//! Why a separate crate rather than cfg-gating `storage-sqlite` in place:
//! `sqlx` cannot compile to `wasm32-unknown-unknown` at all, and it is woven
//! through ~915 sites in `storage-sqlite`. An in-place cfg-gate would keep the
//! wasm build red until every one of those sites was abstracted in a single
//! pass. A parallel crate keeps the wasm build compiling and testable from the
//! first milestone (M2a) and isolates the native build from all porting risk.
//! Divergence is bounded because the SQL strings (the conformance-critical part)
//! are copied verbatim; only the execution primitive differs.
//!
//! Concurrency: wasm32 is single-threaded and `sqlite-wasm-rs` is built
//! `SQLITE_THREADSAFE=0`, so the native backend's `write_lock` /
//! multi-connection pool is unnecessary here. `BEGIN IMMEDIATE` is retained to
//! preserve #182's transaction semantics.
//!
//! Off wasm32 the crate is empty (see the `cfg`), so native
//! `cargo build --workspace` stays green.

#![cfg(target_arch = "wasm32")]

pub mod db;
pub mod engine;
mod ops;
mod schema;
mod vector;

pub use engine::SqliteWasmEngine;

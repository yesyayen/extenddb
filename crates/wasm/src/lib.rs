// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0

//! Browser/WASM entry point for ExtendDB.
//!
//! `dispatch(target, body)` runs a DynamoDB request through the real
//! `extenddb_engine::dispatch` over a `SqliteWasmEngine` (SQLite compiled to
//! wasm via `sqlite-wasm-rs`, in-memory). This is the M2a vertical slice: the
//! full engine and storage traits cross to wasm, with `create_table`,
//! `put_item`, and `get_item` implemented end to end and the remaining
//! operations returning a clear "not yet ported" error.
//!
//! The engine is async but the in-memory SQLite backend is synchronous, so a
//! minimal poll-once executor drives each dispatch to completion. Phase 2
//! (OPFS persistence in a Web Worker) will move this to a real async surface.
//!
//! Off wasm32 the crate is empty so native `cargo build --workspace` stays
//! green.

#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use extenddb_core::limits::LimitsConfig;
use extenddb_engine::{OperationContext, dispatch as engine_dispatch};
use extenddb_storage_sqlite_wasm::SqliteWasmEngine;
use serde_json::Value;
use wasm_bindgen::prelude::*;

const REGION: &str = "us-east-1";
const ACCOUNT_ID: &str = "000000000000";
const SERVER_ADDR: &str = "localhost:8000";

thread_local! {
    static ENGINE: RefCell<Option<Arc<SqliteWasmEngine>>> = const { RefCell::new(None) };
}

/// Initialize the in-memory engine. Call once before `dispatch`.
#[wasm_bindgen]
pub fn init() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    let engine = SqliteWasmEngine::open_memory(REGION)
        .map_err(|e| JsValue::from_str(&format!("engine init failed: {e:?}")))?;
    ENGINE.with(|slot| *slot.borrow_mut() = Some(Arc::new(engine)));
    Ok(())
}

/// Run one request, returning (HTTP status code, JSON body string).
fn run(target: &str, body: &str) -> (u16, String) {
    let op = target.rsplit('.').next().unwrap_or(target).to_string();
    let body_json: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => {
            return (
                400,
                error_doc(
                    "com.amazon.coral.service#SerializationException",
                    &e.to_string(),
                ),
            );
        }
    };

    ENGINE.with(|slot| {
        let guard = slot.borrow();
        let Some(engine) = guard.as_ref() else {
            return (
                500,
                error_doc(
                    "com.amazonaws.dynamodb.v20120810#InternalServerError",
                    "engine not initialized; call init() first",
                ),
            );
        };

        let storage: Arc<dyn extenddb_storage::StorageEngine> = engine.clone();
        let ctx = OperationContext {
            storage,
            limits: Arc::new(LimitsConfig::default()),
            region: Arc::from(REGION),
            account_id: Arc::from(ACCOUNT_ID),
            import_paths: Arc::from(Vec::<Arc<PathBuf>>::new()),
            export_paths: Arc::from(Vec::<Arc<PathBuf>>::new()),
            pre_fetched_key_info: None,
            auth_cache: extenddb_auth::AuthCacheRegistry::empty(),
            table_key_info_lookup: None,
        };

        match block_on(engine_dispatch(&op, body_json, &ctx, SERVER_ADDR)) {
            Ok(result) => (200, result.body.to_string()),
            Err(err) => {
                let status = err.status_code();
                let mut doc = serde_json::json!({ "__type": err.full_error_type() });
                let msg = err.message();
                if !msg.is_empty() {
                    doc["message"] = Value::String(msg.to_string());
                }
                (status, doc.to_string())
            }
        }
    })
}

/// Service one DynamoDB request, returning the JSON response body (or a
/// DynamoDB-shaped error document). `target` is the `X-Amz-Target` value
/// (e.g. `DynamoDB_20120810.PutItem`); `body` is the JSON request.
#[wasm_bindgen]
pub fn dispatch(target: &str, body: &str) -> String {
    run(target, body).1
}

/// Like `dispatch`, but returns `{"statusCode": <u16>, "body": <json string>}`
/// so an HTTP-shaped caller (the AWS SDK v3 requestHandler shim) can build a
/// proper HttpResponse and let the SDK map non-2xx + `__type` to the right
/// exception class.
#[wasm_bindgen]
pub fn dispatch_http(target: &str, body: &str) -> String {
    let (status_code, body) = run(target, body);
    serde_json::json!({ "statusCode": status_code, "body": body }).to_string()
}

fn error_doc(full_type: &str, message: &str) -> String {
    serde_json::json!({ "__type": full_type, "message": message }).to_string()
}

/// Poll-once executor. The in-memory SQLite backend resolves synchronously, so
/// the dispatch future is Ready on the first poll. If it ever returns Pending
/// (e.g. a real async I/O path added later without wiring wasm-bindgen-futures),
/// fail fast rather than spin: a no-op waker can never wake it.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut cx = Context::from_waker(Waker::noop());
    let mut fut = Box::pin(future);
    match fut.as_mut().poll(&mut cx) {
        Poll::Ready(v) => v,
        Poll::Pending => panic!(
            "wasm dispatch future returned Pending; the in-memory backend must resolve \
             synchronously (async I/O is not wired until Phase 2)"
        ),
    }
}

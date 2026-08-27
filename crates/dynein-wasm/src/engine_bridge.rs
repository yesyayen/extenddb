// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0

//! Transport bridge: a custom aws-smithy `HttpConnector` that routes DynamoDB
//! requests to `extenddb_engine::dispatch` over an in-memory SQLite-wasm
//! backend, plus a hand-built `SdkConfig` (no aws-config, no network) that
//! dynein's `build_sdk_config` returns on wasm.

use std::cell::RefCell;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, UNIX_EPOCH};

use aws_credential_types::Credentials;
use aws_smithy_runtime_api::client::http::{
    HttpClient, HttpConnector, HttpConnectorFuture, HttpConnectorSettings, SharedHttpConnector,
};
use aws_smithy_runtime_api::client::identity::SharedIdentityCache;
use aws_smithy_runtime_api::client::orchestrator::{HttpRequest, HttpResponse};
use aws_smithy_runtime_api::client::runtime_components::RuntimeComponents;
use aws_smithy_runtime_api::http::StatusCode;
use aws_smithy_types::body::SdkBody;
use aws_types::region::Region;
use aws_types::sdk_config::SharedCredentialsProvider;
use aws_types::SdkConfig;

use extenddb_core::limits::LimitsConfig;
use extenddb_engine::{dispatch as engine_dispatch, OperationContext};
use extenddb_storage_sqlite_wasm::SqliteWasmEngine;
use serde_json::Value;
use wasm_bindgen::prelude::*;

const REGION: &str = "us-east-1";
const ACCOUNT_ID: &str = "000000000000";
const SERVER_ADDR: &str = "localhost:8000";

thread_local! {
    static ENGINE: RefCell<Option<Arc<SqliteWasmEngine>>> = const { RefCell::new(None) };
    // Optional JS host dispatch. When set, the connector routes every request to
    // it instead of the in-process engine, so an external engine backs dynein.
    static HOST_DISPATCH: RefCell<Option<js_sys::Function>> = const { RefCell::new(None) };
}

/// Register a JS host dispatch `(target, body) -> '{"statusCode":N,"body":"..."}'`
/// (the same shape as the main module's `dispatch_http`). Once set, every dynein
/// request is routed to it instead of this module's in-process engine, so one
/// shared engine can back dynein alongside the CLI / SDK / Raw interfaces and
/// they all see the same database. Without it, the in-process engine is used
/// (the standalone / node path).
#[wasm_bindgen]
pub fn set_host_dispatch(f: js_sys::Function) {
    HOST_DISPATCH.with(|slot| *slot.borrow_mut() = Some(f));
}

/// Clear a previously registered host dispatch, reverting to the in-process engine.
#[wasm_bindgen]
pub fn clear_host_dispatch() {
    HOST_DISPATCH.with(|slot| *slot.borrow_mut() = None);
}

/// Route a request to the registered JS host dispatch, if any. Returns
/// `(status, body)` parsed from the host's `{"statusCode","body"}` reply.
fn host_sink(target: &str, body: &str) -> Option<(u16, String)> {
    HOST_DISPATCH.with(|slot| {
        let borrowed = slot.borrow();
        let f = borrowed.as_ref()?;
        let res = f
            .call2(
                &JsValue::NULL,
                &JsValue::from_str(target),
                &JsValue::from_str(body),
            )
            .ok()?;
        let s = res.as_string()?;
        let v: Value = serde_json::from_str(&s).ok()?;
        let status = v.get("statusCode").and_then(|x| x.as_u64()).unwrap_or(200) as u16;
        let resp_body = match v.get("body") {
            Some(Value::String(s)) => s.clone(),
            Some(other) => other.to_string(),
            None => String::new(),
        };
        Some((status, resp_body))
    })
}

fn ensure_engine() -> Arc<SqliteWasmEngine> {
    ENGINE.with(|slot| {
        if slot.borrow().is_none() {
            let engine = SqliteWasmEngine::open_memory(REGION).expect("engine init");
            *slot.borrow_mut() = Some(Arc::new(engine));
        }
        slot.borrow().as_ref().unwrap().clone()
    })
}

fn engine_sink(target: &str, body: &str) -> (u16, String) {
    // Shared-engine path: if a JS host dispatch is registered, route to it.
    if let Some(resp) = host_sink(target, body) {
        return resp;
    }
    let op = target.rsplit('.').next().unwrap_or(target).to_string();
    let body_json: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => {
            return (
                400,
                serde_json::json!({
                    "__type": "com.amazon.coral.service#SerializationException",
                    "message": e.to_string()
                })
                .to_string(),
            )
        }
    };
    let engine = ensure_engine();
    let storage: Arc<dyn extenddb_storage::StorageEngine> = engine;
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
}

#[derive(Clone)]
struct EngineConnector;

impl std::fmt::Debug for EngineConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EngineConnector")
    }
}

impl HttpConnector for EngineConnector {
    fn call(&self, request: HttpRequest) -> HttpConnectorFuture {
        let target = request
            .headers()
            .get("x-amz-target")
            .unwrap_or_default()
            .to_string();
        let body = request
            .body()
            .bytes()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();
        let (status, resp_body) = engine_sink(&target, &body);
        let mut response =
            HttpResponse::new(StatusCode::try_from(status).unwrap(), SdkBody::from(resp_body));
        response
            .headers_mut()
            .insert("content-type", "application/x-amz-json-1.0");
        HttpConnectorFuture::ready(Ok(response))
    }
}

impl HttpClient for EngineConnector {
    fn http_connector(
        &self,
        _settings: &HttpConnectorSettings,
        _components: &RuntimeComponents,
    ) -> SharedHttpConnector {
        SharedHttpConnector::new(self.clone())
    }
}

/// Hand-built SdkConfig with the engine transport. No aws-config, no network;
/// every sleep-driven subsystem is disabled so no async runtime is required.
pub fn wasm_sdk_config(region_name: &str) -> SdkConfig {
    SdkConfig::builder()
        .region(Region::new(region_name.to_owned()))
        .endpoint_url("http://in-process.invalid")
        .credentials_provider(SharedCredentialsProvider::new(Credentials::new(
            "fake", "fake", None, None, "wasm",
        )))
        .identity_cache(SharedIdentityCache::from(
            aws_smithy_runtime::client::identity::IdentityCache::no_cache(),
        ))
        .http_client(EngineConnector)
        .time_source(aws_smithy_async::time::StaticTimeSource::new(
            UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        ))
        .retry_config(aws_smithy_types::retry::RetryConfig::disabled())
        .timeout_config(aws_smithy_types::timeout::TimeoutConfig::disabled())
        .stalled_stream_protection(
            aws_smithy_runtime_api::client::stalled_stream_protection::StalledStreamProtectionConfig::disabled(),
        )
        .behavior_version(aws_smithy_runtime_api::client::behavior_version::BehaviorVersion::latest())
        .build()
}

/// Poll-loop executor: the engine and connector resolve synchronously and all
/// sleep-driven subsystems are disabled, so a noop waker makes progress.
pub fn block_on<F: Future>(fut: F) -> F::Output {
    let mut fut = Box::pin(fut);
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    for _ in 0..1_000_000 {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
    }
    panic!("block_on: future did not resolve (unexpected Pending; no async IO expected)");
}

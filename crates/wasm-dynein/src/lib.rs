// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0

//! One wasm module: the real `aws-sdk-dynamodb` client driving the real
//! `extenddb_engine::dispatch` through an in-process transport. No network,
//! no tokio, no JS bounce: the SDK's `HttpConnector` calls the engine directly.
//!
//! `run_demo()` performs CreateTable + PutItem + GetItem + Scan via the SDK's
//! typed fluent API and returns a JSON summary, proving the SDK codec and the
//! engine execute together in a single wasm binary.

#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, UNIX_EPOCH};

use aws_sdk_dynamodb::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_dynamodb::types::{
    AttributeDefinition, AttributeValue, BillingMode, KeySchemaElement, KeyType,
    ScalarAttributeType,
};
use aws_sdk_dynamodb::{Client, Config};
use aws_smithy_runtime_api::client::http::{
    HttpClient, HttpConnector, HttpConnectorFuture, HttpConnectorSettings, SharedHttpConnector,
};
use aws_smithy_runtime_api::client::orchestrator::{HttpRequest, HttpResponse};
use aws_smithy_runtime_api::client::runtime_components::RuntimeComponents;
use aws_smithy_runtime_api::http::StatusCode;
use aws_smithy_types::body::SdkBody;
use wasm_bindgen::prelude::*;

use extenddb_core::limits::LimitsConfig;
use extenddb_engine::{dispatch as engine_dispatch, OperationContext};
use extenddb_storage_sqlite_wasm::SqliteWasmEngine;
use serde_json::Value;

const REGION: &str = "us-east-1";
const ACCOUNT_ID: &str = "000000000000";
const SERVER_ADDR: &str = "localhost:8000";

thread_local! {
    static ENGINE: RefCell<Option<Arc<SqliteWasmEngine>>> = const { RefCell::new(None) };
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

/// The transport sink: (X-Amz-Target, JSON body) -> (HTTP status, JSON body),
/// executed by the real engine over an in-memory SQLite-wasm backend. This is
/// the same dispatch the browser demo uses, reached here from inside the SDK.
fn engine_sink(target: &str, body: &str) -> (u16, String) {
    let op = target.rsplit('.').next().unwrap_or(target).to_string();
    let body_json: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => {
            return (
                400,
                error_doc("com.amazon.coral.service#SerializationException", &e.to_string()),
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

fn error_doc(full_type: &str, message: &str) -> String {
    serde_json::json!({ "__type": full_type, "message": message }).to_string()
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

fn build_client() -> Client {
    let config = Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("local"))
        .credentials_provider(Credentials::new("fake", "fake", None, None, "wasm"))
        .identity_cache(aws_sdk_dynamodb::config::IdentityCache::no_cache())
        .endpoint_url("http://in-process.invalid")
        .http_client(EngineConnector)
        // wasm32-unknown-unknown has no SystemTime::now; the SDK signer calls
        // it. We discard signatures anyway, so pin a fixed time source.
        .time_source(aws_smithy_async::time::StaticTimeSource::new(
            UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        ))
        .retry_config(aws_sdk_dynamodb::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_dynamodb::config::timeout::TimeoutConfig::disabled())
        .stalled_stream_protection(
            aws_sdk_dynamodb::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    Client::from_conf(config)
}

/// Poll-loop executor. Every await in this path resolves to a ready value (the
/// engine is synchronous, the connector returns a ready future, and all
/// sleep-driven subsystems are disabled), so a noop waker makes progress.
fn block_on<F: Future>(fut: F) -> F::Output {
    let mut fut = Box::pin(fut);
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    for _ in 0..100_000 {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => continue,
        }
    }
    panic!("block_on: future did not resolve (unexpected Pending; no async IO expected)");
}

/// End-to-end proof: real SDK operations over the in-module engine.
#[wasm_bindgen]
pub fn run_demo() -> String {
    console_error_panic_hook::set_once();
    let client = build_client();

    block_on(async move {
        macro_rules! tryop {
            ($e:expr, $label:expr) => {
                match $e.await {
                    Ok(v) => v,
                    Err(e) => {
                        return serde_json::json!({ "ok": false, "at": $label, "err": format!("{:?}", e) })
                            .to_string()
                    }
                }
            };
        }

        tryop!(
            client
                .create_table()
                .table_name("dyn0")
                .billing_mode(BillingMode::PayPerRequest)
                .attribute_definitions(
                    AttributeDefinition::builder()
                        .attribute_name("pk")
                        .attribute_type(ScalarAttributeType::S)
                        .build()
                        .unwrap(),
                )
                .key_schema(
                    KeySchemaElement::builder()
                        .attribute_name("pk")
                        .key_type(KeyType::Hash)
                        .build()
                        .unwrap(),
                )
                .send(),
            "create_table"
        );

        tryop!(
            client
                .put_item()
                .table_name("dyn0")
                .item("pk", AttributeValue::S("a".into()))
                .item("v", AttributeValue::N("42".into()))
                .send(),
            "put_item"
        );

        let got = tryop!(
            client
                .get_item()
                .table_name("dyn0")
                .key("pk", AttributeValue::S("a".into()))
                .send(),
            "get_item"
        );
        let v = got
            .item()
            .and_then(|i| i.get("v"))
            .and_then(|av| av.as_n().ok())
            .cloned()
            .unwrap_or_default();

        let scan = tryop!(client.scan().table_name("dyn0").send(), "scan");

        serde_json::json!({
            "ok": true,
            "created": "dyn0",
            "get_v": v,
            "scan_count": scan.count(),
            "note": "aws-sdk-dynamodb codec + extenddb_engine::dispatch, one wasm module, no network"
        })
        .to_string()
    })
}

// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0

//! Dual-target parity: the native SQLite backend and the browser/wasm backend
//! must answer the same request sequence identically.
//!
//! The sequence and the expected responses live beside the wasm Node tests
//! (`crates/wasm/tests-node/vector-parity.requests.json` and
//! `vector-parity.golden.json`). This test drives the sequence through the real
//! engine dispatch over the native backend and asserts the golden file; the
//! Node script `vector-parity.mjs` asserts the same golden through the wasm
//! artifact. Both sides matching one committed file is what makes the equality
//! transitive without either harness having to host the other's runtime.
//!
//! The backend is configured to the wasm backend's fixed semantics
//! (`control_plane_delay_seconds = 0`, `index_propagation_delay_ms = 0`):
//! tables ACTIVE at birth and index maintenance inline. That is a supported
//! native configuration, not a test-only contrivance, so the parity claim is
//! against behaviour a native deployment can actually exhibit.
//!
//! Volatile fields are masked on both sides with the same rules: `TableId` is
//! random and every `*DateTime` is wall-clock. ARNs are deterministic (fixed
//! region and account) and stay unmasked. Scores are asserted bit-exact, which
//! is the point: the scan arithmetic is shared code.
//!
//! Regenerate the golden after an intended behaviour change with
//! `UPDATE_GOLDEN=1 cargo test -p extenddb-storage-sqlite wasm_parity`, then
//! re-run the wasm side.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};

const REQUESTS: &str = include_str!("../../wasm/tests-node/vector-parity.requests.json");

fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests-node/vector-parity.golden.json")
}

/// Mask the volatile fields, identically to `vector-parity.mjs`.
fn mask(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                if key == "TableId" {
                    *v = Value::String("MASKED".to_owned());
                } else if matches!(
                    key.as_str(),
                    "CreationDateTime"
                        | "LastUpdateToPayPerRequestDateTime"
                        | "LastIncreaseDateTime"
                        | "LastDecreaseDateTime"
                ) {
                    *v = json!(0);
                } else {
                    mask(v);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                mask(item);
            }
        }
        _ => {}
    }
}

async fn run_sequence() -> Vec<Value> {
    let engine = crate::SqliteEngine::new(":memory:", 1, "us-east-1", 409_600)
        .await
        .expect("engine");
    crate::schema::apply(&engine.pool).await.expect("schema");
    // The engine dispatches under a fixed account, which must exist: the
    // catalog enforces the foreign key.
    sqlx::query(
        "INSERT INTO accounts (account_id, account_name) VALUES ('000000000000', 'parity')",
    )
    .execute(&engine.pool)
    .await
    .expect("account");
    // The wasm backend's fixed semantics, as native settings.
    for (key, value) in [
        ("control_plane_delay_seconds", "0"),
        ("index_propagation_delay_ms", "0"),
    ] {
        sqlx::query("INSERT OR REPLACE INTO settings (key, value) VALUES (?, ?)")
            .bind(key)
            .bind(value)
            .execute(&engine.pool)
            .await
            .expect("setting");
    }

    let storage: Arc<dyn extenddb_storage::StorageEngine> = Arc::new(engine);
    let ctx = extenddb_engine::OperationContext {
        storage,
        limits: Arc::new(extenddb_core::limits::LimitsConfig::default()),
        region: Arc::from("us-east-1"),
        account_id: Arc::from("000000000000"),
        import_paths: Arc::from(Vec::<Arc<PathBuf>>::new()),
        export_paths: Arc::from(Vec::<Arc<PathBuf>>::new()),
        pre_fetched_key_info: None,
        auth_cache: extenddb_auth::AuthCacheRegistry::empty(),
        table_key_info_lookup: None,
    };

    let requests: Vec<Value> = serde_json::from_str(REQUESTS).expect("requests fixture");
    let mut observed = Vec::with_capacity(requests.len());
    for request in requests {
        let target = request["target"].as_str().expect("target").to_owned();
        let body = request["body"].clone();
        // Status and error-document shape mirror `crates/wasm/src/lib.rs::run`,
        // which is what the wasm side of this comparison serves.
        let (status, response_body) =
            match extenddb_engine::dispatch(&target, body, &ctx, "localhost:8000").await {
                Ok(result) => (200u16, result.body),
                Err(err) => {
                    let mut doc = json!({ "__type": err.full_error_type() });
                    let msg = err.message();
                    if !msg.is_empty() {
                        doc["message"] = Value::String(msg.to_owned());
                    }
                    (err.status_code(), doc)
                }
            };
        let mut entry = json!({ "target": target, "status": status, "body": response_body });
        mask(&mut entry);
        observed.push(entry);
    }
    observed
}

/// The native backend must answer the wasm parity sequence exactly as the
/// committed golden says, score bits included.
#[tokio::test]
async fn the_native_backend_matches_the_wasm_parity_golden() {
    let observed = run_sequence().await;
    let rendered = serde_json::to_string_pretty(&observed).expect("render") + "\n";

    let path = golden_path();
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &rendered).expect("write golden");
        return;
    }
    let golden = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("golden file missing ({e}); regenerate with UPDATE_GOLDEN=1"));
    let golden_value: Vec<Value> = serde_json::from_str(&golden).expect("golden JSON");
    for (i, (got, want)) in observed.iter().zip(&golden_value).enumerate() {
        assert_eq!(
            got, want,
            "response {i} ({}) diverged from the golden",
            got["target"]
        );
    }
    assert_eq!(
        observed.len(),
        golden_value.len(),
        "sequence length changed"
    );
}

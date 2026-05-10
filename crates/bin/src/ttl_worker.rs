// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0

//! TTL cleanup background worker.
//!
//! Uses an indexed sweep: only processes tables where the TTL expression index
//! is ready. Queries use ORDER BY + LIMIT for efficient index scans.

use std::sync::Arc;

use extenddb_storage::management_store::SettingsStore;
use extenddb_storage::{DataEngine, MetadataEngine, StreamEngine, TableEngine};

/// REQ-CTRL-006: Background worker that periodically scans for and deletes
/// expired items on tables with TTL enabled.
///
/// Uses an indexed sweep: only processes tables where the TTL expression index
/// is ready. Queries use ORDER BY + LIMIT for efficient index scans.
///
/// Deletions are routed through `DataEngine::delete_item` so that GSI/LSI
/// index tables are kept in sync. Stream records are captured with
/// `userIdentity` set to the `DynamoDB` service principal per AWS docs.
///
/// Records staleness metrics (seconds between TTL expiry and deletion).
/// Iterates all accounts to handle multi-account deployments.
pub(crate) async fn ttl_cleanup_worker<E, S>(
    storage: Arc<E>,
    region: String,
    metrics: Arc<extenddb_core::metrics::MetricsCollector>,
    _settings: Arc<S>,
) where
    E: DataEngine + MetadataEngine + TableEngine + StreamEngine + Send + Sync + 'static,
    S: SettingsStore,
{
    use std::time::Duration;

    const SCAN_INTERVAL: Duration = Duration::from_secs(60);

    let region_arc: Arc<str> = Arc::from(region.as_str());

    loop {
        tokio::time::sleep(SCAN_INTERVAL).await;
        retry_pending_indexes(&*storage).await;
        sweep_expired_items(&*storage, &metrics, &region_arc).await;
    }
}

/// Retry index creation for tables that have TTL enabled but index not ready.
async fn retry_pending_indexes<E>(storage: &E)
where
    E: DataEngine + MetadataEngine + TableEngine + StreamEngine + Send + Sync + 'static,
{
    let Ok(pending) = storage.all_tables_with_ttl().await else {
        return;
    };
    let Ok(ready) = storage.all_tables_with_ttl_index_ready().await else {
        return;
    };
    let ready_set: std::collections::HashSet<(&str, &str)> = ready
        .iter()
        .map(|(a, t, _)| (a.as_str(), t.as_str()))
        .collect();
    for (account_id, table_name, ttl_attr) in &pending {
        if !ready_set.contains(&(account_id.as_str(), table_name.as_str())) {
            if let Err(e) = storage
                .create_ttl_index(account_id, table_name, ttl_attr)
                .await
            {
                tracing::debug!("TTL worker: index creation retry failed for {table_name}: {e}");
            } else {
                tracing::info!("TTL worker: index created for {table_name}");
            }
        }
    }
}

/// Sweep tables with ready indexes and delete expired items.
async fn sweep_expired_items<E>(
    storage: &E,
    metrics: &extenddb_core::metrics::MetricsCollector,
    region: &Arc<str>,
) where
    E: DataEngine + MetadataEngine + TableEngine + StreamEngine + Send + Sync + 'static,
{
    use extenddb_core::types::UserIdentity;
    use extenddb_engine::stream_capture;

    const BATCH_SIZE: usize = 100;

    let ttl_identity = UserIdentity {
        identity_type: "Service".to_owned(),
        principal_id: "dynamodb.amazonaws.com".to_owned(),
    };

    let tables = match storage.all_tables_with_ttl_index_ready().await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("TTL worker: failed to list tables: {e}");
            return;
        }
    };

    let now_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    for (account_id, table_name, ttl_attribute) in &tables {
        let items = match storage
            .find_expired_items_indexed(account_id, table_name, ttl_attribute, BATCH_SIZE)
            .await
        {
            Ok(items) => items,
            Err(e) => {
                tracing::warn!("TTL worker: find expired failed for {table_name}: {e}");
                continue;
            }
        };

        if items.is_empty() {
            continue;
        }

        let key_info = match storage.table_key_info(account_id, table_name).await {
            Ok(ki) => ki,
            Err(e) => {
                tracing::warn!("TTL worker: key info failed for {table_name}: {e}");
                continue;
            }
        };

        let view_type = stream_capture::stream_view_type(&key_info);

        // Build condition guard: attribute_exists(#ttl) AND #ttl <= :now
        // Prevents deleting items whose TTL attribute was updated/removed by a
        // foreground operation between the sweep read and this delete.
        let (condition_expr, maps) = build_ttl_condition(ttl_attribute, now_epoch);

        let mut deleted = 0usize;
        for item in &items {
            // Compute staleness: now - item's TTL value.
            let staleness = item
                .get(ttl_attribute.as_str())
                .and_then(|av| {
                    if let extenddb_core::types::AttributeValue::N(n) = av {
                        n.parse::<u64>().ok()
                    } else {
                        None
                    }
                })
                .map(|ttl_val| now_epoch.saturating_sub(ttl_val));

            // Extract key attributes from the full item.
            let key: extenddb_core::types::Item = key_info
                .key_schema
                .iter()
                .filter_map(|ks| {
                    item.get(&ks.attribute_name)
                        .map(|v| (ks.attribute_name.clone(), v.clone()))
                })
                .collect();

            let return_old = view_type.is_some();
            let stream = view_type.map(|vt| extenddb_storage::StreamCapture {
                view_type: vt,
                user_identity: Some(ttl_identity.clone()),
                region: region.clone(),
            });
            match storage
                .delete_item(
                    &key_info,
                    &key,
                    return_old,
                    Some(&condition_expr),
                    &maps,
                    stream.as_ref(),
                )
                .await
            {
                Err(extenddb_storage::error::StorageError::ConditionFailed(_)) => {
                    // Item was updated by a foreground op — no longer eligible.
                }
                Err(e) => {
                    tracing::warn!("TTL worker: delete failed for {table_name}: {e}");
                }
                Ok(_old_item) => {
                    deleted += 1;
                    metrics.record_ttl_deletion(table_name);
                    if let Some(s) = staleness {
                        #[allow(clippy::cast_precision_loss)]
                        metrics.record_ttl_staleness(table_name, s as f64);
                    }
                }
            }
        }

        if deleted > 0 {
            tracing::info!("TTL worker: deleted {deleted} expired items from {table_name}");
        }
    }

    // ttl_deletion_target_seconds is a documentation-only setting for operators.
    // No runtime action needed — scan interval is fixed at 60s.
}

/// Build the TTL condition guard expression and maps.
///
/// Returns `attribute_exists(#ttl) AND #ttl <= :now` with the appropriate
/// expression maps. Keys follow the internal convention (no `#`/`:` prefixes).
fn build_ttl_condition(
    ttl_attribute: &str,
    now_epoch: u64,
) -> (
    extenddb_core::expression::Expr,
    extenddb_core::expression::ExpressionMaps,
) {
    use extenddb_core::expression::{CompareOp, Expr, ExpressionMaps, PathElement};
    use std::collections::HashMap;

    let ttl_path = vec![PathElement::Attribute("#ttl".to_owned())];
    let condition_expr = Expr::And(
        Box::new(Expr::Function {
            name: "attribute_exists".to_owned(),
            args: vec![Expr::Path(ttl_path.clone())],
        }),
        Box::new(Expr::Compare {
            left: Box::new(Expr::Path(ttl_path)),
            op: CompareOp::Le,
            right: Box::new(Expr::Placeholder("now".to_owned())),
        }),
    );

    let mut names = HashMap::new();
    names.insert("ttl".to_owned(), ttl_attribute.to_owned());
    let mut values = HashMap::new();
    values.insert(
        "now".to_owned(),
        extenddb_core::types::AttributeValue::N(now_epoch.to_string()),
    );

    (condition_expr, ExpressionMaps::new(names, values))
}

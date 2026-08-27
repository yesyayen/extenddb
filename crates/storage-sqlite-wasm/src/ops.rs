// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0

//! Real implementations of the M2a op slice: create_table, table_key_info,
//! put_item, get_item. SQL is ported from PR #182's `storage-sqlite` (adapted
//! to the streamlined M2a schema in `schema.rs`). Execution goes through
//! `WasmDb` instead of `sqlx`.

use std::cmp::Ordering;

use bigdecimal::BigDecimal;
use extenddb_core::expression::{
    CompareOp, Expr, ExpressionMaps, KeyCondition, SortKeyCondition, UpdateAction,
    apply_update_validated, evaluate_condition,
};
use extenddb_core::types::{
    AttributeDefinition, AttributeValue, BillingMode, BillingModeSummary, CreateTableInput,
    DeleteTableInput, Item, KeySchemaElement, KeyType, ListTablesInput, ListTablesOutput,
    ProvisionedThroughput, ProvisionedThroughputDescription, ScalarAttributeType, TableDescription,
    TableKeyInfo, TableStatus,
};
use extenddb_storage::error::StorageError;
use extenddb_storage::util::table_arn;

use crate::db::{Val, is_unique_violation};
use crate::engine::SqliteWasmEngine;

fn intern<E: std::fmt::Display>(e: E) -> StorageError {
    StorageError::Internal(e.to_string())
}

/// Extract the `(hash_val, range_val)` storage key from an item/key map using
/// the table's key schema. Each key attribute value is canonicalized as its
/// serde_json string (Item is a BTreeMap so serialization is deterministic).
///
/// M2a limitation (tracked for M2): this is not type-aware, so numeric keys
/// that differ only by formatting (`{"N":"1"}` vs `{"N":"1.0"}`) would compare
/// as distinct. Correct for string keys and for equal formatting on both write
/// and read paths.
fn extract_keys(
    key_schema: &[KeySchemaElement],
    item: &Item,
) -> Result<(String, String), StorageError> {
    let mut hash_val: Option<String> = None;
    let mut range_val = String::new();
    for k in key_schema {
        let av = item.get(&k.attribute_name).ok_or_else(|| {
            StorageError::Validation(format!(
                "One of the required keys was not given a value ({})",
                k.attribute_name
            ))
        })?;
        let canon = serde_json::to_string(av).map_err(intern)?;
        match k.key_type {
            KeyType::Hash => hash_val = Some(canon),
            KeyType::Range => range_val = canon,
        }
    }
    let hash_val = hash_val
        .ok_or_else(|| StorageError::Internal("table key schema has no HASH key".to_string()))?;
    Ok((hash_val, range_val))
}

/// Project just the key attributes out of a full item (for LastEvaluatedKey).
fn project_key(key_schema: &[KeySchemaElement], item: &Item) -> Item {
    let mut k = Item::new();
    for e in key_schema {
        if let Some(av) = item.get(&e.attribute_name) {
            k.insert(e.attribute_name.clone(), av.clone());
        }
    }
    k
}

/// Resolve a key-condition operand (always a placeholder in DynamoDB key
/// conditions) to its concrete `AttributeValue` via the expression maps.
fn resolve_expr_to_av(expr: &Expr, maps: &ExpressionMaps) -> Result<AttributeValue, StorageError> {
    match expr {
        Expr::Placeholder(name) => maps
            .resolve_value(name)
            .cloned()
            .map_err(|e| StorageError::Validation(e.to_string())),
        _ => Err(StorageError::Validation(
            "expected a placeholder in the key condition".to_owned(),
        )),
    }
}

/// Evaluate an optional ConditionExpression against a target item. Returns
/// `ConditionFailed(None)` when the condition is false (the caller substitutes
/// the pre-image so the engine can surface it on the wire).
fn check_condition(
    condition: Option<&Expr>,
    item: &Item,
    maps: &ExpressionMaps,
) -> Result<(), StorageError> {
    if let Some(cond) = condition {
        let ok = evaluate_condition(cond, item, maps)
            .map_err(|e| StorageError::Validation(e.to_string()))?;
        if !ok {
            return Err(StorageError::ConditionFailed(None));
        }
    }
    Ok(())
}

/// The table's RANGE key attribute name and scalar type, if any.
fn sk_attr_and_type(key_info: &TableKeyInfo) -> Option<(String, ScalarAttributeType)> {
    let range = key_info
        .key_schema
        .iter()
        .find(|e| e.key_type == KeyType::Range)?;
    let ty = key_info
        .attribute_definitions
        .iter()
        .find(|d| d.attribute_name == range.attribute_name)
        .map(|d| d.attribute_type)?;
    Some((range.attribute_name.clone(), ty))
}

/// Compare two sort-key AttributeValues by DynamoDB semantics: S by UTF-8
/// bytes, B by raw bytes, N numerically (BigDecimal, full precision).
fn cmp_av(ty: ScalarAttributeType, a: &AttributeValue, b: &AttributeValue) -> Ordering {
    match (ty, a, b) {
        (ScalarAttributeType::S, AttributeValue::S(x), AttributeValue::S(y)) => x.cmp(y),
        (ScalarAttributeType::B, AttributeValue::B(x), AttributeValue::B(y)) => x.cmp(y),
        (ScalarAttributeType::N, AttributeValue::N(x), AttributeValue::N(y)) => {
            let xd = x.parse::<BigDecimal>().unwrap_or_default();
            let yd = y.parse::<BigDecimal>().unwrap_or_default();
            xd.cmp(&yd)
        }
        _ => Ordering::Equal,
    }
}

/// Evaluate a sort-key condition against an item's sort-key value.
fn sk_satisfies(
    ty: ScalarAttributeType,
    item_sk: &AttributeValue,
    cond: &SortKeyCondition,
    maps: &ExpressionMaps,
) -> Result<bool, StorageError> {
    match cond {
        SortKeyCondition::Compare { op, value, .. } => {
            let v = resolve_expr_to_av(value, maps)?;
            let ord = cmp_av(ty, item_sk, &v);
            Ok(match op {
                CompareOp::Eq => ord == Ordering::Equal,
                CompareOp::Ne => ord != Ordering::Equal,
                CompareOp::Lt => ord == Ordering::Less,
                CompareOp::Le => ord != Ordering::Greater,
                CompareOp::Gt => ord == Ordering::Greater,
                CompareOp::Ge => ord != Ordering::Less,
            })
        }
        SortKeyCondition::Between { low, high, .. } => {
            let lo = resolve_expr_to_av(low, maps)?;
            let hi = resolve_expr_to_av(high, maps)?;
            Ok(cmp_av(ty, item_sk, &lo) != Ordering::Less
                && cmp_av(ty, item_sk, &hi) != Ordering::Greater)
        }
        SortKeyCondition::BeginsWith { prefix, .. } => {
            let p = resolve_expr_to_av(prefix, maps)?;
            match (item_sk, &p) {
                (AttributeValue::S(s), AttributeValue::S(pre)) => Ok(s.starts_with(pre.as_str())),
                (AttributeValue::B(b), AttributeValue::B(pre)) => Ok(b.starts_with(pre.as_slice())),
                _ => Err(StorageError::Validation(
                    "begins_with is supported on string or binary sort keys only".to_owned(),
                )),
            }
        }
    }
}

impl SqliteWasmEngine {
    pub(crate) fn create_table_impl(
        &self,
        account_id: &str,
        input: CreateTableInput,
    ) -> Result<TableDescription, StorageError> {
        // This backend implements base tables only. Refuse index requests
        // instead of accepting them and then lying in DescribeTable: a
        // silently dropped index would surface later as missing data, with
        // no error pointing back here.
        if input
            .global_secondary_indexes
            .as_ref()
            .is_some_and(|v| !v.is_empty())
            || input
                .local_secondary_indexes
                .as_ref()
                .is_some_and(|v| !v.is_empty())
            || input.vector_indexes.as_ref().is_some_and(|v| !v.is_empty())
        {
            return Err(StorageError::Validation(
                "secondary and vector indexes are not supported by the browser/WASM backend"
                    .to_string(),
            ));
        }
        let table_id = uuid::Uuid::new_v4().to_string();
        let table_arn = table_arn(&self.region, account_id, &input.table_name);
        let key_schema_json = serde_json::to_string(&input.key_schema).map_err(intern)?;
        let attr_defs_json = serde_json::to_string(&input.attribute_definitions).map_err(intern)?;
        let now = time::OffsetDateTime::now_utc();
        let creation_epoch = now.unix_timestamp();

        let billing_mode = input.billing_mode.unwrap_or(BillingMode::Provisioned);
        let billing_str = match billing_mode {
            BillingMode::PayPerRequest => "PAY_PER_REQUEST",
            BillingMode::Provisioned => "PROVISIONED",
        };
        let pt_json = input
            .provisioned_throughput
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(intern)?;
        let deletion_protection = input.deletion_protection_enabled.unwrap_or(false);

        let result = self.db.execute(
            "INSERT INTO tables \
             (account_id, table_name, table_id, key_schema, attribute_definitions, \
              billing_mode, provisioned_throughput, deletion_protection, \
              table_status, creation_epoch, table_arn) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'ACTIVE', ?, ?)",
            &[
                Val::Text(account_id),
                Val::Text(&input.table_name),
                Val::Text(&table_id),
                Val::Text(&key_schema_json),
                Val::Text(&attr_defs_json),
                Val::Text(billing_str),
                match &pt_json {
                    Some(s) => Val::Text(s),
                    None => Val::Null,
                },
                Val::Int(i64::from(deletion_protection)),
                Val::Int(creation_epoch),
                Val::Text(&table_arn),
            ],
        );
        if let Err(e) = result {
            return if is_unique_violation(&e) {
                Err(StorageError::TableAlreadyExists(input.table_name.clone()))
            } else {
                Err(StorageError::Internal(e))
            };
        }

        let (rcu, wcu) = input.provisioned_throughput.as_ref().map_or((0, 0), |pt| {
            (pt.read_capacity_units, pt.write_capacity_units)
        });
        Ok(build_table_description(
            input.table_name,
            input.key_schema,
            input.attribute_definitions,
            table_id,
            table_arn,
            creation_epoch,
            TableStatus::Active,
            billing_mode,
            rcu,
            wcu,
            deletion_protection,
        ))
    }

    pub(crate) fn table_key_info_impl(
        &self,
        account_id: &str,
        table_name: &str,
    ) -> Result<TableKeyInfo, StorageError> {
        let row = self
            .db
            .query_opt(
                "SELECT table_id, key_schema, attribute_definitions, table_status \
                 FROM tables WHERE account_id = ? AND table_name = ?",
                &[Val::Text(account_id), Val::Text(table_name)],
            )
            .map_err(StorageError::Internal)?
            .ok_or_else(|| StorageError::TableNotFound(table_name.to_string()))?;

        let status = row.text(3).unwrap_or_default();
        if status != "ACTIVE" {
            return Err(StorageError::TableNotActive(table_name.to_string()));
        }
        let table_id = row.text(0).unwrap_or_default().to_string();
        let key_schema: Vec<KeySchemaElement> =
            serde_json::from_str(row.text(1).unwrap_or("[]")).map_err(intern)?;
        let attribute_definitions: Vec<AttributeDefinition> =
            serde_json::from_str(row.text(2).unwrap_or("[]")).map_err(intern)?;

        Ok(TableKeyInfo {
            table_name: table_name.to_string(),
            account_id: account_id.to_string(),
            table_id,
            base_key_schema: key_schema.clone(),
            key_schema,
            attribute_definitions,
            has_lsi: false,
            // The wasm backend does not implement secondary or vector indexes
            // (CreateTable rejects them), so these are always empty here.
            global_secondary_indexes: Vec::new(),
            local_secondary_indexes: Vec::new(),
            stream_specification: None,
            vector_indexes: Vec::new(),
        })
    }

    pub(crate) fn put_item_impl(
        &self,
        key_info: &TableKeyInfo,
        item: &Item,
        return_old: bool,
        condition: Option<&Expr>,
        maps: &ExpressionMaps,
    ) -> Result<Option<Item>, StorageError> {
        let old = if return_old || condition.is_some() {
            self.get_item_impl(key_info, item)?
        } else {
            None
        };
        if condition.is_some() {
            let empty = Item::new();
            let target = old.as_ref().unwrap_or(&empty);
            check_condition(condition, target, maps).map_err(|e| match e {
                StorageError::ConditionFailed(_) => StorageError::ConditionFailed(old.clone()),
                other => other,
            })?;
        }
        let (hash_val, range_val) = extract_keys(&key_info.key_schema, item)?;
        let item_json = serde_json::to_string(item).map_err(intern)?;
        self.db
            .execute(
                "INSERT OR REPLACE INTO items (table_id, hash_val, range_val, item) \
                 VALUES (?, ?, ?, ?)",
                &[
                    Val::Text(&key_info.table_id),
                    Val::Text(&hash_val),
                    Val::Text(&range_val),
                    Val::Text(&item_json),
                ],
            )
            .map_err(StorageError::Internal)?;
        Ok(if return_old { old } else { None })
    }

    pub(crate) fn get_item_impl(
        &self,
        key_info: &TableKeyInfo,
        key: &Item,
    ) -> Result<Option<Item>, StorageError> {
        let (hash_val, range_val) = extract_keys(&key_info.key_schema, key)?;
        let row = self
            .db
            .query_opt(
                "SELECT item FROM items WHERE table_id = ? AND hash_val = ? AND range_val = ?",
                &[
                    Val::Text(&key_info.table_id),
                    Val::Text(&hash_val),
                    Val::Text(&range_val),
                ],
            )
            .map_err(StorageError::Internal)?;
        match row {
            Some(r) => {
                let item: Item = serde_json::from_str(r.text(0).unwrap_or("{}")).map_err(intern)?;
                Ok(Some(item))
            }
            None => Ok(None),
        }
    }

    pub(crate) fn describe_table_impl(
        &self,
        account_id: &str,
        table_name: &str,
    ) -> Result<TableDescription, StorageError> {
        self.read_table_desc(account_id, table_name)?
            .ok_or_else(|| StorageError::TableNotFound(table_name.to_string()))
    }

    pub(crate) fn list_tables_impl(
        &self,
        account_id: &str,
        input: ListTablesInput,
    ) -> Result<ListTablesOutput, StorageError> {
        let rows = self
            .db
            .query(
                "SELECT table_name FROM tables WHERE account_id = ? ORDER BY table_name",
                &[Val::Text(account_id)],
            )
            .map_err(StorageError::Internal)?;
        let mut names: Vec<String> = rows
            .iter()
            .filter_map(|r| r.text(0).map(str::to_string))
            .collect();
        // ExclusiveStartTableName: return names strictly after it.
        if let Some(start) = input.exclusive_start_table_name.as_deref() {
            names.retain(|n| n.as_str() > start);
        }
        let limit = input
            .limit
            .and_then(|l| usize::try_from(l).ok())
            .unwrap_or(usize::MAX);
        let mut last_evaluated_table_name = None;
        if names.len() > limit {
            names.truncate(limit);
            last_evaluated_table_name = names.last().cloned();
        }
        Ok(ListTablesOutput {
            table_names: names,
            last_evaluated_table_name,
        })
    }

    pub(crate) fn delete_table_impl(
        &self,
        account_id: &str,
        input: DeleteTableInput,
    ) -> Result<TableDescription, StorageError> {
        let mut desc = self
            .read_table_desc(account_id, &input.table_name)?
            .ok_or_else(|| StorageError::TableNotFound(input.table_name.clone()))?;
        if desc.deletion_protection_enabled {
            return Err(StorageError::DeletionProtected(desc.table_arn.clone()));
        }
        // Drop items + catalog row atomically (parity with #182's transactional delete).
        self.db.begin_immediate().map_err(StorageError::Internal)?;
        let deleted = (|| {
            self.db.execute(
                "DELETE FROM items WHERE table_id = ?",
                &[Val::Text(&desc.table_id)],
            )?;
            self.db.execute(
                "DELETE FROM tables WHERE account_id = ? AND table_name = ?",
                &[Val::Text(account_id), Val::Text(&input.table_name)],
            )?;
            Ok::<(), String>(())
        })();
        match deleted {
            Ok(()) => self.db.commit().map_err(StorageError::Internal)?,
            Err(e) => {
                let _ = self.db.rollback();
                return Err(StorageError::Internal(e));
            }
        }
        // DynamoDB returns the table with DELETING status.
        desc.table_status = TableStatus::Deleting;
        Ok(desc)
    }

    pub(crate) fn delete_item_impl(
        &self,
        key_info: &TableKeyInfo,
        key: &Item,
        return_old: bool,
        condition: Option<&Expr>,
        maps: &ExpressionMaps,
    ) -> Result<Option<Item>, StorageError> {
        let old = if return_old || condition.is_some() {
            self.get_item_impl(key_info, key)?
        } else {
            None
        };
        if condition.is_some() {
            let empty = Item::new();
            let target = old.as_ref().unwrap_or(&empty);
            check_condition(condition, target, maps).map_err(|e| match e {
                StorageError::ConditionFailed(_) => StorageError::ConditionFailed(old.clone()),
                other => other,
            })?;
        }
        let (hash_val, range_val) = extract_keys(&key_info.key_schema, key)?;
        self.db
            .execute(
                "DELETE FROM items WHERE table_id = ? AND hash_val = ? AND range_val = ?",
                &[
                    Val::Text(&key_info.table_id),
                    Val::Text(&hash_val),
                    Val::Text(&range_val),
                ],
            )
            .map_err(StorageError::Internal)?;
        Ok(if return_old { old } else { None })
    }

    /// UpdateItem: upsert with SET/REMOVE/ADD/DELETE actions and optional
    /// ConditionExpression. Reuses core's `apply_update` / `evaluate_condition`
    /// so update and condition semantics match the native engine exactly.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn update_item_impl(
        &self,
        key_info: &TableKeyInfo,
        key: &Item,
        actions: &[UpdateAction],
        return_old: bool,
        return_new: bool,
        condition: Option<&Expr>,
        maps: &ExpressionMaps,
    ) -> Result<(Option<Item>, Option<Item>), StorageError> {
        let old = self.get_item_impl(key_info, key)?;
        if condition.is_some() {
            let empty = Item::new();
            let target = old.as_ref().unwrap_or(&empty);
            check_condition(condition, target, maps).map_err(|e| match e {
                StorageError::ConditionFailed(_) => StorageError::ConditionFailed(old.clone()),
                other => other,
            })?;
        }
        // Start from the existing image, or from the key for a fresh upsert.
        let mut item = old.clone().unwrap_or_else(|| key.clone());
        // `apply_update_validated` re-validates vector attributes on the
        // post-apply image; this backend has no vector indexes (CreateTable
        // rejects them), so `key_info.vector_indexes` is always empty and the
        // call reduces to the plain update semantics.
        apply_update_validated(
            actions,
            &mut item,
            maps,
            &key_info.vector_indexes,
            &key_info.attribute_definitions,
        )
        .map_err(|e| StorageError::Validation(e.to_string()))?;
        // Enforce the item-size limit on the post-apply image (native enforces
        // this in storage; the engine's UpdateItem handler only meters capacity).
        extenddb_core::validation::validate_item_size(&item, self.max_item_size_bytes)
            .map_err(|e| StorageError::Validation(e.to_string()))?;

        let (hash_val, range_val) = extract_keys(&key_info.key_schema, &item)?;
        let item_json = serde_json::to_string(&item).map_err(intern)?;
        self.db
            .execute(
                "INSERT OR REPLACE INTO items (table_id, hash_val, range_val, item) \
                 VALUES (?, ?, ?, ?)",
                &[
                    Val::Text(&key_info.table_id),
                    Val::Text(&hash_val),
                    Val::Text(&range_val),
                    Val::Text(&item_json),
                ],
            )
            .map_err(StorageError::Internal)?;

        let old_ret = if return_old { old } else { None };
        let new_ret = if return_new { Some(item) } else { None };
        Ok((old_ret, new_ret))
    }

    /// Scan the base table. Returns items in (hash_val, range_val) order with
    /// LastEvaluatedKey-style pagination. FilterExpression is applied by the
    /// engine after this returns.
    pub(crate) fn scan_impl(
        &self,
        key_info: &TableKeyInfo,
        limit: Option<i64>,
        exclusive_start_key: Option<&Item>,
    ) -> Result<(Vec<Item>, Option<Item>), StorageError> {
        let rows = self
            .db
            .query(
                "SELECT hash_val, range_val, item FROM items WHERE table_id = ? \
                 ORDER BY hash_val, range_val",
                &[Val::Text(&key_info.table_id)],
            )
            .map_err(StorageError::Internal)?;

        let start = exclusive_start_key
            .map(|k| extract_keys(&key_info.key_schema, k))
            .transpose()?;

        let mut items: Vec<Item> = Vec::new();
        for r in &rows {
            // Rows are ordered by (hash_val, range_val) ASC. Keep rows strictly
            // after the exclusive start tuple. Comparing the tuple (rather than
            // matching the exact row) is robust to the ESK row having been
            // deleted between pages, which would otherwise leave `past_start`
            // never set -> an empty page with no LastEvaluatedKey -> the client
            // silently drops the remaining items.
            if let Some((shv, srv)) = &start {
                let hv = r.text(0).unwrap_or_default();
                let rv = r.text(1).unwrap_or_default();
                if (hv, rv) <= (shv.as_str(), srv.as_str()) {
                    continue;
                }
            }
            let item: Item = serde_json::from_str(r.text(2).unwrap_or("{}")).map_err(intern)?;
            items.push(item);
        }

        let mut last_evaluated_key = None;
        if let Some(lim) = limit
            .filter(|l| *l > 0)
            .and_then(|l| usize::try_from(l).ok())
            && items.len() > lim
        {
            items.truncate(lim);
            last_evaluated_key = items.last().map(|it| project_key(&key_info.key_schema, it));
        }
        Ok((items, last_evaluated_key))
    }

    /// Query one partition. Resolves the partition key + sort-key condition from
    /// the expression maps, filters and orders items by their typed sort-key
    /// value (correct S/N/B ordering, done in Rust since the schema stores
    /// canonical JSON), and paginates. Base table only for M2c; index queries
    /// return the not-ported error.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn query_impl(
        &self,
        key_info: &TableKeyInfo,
        key_condition: &KeyCondition,
        maps: &ExpressionMaps,
        forward: bool,
        limit: Option<i64>,
        exclusive_start_key: Option<&Item>,
    ) -> Result<(Vec<Item>, Option<Item>), StorageError> {
        let pk_av = resolve_expr_to_av(&key_condition.pk_value, maps)?;
        let hash_val = serde_json::to_string(&pk_av).map_err(intern)?;

        let rows = self
            .db
            .query(
                "SELECT item FROM items WHERE table_id = ? AND hash_val = ?",
                &[Val::Text(&key_info.table_id), Val::Text(&hash_val)],
            )
            .map_err(StorageError::Internal)?;

        let sk_meta = sk_attr_and_type(key_info);

        let mut matched: Vec<Item> = Vec::new();
        for r in &rows {
            let item: Item = serde_json::from_str(r.text(0).unwrap_or("{}")).map_err(intern)?;
            if let Some(cond) = &key_condition.sk_condition {
                let (sk_name, ty) = sk_meta.as_ref().ok_or_else(|| {
                    StorageError::Validation(
                        "Query key condition has a sort-key clause, but the table has no range key"
                            .to_owned(),
                    )
                })?;
                let Some(item_sk) = item.get(sk_name) else {
                    continue;
                };
                if !sk_satisfies(*ty, item_sk, cond, maps)? {
                    continue;
                }
            }
            matched.push(item);
        }

        if let Some((sk_name, ty)) = &sk_meta {
            matched.sort_by(|a, b| match (a.get(sk_name), b.get(sk_name)) {
                (Some(x), Some(y)) => cmp_av(*ty, x, y),
                _ => Ordering::Equal,
            });
            if !forward {
                matched.reverse();
            }
        }

        // ExclusiveStartKey: keep items strictly beyond the previous key in the
        // scan direction, by TYPED comparison. Robust to the ESK row having been
        // deleted between pages, and numeric-aware (unlike exact AttributeValue
        // equality, which would restart the partition and return duplicates).
        if let (Some(esk), Some((sk_name, ty))) = (exclusive_start_key, &sk_meta)
            && let Some(esk_sk) = esk.get(sk_name)
        {
            let beyond = if forward {
                Ordering::Greater
            } else {
                Ordering::Less
            };
            matched.retain(|it| {
                it.get(sk_name)
                    .is_some_and(|v| cmp_av(*ty, v, esk_sk) == beyond)
            });
        }

        let mut last_evaluated_key = None;
        if let Some(lim) = limit
            .filter(|l| *l > 0)
            .and_then(|l| usize::try_from(l).ok())
            && matched.len() > lim
        {
            matched.truncate(lim);
            last_evaluated_key = matched
                .last()
                .map(|it| project_key(&key_info.key_schema, it));
        }
        Ok((matched, last_evaluated_key))
    }

    /// Read a catalog row and build a `TableDescription`. `None` if absent.
    fn read_table_desc(
        &self,
        account_id: &str,
        table_name: &str,
    ) -> Result<Option<TableDescription>, StorageError> {
        let Some(r) = self
            .db
            .query_opt(
                "SELECT table_id, key_schema, attribute_definitions, table_status, \
                 creation_epoch, table_arn, billing_mode, provisioned_throughput, \
                 deletion_protection \
                 FROM tables WHERE account_id = ? AND table_name = ?",
                &[Val::Text(account_id), Val::Text(table_name)],
            )
            .map_err(StorageError::Internal)?
        else {
            return Ok(None);
        };
        let table_id = r.text(0).unwrap_or_default().to_string();
        let key_schema: Vec<KeySchemaElement> =
            serde_json::from_str(r.text(1).unwrap_or("[]")).map_err(intern)?;
        let attribute_definitions: Vec<AttributeDefinition> =
            serde_json::from_str(r.text(2).unwrap_or("[]")).map_err(intern)?;
        let status = match r.text(3).unwrap_or("ACTIVE") {
            "CREATING" => TableStatus::Creating,
            "DELETING" => TableStatus::Deleting,
            "UPDATING" => TableStatus::Updating,
            _ => TableStatus::Active,
        };
        let creation_epoch = r.i64(4).unwrap_or(0);
        let table_arn = r.text(5).unwrap_or_default().to_string();
        let billing_mode = match r.text(6).unwrap_or("PROVISIONED") {
            "PAY_PER_REQUEST" => BillingMode::PayPerRequest,
            _ => BillingMode::Provisioned,
        };
        let (rcu, wcu) = match r.text(7) {
            Some(js) if !js.is_empty() => {
                let pt: ProvisionedThroughput = serde_json::from_str(js).map_err(intern)?;
                (pt.read_capacity_units, pt.write_capacity_units)
            }
            _ => (0, 0),
        };
        let deletion_protection = r.i64(8).unwrap_or(0) != 0;
        Ok(Some(build_table_description(
            table_name.to_string(),
            key_schema,
            attribute_definitions,
            table_id,
            table_arn,
            creation_epoch,
            status,
            billing_mode,
            rcu,
            wcu,
            deletion_protection,
        )))
    }
}

/// Build a `TableDescription` from stored catalog fields (M2a/M2b: zeros for
/// sizes/throughput, no indexes/streams).
#[allow(clippy::cast_precision_loss, clippy::too_many_arguments)]
fn build_table_description(
    table_name: String,
    key_schema: Vec<KeySchemaElement>,
    attribute_definitions: Vec<AttributeDefinition>,
    table_id: String,
    table_arn: String,
    creation_epoch: i64,
    status: TableStatus,
    billing_mode: BillingMode,
    rcu: i64,
    wcu: i64,
    deletion_protection_enabled: bool,
) -> TableDescription {
    let creation_date_time = creation_epoch as f64;
    let billing_mode_summary = match billing_mode {
        BillingMode::PayPerRequest => Some(BillingModeSummary {
            billing_mode: BillingMode::PayPerRequest,
            last_update_to_pay_per_request_date_time: Some(creation_date_time),
        }),
        BillingMode::Provisioned => None,
    };
    TableDescription {
        table_name,
        key_schema,
        attribute_definitions,
        table_status: status,
        creation_date_time,
        table_size_bytes: 0,
        item_count: 0,
        table_arn,
        table_id,
        provisioned_throughput: ProvisionedThroughputDescription {
            read_capacity_units: rcu,
            write_capacity_units: wcu,
            number_of_decreases_today: 0,
            last_increase_date_time: None,
            last_decrease_date_time: None,
        },
        billing_mode_summary,
        global_secondary_indexes: None,
        local_secondary_indexes: None,
        stream_specification: None,
        latest_stream_arn: None,
        latest_stream_label: None,
        deletion_protection_enabled,
        sse_description: None,
        table_class_summary: None,
        on_demand_throughput: None,
        restore_summary: None,
        vector_indexes: None,
    }
}

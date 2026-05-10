// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0

//! Query and scan SQL helpers for the `PostgreSQL` backend.
//!
//! Contains condition evaluation, sort-key SQL generation, and dynamic
//! parameter binding for `Query` and `Scan` operations.

use extenddb_core::expression::{self, Expr, ExpressionMaps, KeyCondition, SortKeyCondition};
use extenddb_core::types::{
    AttributeDefinition, AttributeValue, Item, KeySchemaElement, ScalarAttributeType, extract_key,
};
use extenddb_storage::error::StorageError;
use extenddb_storage::util::SortKeyValue;
use extenddb_storage::util::{parse_sk, pk_to_text, sk_info};

/// Evaluate a condition expression against an item inside a transaction.
///
/// Returns `Ok(())` if the condition passes or is `None`.
/// Returns `Err(StorageError::ConditionFailed)` if the condition fails.
pub(crate) fn check_condition(
    condition: Option<&Expr>,
    item: &std::collections::BTreeMap<String, AttributeValue>,
    maps: &ExpressionMaps,
) -> Result<(), StorageError> {
    if let Some(cond) = condition {
        let passed = expression::evaluate_condition(cond, item, maps)
            .map_err(|e| StorageError::Validation(e.to_string()))?;
        if !passed {
            return Err(StorageError::ConditionFailed(None));
        }
    }
    Ok(())
}

/// Resolve an expression (placeholder) to an `AttributeValue`.
pub(crate) fn resolve_expr_to_av(
    expr: &expression::Expr,
    maps: &ExpressionMaps,
) -> Result<AttributeValue, StorageError> {
    match expr {
        expression::Expr::Placeholder(name) => maps
            .resolve_value(name)
            .cloned()
            .map_err(|e| StorageError::Validation(e.to_string())),
        _ => Err(StorageError::Internal(
            "expected placeholder in key condition".to_owned(),
        )),
    }
}

/// SQL fragment for a sort key condition.
pub(crate) struct SkSqlInfo {
    pub(crate) fragment: String,
}

/// Build a SQL WHERE fragment for a sort key condition.
///
/// DynamoDB sorts strings by UTF-8 byte order, not by locale. We use
/// `COLLATE "C"` on string columns to match this behavior regardless of
/// the PostgreSQL database's `lc_collate` setting.
pub(crate) fn build_sk_sql(
    sk_cond: &SortKeyCondition,
    sk_col: &str,
    param_idx: &mut u32,
) -> SkSqlInfo {
    // Apply COLLATE "C" for string sort key columns to get byte-order comparison.
    let collate = if sk_col == "sk_s" || sk_col.ends_with("_s") {
        " COLLATE \"C\""
    } else {
        ""
    };
    match sk_cond {
        SortKeyCondition::Compare { op, .. } => {
            let sql_op = match op {
                expression::CompareOp::Eq => "=",
                expression::CompareOp::Ne => "<>",
                expression::CompareOp::Lt => "<",
                expression::CompareOp::Le => "<=",
                expression::CompareOp::Gt => ">",
                expression::CompareOp::Ge => ">=",
            };
            let frag = format!(" AND {sk_col}{collate} {sql_op} ${param_idx}");
            *param_idx += 1;
            SkSqlInfo { fragment: frag }
        }
        SortKeyCondition::Between { .. } => {
            let frag = format!(
                " AND {sk_col}{collate} BETWEEN ${lo} AND ${hi}",
                lo = *param_idx,
                hi = *param_idx + 1
            );
            *param_idx += 2;
            SkSqlInfo { fragment: frag }
        }
        SortKeyCondition::BeginsWith { .. } => {
            // For string sort keys, use >= prefix AND < prefix+1 pattern.
            // chr(1114111) is the max Unicode code point; with COLLATE "C"
            // (byte order), prefix || chr(1114111) is strictly greater than
            // any string starting with prefix.
            let frag = format!(
                " AND {sk_col}{collate} >= ${p} AND {sk_col}{collate} < (${p} || chr(1114111))",
                p = *param_idx
            );
            *param_idx += 1;
            SkSqlInfo { fragment: frag }
        }
    }
}

/// Execute a query SQL statement with dynamic parameter binding.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_query_sql(
    sql: &str,
    pk_text: &str,
    key_condition: &KeyCondition,
    maps: &ExpressionMaps,
    sk_info: Option<(&str, ScalarAttributeType)>,
    extra_sk_col_indices: &[(usize, ScalarAttributeType)],
    exclusive_start_key: Option<&Item>,
    pool: &sqlx::PgPool,
) -> Result<Vec<serde_json::Value>, StorageError> {
    let mut query = sqlx::query_as::<_, (serde_json::Value,)>(sql);
    query = query.bind(pk_text.to_owned());

    // Bind sort key condition values
    if let (Some(sk_cond), Some((_, sk_type))) = (&key_condition.sk_condition, sk_info) {
        query = bind_sk_condition(query, sk_cond, sk_type, maps)?;
    }

    // Bind extra RANGE key equality values
    for (i, &(_pos, sk_type)) in extra_sk_col_indices.iter().enumerate() {
        if let Some((_, value)) = key_condition.extra_sk_conditions.get(i) {
            let av = resolve_expr_to_av(value, maps)?;
            let sk = parse_sk(&av, sk_type)?;
            query = bind_sk_value(query, &sk);
        }
    }

    // Bind exclusive start key SK value
    if let (Some(start_key), Some((sk_name, sk_type))) = (exclusive_start_key, sk_info) {
        if let Some(sk_val) = start_key.get(sk_name) {
            let sk = parse_sk(sk_val, sk_type)?;
            query = bind_sk_value(query, &sk);
        }
    }

    let rows: Vec<(serde_json::Value,)> = query
        .fetch_all(pool)
        .await
        .map_err(|e| StorageError::Internal(e.to_string()))?;

    Ok(rows.into_iter().map(|(v,)| v).collect())
}

/// Bind sort key condition values to a query.
fn bind_sk_condition<'q>(
    query: sqlx::query::QueryAs<
        'q,
        sqlx::Postgres,
        (serde_json::Value,),
        sqlx::postgres::PgArguments,
    >,
    sk_cond: &SortKeyCondition,
    sk_type: ScalarAttributeType,
    maps: &ExpressionMaps,
) -> Result<
    sqlx::query::QueryAs<'q, sqlx::Postgres, (serde_json::Value,), sqlx::postgres::PgArguments>,
    StorageError,
> {
    match sk_cond {
        SortKeyCondition::Compare { value, .. }
        | SortKeyCondition::BeginsWith { prefix: value, .. } => {
            let av = resolve_expr_to_av(value, maps)?;
            let sk = parse_sk(&av, sk_type)?;
            Ok(bind_sk_value(query, &sk))
        }
        SortKeyCondition::Between { low, high, .. } => {
            let lo_av = resolve_expr_to_av(low, maps)?;
            let hi_av = resolve_expr_to_av(high, maps)?;
            let lo_sk = parse_sk(&lo_av, sk_type)?;
            let hi_sk = parse_sk(&hi_av, sk_type)?;
            let q = bind_sk_value(query, &lo_sk);
            Ok(bind_sk_value(q, &hi_sk))
        }
    }
}

/// Bind a single `SortKeyValue` to a query.
pub(crate) fn bind_sk_value<'q>(
    query: sqlx::query::QueryAs<
        'q,
        sqlx::Postgres,
        (serde_json::Value,),
        sqlx::postgres::PgArguments,
    >,
    sk: &SortKeyValue,
) -> sqlx::query::QueryAs<'q, sqlx::Postgres, (serde_json::Value,), sqlx::postgres::PgArguments> {
    match sk {
        SortKeyValue::S(s) => query.bind(s.clone()),
        SortKeyValue::N(n) => query.bind(n.clone()),
        SortKeyValue::B(b) => query.bind(b.clone()),
    }
}

/// Execute a scan SQL statement with dynamic parameter binding.
pub(crate) async fn execute_scan_sql(
    sql: &str,
    exclusive_start_key: Option<&Item>,
    key_schema: &[KeySchemaElement],
    attr_defs: &[AttributeDefinition],
    pool: &sqlx::PgPool,
) -> Result<Vec<serde_json::Value>, StorageError> {
    let mut query = sqlx::query_as::<_, (serde_json::Value,)>(sql);

    if let Some(start_key) = exclusive_start_key {
        let pk_name = &key_schema[0].attribute_name;
        let pk_val = start_key
            .get(pk_name)
            .ok_or_else(|| StorageError::Internal("missing pk in start key".to_owned()))?;
        let pk_text = pk_to_text(pk_val)?;
        query = query.bind(pk_text.into_owned());

        if let Some((sk_name, sk_type)) = sk_info(key_schema, attr_defs) {
            if let Some(sk_val) = start_key.get(sk_name) {
                let sk = parse_sk(sk_val, sk_type)?;
                query = bind_sk_value(query, &sk);
            }
        }
    }

    let rows: Vec<(serde_json::Value,)> = query
        .fetch_all(pool)
        .await
        .map_err(|e| StorageError::Internal(e.to_string()))?;

    Ok(rows.into_iter().map(|(v,)| v).collect())
}

/// Build a `LastEvaluatedKey` from an item by extracting key attributes.
pub(crate) fn build_key(item: &Item, key_schema: &[KeySchemaElement]) -> Item {
    extract_key(item, key_schema)
}

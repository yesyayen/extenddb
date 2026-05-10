// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0

//! Thin `DataEngine` trait implementation that delegates to `impl PostgresEngine`
//! methods in sibling modules.

use extenddb_core::expression::{Expr, ExpressionMaps, KeyCondition, UpdateAction};
use extenddb_core::types::{Item, TableKeyInfo};
use extenddb_storage::error::StorageError;
use extenddb_storage::{DataEngine, StreamCapture, TransactGetOp, TransactWriteOp};

use crate::PostgresEngine;

impl DataEngine for PostgresEngine {
    async fn put_item(
        &self,
        key_info: &TableKeyInfo,
        item: Item,
        return_old: bool,
        condition: Option<&Expr>,
        maps: &ExpressionMaps,
        stream: Option<&StreamCapture>,
    ) -> Result<Option<Item>, StorageError> {
        self.put_item_impl(key_info, item, return_old, condition, maps, stream)
            .await
    }

    async fn get_item(
        &self,
        key_info: &TableKeyInfo,
        key: &Item,
    ) -> Result<Option<Item>, StorageError> {
        self.get_item_impl(key_info, key).await
    }

    async fn delete_item(
        &self,
        key_info: &TableKeyInfo,
        key: &Item,
        return_old: bool,
        condition: Option<&Expr>,
        maps: &ExpressionMaps,
        stream: Option<&StreamCapture>,
    ) -> Result<Option<Item>, StorageError> {
        self.delete_item_impl(key_info, key, return_old, condition, maps, stream)
            .await
    }

    async fn update_item(
        &self,
        key_info: &TableKeyInfo,
        key: &Item,
        actions: &[UpdateAction],
        return_old: bool,
        return_new: bool,
        condition: Option<&Expr>,
        maps: &ExpressionMaps,
        stream: Option<&StreamCapture>,
    ) -> Result<(Option<Item>, Option<Item>), StorageError> {
        self.update_item_impl(
            key_info, key, actions, return_old, return_new, condition, maps, stream,
        )
        .await
    }

    async fn query(
        &self,
        key_info: &TableKeyInfo,
        key_condition: &KeyCondition,
        maps: &ExpressionMaps,
        forward: bool,
        limit: Option<i64>,
        exclusive_start_key: Option<&Item>,
        index_name: Option<&str>,
    ) -> Result<(Vec<Item>, Option<Item>), StorageError> {
        self.query_impl(
            key_info,
            key_condition,
            maps,
            forward,
            limit,
            exclusive_start_key,
            index_name,
        )
        .await
    }

    async fn scan(
        &self,
        key_info: &TableKeyInfo,
        limit: Option<i64>,
        exclusive_start_key: Option<&Item>,
        segment: Option<i64>,
        total_segments: Option<i64>,
        index_name: Option<&str>,
    ) -> Result<(Vec<Item>, Option<Item>), StorageError> {
        self.scan_impl(
            key_info,
            limit,
            exclusive_start_key,
            segment,
            total_segments,
            index_name,
        )
        .await
    }

    async fn transact_get_items(
        &self,
        ops: &[TransactGetOp<'_>],
    ) -> Result<Vec<Option<Item>>, StorageError> {
        self.transact_get_items_impl(ops).await
    }

    async fn transact_write_items(
        &self,
        ops: &[TransactWriteOp<'_>],
        token: Option<(&str, &str)>,
    ) -> Result<(), StorageError> {
        self.transact_write_items_impl(ops, token).await
    }

    async fn cleanup_expired_idempotency_tokens(
        &self,
        max_age_seconds: i64,
    ) -> Result<u64, StorageError> {
        self.cleanup_expired_idempotency_tokens_impl(max_age_seconds)
            .await
    }
}

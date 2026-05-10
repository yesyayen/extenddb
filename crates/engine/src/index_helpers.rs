// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0

//! Shared helpers for secondary index operations in the engine layer.

use extenddb_core::types::{IndexInfo, Item, KeySchemaElement, ProjectionType};

/// Build the combined key schema for `LastEvaluatedKey` extraction.
///
/// For index queries/scans, the LEK includes both the base table key attributes
/// and the index key attributes (deduplicated), matching real `DynamoDB` behavior.
pub fn combined_lek_key_schema(
    base_key_schema: &[KeySchemaElement],
    index_info: Option<&IndexInfo>,
) -> Vec<KeySchemaElement> {
    let Some(idx) = index_info else {
        return base_key_schema.to_vec();
    };
    let mut combined = base_key_schema.to_vec();
    for ks in &idx.key_schema {
        if !combined
            .iter()
            .any(|k| k.attribute_name == ks.attribute_name)
        {
            combined.push(ks.clone());
        }
    }
    combined
}

/// Filter an item to only the attributes projected into a secondary index.
///
/// For `ProjectionType::All`, returns the item unchanged.
/// For `KeysOnly`, retains only the base table and index key attributes.
/// For `Include`, retains keys plus the explicitly included `NonKeyAttributes`.
pub fn apply_index_projection(
    item: &Item,
    index_info: &IndexInfo,
    base_key_schema: &[KeySchemaElement],
) -> Item {
    match index_info.projection.projection_type {
        ProjectionType::All => item.clone(),
        ProjectionType::KeysOnly | ProjectionType::Include => {
            let mut allowed: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
            for ks in base_key_schema {
                allowed.insert(&ks.attribute_name);
            }
            for ks in &index_info.key_schema {
                allowed.insert(&ks.attribute_name);
            }
            if let Some(ref non_key) = index_info.projection.non_key_attributes {
                for attr in non_key {
                    allowed.insert(attr);
                }
            }
            item.iter()
                .filter(|(k, _)| allowed.contains(k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        }
    }
}

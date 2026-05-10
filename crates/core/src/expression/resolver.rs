// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0

//! Expression attribute name and value resolution.
//!
//! Resolves `#name` references and `:value` placeholders against the maps
//! provided in `ExpressionAttributeNames` and `ExpressionAttributeValues`.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};

use crate::error::DynamoDbError;
use crate::types::AttributeValue;

use super::ast::PathElement;

/// Resolved expression attribute names and values.
///
/// Constructed from the request's `ExpressionAttributeNames` and
/// `ExpressionAttributeValues` maps. Used by the parser and evaluator.
///
/// Numeric placeholder values are pre-parsed into `BigDecimal` at construction
/// time so that filter expressions comparing a placeholder against many items
/// parse the placeholder only once per request.
#[derive(Debug, Clone, Default)]
pub struct ExpressionMaps {
    pub names: HashMap<String, String>,
    pub values: HashMap<String, AttributeValue>,
    /// Pre-parsed `BigDecimal` values for numeric placeholders.
    /// Populated eagerly by `new()` and refreshed by `pre_parse_numerics()`.
    parsed_numerics: HashMap<String, bigdecimal::BigDecimal>,
}

impl ExpressionMaps {
    /// Create a new `ExpressionMaps` with the given names and values.
    ///
    /// Pre-parses all numeric placeholder values into `BigDecimal`.
    #[must_use]
    pub fn new(names: HashMap<String, String>, values: HashMap<String, AttributeValue>) -> Self {
        let mut maps = Self {
            names,
            values,
            parsed_numerics: HashMap::new(),
        };
        maps.pre_parse_numerics();
        maps
    }

    /// Resolve a `#name` reference to an attribute name.
    ///
    /// # Errors
    ///
    /// Returns `ValidationException` if the name is not in the map.
    pub fn resolve_name(&self, name_ref: &str) -> Result<&str, DynamoDbError> {
        self.names.get(name_ref).map(String::as_str).ok_or_else(|| {
            DynamoDbError::ValidationException(format!(
                "An expression attribute name used in the document path is not defined; attribute name: #{name_ref}"
            ))
        })
    }

    /// Resolve a `:value` placeholder to an `AttributeValue`.
    ///
    /// # Errors
    ///
    /// Returns `ValidationException` if the placeholder is not in the map.
    pub fn resolve_value(&self, placeholder: &str) -> Result<&AttributeValue, DynamoDbError> {
        self.values.get(placeholder).ok_or_else(|| {
            DynamoDbError::ValidationException(format!(
                "An expression attribute value used in expression is not defined; attribute value: :{placeholder}"
            ))
        })
    }

    /// Pre-parse all numeric placeholder values into `BigDecimal`.
    ///
    /// Call once per request before evaluating filter expressions against
    /// multiple items. Invalid numeric strings are silently skipped — they
    /// will produce `false` comparisons at evaluation time, matching the
    /// existing behavior.
    pub fn pre_parse_numerics(&mut self) {
        for (key, value) in &self.values {
            if let AttributeValue::N(n) = value {
                if let Ok(d) = n.parse::<bigdecimal::BigDecimal>() {
                    self.parsed_numerics.insert(key.clone(), d);
                }
            }
        }
    }

    /// Look up a pre-parsed `BigDecimal` for a numeric placeholder.
    ///
    /// Returns `None` if the placeholder was not numeric or failed to parse.
    #[must_use]
    pub fn get_parsed_numeric(&self, placeholder: &str) -> Option<&bigdecimal::BigDecimal> {
        self.parsed_numerics.get(placeholder)
    }
}

/// Resolve a `#name` reference or return the bare name.
///
/// Shared by the condition evaluator and update evaluator for resolving
/// attribute names in document paths.
///
/// # Errors
///
/// Returns `ValidationException` if a `#name` reference is not in the map.
pub fn resolve_name_ref<'a>(
    name: &'a str,
    maps: &'a ExpressionMaps,
) -> Result<Cow<'a, str>, DynamoDbError> {
    if let Some(ref_name) = name.strip_prefix('#') {
        Ok(Cow::Owned(maps.resolve_name(ref_name)?.to_owned()))
    } else {
        Ok(Cow::Borrowed(name))
    }
}

/// Walk a document path to find the target attribute value.
///
/// Resolves `#name` references along the way. Returns `None` for missing paths.
/// Shared by the condition evaluator and update evaluator.
///
/// # Errors
///
/// Returns `ValidationException` for unresolvable `#name` references.
pub fn resolve_path<'a>(
    elements: &[PathElement],
    item: &'a BTreeMap<String, AttributeValue>,
    maps: &ExpressionMaps,
) -> Result<Option<&'a AttributeValue>, DynamoDbError> {
    if elements.is_empty() {
        return Ok(None);
    }

    let first_name = resolve_element_name(&elements[0], maps)?;
    let Some(mut current) = item.get(first_name.as_ref()) else {
        return Ok(None);
    };

    for element in &elements[1..] {
        match element {
            PathElement::Attribute(name) => {
                let resolved = resolve_name_ref(name, maps)?;
                match current {
                    AttributeValue::M(map) => {
                        current = match map.get(resolved.as_ref()) {
                            Some(v) => v,
                            None => return Ok(None),
                        };
                    }
                    _ => return Ok(None),
                }
            }
            PathElement::Index(idx) => match current {
                AttributeValue::L(list) => {
                    current = match list.get(*idx) {
                        Some(v) => v,
                        None => return Ok(None),
                    };
                }
                _ => return Ok(None),
            },
        }
    }

    Ok(Some(current))
}

/// Resolve the attribute name from a path element, handling `#name` references.
///
/// Returns an error if the path starts with an index.
///
/// # Errors
///
/// Returns `ValidationException` for unresolvable references or index-start paths.
pub fn resolve_element_name<'a>(
    element: &'a PathElement,
    maps: &'a ExpressionMaps,
) -> Result<Cow<'a, str>, DynamoDbError> {
    match element {
        PathElement::Attribute(name) => resolve_name_ref(name, maps),
        PathElement::Index(_) => Err(DynamoDbError::ValidationException(
            "Invalid expression: path cannot start with an index".to_owned(),
        )),
    }
}

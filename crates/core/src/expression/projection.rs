// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0

//! `ProjectionExpression` parser and evaluator.
//!
//! Parses a comma-separated list of document paths and applies them to an item,
//! returning only the requested attributes.

use std::collections::BTreeMap;

use super::ast::PathElement;
use super::resolver::{ExpressionMaps, resolve_element_name, resolve_path};
use super::tokenizer::Token;
use crate::error::DynamoDbError;
use crate::types::{AttributeValue, Item};

/// Parse a `ProjectionExpression` token stream into a list of document paths.
///
/// Grammar: `path ( ',' path )*`
///
/// # Errors
///
/// Returns `ValidationException` for syntax errors.
pub fn parse_projection(tokens: &[Token]) -> Result<Vec<Vec<PathElement>>, DynamoDbError> {
    if tokens.is_empty() {
        return Ok(Vec::new());
    }
    let mut pos = 0;
    let mut paths = vec![super::parser_common::parse_path(tokens, &mut pos)?];
    while pos < tokens.len() {
        if tokens[pos] != Token::Comma {
            return Err(DynamoDbError::ValidationException(format!(
                "Invalid ProjectionExpression: unexpected token at position {pos}"
            )));
        }
        pos += 1;
        paths.push(super::parser_common::parse_path(tokens, &mut pos)?);
    }
    Ok(paths)
}

/// Apply a projection to an item, returning only the requested attributes.
///
/// # Errors
///
/// Returns `ValidationException` for unresolvable `#name` references.
pub fn apply_projection(
    item: &Item,
    paths: &[Vec<PathElement>],
    maps: &ExpressionMaps,
) -> Result<Item, DynamoDbError> {
    let mut result = BTreeMap::new();

    for path in paths {
        if path.is_empty() {
            continue;
        }
        let top_name = resolve_element_name(&path[0], maps)?;
        if path.len() == 1 {
            // Top-level attribute
            if let Some(val) = item.get(top_name.as_ref()) {
                result.insert(top_name.into_owned(), val.clone());
            }
        } else {
            // Nested path — resolve the value and insert at the top level
            // with the nested structure preserved
            if let Some(val) = resolve_path(path, item, maps)? {
                insert_nested(&mut result, path, maps, val)?;
            }
        }
    }

    Ok(result)
}

/// Insert a value at a nested path in the result item, creating intermediate
/// maps/lists as needed.
fn insert_nested(
    result: &mut Item,
    path: &[PathElement],
    maps: &ExpressionMaps,
    value: &AttributeValue,
) -> Result<(), DynamoDbError> {
    if path.is_empty() {
        return Ok(());
    }

    let top_name = resolve_element_name(&path[0], maps)?.into_owned();

    if path.len() == 1 {
        result.insert(top_name, value.clone());
        return Ok(());
    }

    // For nested paths, we need to build the intermediate structure
    let entry = result
        .entry(top_name)
        .or_insert_with(|| AttributeValue::M(BTreeMap::new()));

    let mut current = entry;
    for element in &path[1..path.len() - 1] {
        match element {
            PathElement::Attribute(name) => {
                let resolved = super::resolver::resolve_name_ref(name, maps)?;
                if let AttributeValue::M(map) = current {
                    current = map
                        .entry(resolved.into_owned())
                        .or_insert_with(|| AttributeValue::M(BTreeMap::new()));
                } else {
                    return Ok(());
                }
            }
            PathElement::Index(_) => {
                // Nested list index projection — complex case, skip for now
                return Ok(());
            }
        }
    }

    // Set the final value
    match &path[path.len() - 1] {
        PathElement::Attribute(name) => {
            let resolved = super::resolver::resolve_name_ref(name, maps)?;
            if let AttributeValue::M(map) = current {
                map.insert(resolved.into_owned(), value.clone());
            }
        }
        PathElement::Index(_) => {
            // List index at leaf — skip for now
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expression::resolver::ExpressionMaps;
    use crate::expression::tokenizer::tokenize;
    use std::collections::HashMap;

    fn project(
        expr_str: &str,
        item: &Item,
        names: HashMap<String, String>,
    ) -> Result<Item, DynamoDbError> {
        let tokens = tokenize(expr_str)?;
        let paths = parse_projection(&tokens)?;
        let maps = ExpressionMaps::new(names, HashMap::new());
        apply_projection(item, &paths, &maps)
    }

    fn sample_item() -> Item {
        let mut inner = BTreeMap::new();
        inner.insert("city".into(), AttributeValue::S("NYC".into()));
        inner.insert("zip".into(), AttributeValue::S("10001".into()));
        let mut item = BTreeMap::new();
        item.insert("pk".into(), AttributeValue::S("key1".into()));
        item.insert("name".into(), AttributeValue::S("Alice".into()));
        item.insert("age".into(), AttributeValue::N("30".into()));
        item.insert("address".into(), AttributeValue::M(inner));
        item
    }

    #[test]
    fn project_single_attribute() {
        let item = sample_item();
        let result = project("name", &item, HashMap::new()).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result.get("name"), Some(&AttributeValue::S("Alice".into())));
    }

    #[test]
    fn project_multiple_attributes() {
        let item = sample_item();
        let result = project("name, age", &item, HashMap::new()).unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.contains_key("name"));
        assert!(result.contains_key("age"));
    }

    #[test]
    fn project_missing_attribute_omitted() {
        let item = sample_item();
        let result = project("name, missing", &item, HashMap::new()).unwrap();
        assert_eq!(result.len(), 1);
        assert!(result.contains_key("name"));
    }

    #[test]
    fn project_nested_path() {
        let item = sample_item();
        let result = project("address.city", &item, HashMap::new()).unwrap();
        assert!(result.contains_key("address"));
        if let Some(AttributeValue::M(m)) = result.get("address") {
            assert_eq!(m.get("city"), Some(&AttributeValue::S("NYC".into())));
            assert!(!m.contains_key("zip")); // Only city projected
        } else {
            panic!("Expected M");
        }
    }

    #[test]
    fn project_with_name_ref() {
        let item = sample_item();
        let mut names = HashMap::new();
        names.insert("n".into(), "name".into());
        let result = project("#n", &item, names).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result.get("name"), Some(&AttributeValue::S("Alice".into())));
    }

    #[test]
    fn project_empty_expression() {
        let item = sample_item();
        let tokens = tokenize("").unwrap();
        let paths = parse_projection(&tokens).unwrap();
        assert!(paths.is_empty());
        let maps = ExpressionMaps::new(HashMap::new(), HashMap::new());
        let result = apply_projection(&item, &paths, &maps).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn project_all_attributes() {
        let item = sample_item();
        let result = project("pk, name, age, address", &item, HashMap::new()).unwrap();
        assert_eq!(result.len(), 4);
    }
}

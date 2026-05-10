// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0

//! Shared parsing utilities for condition and update expression parsers.

use super::ast::PathElement;
use super::tokenizer::Token;
use crate::error::DynamoDbError;

/// Parse a document path from a token stream.
///
/// Handles `ident`, `#name`, `.attr`, and `[index]` syntax.
/// Used by both the condition expression parser and the update expression parser.
pub fn parse_path(tokens: &[Token], pos: &mut usize) -> Result<Vec<PathElement>, DynamoDbError> {
    let mut elements = Vec::new();

    match &tokens.get(*pos) {
        Some(Token::Ident(name)) => {
            elements.push(PathElement::Attribute(name.clone()));
            *pos += 1;
        }
        Some(Token::NameRef(name)) => {
            elements.push(PathElement::Attribute(format!("#{name}")));
            *pos += 1;
        }
        _ => {
            return Err(validation_err("expected attribute name"));
        }
    }

    while *pos < tokens.len() {
        if tokens[*pos] == Token::Dot {
            *pos += 1;
            match &tokens.get(*pos) {
                Some(Token::Ident(name)) => {
                    elements.push(PathElement::Attribute(name.clone()));
                    *pos += 1;
                }
                Some(Token::NameRef(name)) => {
                    elements.push(PathElement::Attribute(format!("#{name}")));
                    *pos += 1;
                }
                _ => {
                    return Err(validation_err("expected attribute name after '.'"));
                }
            }
        } else if tokens[*pos] == Token::LBracket {
            *pos += 1;
            if let Some(Token::Ident(idx_str)) = tokens.get(*pos) {
                let idx: usize = idx_str
                    .parse()
                    .map_err(|_| validation_err("expected numeric index in brackets"))?;
                elements.push(PathElement::Index(idx));
                *pos += 1;
            } else {
                return Err(validation_err("expected numeric index in brackets"));
            }
            expect_token(tokens, pos, &Token::RBracket, "]", "expression")?;
        } else {
            break;
        }
    }

    Ok(elements)
}

/// Expect a specific token at the current position.
pub fn expect_token(
    tokens: &[Token],
    pos: &mut usize,
    expected: &Token,
    label: &str,
    context: &str,
) -> Result<(), DynamoDbError> {
    if *pos >= tokens.len() || tokens[*pos] != *expected {
        return Err(DynamoDbError::ValidationException(format!(
            "Invalid {context}: expected '{label}'"
        )));
    }
    *pos += 1;
    Ok(())
}

fn validation_err(msg: &str) -> DynamoDbError {
    DynamoDbError::ValidationException(format!("Invalid expression: {msg}"))
}

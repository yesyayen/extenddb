// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0

//! In-browser dynein. The vendored awslabs/dynein command layer (app/cmd/
//! control/data/parser/ddb) runs on wasm, with its DynamoDB client transport
//! swapped for a custom aws-smithy `HttpConnector` that calls
//! `extenddb_engine::dispatch` in-process. `dy_exec(line)` parses a dynein
//! command line with dynein's real clap parser and returns captured output.

#![cfg(target_arch = "wasm32")]

// parser.rs refers to the pest crate as `crate::pest` (dynein declared it via
// `extern crate pest;` at its crate root); mirror that here.
extern crate pest;

// --- output capture -------------------------------------------------------
// Shadow std's println!/print! across the whole crate so the vendored dynein
// code's stdout prints are collected into a buffer we can return to the caller.
#[macro_use]
mod capture {
    use std::cell::RefCell;
    thread_local! {
        static OUT: RefCell<String> = const { RefCell::new(String::new()) };
    }
    pub fn push(s: &str) {
        OUT.with(|o| o.borrow_mut().push_str(s));
    }
    pub fn take() -> String {
        OUT.with(|o| std::mem::take(&mut *o.borrow_mut()))
    }

    macro_rules! println {
        () => { $crate::capture::push("\n") };
        ($($arg:tt)*) => { $crate::capture::push(&format!("{}\n", format_args!($($arg)*))) };
    }
    macro_rules! print {
        ($($arg:tt)*) => { $crate::capture::push(&format!("{}", format_args!($($arg)*))) };
    }
}

// --- vendored dynein modules ----------------------------------------------
pub mod app;
mod cmd;
mod control;
mod data;
mod ddb;
mod parser;

// --- engine bridge --------------------------------------------------------
mod engine_bridge;
pub use engine_bridge::wasm_sdk_config;

use wasm_bindgen::prelude::*;
use clap::Parser;

/// Tokenize a command line (single-quote aware), mirroring dynein's shell.
fn tokenize(line: &str) -> Result<Vec<String>, String> {
    let mut ret = vec![];
    let mut input = line.trim_start();
    while !input.is_empty() {
        if let Some(rest) = input.strip_prefix('\'') {
            let mut tok = String::new();
            let mut iter = rest.chars();
            loop {
                match iter.next() {
                    Some('\'') => break,
                    Some('\\') => match iter.next() {
                        Some(c) => tok.push(c),
                        None => return Err("escape('\\') is incomplete".into()),
                    },
                    Some(c) => tok.push(c),
                    None => return Err("quote isn't closed".into()),
                }
            }
            input = iter.as_str().trim_start();
            ret.push(tok);
        } else {
            let pos = input.find(' ').unwrap_or(input.len());
            let (tok, rest) = input.split_at(pos);
            ret.push(tok.into());
            input = rest.trim_start();
        }
    }
    Ok(ret)
}

/// Parse and run one dynein command line against the wasm engine; return the
/// captured stdout (or an error string).
#[wasm_bindgen]
pub fn dy_exec(line: &str) -> String {
    console_error_panic_hook::set_once();
    let argv = match tokenize(line) {
        Ok(v) => v,
        Err(e) => return format!("parse error: {e}"),
    };
    if argv.is_empty() {
        return String::new();
    }
    let mut args = vec!["dy".to_string()];
    args.extend(argv);
    let cli = match cmd::Dynein::try_parse_from(args) {
        Ok(c) => c,
        Err(e) => return format!("{e}"),
    };
    let mut ctx = app::Context::new_wasm(cli.table, cli.port);
    if let Some(r) = cli.region {
        ctx.overwritten_region = app::region_from_str(Some(r));
    }
    if let Some(sub) = cli.child {
        engine_bridge::block_on(dispatch(&mut ctx, sub));
    }
    capture::take()
}

/// Data-plane + control-read dispatch (subset of dynein's main.rs dispatch).
async fn dispatch(cx: &mut app::Context, sub: cmd::Sub) {
    use cmd::*;
    match sub {
        Sub::Admin { grandchild } => match grandchild {
            AdminSub::List { .. } => control::list_tables(cx, None).await,
            AdminSub::Desc { target_table_to_desc, all_tables, output } => {
                cx.output = output;
                if all_tables {
                    control::describe_all_tables(cx).await
                } else {
                    control::describe_table(cx, target_table_to_desc).await
                }
            }
            AdminSub::Create { target_type } => match target_type {
                CreateSub::Table { new_table_name, keys } => {
                    control::create_table(cx, new_table_name, keys).await
                }
                CreateSub::Index { index_name, keys } => {
                    control::create_index(cx, index_name, keys).await
                }
            },
            AdminSub::Update { target_type } => match target_type {
                UpdateSub::Table { table_name_to_update, mode, wcu, rcu } => {
                    control::update_table(cx, table_name_to_update, mode, wcu, rcu).await
                }
            },
            AdminSub::Delete { target_type } => match target_type {
                DeleteSub::Table { table_name_to_delete, .. } => {
                    control::delete_table(cx, table_name_to_delete, true).await
                }
            },
            AdminSub::Apply { .. } => println!("not supported in browser demo"),
        },
        Sub::Scan { index, consistent_read, attributes, keys_only, limit, output } => {
            cx.output = output;
            data::scan(cx, index, consistent_read, &attributes, keys_only, limit).await
        }
        Sub::Query { pval, sort_key_expression, index, limit, attributes, consistent_read, keys_only, descending, strict, non_strict, output } => {
            cx.output = output;
            if strict || non_strict {
                cx.should_strict_for_query = Some(strict || !non_strict);
            }
            data::query(cx, data::QueryParams { pval, sort_key_expression, index, limit, consistent_read, descending, attributes, keys_only }).await
        }
        Sub::Get { pval, sval, consistent_read, output } => {
            cx.output = output;
            data::get_item(cx, pval, sval, consistent_read).await
        }
        Sub::Put { pval, sval, item } => data::put_item(cx, pval, sval, item).await,
        Sub::Del { pval, sval } => data::delete_item(cx, pval, sval).await,
        Sub::Upd { pval, sval, set, remove, atomic_counter } => {
            if let Some(target) = atomic_counter {
                data::atomic_counter(cx, pval, sval, set, remove, target).await;
            } else {
                data::update_item(cx, pval, sval, set, remove).await;
            }
        }
        Sub::List { .. } => control::list_tables(cx, None).await,
        Sub::Desc { target_table_to_desc, all_tables, output } => {
            cx.output = output;
            if all_tables {
                control::describe_all_tables(cx).await
            } else {
                control::describe_table(cx, target_table_to_desc).await
            }
        }
        _ => println!("command not supported in the browser demo (data-plane + table admin only)"),
    }
}

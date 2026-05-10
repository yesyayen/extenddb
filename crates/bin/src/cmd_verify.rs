// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0

//! `extenddb verify` — validate a extenddb deployment (REQ-CAT-013).
//!
//! Connects to catalog, checks version, enumerates tables and indexes,
//! connects to data database, reports healthy/unhealthy.
//!
//! This command uses raw `sqlx` queries intentionally. It is a diagnostic
//! tool that runs outside the server process and needs direct database
//! access to verify infrastructure health. Routing through the storage
//! abstraction would defeat the purpose of an independent health check.

use clap::Args;
use extenddb_storage_postgres::CATALOG_VERSION;
use extenddb_storage_postgres::parse_connection_string;
use sqlx::postgres::PgPoolOptions;

use crate::config;

#[derive(Args)]
pub struct VerifyArgs {
    /// Path to configuration file
    #[arg(short, long, default_value = "extenddb.toml")]
    config: String,
}

pub async fn run(args: VerifyArgs) -> anyhow::Result<()> {
    if !std::path::Path::new(&args.config).exists() {
        anyhow::bail!(
            "Config file '{}' not found. Run 'extenddb init' to set up a deployment, \
             or use --config <path> to specify a different location.",
            args.config,
        );
    }
    let app_config = config::load(&args.config)?;
    let parts = parse_connection_string(&app_config.storage.postgres.connection_string)?;
    let mut errors = 0u32;

    println!("=== extenddb verify ===");
    println!("Config:           {}", args.config);
    println!("Catalog database: {}", parts.database);
    println!();

    // Check 1: Catalog connection.
    println!("--- Checking catalog connection...");
    let catalog_pool = match PgPoolOptions::new()
        .max_connections(1)
        .connect(&app_config.storage.postgres.connection_string)
        .await
    {
        Ok(pool) => {
            println!("  OK: Connected to catalog.");
            pool
        }
        Err(e) => {
            println!("  FAIL: Cannot connect to catalog database: {e}");
            anyhow::bail!("Cannot proceed without catalog connection");
        }
    };

    // Check 2: Catalog version (D-10: strict parsing).
    println!("--- Checking catalog version...");
    let version_row: Option<(String,)> =
        sqlx::query_as("SELECT value FROM settings WHERE key = 'catalog_version'")
            .fetch_optional(&catalog_pool)
            .await?;
    match version_row {
        Some((v,)) => match v.parse::<extenddb_core::version::CatalogVersion>() {
            Ok(found) if found == CATALOG_VERSION => {
                println!("  OK: Catalog version {found}");
            }
            Ok(found) => {
                println!("  WARN: Catalog version {found} (binary expects {CATALOG_VERSION})");
                errors += 1;
            }
            Err(e) => {
                println!("  FAIL: Malformed catalog version in database: {e}");
                errors += 1;
            }
        },
        None => {
            println!("  FAIL: No catalog version found. Run 'extenddb init'.");
            errors += 1;
        }
    }

    // Check 3: Data database connection.
    println!("--- Checking data database...");
    let data_conn: Result<Option<(String,)>, _> =
        sqlx::query_as("SELECT value FROM settings WHERE key = 'data_database_connection_string'")
            .fetch_optional(&catalog_pool)
            .await;

    let data_name: Result<Option<(String,)>, _> =
        sqlx::query_as("SELECT value FROM settings WHERE key = 'data_database_name'")
            .fetch_optional(&catalog_pool)
            .await;

    if let (Ok(Some((conn,))), Ok(Some((db_name,)))) = (data_conn, data_name) {
        match PgPoolOptions::new().max_connections(1).connect(&conn).await {
            Ok(_) => println!("  OK: Connected to data database '{db_name}'."),
            Err(e) => {
                println!("  FAIL: Cannot connect to data database '{db_name}': {e}");
                errors += 1;
            }
        }
    } else {
        println!("  FAIL: No data database registered in catalog. Run 'extenddb init'.");
        errors += 1;
    }

    // Check 4: Enumerate tables and indexes.
    println!("--- Enumerating tables...");
    let table_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM tables")
        .fetch_one(&catalog_pool)
        .await
        .unwrap_or((0,));
    println!("  Tables: {}", table_count.0);

    let index_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM indexes")
        .fetch_one(&catalog_pool)
        .await
        .unwrap_or((0,));
    println!("  Indexes: {}", index_count.0);

    println!();
    if errors == 0 {
        println!("=== HEALTHY: All checks passed ===");
    } else {
        println!("=== UNHEALTHY: {errors} check(s) failed ===");
        std::process::exit(1);
    }

    Ok(())
}

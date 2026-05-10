// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0

//! `extenddb destroy` — tear down a extenddb deployment (REQ-CAT-012).
//!
//! Reads config, enumerates tables, requires `--yes` to confirm, drops both databases.

use clap::Args;
use extenddb_storage::bootstrapper::{BootstrapConfig, Bootstrapper};
use extenddb_storage_postgres::PostgresBootstrapper;
use extenddb_storage_postgres::parse_connection_string;

use crate::config;

#[derive(Args)]
#[allow(clippy::doc_markdown)] // Clap help text, not rustdoc
pub struct DestroyArgs {
    /// Path to configuration file
    #[arg(short, long, default_value = "extenddb.toml")]
    config: String,

    /// PostgreSQL admin user (for DROP DATABASE)
    #[arg(long, default_value_t = config::whoami("postgres"))]
    pg_user: String,

    /// PostgreSQL admin password
    #[arg(long)]
    pg_pass: Option<String>,

    /// Confirm destruction (required, no interactive prompt)
    #[arg(long)]
    yes: bool,
}

pub async fn run(args: DestroyArgs) -> anyhow::Result<()> {
    if !std::path::Path::new(&args.config).exists() {
        anyhow::bail!(
            "Config file '{}' not found. Nothing to destroy, or use --config <path> \
             to specify a different location.",
            args.config,
        );
    }
    let app_config = config::load(&args.config)?;
    let parts = parse_connection_string(&app_config.storage.postgres.connection_string)?;

    // Defense-in-depth: validate identifiers used in format!-based DDL.
    config::validate_pg_identifier(&parts.database, "catalog database name")?;
    config::validate_pg_identifier(&args.pg_user, "--pg-user")?;

    println!("=== extenddb destroy ===");
    println!("Config:           {}", args.config);
    println!("Catalog database: {}", parts.database);
    println!("PostgreSQL:       {}:{}", parts.host, parts.port);
    println!();

    // Create bootstrap store for catalog queries and database teardown.
    let bootstrap = PostgresBootstrapper::connect(BootstrapConfig {
        host: parts.host.clone(),
        port: parts.port,
        admin_user: args.pg_user.clone(),
        admin_password: args.pg_pass.clone(),
        app_user: parts.user.clone(),
        app_password: parts.password.clone(),
        catalog_db: parts.database.clone(),
        data_db: String::new(), // Not known yet; will be read from catalog.
    })
    .await;

    let mut data_db = String::new();

    if let Ok(ref bs) = bootstrap {
        println!("--- Tables in catalog:");
        let tables = bs.list_table_names().await.unwrap_or_default();
        if tables.is_empty() {
            println!("  (none)");
        } else {
            for name in &tables {
                println!("  {name}");
            }
        }

        // Get data database name.
        if let Ok(Some(db)) = bs.get_data_db_name().await {
            data_db = db;
            println!();
            println!("Data database:    {data_db}");
        }
    } else {
        println!("--- (could not connect to catalog)");
    }

    println!();
    println!("WARNING: This will permanently destroy ALL data in both databases.");
    println!();

    if !args.yes {
        anyhow::bail!(
            "--yes is required to confirm destruction. This will permanently destroy \
             ALL data in both databases."
        );
    }

    // For drop, we need a fresh bootstrap store connected as admin (not to the
    // catalog DB we're about to drop). The existing bootstrap store's admin pool
    // connects to the `postgres` database, so we can reuse it.
    if !data_db.is_empty() {
        // Defense-in-depth: validate even though this came from the catalog.
        config::validate_pg_identifier(&data_db, "data database name")?;
    }

    // Reconnect as admin for DDL operations (the catalog pool must be dropped
    // before we can DROP DATABASE).
    drop(bootstrap);
    let bs = PostgresBootstrapper::connect(BootstrapConfig {
        host: parts.host,
        port: parts.port,
        admin_user: args.pg_user,
        admin_password: args.pg_pass,
        app_user: parts.user,
        app_password: parts.password,
        catalog_db: parts.database,
        data_db: data_db.clone(),
    })
    .await
    .map_err(|e| anyhow::anyhow!("Cannot connect as admin: {e:?}"))?;

    bs.drop_databases(&data_db)
        .await
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;

    println!();
    println!("=== extenddb destroy complete ===");
    Ok(())
}

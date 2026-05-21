# Deployment Guide

> See [NOTICE](../NOTICE.md) for important disclaimers.

This guide covers deploying extenddb in various environments beyond local development.

## Architecture Overview

extenddb is a single Rust binary that connects to PostgreSQL. All state lives in PostgreSQL — extenddb itself is stateless (no in-process caching). This means:

- Multiple extenddb instances can share a PostgreSQL catalog (with caveats — see Multi-Instance below)
- Standard PostgreSQL HA, backup, and replication tools provide durability
- extenddb can run anywhere PostgreSQL is reachable

## Deployment Models

### Single-Node

extenddb and PostgreSQL on the same host. Simplest setup, suitable for development, CI, and small workloads.

```
┌─────────────────────────┐
│  Host                   │
│  ┌─────┐  ┌──────────┐ │
│  │ extenddb│──│PostgreSQL │ │
│  └─────┘  └──────────┘ │
└─────────────────────────┘
```

```bash
extenddb init
extenddb serve --config extenddb.toml
```

### Separated Database

extenddb on an application server, PostgreSQL on a dedicated database server or managed service.

```
┌──────────┐       ┌──────────────┐
│  App Host│──────▶│  DB Host     │
│  extenddb    │       │  PostgreSQL  │
└──────────┘       └──────────────┘
```

```bash
extenddb init \
  --pg-host db.example.com \
  --pg-pass
```

Configure the connection string in `extenddb.toml`:

```toml
[storage.postgres]
connection_string = "postgresql://extenddb:<password>@db.example.com:5432/extenddb_catalog?sslmode=require"
pool_size = 20
```

Use `sslmode=require` (or `verify-full` with a CA certificate) for encrypted database connections.

### Managed PostgreSQL

extenddb works with any PostgreSQL 14+ service:

- **Amazon RDS for PostgreSQL** / **Amazon Aurora PostgreSQL**
- **Google Cloud SQL for PostgreSQL**
- **Azure Database for PostgreSQL**
- **Self-managed PostgreSQL** with streaming replication

```bash
# Amazon RDS / Aurora example
extenddb init \
  --pg-host mydb.cluster-abc123.us-east-1.rds.amazonaws.com \
  --pg-user extenddb_admin \
  --pg-pass
```

### Containerized

ExtendDB ships an OCI container image. The image runs `extenddb serve`
only: operators bootstrap the deployment by running `extenddb init`
separately, then mount the resulting `extenddb.toml` as a config volume.

A `Dockerfile` and a Docker Compose demo live in the repository:

- [`Dockerfile`](../../Dockerfile) at the repo root: multi-stage build,
  `debian:bookworm-slim` runtime, non-root user, tini as PID 1.
- [`samples/docker/compose.yaml`](../../samples/docker/compose.yaml):
  PostgreSQL plus an idempotent `extenddb-init` one-shot plus the
  long-running `extenddb` service.
- [`samples/docker/bootstrap-iam.sh`](../../samples/docker/bootstrap-iam.sh):
  helper script that creates an IAM user with full DynamoDB access and
  emits a `extenddb-creds.env` ready to `source`.
- [`samples/docker/README.md`](../../samples/docker/README.md): full
  walkthrough including the AWS CLI smoke test.

For a minute-long evaluation:

```bash
cd samples/docker
docker compose -f compose.yaml -f compose.dev.yaml up --build -d
./bootstrap-iam.sh
source ./extenddb-creds.env
aws dynamodb list-tables --endpoint-url "$EXTENDDB_ENDPOINT"
```

For production, do not use the demo `compose.yaml` as-is: passwords
are hard-coded and the cert is self-signed. Use it as a reference and
supply your own values.

#### Daemonization in containers

`extenddb serve` always daemonizes. The container entrypoint script
(`docker/entrypoint.sh`) runs `serve`, waits for the daemon's PID file
to appear, then polls the daemon process and forwards SIGTERM/SIGINT
for graceful shutdown. `docker stop` triggers a clean shutdown
(verified to return exit 0 well within the default 10 second grace
period).

#### Kubernetes

For Kubernetes deployments:

- Run `extenddb init` as an `initContainer` or one-time `Job` against a
  PersistentVolumeClaim that holds `extenddb.toml`.
- Run `extenddb serve` as the main container in a `Deployment`,
  mounting the same PVC at `/etc/extenddb/extenddb.toml` (or use a
  ConfigMap / Secret for the rendered config).
- Use a `livenessProbe` / `readinessProbe` against
  `https://<pod-ip>:8000/health` (with `httpHeaders` skipping TLS
  verification, or with the cert mounted from a Secret).

### Air-Gapped

extenddb requires no internet connectivity. All functionality is self-contained in the binary. Build on a connected host, transfer the binary and `extenddb.toml` to the air-gapped environment, and run.

Requirements in the air-gapped environment:
- PostgreSQL 14+ (reachable from the extenddb host)
- The `extenddb` binary (statically linked or with matching libc)

## Production Checklist

### Authentication

- [ ] Set `auth.provider = "builtin"` in `extenddb.toml`
- [ ] Change the admin password from the auto-generated one
- [ ] Create named accounts and IAM users with least-privilege policies
- [ ] Disable credential import if not needed: `extenddb settings set allow_credential_import false`

### Transport

- [ ] TLS is enabled by default. Verify it is not disabled in `extenddb.toml`
- [ ] Replace the self-signed certificate with a CA-signed certificate
- [ ] Use `sslmode=require` or `sslmode=verify-full` in the PostgreSQL connection string

### Network

- [ ] Set `bind_addr` to the appropriate interface (not `0.0.0.0` unless behind a load balancer)
- [ ] Firewall: allow only necessary ports (extenddb port, PostgreSQL port)
- [ ] Consider a reverse proxy (nginx, HAProxy) for TLS termination, rate limiting, and access logging

### Database

- [ ] Use a dedicated PostgreSQL user for extenddb with minimal privileges
- [ ] Configure PostgreSQL `max_connections` ≥ extenddb `pool_size` + 3
- [ ] Enable PostgreSQL TLS
- [ ] Set up automated backups (pg_dump, WAL archiving, or managed service snapshots)
- [ ] Monitor PostgreSQL disk usage, connection count, and query performance

### Monitoring

- [ ] Scrape `/metrics` with Prometheus (or compatible collector)
- [ ] Forward syslog to a log aggregation service
- [ ] Set up alerts on health check failures (`/health`)
- [ ] Monitor extenddb process with systemd, supervisord, or equivalent

### Upgrades

- [ ] Test upgrades in a staging environment first
- [ ] Run `extenddb migrate --config extenddb.toml` after binary upgrade
- [ ] Verify catalog version matches: `extenddb verify --config extenddb.toml`

## systemd Service

Example unit file for Linux:

```ini
[Unit]
Description=ExtendDB
After=postgresql.service
Requires=postgresql.service

[Service]
Type=forking
ExecStart=/usr/local/bin/extenddb serve --config /etc/extenddb/extenddb.toml
ExecStop=/usr/local/bin/extenddb stop --config /etc/extenddb/extenddb.toml
# PID file path: {run_dir}/extenddb-{port}.pid
# Default run_dir is ~/.extenddb/run (~ expands to $HOME of the User= below)
# Adjust if run_dir or port are overridden in extenddb.toml
PIDFile=/home/extenddb/.extenddb/run/extenddb-8000.pid
User=extenddb
Group=extenddb
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl enable extenddb
sudo systemctl start extenddb
```

## Multi-Instance Considerations

Multiple extenddb instances can connect to the same PostgreSQL catalog. However:

- extenddb does not cache database state in-process — every request reads directly from PostgreSQL
- This means multiple instances see consistent data without cache invalidation
- PostgreSQL's connection pool and row-level locking handle concurrent access
- Ensure `pool_size × instance_count + 3 × instance_count ≤ PostgreSQL max_connections`

## Performance Tuning

### Connection Pool

The default `pool_size = 20` is suitable for moderate workloads. For high-concurrency deployments:

```toml
[storage.postgres]
pool_size = 50  # Increase for higher concurrency
```

Ensure PostgreSQL `max_connections` accommodates the total pool size plus overhead.

### PostgreSQL Tuning

Key PostgreSQL settings for extenddb workloads:

- `shared_buffers`: 25% of available RAM
- `effective_cache_size`: 75% of available RAM
- `work_mem`: 64MB (for sort operations in Query/Scan)
- `max_connections`: ≥ extenddb pool_size + 10

### Monitoring Queries

```sql
-- Active connections from extenddb
SELECT count(*) FROM pg_stat_activity WHERE usename = 'extenddb';

-- Table sizes
SELECT relname, pg_size_pretty(pg_total_relation_size(oid))
FROM pg_class WHERE relname LIKE 'ddb_%' ORDER BY pg_total_relation_size(oid) DESC;

-- Slow queries
SELECT query, mean_exec_time, calls
FROM pg_stat_statements WHERE usename = 'extenddb'
ORDER BY mean_exec_time DESC LIMIT 10;
```

---

## License

Copyright 2026 ExtendDB contributors. Licensed under the Apache License, Version 2.0.
See [LICENSE](../../LICENSE) for the full text.

This software is provided "as is" without warranty of any kind. ExtendDB is not
affiliated with, endorsed by, or sponsored by Amazon Web Services. "DynamoDB" is a trademark
of Amazon.com, Inc.

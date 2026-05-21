# Run ExtendDB with Docker

This directory contains a `docker compose` setup that brings up PostgreSQL,
runs `extenddb init` once, then starts ExtendDB on `https://127.0.0.1:8000`.

It is meant for evaluation and local development. For production deployments,
see [`docs/manuals/11-deployment-guide.md`](../../docs/manuals/11-deployment-guide.md).

## Prerequisites

- Docker 25+ with the `docker compose` plugin (or `docker-compose` v2+).
- Free TCP port 8000 on the host (override with `EXTENDDB_HOST_PORT=...`).

That is the entire prerequisite list. ExtendDB is a single static-ish binary
in the runtime image. PostgreSQL runs as a sibling container; you do not
need a local Postgres install.

## Quickstart

```bash
git clone https://github.com/ExtendDB/extenddb.git
cd extenddb/samples/docker

# Until the image is published to GHCR, build it from source.
docker compose -f compose.yaml -f compose.dev.yaml up --build -d

# Once published:
#   docker compose up -d
```

You should see PostgreSQL come up healthy, the `extenddb-init` one-shot
exit cleanly, then `extenddb` start. Verify:

```bash
curl -k https://127.0.0.1:8000/health
# {"status":"healthy"}
```

The DynamoDB API is now live on `https://127.0.0.1:8000`. The web console is
at [`https://127.0.0.1:8000/console/`](https://127.0.0.1:8000/console/) (your
browser will warn about the self-signed certificate; accept it).

## Make your first DynamoDB request

ExtendDB requires SigV4 authentication. The compose stack creates an admin
user but no IAM user, so you need one extra step to get usable AWS
credentials. Two paths are documented below.

### Fast path: bootstrap script

```bash
./bootstrap-iam.sh                           # creates user, policy, key + cert
source ./extenddb-creds.env                  # exports AWS_* env vars
aws dynamodb list-tables --endpoint-url "$EXTENDDB_ENDPOINT"
# {"TableNames": []}
```

The script is idempotent (safe to re-run; it bails out if
`extenddb-creds.env` already exists). It writes:

- `extenddb-creds.env` (mode `0600`): `AWS_ACCESS_KEY_ID`,
  `AWS_SECRET_ACCESS_KEY`, `AWS_DEFAULT_REGION`, `AWS_CA_BUNDLE`,
  `EXTENDDB_ENDPOINT`.
- `extenddb-cert.pem`: the server's self-signed certificate, referenced
  by `AWS_CA_BUNDLE`.

Requires `jq` and `docker` on the host. Override the IAM user name with
`EXTENDDB_BOOTSTRAP_USER=...`, the host port with `EXTENDDB_HOST_PORT=...`.

To re-bootstrap (rotate the access key, refresh the env file):

```bash
rm extenddb-creds.env
./bootstrap-iam.sh
```

### Manual path: what the script does

If you prefer to understand the steps, run them by hand. The same
result, just verbose. Skip this section if the fast path worked.

#### 1. Find the default account ID

The compose stack created a single account during `init`. Look it up:

```bash
docker compose exec -e EXTENDDB_PASSWORD=admin-local-dev-password \
    extenddb extenddb manage \
    --config /etc/extenddb/extenddb.toml \
    --user admin \
    list-accounts
```

You will see one account with an `account_id` like `840625254687`. Note it
down; the rest of the walkthrough uses `$ACCOUNT_ID`.

```bash
ACCOUNT_ID=<paste-the-id-here>
```

#### 2. Create an IAM user, attach a policy, and generate an access key

Create the user:

```bash
docker compose exec -e EXTENDDB_PASSWORD=admin-local-dev-password \
    extenddb extenddb manage \
    --config /etc/extenddb/extenddb.toml \
    --user admin \
    create-user --account-id "$ACCOUNT_ID" --user-name app
```

Attach a policy. ExtendDB enforces IAM authorization on every DynamoDB
request. Without an explicit allow, calls return `AccessDeniedException`.
For evaluation, grant full DynamoDB access:

```bash
docker compose exec -e EXTENDDB_PASSWORD=admin-local-dev-password \
    extenddb extenddb manage \
    --config /etc/extenddb/extenddb.toml \
    --user admin \
    put-user-policy \
        --account-id "$ACCOUNT_ID" \
        --user-name app \
        --policy-name AllowAllDynamoDB \
        --policy-document '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"dynamodb:*","Resource":"*"}]}'
```

For production, use a least-privilege policy listing only the actions and
resources the application needs.

Generate an access key:

```bash
docker compose exec -e EXTENDDB_PASSWORD=admin-local-dev-password \
    extenddb extenddb manage \
    --config /etc/extenddb/extenddb.toml \
    --user admin \
    create-access-key --account-id "$ACCOUNT_ID" --user-name app
```

The last command prints an access key ID and secret. Save both. The
secret is shown once.

```bash
export AWS_ACCESS_KEY_ID=<from-output>
export AWS_SECRET_ACCESS_KEY=<from-output>
export AWS_DEFAULT_REGION=us-east-1
```

#### 3. Trust the self-signed certificate

Copy the cert out of the container:

```bash
docker compose cp extenddb:/var/lib/extenddb/.extenddb/tls/cert.pem ./extenddb-cert.pem
export AWS_CA_BUNDLE=$PWD/extenddb-cert.pem
```

If you do not want to copy the cert, you can pass `--no-verify-ssl` to the
AWS CLI for every command. The cert approach is safer.

#### 4. Call the DynamoDB API

```bash
aws dynamodb list-tables --endpoint-url https://127.0.0.1:8000
# { "TableNames": [] }

aws dynamodb create-table \
    --endpoint-url https://127.0.0.1:8000 \
    --table-name greetings \
    --attribute-definitions AttributeName=id,AttributeType=S \
    --key-schema AttributeName=id,KeyType=HASH \
    --billing-mode PAY_PER_REQUEST

aws dynamodb put-item \
    --endpoint-url https://127.0.0.1:8000 \
    --table-name greetings \
    --item '{"id":{"S":"a"},"msg":{"S":"hello from extenddb"}}'

aws dynamodb get-item \
    --endpoint-url https://127.0.0.1:8000 \
    --table-name greetings \
    --key '{"id":{"S":"a"}}'
```

You should see the item come back. ExtendDB is now running.

## Common operations

### Tail server logs

```bash
docker compose logs -f extenddb
```

The first time the server starts you will see lines like:

```
extenddb 0.1.0 (catalog 0.0.2) starting on 0.0.0.0:8000
extenddb server started (pid 10, 0.0.0.0:8000)
extenddb-entrypoint: daemon started (pid 10, pid-file ...)
```

### Stop without losing data

```bash
docker compose stop
```

State persists in named volumes. `docker compose start` brings the same
data back.

### Reset everything

```bash
docker compose down -v
```

This removes containers, networks, **and the named volumes**, including
PostgreSQL data and the ExtendDB config and TLS cert. The next `up` will
re-initialize from scratch.

### Use a custom host port

```bash
EXTENDDB_HOST_PORT=8443 docker compose up -d
# now reachable at https://127.0.0.1:8443
```

The container always listens on 8000 internally; this only changes the
host-side mapping.

### Pin to a specific image tag

```bash
EXTENDDB_IMAGE=ghcr.io/extenddb/extenddb:v0.1.0 docker compose up -d
```

## Going further

- For a clean walk-through of `init`, `serve`, and credential setup outside
  containers, see [`docs/getting-started.md`](../../docs/getting-started.md).
- For production deployment patterns (Kubernetes, RDS/Aurora, multi-arch),
  see [`docs/manuals/11-deployment-guide.md`](../../docs/manuals/11-deployment-guide.md).
- For the full set of `extenddb manage` commands, see
  [`docs/manuals/05-admin-guide.md`](../../docs/manuals/05-admin-guide.md).

## Troubleshooting

### `bind: address already in use`

Port 8000 is taken on your host. Either stop the conflicting service or
remap:

```bash
EXTENDDB_HOST_PORT=8080 docker compose up -d
```

### `extenddb-init` failed with `Database 'extenddb_catalog' already exists`

This compose file expects a clean PostgreSQL. If you reused a Postgres
volume from a different ExtendDB install, run:

```bash
docker compose down -v
docker compose up -d
```

### `AccessDeniedException: User ... is not authorized to perform: dynamodb:...`

The IAM user has an access key but no policy granting DynamoDB actions.
Review step 2: `put-user-policy` must be run before any DynamoDB calls
will succeed. Auto-attached policies cover credential self-service only,
not DynamoDB.

### `Missing Authentication Token` when calling the DynamoDB API

The AWS CLI is not signing the request. Confirm you exported
`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, and `AWS_DEFAULT_REGION`
in the same shell, and that the access key was created in step 2 above.

### `unable to get local issuer certificate`

The CLI is not trusting the self-signed cert. Either set
`AWS_CA_BUNDLE` to the path of `extenddb-cert.pem` (see step 3), or pass
`--no-verify-ssl` to each `aws dynamodb` invocation.

### Container exits with `config file not found at /etc/extenddb/extenddb.toml`

The init step did not run, or its volume was wiped while keeping the
serve volume. Run `docker compose down -v && docker compose up -d` to
restart from a clean state.

## Security notes for evaluation use

This compose file is intentionally simple and **not safe for production**:

- The PostgreSQL admin password is hard-coded in `compose.yaml`.
- The ExtendDB admin password is hard-coded in `compose.yaml`.
- The DynamoDB API uses a self-signed certificate.
- Throttling is off (`throttling_enabled = false` in the generated config).

For production, follow the production checklist in the deployment guide.

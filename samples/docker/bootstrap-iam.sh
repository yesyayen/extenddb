#!/usr/bin/env bash
# Copyright 2026 ExtendDB contributors
# SPDX-License-Identifier: Apache-2.0
#
# Bootstrap an IAM user with full DynamoDB access against the demo
# compose stack in this directory. After the script succeeds, you can:
#
#     source ./extenddb-creds.env
#     aws dynamodb list-tables --endpoint-url "$EXTENDDB_ENDPOINT"
#
# Outputs:
#     ./extenddb-creds.env  -- AWS_* env vars + EXTENDDB_ENDPOINT, ready to source
#     ./extenddb-cert.pem   -- the server's self-signed certificate
#
# Re-running the script is safe:
#   - If extenddb-creds.env already exists, the script exits 0 without
#     touching anything. Delete the file to force re-bootstrap.
#   - On re-bootstrap, any existing access keys for the IAM user are
#     deleted (we cannot recover their secrets) and a fresh key is minted.
#
# Configuration (env vars, all optional):
#     EXTENDDB_BOOTSTRAP_USER    IAM user name to create  (default: app)
#     EXTENDDB_BOOTSTRAP_POLICY  Inline policy name       (default: AllowAllDynamoDB)
#     EXTENDDB_ADMIN_USER        Admin username           (default: admin)
#     EXTENDDB_ADMIN_PASSWORD    Admin password           (default: admin-local-dev-password)
#     EXTENDDB_HOST_PORT         Host-side port mapping   (default: 8000)
#     EXTENDDB_COMPOSE           Compose file flags       (default: -f compose.yaml -f compose.dev.yaml)
#
# Requires: docker (with compose plugin), jq.

set -euo pipefail

# --- config ---
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENV_FILE="$SCRIPT_DIR/extenddb-creds.env"
CERT_FILE="$SCRIPT_DIR/extenddb-cert.pem"

USER_NAME="${EXTENDDB_BOOTSTRAP_USER:-app}"
POLICY_NAME="${EXTENDDB_BOOTSTRAP_POLICY:-AllowAllDynamoDB}"
ADMIN_USER="${EXTENDDB_ADMIN_USER:-admin}"
ADMIN_PASSWORD="${EXTENDDB_ADMIN_PASSWORD:-admin-local-dev-password}"
HOST_PORT="${EXTENDDB_HOST_PORT:-8000}"
ENDPOINT="https://127.0.0.1:${HOST_PORT}"
POLICY_DOCUMENT='{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"dynamodb:*","Resource":"*"}]}'

# Compose flags. Word-split intentional to support multiple `-f` flags.
# shellcheck disable=SC2206
COMPOSE_ARGS=(${EXTENDDB_COMPOSE:--f compose.yaml -f compose.dev.yaml})

# --- preflight ---
need() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "bootstrap-iam: '$1' is required but not installed." >&2
        exit 2
    }
}
need docker
need jq

cd "$SCRIPT_DIR"

if [ -f "$ENV_FILE" ]; then
    echo "$(basename "$ENV_FILE") already exists. Delete it and re-run to re-bootstrap."
    exit 0
fi

# Wrapper for `extenddb manage` inside the compose service.
# Pass the admin password through EXTENDDB_PASSWORD rather than the
# --password flag. Inside the container, this keeps the password out of
# the `extenddb manage` argv (where `ps` could see it). On the host, the
# value still appears briefly in `docker compose exec -e ...` argv during
# the call; if that matters in your environment, set EXTENDDB_PASSWORD in
# the script's environment yourself and remove the -e here.
manage() {
    docker compose "${COMPOSE_ARGS[@]}" exec -T \
        -e EXTENDDB_PASSWORD="$ADMIN_PASSWORD" \
        extenddb extenddb manage \
        --config /etc/extenddb/extenddb.toml \
        --user "$ADMIN_USER" \
        "$@"
}

# --- ensure stack is up and healthy ---
if ! docker compose "${COMPOSE_ARGS[@]}" ps --status running --services 2>/dev/null \
        | grep -qx 'extenddb'; then
    cat >&2 <<EOF
bootstrap-iam: extenddb is not running. Start it first:

    docker compose ${COMPOSE_ARGS[*]} up -d

EOF
    exit 1
fi

# Wait for the server's HEALTHCHECK to flip to healthy.
echo "==> Waiting for extenddb to become healthy..."
i=0
while [ "$i" -lt 60 ]; do  # 60 ticks * 1 s = 60 s ceiling
    STATUS="$(docker compose "${COMPOSE_ARGS[@]}" ps --format json extenddb 2>/dev/null \
            | jq -r 'if type == "array" then .[0].Health else .Health end // "unknown"')"
    case "$STATUS" in
        healthy)
            echo "    healthy."
            break
            ;;
        starting|unknown|"")
            sleep 1
            ;;
        *)
            echo "bootstrap-iam: extenddb container reported unhealthy state: $STATUS" >&2
            exit 1
            ;;
    esac
    i=$((i + 1))
done

if [ "$STATUS" != "healthy" ]; then
    echo "bootstrap-iam: extenddb did not become healthy within 60 s (last status: $STATUS)." >&2
    echo "bootstrap-iam: check 'docker compose ${COMPOSE_ARGS[*]} logs extenddb'." >&2
    exit 1
fi

# --- discover account ---
echo "==> Discovering default account..."
ACCOUNT_ID="$(manage list-accounts | jq -r '.[0].account_id // empty')"
if [ -z "$ACCOUNT_ID" ]; then
    echo "bootstrap-iam: no account found. Was 'extenddb init' run?" >&2
    exit 1
fi
echo "    account_id=$ACCOUNT_ID"

# --- ensure IAM user ---
echo "==> Ensuring IAM user '$USER_NAME' exists..."
if manage list-users --account-id "$ACCOUNT_ID" \
        | jq -e --arg u "$USER_NAME" '.[] | select(.user_name == $u)' >/dev/null; then
    echo "    user already exists."
else
    manage create-user --account-id "$ACCOUNT_ID" --user-name "$USER_NAME" >/dev/null
    echo "    user created."
fi

# --- ensure policy ---
echo "==> Ensuring policy '$POLICY_NAME' is attached..."
if manage list-user-policies --account-id "$ACCOUNT_ID" --user-name "$USER_NAME" \
        | jq -e --arg p "$POLICY_NAME" '.[] | select(.policy_name == $p)' >/dev/null; then
    echo "    policy already attached."
else
    manage put-user-policy \
        --account-id "$ACCOUNT_ID" \
        --user-name "$USER_NAME" \
        --policy-name "$POLICY_NAME" \
        --policy-document "$POLICY_DOCUMENT" >/dev/null
    echo "    policy attached."
fi

echo "==> Minting access key..."
EXISTING_KEYS="$(manage list-access-keys --account-id "$ACCOUNT_ID" --user-name "$USER_NAME" \
        | jq -r '.[].access_key_id // empty')"
if [ -n "$EXISTING_KEYS" ]; then
    while IFS= read -r KEY_ID; do
        [ -z "$KEY_ID" ] && continue
        echo "    deleting stale key $KEY_ID..."
        manage delete-access-key \
            --account-id "$ACCOUNT_ID" \
            --user-name "$USER_NAME" \
            --access-key-id "$KEY_ID" >/dev/null
    done <<< "$EXISTING_KEYS"
fi

KEY_JSON="$(manage create-access-key --account-id "$ACCOUNT_ID" --user-name "$USER_NAME")"
ACCESS_KEY_ID="$(echo "$KEY_JSON"     | jq -r '.access_key_id')"
SECRET_ACCESS_KEY="$(echo "$KEY_JSON" | jq -r '.secret_access_key')"
if [ -z "$ACCESS_KEY_ID" ] || [ -z "$SECRET_ACCESS_KEY" ] \
        || [ "$ACCESS_KEY_ID" = "null" ] || [ "$SECRET_ACCESS_KEY" = "null" ]; then
    echo "bootstrap-iam: failed to parse access key from create-access-key output:" >&2
    echo "$KEY_JSON" >&2
    exit 1
fi
echo "    new key: $ACCESS_KEY_ID"

# --- export cert ---
echo "==> Exporting TLS certificate..."
docker compose "${COMPOSE_ARGS[@]}" cp \
    extenddb:/var/lib/extenddb/.extenddb/tls/cert.pem "$CERT_FILE" >/dev/null
chmod 0644 "$CERT_FILE"

# --- write env file ---
umask 0177  # ensure new file is 0600
cat > "$ENV_FILE" <<EOF
# Generated by samples/docker/bootstrap-iam.sh on $(date -u +%Y-%m-%dT%H:%M:%SZ)
# Source this file before running aws CLI commands:
#     source $(basename "$ENV_FILE")
export AWS_ACCESS_KEY_ID="$ACCESS_KEY_ID"
export AWS_SECRET_ACCESS_KEY="$SECRET_ACCESS_KEY"
export AWS_DEFAULT_REGION="us-east-1"
export AWS_CA_BUNDLE="$CERT_FILE"
export EXTENDDB_ENDPOINT="$ENDPOINT"
EOF

echo
echo "Bootstrap complete."
echo "    env file:    $ENV_FILE  (mode 0600)"
echo "    certificate: $CERT_FILE"
echo
echo "Next:"
echo "    source $(basename "$ENV_FILE")"
echo "    aws dynamodb list-tables --endpoint-url \"\$EXTENDDB_ENDPOINT\""

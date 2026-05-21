#!/bin/sh
# Copyright 2026 ExtendDB contributors
# SPDX-License-Identifier: Apache-2.0
#
# Container entrypoint for ExtendDB.
#
# Argument handling:
#   1. If the first arg is "extenddb", strip it. This lets users copy-paste
#      `extenddb <subcommand>` invocations from docs without surprise:
#        docker run <image> extenddb init ...
#        docker run <image>          init ...
#      both work and behave identically.
#   2. If the (remaining) first arg is "serve" or empty (the default CMD),
#      run `extenddb serve` and wait on the daemon.
#   3. Otherwise, exec `extenddb "$@"`. Used for init, manage, status, etc.
#
# `extenddb serve` always forks into the background. Without this wrapper
# the container would exit immediately after the parent process returns.
# Once a foreground/no-detach mode lands upstream, this whole script
# collapses to `exec extenddb "$@"`.

set -eu

CONFIG="${EXTENDDB_CONFIG:-/etc/extenddb/extenddb.toml}"
STATE_DIR="${HOME:-/var/lib/extenddb}/.extenddb"
RUN_DIR="${STATE_DIR}/run"

# Strip an optional leading "extenddb" arg so both invocation styles work.
if [ "${1:-}" = "extenddb" ]; then
    shift
fi

# Anything other than "serve" (or no args) goes straight to the binary.
case "${1:-serve}" in
    serve) ;;          # fall through to the serve path below
    *)     exec extenddb "$@" ;;
esac

# --- serve path ---

if [ ! -f "$CONFIG" ]; then
    cat >&2 <<EOF
extenddb-entrypoint: config file not found at $CONFIG.

This image does not auto-initialize. Run \`init\` first, e.g.:

  docker run --rm \\
    -v extenddb-config:/etc/extenddb \\
    -v extenddb-state:/var/lib/extenddb \\
    <image> init \\
      --config $CONFIG \\
      --pg-host <postgres-host> --pg-user <user> --pg-pass <pass> \\
      --bind-addr 0.0.0.0

See samples/docker/README.md for the full bootstrap walkthrough.
EOF
    exit 1
fi

extenddb serve --config "$CONFIG"

# Install the signal-forwarding trap BEFORE waiting for the PID file.
# `extenddb serve` returns to the parent immediately after the double-fork;
# the daemon may take up to ~15 s on a slow runner to write its PID file.
# A SIGTERM landing in that window must still trigger graceful shutdown,
# so the trap tolerates an unset $PID and falls through to the polling
# loop below (which will exit naturally once the daemon is running and
# then dies, or once the wait loop times out).
#
# Why not `tail --pid`? It blocks but does not forward signals. SIGTERM
# to the container would reach `tail`, kill it, and orphan the daemon.
# The container runtime would then fall through to SIGKILL after the
# grace period, skipping graceful shutdown.
PID=""
shutdown() {
    if [ -n "${PID:-}" ]; then
        echo "extenddb-entrypoint: forwarding ${1:-TERM} to daemon pid $PID"
        kill -"${1:-TERM}" "$PID" 2>/dev/null || true
    else
        echo "extenddb-entrypoint: ${1:-TERM} received before daemon PID was known; exiting"
        exit 143
    fi
}
trap 'shutdown TERM' TERM
trap 'shutdown INT'  INT

# extenddb writes a PID file at $run_dir/extenddb-$port.pid. The port comes
# from the config file, which the operator may have changed, so we glob.
# Wait up to ~15 s for the daemon to write its PID. The daemon normally
# writes within ~200 ms but slow CI runners and cold caches can stretch it.
i=0
while [ "$i" -lt 30 ]; do
    PID_FILE="$(ls "${RUN_DIR}"/extenddb-*.pid 2>/dev/null | head -n 1 || true)"
    if [ -n "$PID_FILE" ] && [ -f "$PID_FILE" ]; then
        break
    fi
    sleep 0.5
    i=$((i + 1))
done

if [ -z "${PID_FILE:-}" ] || [ ! -f "$PID_FILE" ]; then
    echo "extenddb-entrypoint: daemon failed to start (no PID file under $RUN_DIR)" >&2
    exit 1
fi

PID="$(cat "$PID_FILE")"
echo "extenddb-entrypoint: daemon started (pid $PID, pid-file $PID_FILE)"

# Poll on the daemon. kill -0 returns 0 while the process is alive.
# 500 ms granularity keeps shutdown responsive without burning CPU.
while kill -0 "$PID" 2>/dev/null; do
    sleep 0.5
done

echo "extenddb-entrypoint: daemon (pid $PID) exited"

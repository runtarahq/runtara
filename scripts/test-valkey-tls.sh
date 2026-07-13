#!/usr/bin/env bash
# Copyright (C) 2025 SyncMyOrders Sp. z o.o.
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Run the fail-closed TLS Valkey integration suite against an isolated
# TLS-only container. Requires Docker and OpenSSL.

set -euo pipefail

workspace="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tls_dir="$(mktemp -d)"
container_name="runtara-valkey-tls-${RANDOM}-${RANDOM}"
host_port="${VALKEY_TLS_TEST_PORT:-6390}"

cleanup() {
    docker rm -f "$container_name" >/dev/null 2>&1 || true
    rm -rf "$tls_dir"
}
trap cleanup EXIT

openssl req -x509 -newkey rsa:2048 -nodes -days 1 \
    -keyout "$tls_dir/ca.key" -out "$tls_dir/ca.crt" \
    -subj '/CN=Runtara Test CA' \
    -addext 'basicConstraints=critical,CA:TRUE' >/dev/null 2>&1

openssl req -new -newkey rsa:2048 -nodes \
    -keyout "$tls_dir/server.key" -out "$tls_dir/server.csr" \
    -subj '/CN=localhost' \
    -addext 'subjectAltName=DNS:localhost,IP:127.0.0.1' >/dev/null 2>&1

printf '%s\n' \
    'basicConstraints=critical,CA:FALSE' \
    'keyUsage=critical,digitalSignature,keyEncipherment' \
    'extendedKeyUsage=serverAuth' \
    'subjectAltName=DNS:localhost,IP:127.0.0.1' \
    | openssl x509 -req -in "$tls_dir/server.csr" \
        -CA "$tls_dir/ca.crt" -CAkey "$tls_dir/ca.key" -CAcreateserial \
        -days 1 -out "$tls_dir/server.crt" -extfile /dev/stdin >/dev/null 2>&1

openssl req -x509 -newkey rsa:2048 -nodes -days 1 \
    -keyout "$tls_dir/wrong.key" -out "$tls_dir/wrong.crt" \
    -subj '/CN=Wrong Test CA' \
    -addext 'basicConstraints=critical,CA:TRUE' >/dev/null 2>&1

# The valkey image's docker-entrypoint.sh re-execs valkey-server as the
# non-root `valkey` user. `mktemp -d` is 0700 and openssl writes keys 0600,
# both owned by the runner user — on Linux CI that user isn't the container's
# `valkey` user, so it can't traverse the dir or read the certs and the
# server exits immediately. (Docker Desktop on macOS masks this, so it only
# bites in CI.) Make the throwaway test material world-readable.
chmod 0755 "$tls_dir"
chmod 0644 "$tls_dir"/*.crt "$tls_dir"/*.key

# No --rm: a container that exits on a cert/config error must survive long
# enough for `docker logs` to explain why. The cleanup trap removes it.
docker run -d --name "$container_name" \
    -p "127.0.0.1:${host_port}:6379" \
    -v "$tls_dir:/tls:ro" \
    valkey/valkey:8-alpine \
    valkey-server \
    --port 0 \
    --tls-port 6379 \
    --tls-cert-file /tls/server.crt \
    --tls-key-file /tls/server.key \
    --tls-ca-cert-file /tls/ca.crt \
    --tls-auth-clients no >/dev/null

for attempt in $(seq 1 50); do
    if docker exec "$container_name" valkey-cli --tls --insecure -p 6379 ping \
        2>/dev/null | grep -q PONG; then
        break
    fi
    # Bail out early with diagnostics if the container has already exited,
    # rather than spinning the full readiness budget against a dead container.
    running="$(docker inspect -f '{{.State.Running}}' "$container_name" 2>/dev/null || echo missing)"
    if [ "$running" != "true" ] || [ "$attempt" -eq 50 ]; then
        echo "valkey container is not ready (running=$running, attempt=$attempt); logs:" >&2
        docker logs "$container_name" 2>&1 || true
        exit 1
    fi
    sleep 0.1
done

cd "$workspace"
VALKEY_HOST=localhost \
VALKEY_PORT="$host_port" \
VALKEY_TLS=1 \
VALKEY_TLS_CA_CERT="$tls_dir/ca.crt" \
VALKEY_TLS_WRONG_CA="$tls_dir/wrong.crt" \
cargo test -p runtara-server \
    --features valkey-tls-integration-tests \
    --test valkey_tls \
    -- --test-threads=1

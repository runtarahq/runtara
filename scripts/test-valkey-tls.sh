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

docker run -d --rm --name "$container_name" \
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
    if [ "$attempt" -eq 50 ]; then
        docker logs "$container_name"
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

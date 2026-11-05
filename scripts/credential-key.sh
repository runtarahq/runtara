# shellcheck shell=bash
# Source this file; do not run it.
#
# runtara-server refuses to start without RUNTARA_CONNECTIONS_ENCRYPTION_KEY:
# connection credentials are always encrypted at rest. Local launchers and
# e2e scripts call `ensure_credential_key` so they keep working with no setup.
#
#   ensure_credential_key [KEY_FILE]
#
# Leaves an already-set RUNTARA_CONNECTIONS_ENCRYPTION_KEY alone. Otherwise:
#   - with KEY_FILE, reads the key from it, creating it (mode 0600) on first
#     use, so credentials stored by earlier runs stay readable;
#   - without one, makes a throwaway key for this process (fine for e2e runs
#     whose databases are dropped afterwards).
# Exports the key. Never prints it.

generate_credential_key() {
    if command -v openssl >/dev/null 2>&1; then
        openssl rand -base64 32
    else
        head -c 32 /dev/urandom | base64
    fi
}

ensure_credential_key() {
    local key_file="${1:-}"
    if [ -n "${RUNTARA_CONNECTIONS_ENCRYPTION_KEY:-}" ]; then
        export RUNTARA_CONNECTIONS_ENCRYPTION_KEY
        return 0
    fi
    if [ -z "$key_file" ]; then
        RUNTARA_CONNECTIONS_ENCRYPTION_KEY="$(generate_credential_key)"
        export RUNTARA_CONNECTIONS_ENCRYPTION_KEY
        return 0
    fi
    if [ ! -s "$key_file" ]; then
        mkdir -p "$(dirname "$key_file")"
        (umask 077 && generate_credential_key >"$key_file")
        echo "Generated a credential encryption key at ${key_file}." \
            "Keep it: credentials stored under a lost key cannot be recovered." >&2
    fi
    RUNTARA_CONNECTIONS_ENCRYPTION_KEY="$(tr -d '[:space:]' <"$key_file")"
    export RUNTARA_CONNECTIONS_ENCRYPTION_KEY
}

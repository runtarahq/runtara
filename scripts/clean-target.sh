#!/usr/bin/env bash
# Bounds target/ growth. Runs `cargo clean` when target/ exceeds THRESHOLD_GIB.
# sccache (configured as rustc-wrapper in .cargo/config.toml) makes the
# subsequent rebuild cheap, so this is safe to run unattended.
#
# Wire to launchd via ~/Library/LaunchAgents/com.runtara.clean-target.plist.
# Manual run: scripts/clean-target.sh [--force]

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
THRESHOLD_GIB="${RUNTARA_TARGET_THRESHOLD_GIB:-15}"
# Log lives outside TARGET_DIR — `cargo clean` removes that directory and
# would yank the log file out from under us mid-run.
LOG_FILE="${RUNTARA_CLEAN_LOG:-$HOME/Library/Logs/runtara/clean-target.log}"

log() {
    mkdir -p "$(dirname "$LOG_FILE")"
    printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*" | tee -a "$LOG_FILE"
}

if [[ ! -d "$TARGET_DIR" ]]; then
    log "no target dir at $TARGET_DIR, nothing to do"
    exit 0
fi

size_gib() {
    local kb
    kb=$(du -sk "$1" 2>/dev/null | awk '{print $1}')
    awk -v k="$kb" 'BEGIN{printf "%.1f", k/1024/1024}'
}

current_gib=$(size_gib "$TARGET_DIR")
force=0
[[ "${1:-}" == "--force" ]] && force=1

log "target=$TARGET_DIR size=${current_gib}GiB threshold=${THRESHOLD_GIB}GiB force=$force"

if [[ $force -eq 0 ]] && awk -v c="$current_gib" -v t="$THRESHOLD_GIB" 'BEGIN{exit !(c<t)}'; then
    log "under threshold, no action"
    exit 0
fi

cd "$REPO_ROOT"
log "running cargo clean"
if cargo clean 2>>"$LOG_FILE"; then
    after_gib=$(size_gib "$TARGET_DIR" 2>/dev/null || echo "0.0")
    log "cleaned: ${current_gib}GiB -> ${after_gib}GiB"
else
    log "cargo clean failed (see log)"
    exit 1
fi

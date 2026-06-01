# syntax=docker/dockerfile:1

# Runtara server image.
#
# Built FROM a pre-built, smoke-tested release bundle (the same
# runtara-<ver>-<arch>-linux.tar.gz published to GitHub Releases) rather than
# compiled from source — the image ships exactly what the release pipeline
# already verified. The bundle embeds wasmtime + the prebuilt agent/shared
# WASM components (workflow compilation is fully in-process), so this image is
# arch-specific: each platform consumes its own-arch bundle and the multi-arch
# manifest is merged in CI.
#
# The CI step stages the extracted bundle into ./runtara/ (tar
# --strip-components=1) before building, so the layout below is deterministic
# regardless of version or arch.
FROM ubuntu:22.04

# glibc note: bundles are built on ubuntu-22.04 runners, so the runtime base
# must be ubuntu:22.04 (or a newer-glibc derivative) for ABI compatibility with
# the dynamically-linked runtara-server binary.
#   - ca-certificates: outbound HTTPS (integrations)
#   - curl:            container HEALTHCHECK only
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

# Unprivileged runtime user whose primary group is root (GID 0). The bundle
# under /opt/runtara stays root-owned and read-only; everything the server
# writes goes to $HOME (cargo cache) and $DATA_DIR (workflow build cache +
# runtime state). Making those dirs GID-0-owned and group-writable (g=u) is the
# OpenShift "arbitrary uid" pattern: the container runs cleanly as the default
# uid 10001 AND under `--user <any-uid>` (Docker assigns GID 0 to unknown
# uids), so bind mounts work without a host-side chown.
RUN useradd --system --create-home --home-dir /home/runtara --uid 10001 --gid 0 runtara \
    && chmod -R g=u /home/runtara

# Bundle pre-extracted by CI: tar -xzf runtara-*-linux.tar.gz \
#   --strip-components=1 -C docker/runtara
COPY runtara/ /opt/runtara/

# Same env contract as scripts/install.sh and e2e/install-test/Dockerfile,
# rebased onto /opt/runtara.
ENV PATH="/opt/runtara/bin:${PATH}" \
    WASMTIME_PATH="/opt/runtara/bin/wasmtime" \
    RUNTARA_AGENT_COMPONENTS_DIR="/opt/runtara/agents" \
    HOME="/home/runtara" \
    DATA_DIR="/data" \
    SERVER_HOST="0.0.0.0" \
    SERVER_PORT="7001"

# Runtime state (instance data, direct compose scratch). Owned by GID 0 and
# group-writable, so a fresh named volume inherits that ownership and bind
# mounts stay writable under any uid. Declared as a volume so it persists.
RUN mkdir -p /data && chown -R 10001:0 /data && chmod -R g=u /data
VOLUME ["/data"]

LABEL org.opencontainers.image.source="https://github.com/runtarahq/runtara" \
      org.opencontainers.image.title="runtara" \
      org.opencontainers.image.description="Runtara server (workflow runtime + API + UI)"

USER runtara
WORKDIR /home/runtara

EXPOSE 7001

# The server still needs Postgres + Valkey + config env (DB URLs, TENANT_ID,
# auth) supplied at runtime — this is a server image, not all-in-one.
HEALTHCHECK --interval=10s --timeout=5s --start-period=40s --retries=5 \
    CMD curl -fsS "http://127.0.0.1:${SERVER_PORT}/health" || exit 1

CMD ["runtara-server"]

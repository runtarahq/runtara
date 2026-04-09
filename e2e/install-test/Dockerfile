FROM ubuntu:22.04

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y curl ca-certificates && rm -rf /var/lib/apt/lists/*

# Default: install from GitHub releases. Override with --build-arg INSTALL_SOURCE=local
ARG INSTALL_SOURCE=github

# For local testing, copy these into the build context:
#   cp scripts/install.sh e2e/install-test/install.sh
#   cp target/bundle/runtara-*-linux.tar.gz e2e/install-test/
COPY install.sh* runtara-*-linux.tar.gz* /tmp/

RUN if [ "$INSTALL_SOURCE" = "local" ] && [ -f /tmp/install.sh ]; then \
        chmod +x /tmp/install.sh && \
        /tmp/install.sh --user --skip-service --bundle "$(ls /tmp/runtara-*-linux.tar.gz 2>/dev/null | head -1)"; \
    else \
        curl -fsSL https://github.com/runtarahq/runtara/releases/latest/download/install.sh | \
        bash -s -- --user --skip-service; \
    fi

ENV PATH="/root/.runtara/toolchain/bin:/root/.runtara/bin:${PATH}"
ENV LD_LIBRARY_PATH="/root/.runtara/toolchain/lib"
ENV RUNTARA_WASM_LIBRARY_DIR="/root/.runtara/stdlib"
ENV WASMTIME_PATH="/root/.runtara/bin/wasmtime"

EXPOSE 7001

CMD ["runtara-server"]

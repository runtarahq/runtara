FROM ubuntu:22.04

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y curl ca-certificates && rm -rf /var/lib/apt/lists/*

# Copy the install script and pre-built bundle for local testing
# For CI/production: remove COPY lines and use curl from GitHub releases instead
COPY install.sh /tmp/install.sh
RUN chmod +x /tmp/install.sh
COPY runtara-*-linux.tar.gz /tmp/

RUN /tmp/install.sh --user --skip-service --bundle "$(ls /tmp/runtara-*-linux.tar.gz | head -1)"

ENV PATH="/root/.runtara/toolchain/bin:/root/.runtara/bin:${PATH}"
ENV LD_LIBRARY_PATH="/root/.runtara/toolchain/lib"
ENV RUNTARA_WASM_LIBRARY_DIR="/root/.runtara/stdlib"
ENV WASMTIME_PATH="/root/.runtara/bin/wasmtime"

EXPOSE 7001

CMD ["runtara-server"]

FROM ubuntu:22.04

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y curl ca-certificates && rm -rf /var/lib/apt/lists/*

# Download and run the install script from the latest GitHub release
RUN curl -fsSL https://github.com/runtarahq/runtara/releases/latest/download/install.sh | \
    bash -s -- --user --skip-service

ENV PATH="/root/.runtara/toolchain/bin:/root/.runtara/bin:${PATH}"
ENV LD_LIBRARY_PATH="/root/.runtara/toolchain/lib"
ENV RUNTARA_WASM_LIBRARY_DIR="/root/.runtara/stdlib"
ENV WASMTIME_PATH="/root/.runtara/bin/wasmtime"

EXPOSE 7001

CMD ["runtara-server"]

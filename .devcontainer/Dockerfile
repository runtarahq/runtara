FROM mcr.microsoft.com/devcontainers/rust:2-1-bookworm

# Include lld linker to improve build times either by using environment variable
# RUSTFLAGS="-C link-arg=-fuse-ld=lld" or with Cargo's configuration file (i.e see .cargo/config.toml).
RUN apt-get update && export DEBIAN_FRONTEND=noninteractive \
   && apt-get -y install clang lld protobuf-compiler \
   && apt-get autoremove -y && apt-get clean -y

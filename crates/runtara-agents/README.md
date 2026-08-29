# runtara-agents

Host-side pieces that don't fit in a WASM component: the SFTP worker, the
connection-type registry, and an S3 client.

## What it is

Agents normally ship as standalone WebAssembly components under
`crates/agents/runtara-agent-*`, and that is where nearly all of them live. This
crate holds what is left over:

- **`sftp`** — the only agent still executed natively. libssh2 is a C library
  with no `wasm32-wasip2` target, so the SFTP component is a thin shell that
  forwards each call to the server's `/api/internal/agents/sftp/{capability}`
  route, which dispatches here. Gated behind the `native` feature.
- **`extractors`** — connection-type descriptors and the `HttpConnectionExtractor`
  trait, consumed by `runtara-connections` to build connection forms and turn
  stored parameters into HTTP auth.
- **`types` / `connections`** — shared `AgentError`, `FileData`, `RawConnection`.
- **`s3_client`** — a standalone S3-compatible client used by the server's
  file-storage service (default file storage, attachments). Not a workflow
  agent; the S3 *capabilities* live in `runtara-agent-s3-storage`.
- **`registry` / `static_registry`** — an explicit static list, the dispatch and
  metadata source for the above. The production agent catalog does not come from
  here: it is built by the component dispatcher from the `meta.json` sidecars
  next to each `.wasm`.

Compression and XLSX used to be native workers here too. They aren't: `zip`'s
C-backed backends (bzip2, zstd, lzma) are optional features those capabilities
never used, and `calamine` is pure Rust, so both build for `wasm32-wasip2` and
now run entirely in the sandbox. `tests/native_registry_scope.rs` pins that
boundary — adding a native worker back is a deliberate decision, not a routine
change.

## Inside Runtara

- Consumed by `runtara-server` (SFTP dispatch, file storage, catalog
  augmentation) and `runtara-connections` (connection types, with
  `default-features = false`, so `ssh2`/OpenSSL stay out of its closure).
- Built on `runtara-dsl` (capability metadata, error model) and
  `runtara-agent-macro` (the `#[capability]` / `CapabilityOutput` derives that
  emit named metadata and executor statics).
- Key integration point: `runtara_agents::registry`.
- Platform features: `native` (default, servers and CLIs) pulls in `ssh2`;
  `wasi` swaps the HTTP transport and leaves SFTP out, so the metadata half
  of the crate compiles for `wasm32-wasip2`.

## License

AGPL-3.0-or-later.

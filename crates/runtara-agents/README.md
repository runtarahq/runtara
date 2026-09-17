# runtara-agents

Host support for connection schemas, shared types, and server file storage.
Workflow agents execute as standalone WebAssembly components under
`crates/agents/runtara-agent-*`.

- **`extractors`** defines connection descriptors and HTTP authentication extraction.
- **`registry` / `static_registry`** provides connection-type discovery for forms,
  credential services, and catalog augmentation. It contains no capability executor.
- **`types` / `connections`** provides shared errors, file data, and connection references.
- **`s3_client`** serves the server's file-storage and attachment services. Workflow
  S3 capabilities live in `runtara-agent-s3-storage`.

The `native` feature selects native HTTP transport for the server's S3 client;
`wasi` selects WASI HTTP. Metadata consumers can disable default features.
The runtime agent catalog comes from WASM component metadata sidecars.

## License

AGPL-3.0-or-later.

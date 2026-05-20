# Third-Party Notices

The Runtara bundle distributed at `/opt/runtara` (or `~/.runtara` for
user-mode installs) includes third-party software in addition to Runtara
itself. This file indexes those components, the licenses they are
distributed under, and where to obtain their source code.

Runtara itself is licensed under the GNU Affero General Public License
v3.0 or later. See `LICENSE-runtara-AGPL-3.0` in this directory for the
full text.

## Bundled components

### The Rust Programming Language

**Bundled as:** `toolchain/bin/rustc`, `toolchain/bin/cargo`,
`toolchain/lib/rustlib/*` and all other files under `toolchain/`.

**License:** Apache License 2.0 OR MIT (dual-licensed; you may choose
either).

**License texts:** `LICENSE-rust-APACHE-2.0`, `LICENSE-rust-MIT`,
`NOTICE-rust`.

**Source code:** The Runtara bundle ships an unmodified upstream Rust
distribution obtained from <https://static.rust-lang.org/dist/>. The
exact Rust version bundled with this Runtara release is recorded in
the bundle's `MANIFEST.json` (key: `rustc_version`). To obtain the
corresponding source code, visit:

    https://github.com/rust-lang/rust

and check out the tag matching that version. Runtara does not patch,
fork, or otherwise modify the Rust compiler, Cargo, or the Rust
standard library.

**Trademarks:** "Rust" and the Rust logo are trademarks of the Rust
Foundation. Runtara is not affiliated with or endorsed by the Rust
Foundation. Runtara redistributes unmodified upstream Rust binaries in
accordance with the Rust Foundation's trademark policy.

### Wasmtime

**Bundled as:** `bin/wasmtime`.

**License:** Apache License 2.0 with LLVM Exception.

**License text:** `LICENSE-wasmtime-APACHE-2.0`.

**Source code:** The Runtara bundle ships an unmodified upstream
Wasmtime CLI binary obtained from the Wasmtime GitHub release assets.
The exact Wasmtime version bundled with this Runtara release is
recorded in the bundle's `MANIFEST.json` (key: `wasmtime_version`).
To obtain the corresponding source code, visit:

    https://github.com/bytecodealliance/wasmtime

and check out the tag matching that version.

### wac (WebAssembly Composition CLI)

**Bundled as:** `bin/wac`.

**License:** Apache License 2.0 with LLVM Exception.

**License text:** `LICENSE-wac-APACHE-2.0`.

**Source code:** The Runtara bundle ships an unmodified upstream `wac`
CLI binary obtained from the wac GitHub release assets. `wac` is used
at workflow compile time to compose a per-workflow logic component
with the agent components it depends on into a single composed
`.wasm`. The exact version bundled is recorded in `MANIFEST.json`
(key: `wac_version`). To obtain the corresponding source code, visit:

    https://github.com/bytecodealliance/wac

and check out the tag matching that version.

### cargo-component

**Bundled as:** `bin/cargo-component`.

**License:** Apache License 2.0 with LLVM Exception.

**License text:** `LICENSE-cargo-component-APACHE-2.0`.

**Source code:** Upstream does not publish prebuilt binaries; the
Runtara bundle compiles `cargo-component` from source via
`cargo install cargo-component --version <pinned> --locked` against
the bundled Rust toolchain. No patches are applied. The exact version
bundled is recorded in `MANIFEST.json` (key:
`cargo_component_version`). To obtain the corresponding source code,
visit:

    https://github.com/bytecodealliance/cargo-component

and check out the tag matching that version. `cargo-component` is
used at workflow compile time to build the per-workflow logic crate
into a WebAssembly Component (`--target wasm32-wasip2`) before `wac`
composes it with the required agent components.

### Runtara agent components (pre-built)

**Bundled as:** `agents/runtara_agent_<id>.wasm` and
`agents/runtara_agent_<id>.meta.json` (one pair per agent).

**License:** GNU Affero General Public License v3.0 or later
(`LICENSE-runtara-AGPL-3.0`). The agent components are Runtara's own
code, pre-compiled in CI against the bundled Rust toolchain for the
`wasm32-wasip2` target. The sibling `.meta.json` files are JSON
sidecars derived deterministically from the Rust source by the
workspace's host-only `runtara-agent-bundle-emit` tool — they are
build artifacts, not hand-authored.

**Source code:** <https://github.com/runtarahq/runtara>. Each
component crate lives at `crates/agents/runtara-agent-<id>/`. The
exact Runtara version is recorded in `MANIFEST.json` (key:
`runtara_version`); the number of components is recorded in
`agent_component_count`.

### Runtara workflow standard library (pre-built)

**Bundled as:** `stdlib/libruntara_workflow_stdlib.rlib` and the
contents of `stdlib/deps/`.

**License:** GNU Affero General Public License v3.0 or later
(`LICENSE-runtara-AGPL-3.0`). This is Runtara's own code, pre-compiled
in CI against the bundled Rust toolchain for the `wasm32-wasip2`
target (scenario rlibs) and the host target (proc-macro dynamic
libraries).

**Source code:** <https://github.com/runtarahq/runtara>. The exact
Runtara version is recorded in the bundle's `MANIFEST.json` (key:
`runtara_version`).

## License files in this bundle

| File | Applies to |
| --- | --- |
| `LICENSE-runtara-AGPL-3.0` | Runtara source, Runtara binaries, the pre-built workflow stdlib, and the pre-built agent components |
| `LICENSE-rust-APACHE-2.0`, `LICENSE-rust-MIT`, `NOTICE-rust` | The bundled Rust toolchain |
| `LICENSE-wasmtime-APACHE-2.0` | The bundled Wasmtime CLI |
| `LICENSE-wac-APACHE-2.0` | The bundled `wac` CLI |
| `LICENSE-cargo-component-APACHE-2.0` | The bundled `cargo-component` CLI |
| `THIRD-PARTY-NOTICES.md` | This index |

## Written offer for source code

For any of the above components, if you would like to obtain the
source code and are unable to access the upstream repositories listed,
you may request a copy by contacting:

    hello@syncmyorders.com

Please include the bundle version (from `/opt/runtara/VERSION`) in
your request so we can provide the exact matching source archive.

# Third-Party Notices

The Runtara bundle distributed at `/opt/runtara` (or `~/.runtara` for
user-mode installs) includes third-party software in addition to Runtara
itself. This file indexes those components, the licenses they are
distributed under, and where to obtain their source code.

Runtara itself is licensed under the GNU Affero General Public License
v3.0 or later. See `LICENSE-runtara-AGPL-3.0` in this directory for the
full text.

## Bundled components

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

### Runtara agent components (pre-built)

**Bundled as:** `agents/runtara_agent_<id>.wasm` and
`agents/runtara_agent_<id>.meta.json` (one pair per agent).

**License:** GNU Affero General Public License v3.0 or later
(`LICENSE-runtara-AGPL-3.0`). The agent components are Runtara's own
code, pre-compiled in CI for the `wasm32-wasip2` target. The server
composes these prebuilt components with the byte-emitted workflow-logic
module in-process at compile time. The sibling `.meta.json` files are JSON
sidecars derived deterministically from the Rust source by the
workspace's host-only `runtara-agent-bundle-emit` tool — they are
build artifacts, not hand-authored.

**Source code:** <https://github.com/runtarahq/runtara>. Each
component crate lives at `crates/agents/runtara-agent-<id>/`. The
exact Runtara version is recorded in `MANIFEST.json` (key:
`runtara_version`); the number of components is recorded in
`agent_component_count`.

### Runtara shared workflow components (pre-built)

**Bundled as:** the shared workflow `.wasm` components and their
`.meta.json` sidecars under `agents/`.

**License:** GNU Affero General Public License v3.0 or later
(`LICENSE-runtara-AGPL-3.0`). This is Runtara's own code, pre-compiled
in CI for the `wasm32-wasip2` target. The server composes these shared
components with the byte-emitted workflow-logic module in-process at
compile time.

**Source code:** <https://github.com/runtarahq/runtara>. The exact
Runtara version is recorded in the bundle's `MANIFEST.json` (key:
`runtara_version`).

## License files in this bundle

| File | Applies to |
| --- | --- |
| `LICENSE-runtara-AGPL-3.0` | Runtara source, Runtara binaries, the pre-built shared workflow components, and the pre-built agent components |
| `LICENSE-wasmtime-APACHE-2.0` | The bundled Wasmtime CLI |
| `THIRD-PARTY-NOTICES.md` | This index |

## Written offer for source code

For any of the above components, if you would like to obtain the
source code and are unable to access the upstream repositories listed,
you may request a copy by contacting:

    hello@syncmyorders.com

Please include the bundle version (from `/opt/runtara/VERSION`) in
your request so we can provide the exact matching source archive.

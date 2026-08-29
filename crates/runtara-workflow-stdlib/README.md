# runtara-workflow-stdlib

[![Crates.io](https://img.shields.io/crates/v/runtara-workflow-stdlib.svg)](https://crates.io/crates/runtara-workflow-stdlib)
[![Documentation](https://docs.rs/runtara-workflow-stdlib/badge.svg)](https://docs.rs/runtara-workflow-stdlib)
[![License](https://img.shields.io/crates/l/runtara-workflow-stdlib.svg)](LICENSE)

The manifest evaluator every direct-emitted runtara workflow composes against.

## What it is

A direct-emitted workflow carries its graph as a JSON manifest and calls into this crate for every pure decision it has to make: resolving a reference path, applying an input mapping, rendering a `minijinja` template, evaluating a condition, processing Switch output, deriving Split/While iteration state and results, and validating agent and child-workflow inputs. `init_manifest` loads the graph once; the rest of the interface is JSON in, JSON out, over an interning value store that keeps large scope values out of the guest's bump allocator.

Everything here is pure. Durability — registration, checkpointing, signals, heartbeats — lives in `runtara-workflow-runtime`, and agent calls go out over each agent's own WIT interface, bound at `wac compose` time. So this crate has no HTTP client, no SDK dependency, and no target-specific backends: `serde`, `serde_json`, and `minijinja` are the whole dependency set, and the same code builds for every target.

## Using it standalone

Not intended for hand-written workflows — author DSL and let `runtara-workflows` emit the manifest. The one module worth depending on directly is `reference_path`, which is what keeps authoring-time validation and runtime resolution agreeing on how a path splits into segments:

```toml
[dependencies]
runtara-workflow-stdlib = { version = "8.7", default-features = false }
```

```rust
use runtara_workflow_stdlib::reference_path::reference_segments;

let segments = reference_segments("steps.fetch.outputs.items[0].id");
assert_eq!(segments, ["steps", "fetch", "outputs", "items", "0", "id"]);
```

## Inside Runtara

- Built two ways. `--features direct-component` for `wasm32-wasip2` produces `runtara_workflow_stdlib.wasm`, which ships in the bundle's `agents/` directory and exports `runtara:workflow-stdlib/json@0.1.0` (see `scripts/build-agent-components.sh`). With no features it is a plain rlib for `runtara-workflows`.
- `direct-component` is the only feature. The crate compiles identically on native, `wasm32-wasip2`, and `wasm32-unknown-unknown` — the last of which matters because `runtara-validation-wasm` pulls it in transitively for the browser validator.
- `runtara-workflows` shares `reference_path` with its authoring-time validator, and `runtara-dsl`'s `step_output_shape` treats `direct_json` as ground truth for per-step output shapes.
- The WIT world it exports lives in `runtara-workflow-wit` (`wit/stdlib/runtara-workflow-stdlib.wit`); adding a guest function means editing that WIT and the `component` module in `src/lib.rs` together.
- `direct_json` is large and load-bearing: its unit tests are the contract between the emitter in `runtara-workflows` and what a running workflow actually does, so changes there need `cargo test -p runtara-workflows` as well as this crate's own tests.

## License

AGPL-3.0-or-later.

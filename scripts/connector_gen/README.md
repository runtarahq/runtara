# connector_gen

Generates a workflow agent's capabilities from the vendor's OpenAPI spec.
[openapi-generator](https://openapi-generator.tech) parses the spec; our
templates in `templates/` render every model and operation into one file,
`src/generated.rs`, that uses the same `#[capability]`, `CapabilityInput` and
`CapabilityOutput` macros as hand-written agents.

This README covers the tool. Wiring an agent into the platform (connection
types, registrations, verification) is in
[docs/adding-an-agent.md](../../docs/adding-an-agent.md).

Run every command below from the repository root.

## Setup

The pip package bundles a JDK through the `jdk4py` extra, so no system Java is
needed. It needs Python 3.10 or newer; macOS's own `python3` is 3.9, so name a
newer interpreter explicitly (Homebrew's, for example):

```bash
/opt/homebrew/bin/python3.14 -m venv scripts/connector_gen/.venv
scripts/connector_gen/.venv/bin/pip install -r scripts/connector_gen/requirements.txt
```

The venv is git-ignored. `connector_gen.py` uses
`scripts/connector_gen/.venv/bin/openapi-generator-cli` when it exists,
otherwise `$OPENAPI_GENERATOR_CLI` or `openapi-generator-cli` on `PATH`.

## Walkthrough: Pipedrive

`crates/agents/runtara-agent-pipedrive` was produced with these steps.

**1. Pin the spec.** Find the vendor's OpenAPI 3 spec URL and its sha256:

```bash
curl -sSL https://developers.pipedrive.com/docs/api/v1/openapi-v2.yaml | shasum -a 256
```

**2. Pick the operations.** List the spec's operationIds and choose the ones
users need; a spec can have hundreds.

```bash
curl -sSL https://developers.pipedrive.com/docs/api/v1/openapi-v2.yaml | grep "operationId:"
```

**3. Create the crate.** Copy `crates/agents/runtara-agent-pipedrive/` to
`runtara-agent-<id>/`, keep `Cargo.toml` (renamed), `src/lib.rs` and
`src/kit.rs`, and write `connector.yaml`:

```yaml
agent:
  id: pipedrive                      # agent id and capability module
  name: Pipedrive
  description: Pipedrive CRM — deals.
  integration_id: pipedrive_api_token  # connection type the agent binds to
  error_prefix: PIPEDRIVE            # error codes: PIPEDRIVE_RATE_LIMITED, …
spec:
  url: https://developers.pipedrive.com/docs/api/v1/openapi-v2.yaml
  sha256: 2b0fb0de3cad7a37158528034f85942e6b22a0a0649d9c811e0676e7c040955d
labels:                              # optional: field name -> label shown in the step editor
  org_id: Organization
operations:                          # operationIds to generate
  - getDeals
  - getDeal
  - addDeal
  - updateDeal
  - deleteDeal
```

**4. Fetch.** Downloads the spec, refuses to continue if its sha256 differs from
the pin, and keeps the listed operations plus every component they reference.
Writes `spec.slice.json` and `tests/fixtures/<operationId>.json` (the spec's own
2xx response examples).

```bash
scripts/connector_gen/.venv/bin/python scripts/connector_gen/connector_gen.py fetch crates/agents/runtara-agent-pipedrive
```

**5. Generate.** Renders `src/generated.rs`, formatted with the repository's
`rustfmt`. Offline and deterministic: the same pin, slice and templates always
give the same file.

```bash
scripts/connector_gen/.venv/bin/python scripts/connector_gen/connector_gen.py generate crates/agents/runtara-agent-pipedrive
```

For Pipedrive this gives five capabilities: `get-deals` and `get-deal` (reads;
inputs such as `id`, `include_fields`, `limit`, `cursor`), and `add-deal`,
`update-deal`, `delete-deal` (side effects; `add-deal` takes the deal's fields
`title`, `value`, `currency`, `person_id`, … as inputs). Outputs are typed, e.g.
`get-deal` returns `data` with the deal's fields.

**6. Register the agent** in `crates/runtara-agent-bundle-emit`: a path
dependency in `Cargo.toml` and an `agents()` entry in `src/main.rs`. Without it
`scripts/build-agent-components.sh` fails and the server will not boot.

```toml
runtara-agent-pipedrive = { path = "../agents/runtara-agent-pipedrive" }
```

```rust
("pipedrive", runtara_agent_pipedrive::agent_info()),
```

**7. Test and verify.** Add tests like `runtara-agent-pipedrive/tests/examples.rs`
(the response examples round-trip; the metadata lists the expected
capabilities and inputs), then:

```bash
cargo test -p runtara-agent-pipedrive
cargo clippy -p runtara-agent-pipedrive --all-targets -- -D warnings
cargo build -p runtara-agent-pipedrive --target wasm32-wasip2 --release
```

A step can only use the agent through a connection of its `integration_id`.
Pipedrive's is `pipedrive_api_token` (an API token sent as `x-api-token` to
`https://api.pipedrive.com`); a new vendor needs one too, see
[docs/adding-an-agent.md](../../docs/adding-an-agent.md#connection-type).

## Everyday changes

**Add operations.** Append operationIds to `connector.yaml`, e.g. Pipedrive
persons:

```yaml
  - getPersons
  - getPerson
  - addPerson
  - updatePerson
```

Run `fetch` and `generate` again and review `git diff`: new capabilities
`get-persons`, `get-person`, `add-person`, `update-person`. Capabilities are
grouped by the operation's (first) tag, then ordered by operationId, whatever
the order of the list.

**The vendor changed the spec.** `fetch` stops with
`… changed: sha256 is <new>, connector.yaml pins <old>`. Put the new hash in
`connector.yaml`, run `fetch` and `generate`, and review the diff of
`spec.slice.json` and `src/generated.rs`: it is exactly the vendor's change to
the operations you use.

**Before committing**, confirm everything derived from the spec is current:

```bash
scripts/connector_gen/.venv/bin/python scripts/connector_gen/connector_gen.py check crates/agents/runtara-agent-pipedrive
```

`check` takes several crates at once and fails when `src/generated.rs` is stale
or edited by hand (printing the diff), when `tests/fixtures` no longer match the
examples in `spec.slice.json`, or when `connector.yaml`'s operations or labels
are not in `spec.slice.json` (edited without running `fetch`). It does not
download the spec, so a hand edit to `spec.slice.json` itself shows only in the
diff under review; `fetch` re-derives it.

**A schema name collides with the generated code** (`Value`, `Result`, or
`<OperationId>Input`): `fetch` stops and asks for a rename in `connector.yaml`:

```yaml
model_names:
  Value: Amount
```

## A generated agent crate

```
crates/agents/runtara-agent-<id>/
  connector.yaml      # spec source and operation allowlist (hand-written)
  spec.slice.json     # the spec trimmed to the listed operations (fetched)
  tests/fixtures/     # the spec's 2xx response examples (fetched)
  tests/*.rs          # hand-written tests
  src/generated.rs    # rendered by `generate`; never edit by hand
  src/kit.rs          # request builder and error envelope the generated code calls
  src/lib.rs          # hand-written: `pub mod kit; mod generated; pub use generated::*;`
```

## How the templates map the spec

- Each operation becomes a `<OperationId>Input` struct and an async
  `#[capability]` function. Path, query and header parameters are inputs, and
  so is every writable field of an object request body: the step takes `title`,
  `value`, … rather than one JSON `body` (that is what the generic HTTP agent is
  for). The function rebuilds the body from those fields. A body that is not an
  object stays a single `body` input. Non-GET operations are marked
  `side_effects`.
- An exploded array query parameter (the OpenAPI default) is sent as
  `ids=1&ids=2`; with `explode: false` it is sent as `ids=1,2`. Optional cookie
  parameters are dropped by `fetch`.
- An optional body field the spec marks `nullable` is an `Option<Option<T>>`
  (deserialized with `kit::nullable`): omitted is not sent, an explicit `null`
  is sent as `null` (to clear the vendor's value), and a value is sent as is.
- `<OperationId>Input::request()` builds the exact request without sending it,
  so tests can check the method, path, query and body; the capability function
  only adds the connection.
- The capability returns the full response type: `Vec<T>` for an array,
  `HashMap<String, T>` for a map, the primitive or model otherwise, and `Value`
  when the operation declares no JSON response. At runtime an empty success
  body (e.g. 204) decodes as `null`, and a non-JSON one as its text.
- Enums keep their wire type (`String`, `i64`) in inputs and outputs, so a value
  the spec does not list passes through unchanged; the allowed values are listed
  in the field's description. A `$ref` to an enum schema is inlined by `fetch`.
- `integer` without a `format` is generated as `i64` (`fetch` adds
  `format: int64`), so large ids and amounts fit.
- Every input and output field gets a label for the step editor, chosen by
  `fetch` in this order: the `labels:` override in `connector.yaml`; the
  vendor's `title` when it is a phrase (one-word titles name models); the field
  name humanized (`person_id`, `personId` → "Person ID", with known acronyms
  such as ID, URL, HTML, VAT kept upper-case). A field that is a bare `$ref`
  keeps its raw name unless overridden, because OpenAPI 3.0 ignores keys next to
  a `$ref`. Scalar `example`s become the field's placeholder and scalar
  `default`s the default value of optional inputs. Descriptions, summaries and
  the agent's name are printed as written; an operation without a summary is
  named after its operationId.
- Each schema becomes a `CapabilityOutput` struct. Every field is an `Option`,
  and a flattened `__extra` map (hidden from the metadata) keeps fields the
  vendor returns but does not declare.
- Type names are written bare (no `models::` prefix) so the output macro can
  resolve nested types; the `pascalcase` lambda turns raw inline schema names
  into the generated struct names.
- `fetch` marks operations with `x-runtara-*` vendor extensions (side effects,
  summaries, which capability carries the agent's module attributes) because
  the templates cannot compute them. An operation with several tags keeps its
  first, so it is rendered once.
- `oneOf` and `anyOf` schemas are kept as raw JSON (`Value`).

## Limits

- OpenAPI 3 only; convert Swagger 2.0 specs first.
- `fetch` refuses, naming the operation, what the templates cannot send yet:
  request bodies that are not JSON (form-encoded, multipart), object
  (`deepObject`) query parameters, and required cookie parameters.
- Hand-written extra capabilities cannot be added to a generated agent yet.
- A request-body field named like a path or query parameter of the same
  operation produces a compile error.

# runtara-text-parser

Parses free-form user text from chat/SMS-style channels into typed JSON values driven by `runtara-dsl` schema fields.

## What it is

A small, stateless library that turns one line of user input into a `serde_json::Value` that conforms to a `SchemaField` definition (type, format, bounds, enum, pattern). The public API is three functions plus helpers: `parse_text(input, field) -> ParseResult::{Ok(Value), Retry(hint)}`, `build_prompt(field_name, field) -> String` for rendering the corresponding question, and form-control helpers (`sort_fields`, `evaluate_visible_when`, `is_message_schema`, `try_single_field_parse`). It understands the same field types as the DSL — string (with `date`/`datetime`/`email`/`url`/`tel`/`color` formats and pattern/length constraints), integer, number, boolean (multilingual yes/no), enum (index, exact, unique-prefix), array (comma-separated, recursive on items), and object (raw JSON or deferred when it has properties). File uploads always return `Retry` since text channels can't carry them. No I/O, no async, no global state — callers drive the retry loop.

## Using it standalone

Add to `Cargo.toml` (path dep; the crate is `publish = false`):

```toml
runtara-text-parser = { path = "../runtara-text-parser" }
runtara-dsl         = { path = "../runtara-dsl" }
```

Typical loop — prompt, parse, retry on `Retry(hint)`:

```rust
use runtara_text_parser::{build_prompt, parse_text, ParseResult};

let prompt = build_prompt("quantity", &field);          // ask the user
match parse_text(&user_input, &field) {
    ParseResult::Ok(value)   => collected.insert("quantity".into(), value),
    ParseResult::Retry(hint) => send_hint(&hint),       // reprompt
};
```

For multi-field forms use `sort_fields` to order by `order`/name and
`evaluate_visible_when` to skip conditionally-hidden fields as you go.

## Inside Runtara

- Sole consumer today is `runtara-server` via `src/channels/collector.rs`, which drives schema-by-schema interactive collection over text-based channels (Slack, SMS, CLI) using `build_prompt` + `parse_text` and `sort_fields`/`evaluate_visible_when` to walk the form.
- Depends on `runtara-dsl` for `SchemaField` / `SchemaFieldType` / `VisibleWhen`, plus `serde_json`, `chrono` (relative-date parsing), and `regex`.
- Integration point is the schema produced by DSL-defined interactions: the same `SchemaField` that renders a web form here renders a text prompt and validates the reply.
- Runs native (inside the `runtara-server` process). It has no WASM-hostile deps, so it is safe to pull into guest crates if ever needed.
- Stateless by design: the server owns the retry counter, cancellation, and any channel plumbing (see `CollectError` for the error shape the caller is expected to surface).

## License

AGPL-3.0-or-later.

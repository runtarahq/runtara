# runtara-ai

Synchronous LLM completion abstraction for runtara workflows, built on the shared `runtara-http` client.

## What it is

A small, no-async LLM client layer tailored to generated workflow code that
runs inside the runtara WASM guest. It defines a `CompletionModel` trait plus
request/response types (messages, tool calls, tool results, usage), an
OpenAI-compatible provider implementation that also covers Azure OpenAI, vLLM,
and Ollama, and a dispatch layer that builds a model from connection
parameters — either directly from an `api_key` / `base_url` or through a
proxy using a `connection_id` header.

The API surface mirrors parts of `rig` but is fully synchronous: no tokio, no
futures, no streaming. A `CompletionRequestBuilder` chains `preamble`,
`chat_history`, `tools`, `temperature`, `max_tokens`, and provider-specific
`additional_params` before `.build()`. Public types (`CompletionRequest`,
`Message`, `AssistantContent`, `OneOrMany`, `ToolDefinition`) are re-exported
from the crate root for use by workflow codegen.

## Using it standalone

```rust
use runtara_ai::{Message, providers::openai};

let client = openai::Client::new(std::env::var("OPENAI_API_KEY")?.as_str());
let model = client.completion_model("gpt-4o");

let request = model
    .completion_request(Message::user("Say hello"))
    .preamble("You are concise.")
    .temperature(0.7)
    .build();

let response = model.completion(request)?;
println!("{:?}", response.choice);
```

For proxied credentials, use `provider::create_openai_model_with_connection` with a `connection_id`; requests become relative paths with an `X-Runtara-Connection-Id` header.

## Inside Runtara

- Consumed by `runtara-workflows` (workflow codegen emits calls against `CompletionModel`) and `runtara-workflow-stdlib` (stdlib helpers that wrap completions, structured output, and tool-using agent loops).
- Built on `runtara-http::HttpClient` so the same binary runs natively and under WASI — no reqwest, no tokio.
- `provider::structured_output_params` shapes JSON Schema into the provider-specific envelope (OpenAI's `response_format.json_schema`, Anthropic's `response_format.schema`).
- Errors are surfaced through `CompletionError` (`HttpError`, `JsonError`, `RequestError`, `ResponseError`, `ProviderError`) so the workflow runtime can distinguish transport, parse, and upstream failures.
- Designed to be called from the WASM guest; the host-side proxy resolves `connection_id` to real credentials, keeping API keys out of workflow code.

## License

AGPL-3.0-or-later.

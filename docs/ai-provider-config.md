# AI Provider Configuration and Connection Resolution

## Goal

Make workflow compilation independent from tenant connection data while keeping AI provider behavior explicit and auditable.

The compiler must not resolve `connection_id -> integration_id`. A workflow should compile from workflow JSON plus a component bundle. Runtime validation and the connection proxy own provider/connection compatibility and credential injection.

## Target Model

Agent and AiAgent steps pass `connection_id` for credential selection. AiAgent also declares provider intent in workflow config:

```json
{
  "stepType": "AiAgent",
  "connectionId": "conn_llm",
  "config": {
    "provider": "openai",
    "model": "gpt-4.1",
    "systemPrompt": { "valueType": "immediate", "value": "..." },
    "userPrompt": { "valueType": "immediate", "value": "..." }
  }
}
```

`provider` is not a connection integration id. It is an AI runtime contract. Current provider values are `openai` and `bedrock`.

## Provider Map

Provider compatibility is a map, not exact string equality:

| Provider | Compatible connection integration ids |
| --- | --- |
| `openai` | `openai_api_key` |
| `bedrock` | `aws_credentials` |

This leaves room for future provider aliases, OAuth/API-key variants, and compatible hosted integrations without changing workflow provider values.

## Responsibilities

- Compile time decides workflow structure, static components, and explicit capability inputs.
- AiAgent config declares provider intent and model selection.
- Validation checks provider/connection compatibility when connection context is available.
- Execution checks provider/connection compatibility again because connection rows can change after deployment.
- The proxy resolves `connection_id`, validates provider compatibility, and injects credentials into outgoing requests.
- Components route provider-specific behavior from explicit `provider`, not `_connection.integration_id`.

## Security Rule

Connection service data is not returned to workflow logic as a credential source. Runtime/proxy enriches outgoing requests. Components should not need raw connection parameters for provider routing, and there is no fallback from `_connection.integration_id`.

## Implemented Cutoff

- Removed compile-time `connection_id -> integration_id` resolution.
- Removed baked per-agent integration IDs from direct WASM static data.
- AI tool capabilities require explicit `provider`.
- The HTTP proxy rejects `ai_provider` and connection integration mismatches before credential injection.
- Server workflow validation checks AiAgent provider compatibility when tenant connection metadata is available.
- Direct WASM AiAgent execution sends `ai_provider` through the proxy envelope.

There is no legacy fallback from `_connection.integration_id`.

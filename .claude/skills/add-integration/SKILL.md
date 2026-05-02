---
name: add-integration
description: Use when adding support for a new external service (Salesforce, Notion, Zendesk, etc.) — i.e. an agent that talks to a third-party API and needs stored credentials. Covers the connection params struct, the HttpConnectionExtractor, and the integration agent file. Includes an advanced section for the rare case of adding a brand-new auth flow to the connection subsystem.
---

# Add a new integration

An **integration** = an agent that talks to an external service + a connection schema describing its credentials. Two halves:

1. **Connection params** in [crates/runtara-agents/src/agents/integrations/connection_types.rs](../../../crates/runtara-agents/src/agents/integrations/connection_types.rs) — declares the credential fields and how to turn them into HTTP headers/URL.
2. **Integration agent** in `crates/runtara-agents/src/agents/integrations/<name>.rs` — capabilities that consume the resolved connection.

Reference templates:
- API key: [stripe_api_key in connection_types.rs](../../../crates/runtara-agents/src/agents/integrations/connection_types.rs:944) + [stripe.rs](../../../crates/runtara-agents/src/agents/integrations/stripe.rs)
- OAuth2 authorization code: [hubspot_private_app](../../../crates/runtara-agents/src/agents/integrations/connection_types.rs:711) + [hubspot.rs](../../../crates/runtara-agents/src/agents/integrations/hubspot.rs)
- OAuth2 client credentials: [shopify_client_credentials](../../../crates/runtara-agents/src/agents/integrations/connection_types.rs:104)
- Basic auth: [mailgun](../../../crates/runtara-agents/src/agents/integrations/connection_types.rs:611) + [mailgun.rs](../../../crates/runtara-agents/src/agents/integrations/mailgun.rs)

## Decision: which auth flow?

Pick the closest match from `auth_type` values already in use:

| Service does... | `auth_type` | Reference |
|---|---|---|
| Static API key in header | `api_key` | stripe, mailgun, telegram |
| OAuth2 with user-driven authorization | `oauth2_authorization_code` | hubspot |
| OAuth2 client_id + client_secret exchange | `oauth2_client_credentials` | shopify_client_credentials, microsoft_entra |
| Custom signing (e.g. AWS SigV4) | none — agent handles it directly | bedrock |

If none fit, see the advanced section below.

## Steps

### 1. Add the connection params struct

In [connection_types.rs](../../../crates/runtara-agents/src/agents/integrations/connection_types.rs):

```rust
#[derive(Debug, Deserialize, ConnectionParams)]
#[connection(
    integration_id = "<service>_<auth_method>",   // unique id, snake_case
    display_name = "Service Name",
    description = "What this integration is for",
    category = "crm" | "ecommerce" | "llm" | "messaging" | "email" | "payment" | "storage" | "database" | "api",
    auth_type = "api_key" | "oauth2_authorization_code" | "oauth2_client_credentials"
)]
pub struct MyServiceParams {
    #[field(
        display_name = "API Key",
        description = "Service API key",
        placeholder = "sk_xxx",
        secret              // marks the field as a secret in the UI
    )]
    pub api_key: String,
    // ... other fields
}
```

For OAuth2 authorization code, also set `oauth_auth_url`, `oauth_token_url`, `oauth_default_scopes` — see hubspot example.

### 2. Add the HttpConnectionExtractor

In the same file, define a unit struct that implements `HttpConnectionExtractor`. Its job is to turn stored credentials into an `HttpConnectionConfig` (headers, URL prefix, query params).

```rust
pub struct MyServiceExtractor;

impl HttpConnectionExtractor for MyServiceExtractor {
    fn integration_id(&self) -> &'static str {
        "<service>_<auth_method>"
    }

    fn extract(&self, params: &Value) -> Result<HttpConnectionConfig, String> {
        let p: MyServiceParams = serde_json::from_value(params.clone())
            .map_err(|e| format!("Invalid <service> connection parameters: {}", e))?;

        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), format!("Bearer {}", p.api_key));
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        Ok(HttpConnectionConfig {
            headers,
            query_parameters: HashMap::new(),
            url_prefix: "https://api.example.com/v1".to_string(),
            rate_limit_config: None,
        })
    }
}

#[cfg(not(target_family = "wasm"))]
inventory::submit! {
    &MyServiceExtractor as &'static dyn HttpConnectionExtractor
}
```

**OAuth2 note:** for flows where the access token is resolved at runtime (not stored), the extractor sets URL + Content-Type only and **omits** the Authorization header. The agent's `resolve_access_token()` adds it later. See `HubSpotExtractor` and `ShopifyClientCredentialsExtractor` for this pattern.

### 3. Create the integration agent file

`crates/runtara-agents/src/agents/integrations/<name>.rs` — same shape as a regular agent (use `add-capability`), with two integration-specific touches on every capability:

```rust
use crate::connections::RawConnection;
use super::integration_utils::{require_connection, ProxyHttpClient};

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "...")]
pub struct MyOpInput {
    #[field(skip)]                                  // hidden from the UI
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,         // injected by the runtime
    // ... user-facing fields
}

#[capability(
    module = "my_service",
    display_name = "Do Op (My Service)",
    description = "...",
    module_display_name = "My Service",
    module_description = "...",
    module_has_side_effects = true,                 // typical for integrations
    module_supports_connections = true,             // required for integrations
    module_integration_ids = "<service>_<auth_method>",  // matches step 1
    module_secure = true                            // handles secrets
)]
pub fn do_op(input: MyOpInput) -> Result<MyOpOutput, AgentError> {
    let connection = require_connection("MY_SERVICE", &input._connection)?;
    // ... use connection.parameters and ProxyHttpClient
}
```

### 4. Re-export from `lib.rs`

Add `pub mod <name>;` in the integrations section of [crates/runtara-agents/src/lib.rs](../../../crates/runtara-agents/src/lib.rs).

### 5. Verify

1. `regen-frontend-api` so the new connection type appears in the Connections UI and capabilities show in the Step Picker.
2. Create a test connection through the UI (or via `/api/runtime/connections`) using real-or-test credentials.
3. Run `e2e-verify` with a workflow that uses the new integration end-to-end.

---

## Advanced: adding a new auth flow

Skip this section unless the service uses an auth pattern not in the table above (e.g. OAuth2 device code, mTLS, signed JWT bearer).

A new auth flow means extending the **connection subsystem** itself, not just adding an integration.

### Files to touch

1. **Enum variant** in [crates/runtara-connections/src/types.rs](../../../crates/runtara-connections/src/types.rs) — add a `ConnectionAuthType::<NewVariant>`.
2. **Mirror enum** in [crates/runtara-dsl/src/agent_meta.rs](../../../crates/runtara-dsl/src/agent_meta.rs) — keep `ConnectionAuthType` in lockstep. Easy to forget — every variant must exist in both files or the DSL ↔ runtime handshake breaks.
3. **Handler dispatch** in [crates/runtara-connections/src/handler/connections.rs:34](../../../crates/runtara-connections/src/handler/connections.rs:34) — `create_connection_handler` routes based on the auth type.
4. **Service logic** in `crates/runtara-connections/src/service/` — implement credential storage, refresh (if any), and any flow-specific endpoints (callback handlers, polling endpoints, etc.).
5. **Token cache** in [crates/runtara-connections/src/auth/token_cache.rs](../../../crates/runtara-connections/src/auth/token_cache.rs) and [provider_auth.rs](../../../crates/runtara-connections/src/auth/provider_auth.rs) — if the flow involves runtime token exchange.

### Verification

After the auth flow exists, return to the main flow above and add an integration that uses it. Run `e2e-verify` with a real credential exchange to confirm the full loop works.

## Files touched (typical integration)

- `crates/runtara-agents/src/agents/integrations/connection_types.rs` — append params struct + extractor
- `crates/runtara-agents/src/agents/integrations/<name>.rs` — new
- `crates/runtara-agents/src/lib.rs` — add `pub mod` line

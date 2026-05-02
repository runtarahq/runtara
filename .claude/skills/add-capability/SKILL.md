---
name: add-capability
description: Use when adding a new #[capability] function to an existing agent module under crates/runtara-agents. Covers the input/output struct shape, the #[capability] macro attributes, and the inventory-based auto-registration. If no agent file exists yet for the area you are extending, start with add-agent first.
---

# Add a capability to an existing agent

A **capability** is a single `#[capability]`-annotated function inside an agent module. It auto-registers via the `inventory` crate at compile time — no manual registry edits needed.

Reference templates:
- Pure agent capability: [xml.rs `from_xml`](../../../crates/runtara-agents/src/agents/xml.rs)
- Integration capability with connection: [mailgun.rs `send_email`](../../../crates/runtara-agents/src/agents/integrations/mailgun.rs:100)

## Steps

### 1. Define the input struct

```rust
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "<Human-readable input name>")]
pub struct MyCapabilityInput {
    #[field(
        display_name = "Field Label",
        description = "What the user sees in tooltips",
        example = "some example",
        default = "default value"
    )]
    pub field_name: FieldType,

    // For optional fields:
    #[serde(default)]
    #[field(display_name = "Optional Field", description = "...")]
    pub optional_field: Option<String>,
}
```

For **integration capabilities** that need a connection, include:

```rust
#[field(skip)]
#[serde(skip_serializing_if = "Option::is_none")]
pub _connection: Option<RawConnection>,
```

The runtime injects `_connection` automatically when the workflow step references a connection.

### 2. Define the output struct

```rust
#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "<Human-readable output name>")]
pub struct MyCapabilityOutput {
    #[field(display_name = "Result", description = "What this returns")]
    pub result: String,
}
```

### 3. Write the capability function

Signature: `fn(InputType) -> Result<OutputType, AgentError>`.

```rust
#[capability(
    module = "my_agent",                      // matches the file name
    display_name = "Do The Thing",            // shown in the step picker
    description = "Does the thing with X",
    module_display_name = "My Agent",         // grouping label
    module_description = "What this agent does",
    module_has_side_effects = false,          // true if it mutates external state
    module_supports_connections = false,      // true for integrations
    module_secure = false                     // true if it handles secrets
)]
pub fn do_the_thing(input: MyCapabilityInput) -> Result<MyCapabilityOutput, AgentError> {
    // ...
    Ok(MyCapabilityOutput { result: "done".to_string() })
}
```

For integration capabilities, also add: `module_integration_ids = "<integration_id>"` (matches the `integration_id` from the connection params struct).

### 4. Error handling

Use `AgentError::permanent(code, msg)` for terminal errors and `AgentError::transient(...)` for retryable ones. Attach context with `.with_attr(attrs::FIELD, "...")` so the UI can highlight the offending field.

### 5. Verify

After build:

1. Run `regen-frontend-api` so the capability appears in the Step Picker.
2. Run `e2e-verify` — compile + register + execute a workflow that uses the new capability and assert observable behavior. Unit tests alone are insufficient (see `always-e2e-verify`).

## Files touched

- `crates/runtara-agents/src/agents/<existing-agent>.rs` — append input/output structs and the function
- No registry edits required (inventory handles it)

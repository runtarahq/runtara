---
name: add-agent
description: Use when creating a new pure-WASM agent module (no external service) under crates/runtara-agents. An agent is a logical grouping of capabilities (e.g. xml, csv, http, crypto). For adding a capability to an existing agent, use add-capability instead. For agents that talk to an external service, use add-integration.
---

# Add a new agent

An **agent** is a Rust module under [crates/runtara-agents/src/agents/](../../../crates/runtara-agents/src/agents/) that exposes one or more `#[capability]` functions. Agents compile to WASM and run inside the workflow runtime.

## When to use this vs other skills

- **Pure utility / transformation logic** (no external HTTP) → this skill. Examples: `xml`, `csv`, `crypto`, `text`, `transform`.
- **Talks to a third-party service** (Shopify, OpenAI, Slack, ...) → use `add-integration` instead.
- **Adding a function to an existing agent** → use `add-capability` instead.

## Steps

### 1. Create the agent file

Add `crates/runtara-agents/src/agents/<name>.rs`. Reference template: [xml.rs](../../../crates/runtara-agents/src/agents/xml.rs).

Minimum boilerplate:

```rust
// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! <Module> operations for workflow execution

use crate::types::AgentError;
use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
```

### 2. Re-export from `lib.rs`

Add to [crates/runtara-agents/src/lib.rs](../../../crates/runtara-agents/src/lib.rs) following the existing `#[path = "agents/MODULE.rs"] pub mod MODULE;` pattern. Pick a category section that matches the agent's role.

### 3. Add the first capability

Follow `add-capability` for the function shape, input/output structs, and `#[capability(...)]` attributes. Capabilities auto-register at compile time via `inventory` — there is **no manual registry**.

### 4. WASM constraints (critical)

The agent compiles to `wasm32-wasip2`. Avoid:

- `std::thread::sleep` (blocks the runtime)
- `reqwest` directly — use the workflow runtime's HTTP facilities
- Native-only crates (anything pulling `tokio` features that aren't WASI-safe, OS-specific syscalls)

Wrap any native-only code in `#[cfg(not(target_family = "wasm"))]`.

### 5. Tests

Test framework lives at [crates/runtara-agents/tests/custom_module_registration_test.rs](../../../crates/runtara-agents/tests/custom_module_registration_test.rs). Add a unit test inside the new module for pure-logic verification, and an integration test that exercises capability registration if the agent introduces new patterns.

### 6. Verify

After build, regenerate the frontend API client (`regen-frontend-api`) so the new capabilities show up in the Step Picker, then run `e2e-verify` to confirm a workflow can compile and execute against the new agent. Unit tests alone are not sufficient — see the `always-e2e-verify` rule.

## Files touched

- `crates/runtara-agents/src/agents/<name>.rs` — new
- `crates/runtara-agents/src/lib.rs` — add `pub mod` line
- (optional) `crates/runtara-agents/tests/...` — new test

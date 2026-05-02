---
name: regen-frontend-api
description: Use after backend changes that affect the OpenAPI surface — new capabilities, new step types, new connection types, new HTTP routes — so the frontend's generated API client and type definitions stay in sync. The Step Picker, Connection UI, and most forms are driven by these generated types.
---

# Regenerate the frontend API client

The frontend doesn't manually mirror backend types. It pulls OpenAPI from the running server and codegens TypeScript clients. Two clients, two scripts.

## When to run

Run this **whenever** any of these change on the backend:

- A `#[capability]` was added/modified/removed (capabilities show up in the Step Picker via this client)
- A new step type was registered with `inventory::submit!` in [step_registration.rs](../../../crates/runtara-dsl/src/step_registration.rs)
- A new `#[connection(...)]` was added in [connection_types.rs](../../../crates/runtara-agents/src/agents/integrations/connection_types.rs)
- An HTTP handler was added/changed under `crates/runtara-server/src/handler` or `crates/runtara-connections/src/handler`
- A request/response struct accessible to the frontend changed

If the Step Picker doesn't show your new capability or step, this is almost always why.

## Steps

### 1. Start the server

The codegen script hits the live server's `/openapi/docs.json` — the server must be running.

```bash
cargo run -p runtara-server
```

The runtime API is on port `7001`, management on `8080` (per the `package.json` scripts).

### 2. Regenerate the relevant client

From [crates/runtara-server/frontend](../../../crates/runtara-server/frontend):

```bash
cd crates/runtara-server/frontend

# For runtime API changes (capabilities, steps, connections, workflow execution)
npm run generate-api-runtime-local

# For management API changes (tenants, users, configuration)
npm run generate-api-management-local
```

When in doubt, run both. They're cheap and idempotent.

Output goes to `src/generated/RuntaraRuntimeApi.ts` and `RuntaraManagementApi.ts`.

### 3. Commit the regenerated files

The generated files are checked in. After regeneration, `git diff` should show only meaningful changes (your additions). Spurious churn (timestamps, reordered fields) suggests the codegen ran against a different server build — rebuild the server and try again.

### 4. Verify

After regen, the affected UI surface should reflect the change without further frontend edits:

- New capability → appears in [StepPickerModal.tsx](../../../crates/runtara-server/frontend/src/features/workflows/components/WorkflowEditor/NodeForm/StepPickerModal.tsx)
- New step type → appears in the same picker's "control" / "execution" / "utility" section
- New connection type → appears in the Connections create flow

If it doesn't show up, check:

1. Server is running and serving the new schema (`curl http://localhost:7001/api/runtime/openapi/docs.json | jq '.paths' | grep <thing>`)
2. The `inventory::submit!` for the new thing isn't gated behind `#[cfg(not(target_family = "wasm"))]` and accidentally excluded
3. You ran the right script (runtime vs management)

## Files touched

- `crates/runtara-server/frontend/src/generated/RuntaraRuntimeApi.ts`
- `crates/runtara-server/frontend/src/generated/RuntaraManagementApi.ts`

(Both committed; review the diff to make sure only intended changes appear.)

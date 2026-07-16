# Removing agent entitlements — plan

_Verified against `feature/workflow-agent-unification` (2026-07-16). Status: PLAN ONLY._

**Directive (owner decision, 2026-07-16): if an agent is present, it is
available.** The per-tenant agent allowlist (`enabled_agents`) is removed from
the entitlement system. Availability of an agent is decided by exactly one
thing: whether its component is present — a native module in
`RUNTARA_AGENT_COMPONENTS_DIR`, or a published workflow-agent in the tenant's
staging overlay. Distribution is the lever; there is no second, runtime one.

## 1. Scope — the carve, precisely

The entitlement system has **three dimensions** (`EntitlementSnapshot`,
entitlements.rs:65-76). Exactly one is being removed:

| Dimension | Contents | Fate |
|---|---|---|
| `enabled_agents: Option<BTreeSet<String>>` | per-tenant agent allowlist (`None` = all registered) | **REMOVED** |
| `features` | 4 booleans: `reports`, `database`, `api`, `mcp` | stays |
| `limits` | 5 caps: `maxWorkflows`, `maxObjectSchemas`, `maxApiKeys`, `objectModelBulkRequestLimit`, `maxConcurrentExecutions` | stays |

Also staying untouched: the `Tier` enum and baselines (minus their agent
lists — only Starter carried one, `{http, csv}`, and the tier table is
documented as "placeholders pending product input"), `RUNTARA_PRICING_TIER`,
the layered `RUNTARA_ENTITLEMENTS_JSON`/`_OVERRIDES_JSON` resolution, the
`plan.changed` boot event, `quota.exceeded` product events, feature-gate
middleware, limit gates, and the `api_key_auth_guard`. **Nothing outside the
agent allowlist changes behavior.**

“Present” keeps its existing meaning: the id resolves in the component
catalog (`discover_component_agent_ids` over `runtara_agent_*.wasm` filename
stems) or the tenant's workflow-agent staging overlay. An id that resolves to
nothing stays unusable — but as *not found* (catalog semantics, 404 / E-code
validation), never as an entitlement denial (403 / `AGENT_NOT_ENABLED`).

## 2. Why this is coherent with the platform model

- **Tenancy is one process per tenant on its own VM.** What a tenant can run
  is already controlled by what is deployed to their VM. A runtime allowlist
  duplicated that boundary inside the process; "presence = availability"
  makes the deployment boundary the single source of truth.
- **The allowlist was never enabled in the fleet.** The only known deployment
  reference is a *commented* `RUNTARA_ENTITLEMENTS_JSON` in smo-provisioning's
  `tenants/syncmyorders.yml` (agent-extraction-plan.md §2.8). No e2e script,
  compose file, or .conf in this repo sets it. Default tier = implicit-all.
- **It taxed the workflow-agent parity work.** Published workflow-agents had
  to be *exempted* from the allowlist (P7: `published_agent_ids` +
  `exempt_agents` threaded through three graph walkers). That machinery —
  added days ago — is deleted wholesale by this plan.

## 3. Design decisions (resolved here)

1. **Config back-compat: accept-and-warn, never fail boot.**
   `EntitlementLayer` is `#[serde(deny_unknown_fields)]` — *deleting* the
   `agents` field would brick boot for any deployed
   `RUNTARA_ENTITLEMENTS_JSON` still carrying it (SMO configs are read live at
   boot). The field stays in the layer struct, is ignored, and logs one boot
   `WARN`: `"agents" entitlement field is deprecated and ignored — agent
   availability is now presence-based; remove it from
   RUNTARA_ENTITLEMENTS_JSON`. Same for `_OVERRIDES_JSON`. The Starter tier
   baseline simply drops its agent list (internal, no compat concern).
   Remove the deprecated field acceptance in a later major release.

2. **`GET /api/runtime/entitlements` keeps its `agents` field**, now always
   the materialised *registered* set (what `Tier::Default` already returns
   today). Rationale: `EntitlementsDto` is a pinned camelCase contract
   consumed by the SPA (`window.__RUNTARA_CONFIG__.entitlements` +
   `useEntitlements`) and potentially external clients; keeping the field
   with "everything available" semantics is zero-break. `materialised_agents()`
   survives as a thin rename-in-place (`registered agents`), `enabled_agents`
   dies. The doc text changes from "allowlist" to "available agents".

3. **403 → 404 flips are intended.** `get_agent`, `get_capability` (REST and
   MCP) currently return 403 `AGENT_NOT_ENABLED` for a *disallowed or
   unregistered* id. After removal, an unregistered id falls through to the
   existing `AgentNotFound` → **404**. Presence-based semantics make
   "absent = not found" the honest answer. MCP clients switching on
   `data.code` never see `AGENT_NOT_ENABLED` again (it is retired, not
   repurposed — per the "codes are stable" doctrine, the string is reserved
   forever and never reused for a different meaning).

4. **The internal dispatcher gate (the one execution-time check) is deleted,
   registration stays.** `POST /api/internal/agents/{module}/{capability}`
   (native-only agents: sftp/xlsx/compression) currently gates on the
   allowlist and returns a 200 envelope with `code: "AGENT_NOT_ENABLED"`.
   After removal, an unregistered module fails in the dispatcher lookup with
   the natural "module not registered" failure envelope; registered modules
   always run. Callers were documented to discriminate on `code` — the only
   retired code is `AGENT_NOT_ENABLED`, which could only fire for
   configurations that no longer exist.

5. **The FE drops agent gating in the same release.** The SPA ships embedded
   in the server binary, so server and FE change atomically. `agentEnabled` /
   `enabledAgentSet` become constant-true dead logic — remove them and their
   consumers (StepPicker/CapabilityPicker filters, the canvas "Agent
   disabled" badge, the TestAgentInline disable, the `AGENT_NOT_ENABLED`
   toast branch). This also deletes the documented fallback trap in
   `helpers.ts:34-41` ("deleting the sentinel check turns the fallback into
   deny-all").

6. **Marketplace reconciliation (amend agent-extraction-plan.md).** The
   extraction plan (§2.8, §5 Phase 4, §9.2, §9.3) pencils the allowlist in as
   the second enforcement lever for paid agents ("two independent levers —
   what's on disk and what a tenant may run"). This plan resolves that to
   **one lever: what's on disk**. Installing/licensing an agent = shipping its
   component to the tenant's VM (per-tenant overlay / image composition);
   uninstalling = removing it. The shared-commercial-image scenario (§5
   Phase 4's "image ships all agents" concern) must be handled at
   provisioning time by composing per-tenant component sets instead of one
   image with everything — amend those sections rather than keeping dormant
   allowlist machinery "just in case". If a future marketplace tier genuinely
   needs a runtime lever, it re-enters as a new, purpose-built mechanism —
   not as this allowlist kept on life support.

## 4. Removal inventory

Numbered sites from the enforcement audit; **(gate)** = pure deletion, no
success-shape change; **(shape)** = response content/shape changes.

### Core machinery
- `entitlements.rs` — delete `enabled_agents` field, `is_agent_enabled`,
  `require_agent`, `EntitlementError::AgentNotEnabled`; `materialised_agents`
  becomes "registered agents"; `EntitlementLayer.agents` stays parsed but
  ignored + boot WARN (decision 1); `validate_agents` + `parse_agents` die
  with their callers; Starter tier drops its agent list; `summarize()` loses
  `agents_explicit`/`agents_allowlist_size` boot-log fields (operator log
  shape — documented in deployment docs, updated in R3).
- `entitlement_error.rs` — delete `AgentNotEnabled` variant,
  `codes::AGENT_NOT_ENABLED`, its arms in `code()`/`error_summary()`/
  `message()`/`json_body()`/`audit_fields`/`From<EntitlementError>`. The enum,
  both renderers (HTTP 403 + rmcp), and the audit line **stay** for
  feature/limit denials.
- `middleware/entitlement.rs` — delete `agent_decision`,
  `walk_graph_for_agents`, `walk_closure_for_agents` (the whole agent section
  incl. the `exempt_agents` threading); feature/limit/api-key machinery
  untouched; fix the module doc.
- `workflow_agents.rs` — delete `published_agent_ids` (its only purpose was
  the exemption set). Publish/staging/catalog-overlay machinery stays.

### Gates (pure deletions)
- `api/services/workflows.rs:666` update_workflow closure walk **(gate)**
- `api/services/workflows.rs:848` patch_version_graph walk **(gate)** — this
  is what every MCP graph-mutation round-trips through
- `api/handlers/workflows.rs:1136-1200` compile-handler re-walk **(gate)** —
  also removes a per-compile DB definition fetch + child load (perf win)
- `api/handlers/agent_testing.rs:66` test gate **(gate)** — keep the id
  canonicalisation (feeds rate-limit buckets/metrics)
- `api/handlers/internal_agents.rs` dispatcher gate + `internal_denial_response`
  **(shape — decision 4)**
- `mcp/entitlement.rs::require_agent` + call sites in `mcp/tools/agents.rs`
  (`get_agent`, `get_capability`, `test_capability`) and
  `mcp/tools/graph_mutations.rs::add_agent_step` **(gate; get_* flip per
  decision 3)**

### Shape-bearing surfaces
- `api/handlers/operators.rs` — delete `filter_agents_by_allowlist` from
  `list_agents_handler` **(shape: previously hidden agents reappear — under
  the default tier, nothing changes)**; `get_agent`/`get_capability`/
  connection-schema handlers lose `require_agent` **(shape: 403→404 flip)**
- `api/dto/entitlements.rs` + `api/handlers/ui.rs` — `agents` field kept,
  fed by the registered set (decision 2)

### Renderers/consumers to simplify, not delete
- `api/handlers/workflows.rs:2440` `EntitlementDenied` render arm (still
  renders `maxWorkflows` LimitExceeded) — stale comment only
- `mcp/tools/internal_api.rs::translate_api_error_response` — keeps carrying
  `ENTITLEMENT_REQUIRED`/`LIMIT_EXCEEDED`; drop the AGENT_NOT_ENABLED test
- `product_events.rs` — the AgentNotEnabled no-op match arm dies with the
  variant (agent denials never emitted events, so no dashboard impact)

### Frontend (same release, decision 5)
- `shared/entitlements/helpers.ts` — remove `agentEnabled`/`enabledAgentSet`
- `StepPickerModal`, `CapabilityPickerModal` — remove entitlement filters
- `CustomNodes/BasicNode` + `BaseNode` — remove the "Agent disabled" badge
- `TestAgentInline` — remove the entitlement disable
- `shared/hooks/api.ts` — remove `AGENT_NOT_ENABLED` from the stable-code
  union + its toast branch
- Matching test updates; no client regen needed (DTO shape unchanged)

### Docs (R3)
- `docs/entitlements.md` + `docs/deployment/entitlements.md` — remove the
  agent-allowlist sections, the stale-workflow freeze table, the
  `AGENT_NOT_ENABLED` troubleshooting entries (incl. the "grep this WARN
  line" operator guidance — that line no longer fires for agents), the
  `agents_explicit`/`agents_allowlist_size` startup-log reference; document
  the deprecated-but-ignored `agents` config field and the retired code.
- `docs/deployment/auth-modes.md:17` — drop the `AGENT_NOT_ENABLED` example.
- `docs/agent-extraction-plan.md` §2.8/§5-Phase-4/§9.2/§9.3 — rewrite to the
  single-lever model (decision 6).
- smo-provisioning (out of repo): delete the commented `agents` example from
  `tenants/syncmyorders.yml` when convenient; harmless until then thanks to
  decision 1.

## 5. What deliberately does NOT change

- Feature gates (`reports`/`database`/`api`/`mcp`) and all five limits,
  including their denial codes `ENTITLEMENT_REQUIRED` /
  `ENTITLEMENT_LIMIT_EXCEEDED`, product events, and audit lines.
- Registration semantics: unknown agent ids are still rejected — by workflow
  validation (unknown-agent E-codes), catalog 404s, and the dispatcher's
  module lookup. "Present = available" is not "any string runs".
- Workflow-agent publish/staging/overlay and the checkpoint-scope compose
  gate (N3) — orthogonal to entitlements.
- The `plan.changed` boot event and tier persistence in `metadata`.

## 6. Risks

- **Starter-tier tenants gain agents.** Only Starter restricted agents
  (`{http, csv}`); tier baselines are documented placeholders and no fleet
  config enables an explicit allowlist. Accepted by the owner directive.
- **Log-based alerting** keyed on the `AGENT_NOT_ENABLED` WARN audit line
  goes quiet. The deployment doc that recommended that grep is updated in R3;
  no in-repo dashboards exist.
- **Retired wire code.** External clients handling `AGENT_NOT_ENABLED`
  simply never see it again; the code string is reserved and never reused.
- **Marketplace roadmap** loses its penciled second lever — resolved
  explicitly by decision 6, not silently.

## 7. Test plan

Update/delete (all inline `#[cfg(test)]`; no integration tests pin the
allowlist):
- `entitlements.rs` allowlist suite (~15 tests) → replaced by: registered-set
  semantics, deprecated-`agents`-field-boots-and-warns (both JSON layers),
  Starter baseline without agents, summarize without allowlist fields.
- `middleware/entitlement.rs` agent/graph-walk/closure/exemption tests
  (~10) → deleted with the walkers; feature/limit tests untouched.
- `operators.rs` filter tests → deleted; add one: listing returns the full
  registered set.
- `entitlement_error.rs` AgentNotEnabled shape tests → deleted; code-stability
  test updated to the two surviving codes.
- `internal_agents.rs` gate tests → replaced by: registered module runs,
  unregistered module → dispatcher not-found envelope.
- `internal_api.rs` translate test → drop the AGENT_NOT_ENABLED case.
- FE: helpers/StepPicker/api-toast test updates per decision 5.

Live verification: full `e2e/test_workflow_agent_parity.sh` (steps 1-9 —
publish/invoke paths no longer depend on the exemption) + one new live
assertion: boot the e2e server with
`RUNTARA_ENTITLEMENTS_JSON='{"agents":["http"]}'` and assert (a) boot
succeeds with the deprecation WARN in the log, (b) a workflow using a
non-listed agent (e.g. `utils`) saves, compiles, and executes.

## 8. Sequencing (each shippable)

1. **R1 — server removal**: core machinery + all gates + shape decisions +
   config accept-and-warn + test replacements. One commit (it is one
   semantic: the allowlist no longer exists), pre-commit clippy-clean,
   gated suite + live sweep green.
2. **R2 — frontend simplification**: drop agent-gating logic + tests.
3. **R3 — docs**: entitlements docs rewrite + agent-extraction-plan
   amendment + deployment-doc updates.

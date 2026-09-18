# Trusted capabilities in isolated WASM instances

Status: implemented for S3 and Azure presigning. The public capability flag is
`trusted`, defaulting to false. This document records the design and rollout
contract; general OAuth and host-retained credential transformations remain future
work.

## Decision

Add `trusted: bool`, defaulting to `false`, to capability metadata. A built-in
capability marked `trusted` executes in a fresh restricted component instance
and receives credentials for a tenant-owned connection of its own integration
type. Ordinary workflow and agent instances continue to receive opaque IDs.

The workflow selects a capability and connection through existing step inputs.
It cannot set the flag, approve an implementation, provide authoritative
credentials, or override the allowed connection types.

```text
Workflow selects agent / capability / connection ID
  -> host resolves approved built-in capability
  -> host checks trusted flag, permissions, connection ownership and type
  -> host resolves credentials
  -> host creates a fresh restricted Store and component instance
  -> host invokes the trusted export with separate input and credentials
  -> host receives a bounded result and destroys the execution instance
  -> host returns the result to the workflow
```

`trusted` grants this specific execution mode. It does not grant network access,
filesystem access, unrestricted host calls, another tenant's connections, or
access to unrelated connection types. No separate `isolated` flag is introduced.

## Trust and connection-type rule

Use separate host-owned built-in and tenant-component registries. Only the
operator-installed built-in bundle is eligible for trusted execution. Bind the
loaded component, approved metadata, and content identity in one registry entry.
Keep provenance internal; uploaded `origin` or `trusted` fields cannot establish
it. Tenant catalog overlays must never determine privileged execution targets.

The initial trust root is protected installation and controlled registration of
the built-in bundle. Hash loaded WASM and metadata to bind versions and detect
mismatches; a caller-supplied hash is not approval. Compile/cache the same bytes
that were registered. Signed release manifests are a separate distribution
enhancement, not a prerequisite for operator-installed built-ins.

Before resolving or decrypting credential values, require:

1. The target comes from the built-in registry and the requested capability
   exists there with `trusted == true`.
2. The authenticated run/test context permits invoking that agent/capability.
3. The selected connection belongs to the current tenant.
4. Its authoritative `integration_id` is an exact member of the built-in agent's
   registered `AgentInfo.integration_ids`.

Require `supports_connections == true` and a nonempty integration list when
registering a built-in with trusted capabilities. Empty lists are not wildcards.
Apply the same checks when an existing default-connection mechanism selects the
record. The v1 trusted host import takes one explicit resolved connection ID;
JSON input cannot request additional connections or credentials.

| Agent | Allowed connection type | Rejected examples |
| --- | --- | --- |
| `s3-storage` | `s3_compatible` | `azure_blob_storage`, `aws_credentials`, `postgres` |
| `azure-blob-storage` | `azure_blob_storage` | `s3_compatible`, `http_bearer` |

Use the built-in agent's exact integration list. Do not use the broader
`object_storage` default-for bucket, tenant metadata, guest
`_connection.integration_id`, tags, name matching, or structural similarity of
credential fields as authorization. Enforce the type and ownership on the same
record/version from which credentials are resolved; avoid a check/use race.

Reject tenant metadata declaring `trusted: true` during publication/loading and
at invocation. Published workflow-agents always have `trusted: false`, even when
their graph calls a trusted built-in. They can request approved operations but
never receive injected credentials. Canonical-name collisions cannot replace or
shadow entries in the privileged registry.

## Metadata and authoring

Extend these existing sources:

- [Capability macro](../crates/runtara-agent-macro/src/lib.rs): parse `trusted`,
  emit it in static metadata, and generate the appropriate execution wrappers.
- [Agent metadata](../crates/runtara-dsl/src/agent_meta.rs): update
  `CapabilityMeta`, `CapabilityInfo`, conversions, all struct literals, fixtures,
  and serialization. Missing serialized fields deserialize as `false`.
- [Bundle emitter](../crates/runtara-agent-bundle-emit/src/main.rs): propagate
  macro-derived metadata into generated sidecars and validate trusted entries.
- [Workflow-agent publication](../crates/runtara-server/src/workflow_agents.rs):
  preserve tenant provenance and prohibit trusted declarations.

Proposed authoring shape:

```rust
#[capability(
    module = "s3-storage",
    id = "storage-generate-presigned-url",
    trusted = true,
    side_effects = false,
    idempotent = true
)]
fn storage_generate_presigned_url(
    input: GeneratePresignedUrlInput,
    context: &TrustedContext,
) -> Result<GeneratePresignedUrlOutput, AgentError> {
    // Sign using host-supplied provider credentials and signing time.
}
```

`TrustedContext` is a separate host-only invocation argument, excluded from
capability input schemas and ordinary executor signatures. The macro validates
trusted versus ordinary function signatures. The agent parses the authorized
credential payload into its own provider type. Do not expose database records,
the connection facade, or credentials for additional connections to WASM.

Expose `trusted` as read-only capability metadata where the API returns these
types. There is no workflow editor checkbox or step-level override. Regenerate
OpenAPI/runtime TypeScript through the normal generator when the API metadata
contract changes. Do not hand-edit sidecars or generated client files.

## Host ABI and invocation routing

Add two versioned interfaces in the contract crates described by the
[host-interface plan](wasm-host-interfaces-plan.md):

1. A workflow-facing host import, conceptually
   `trusted-capabilities.invoke(agent-id, capability-id, connection-id, input)`.
   It accepts no credentials, tenant override, module bytes, filesystem path, or
   arbitrary export name. The host selects an approved built-in implementation
   from the run's validated registry snapshot.
2. A component export for host invocation, conceptually
   `trusted-execution.invoke(capability-id, input, credential-context)`.
   It dispatches only trusted capabilities. Context contains authoritative
   connection type, provider credential payload, and explicit execution data
   such as signing time.

Reuse structured agent errors and preserve workflow-facing input/output schemas.
New operations are async-typed so credential resolution and concurrent trusted
calls do not serialize the whole workflow store.

Keep ordinary `capabilities.invoke` compatible. Its generated dispatcher entry
for a trusted capability forwards the ID and ordinary input to the host import.
It cannot invoke the privileged body or construct `TrustedContext` from guest
JSON. The trusted export calls the body directly, without forwarding back to
the host. Generate/validate dispatch tables so the two paths cannot drift.

This forwarder lets existing invocation lowering, Split branches, AI tool calls,
and composed workflow-agents use the same route. Compiler/composition changes
propagate the new import, validate metadata, and record the required built-in
version. They do not pass credentials through the workflow's composed instance.
A direct compiler-to-host lowering is an optional later optimization.

Standalone agent testing dispatches trusted capabilities through the same host
executor using authenticated tenant context. Supplied `_connection.parameters`,
integration IDs, or flags are never authoritative. Frontend validation is not
the authorization boundary.

Use a host service trait: the server owns connection/entitlement checks and
credential resolution; `runtara-component-host` owns restricted execution. Guest
code and the component host must not depend on server/database implementations.

## Restricted execution and lifecycle

Create a dedicated restricted linker and fresh Store/instance for every trusted
call. Immutable compiled code may be cached; live trusted instances, memories,
WASI resources, and secret-bearing execution contexts must never be pooled.

The full provider component can import HTTP for ordinary capabilities. Simply
omitting these imports may prevent linking. Supply denying implementations for
required imports in restricted mode, or generate a restricted world where
necessary. The acceptance condition is that forbidden capabilities are
unreachable, including during component initialization.

Restricted execution has:

- No outbound HTTP, sockets, DNS, filesystem preopens, inherited environment,
  inherited stdin/stdout/stderr, or sink for guest-provided logs.
- No runtime/checkpoint/event calls, Object Model, connection resolution,
  presigning host call, nested agent invocation, or trusted recursion.
- Only necessary pure/basic WASI support. Pass signing time explicitly rather
  than inheriting ambient configuration.
- Dedicated memory/table/output bounds and execution interruption, capped by the
  caller's remaining deadline. Bound concurrency before acquiring credentials.

Resolve credentials only after authorization, immediately before execution.
Keep them out of workflow input, events, checkpoints, and step debugging. Any
OAuth refresh or other I/O needed to resolve credentials happens on the host
before entering the restricted instance; the guest cannot initiate it.

On success, error, trap, timeout, cancellation, or output decoding failure, stop
execution, release resources, drop Store/instance state, and discard temporary
credential buffers. A timeout must not leave a detached task running. Use
zeroizing native buffers where practical without claiming Store destruction
securely erases every guest-memory copy.

Bound and decode the result, tear down the instance, then return it. Never cache
credential context. Existing workflow result caching/replay retains its current
semantics; replaying a signed URL does not extend its expiry.

The approved implementation controls what it deliberately returns. Signed URLs
are intentionally workflow-visible and sensitive. Isolation does not prove that
arbitrary output bytes are secret-free. Generic credential dumping is not an
approved capability. Transformations whose outputs must remain secret require a
host-retained result path before exposure; this plan introduces no additional
public result-policy flag.

Audit host-owned tenant, agent, capability, connection ID, artifact identity,
duration, and outcome. Do not audit credential arguments, guest memory, raw guest
diagnostics, or signed URL values. Normal workflow visibility of approved output
fields remains a separate existing behavior.

## Initial migration: S3 and Azure presigning

Mark only the two presigning capabilities `trusted: true` initially. Ordinary
storage operations stay unchanged and receive no credentials.

Move or factor AWS SigV4 URL and Azure SAS signing into WASM-compatible provider
code owned by the respective agents. Reuse pure algorithm code from native
compatibility paths where necessary; avoid two diverging signer implementations
and avoid importing host-only `runtara-connections` into WASM.

The host validates connection access and destination/path policy. The trusted
provider implementation performs provider-specific validation and signing.
Preserve operation mappings, expiry caps, URL encoding, session-token handling,
and actual content-type semantics. New capabilities stop POSTing to the internal
presign route; old artifacts may retain it during migration.

The presigning host facade in the broader plan can select an approved trusted
S3/Azure capability from the authoritative connection type and delegate to this
same executor. Workflow steps can select these capabilities directly. Neither
route can select an unapproved implementation.

## Implementation sequence

1. **Metadata and provenance.** Add the flag to macros/DTOs/generated metadata;
   separate privileged built-in registration from tenant overlays; validate
   type declarations and artifact/metadata identity. Gate: old metadata defaults
   false and uploaded declarations/name collisions cannot grant privilege.
2. **Contracts and authorization.** Add host import, trusted export, context,
   service trait, and policy checks. Gate: denied requests never resolve secrets;
   exact type checks use the same authoritative record as credential resolution.
3. **Restricted executor.** Implement fresh-instance execution, denying linker,
   resource bounds, cancellation, and cleanup. Gate: real WASM probes cannot use
   forbidden imports or retain state across calls, including on failed startup.
4. **Invocation wiring.** Generate forwarders/privileged dispatchers; wire
   composition and standalone testing. Gate: direct steps, Split, AI tools, and
   workflow-agent callers share the approved path without receiving credentials.
5. **Provider migration.** Move S3/Azure presigning into trusted execution with
   unchanged public capability schemas. Gate: compatible providers/emulators
   accept outputs; no internal presign listener is needed for new artifacts.
6. **Compatibility and rollout.** Regenerate bundles, clients, and fixtures;
   deploy supporting hosts before new components. Retire legacy routes only
   after supported artifacts no longer use them. Gate: resume/replay and mixed
   versions have explicit tested behavior.

Pin trusted built-in dependencies in generated dependency metadata bound to the
registered workflow artifact. Validate every selected digest against the host's
approved registry; workflow-supplied manifests cannot approve code. Preserve the
approved versions needed by supported suspended executions, or fail explicitly
when unavailable/revoked. Never silently substitute another implementation on
resume. Rollback hosts must understand interfaces already published.

## Verification

Required tests, in addition to existing runtime/agent suites:

- Default false, explicit true, macro signature validation, sidecar/API
  round-trip, and generated workflow-agent metadata forced false.
- Unknown/non-trusted target, tenant metadata spoofing, name collision, replaced
  artifact, missing connection, cross-tenant ID, forged input type/parameters,
  mismatched actual integration, and empty/wildcard declarations. Assert no
  credential resolution occurs on rejected requests.
- S3 trusted + S3 connection succeeds; S3 + Azure/generic AWS fails; Azure +
  Azure succeeds. Ordinary storage capabilities never receive secrets.
- Forbidden imports from initialization and invocation are denied. Sequential
  and concurrent calls cannot read another instance's state.
- Resolution failure, malformed input, trap, deadline, cancellation, oversized
  output, and memory exhaustion all release resources. Cancellation terminates
  isolated execution rather than merely abandoning its result.
- Known fixture secrets never enter workflow input plumbing, checkpoints,
  errors, or diagnostics. Malicious-output fixtures document why only approved
  code may receive credentials; no generic output scanner can establish trust.
- S3/Azure signing parity with controlled time and provider/emulator validation;
  path/operation/expiry restrictions and existing content-type behavior.
- Old/new bundles, missing trusted ABI/version, standalone tests, composed
  workflows, AI tools, and published workflow-agent callers.

Run focused tests for `runtara-agent-macro`, `runtara-dsl`, contract crates,
`runtara-component-host`, `runtara-workflows`, and both storage agents. Build all
components with `scripts/build-agent-components.sh`, then run the feature-gated
component-host/direct-WASM integration suites from CI. Server/connection tests
require isolated PostgreSQL/Valkey where applicable. Run relevant frontend
checks after regenerating its API client. Use the broader plan's verification
commands and current CI matrix; report unavailable checks explicitly.

Complete means only built-in capabilities marked `trusted` receive their own
compatible connection credentials, exclusively in fresh restricted instances,
with unchanged workflow results and tested denial/cleanup paths.

## Implementation notes

The host executor is `crates/runtara-component-host/src/trusted.rs`; the shared
contract and pure signing code live in `crates/runtara-agent-trusted`. The two
storage agents forward ordinary invocations to the host and implement a separate
privileged export. The server credential adapter checks entitlement and fetches
one tenant-owned, exact-type connection before decrypting it.

Each invocation uses a fresh restricted Store, a 64 MiB memory limit, 1 MiB
input/context/output bounds, a maximum 30 second deadline, and one of 16 executor
permits. A separate Tokio task avoids nested Wasmtime component event loops;
the owning JoinSet aborts execution on cancellation and joins it on timeout.
Audit events contain routing identity and outcome, including dropped callers.

Workflow composition adds content-bound artifact imports for trusted dependencies.
The loader explicitly validates these imports against the approved registry
because empty component interface imports alone do not force linker resolution.
Changing either WASM or metadata requires recompilation of dependent workflows;
this implementation does not retain a registry of historical bundle versions.
An unavailable version fails before guest startup or credential resolution.
Scoped child packages preserve the same pins on the root before package assembly.
For a bare built-in child, the host binds the precompile worker's verified WASM
digest to the registry entry and the root's metadata-bound pin; the child cannot
supply either authority. Scoped child calls inherit the runner's tenant separately
from the guest environment. Both ordinary and privileged exports use the current
async component dispatch convention.

S3 and Azure use the same pure signing implementations from native compatibility
routes and restricted WASM. The old internal presign HTTP route remains available
for existing artifacts. OAuth acquisition, refresh, token storage, and controlled
outbound HTTP remain native; they are not migrated by this change.

Verification includes real component execution, direct workflows, parallel Split,
AI tools, published workflow-agent wrappers, and durable replay. Replay preserves
the signed URL and its original expiry without resolving credentials again.
Credential filtering was checked with PostgreSQL and a counting cipher; provider
signatures were checked against the published AWS vector, MinIO, and Azurite.
The full direct-WASM regression suite, component-host suite, focused metadata and
macro tests, component builds, API generation, and frontend build were also run.
The repository-wide CI feature matrix and deployment were not run locally.

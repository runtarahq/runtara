# Trusted capabilities: remove provider knowledge from the host

The trusted execution mechanism is generic, but the server's credential
provider is not. Each new trusted capability currently needs a host-side branch.
This blocks moving channel verification (Slack, Teams, Telegram, Mailgun) out of
native server code. This refactor is separate from
[channel intake durability](instance-events-follow-ups.md) and does not block it.

## Current state

Generic, and staying as is:

- The executor (`crates/runtara-component-host/src/trusted.rs`) runs built-in,
  digest-pinned components in fresh instances with no network or filesystem
  access, a 30s limit and 1 MB input/output caps.
- The WIT interface (`crates/runtara-agent-trusted/wit/trusted.wit`) passes
  opaque bytes: `invoke(capability-id, input, context)`.
- Capability metadata carries only `trusted: bool`.

Provider-specific, in `crates/runtara-server/src/api/services/trusted.rs`:

1. **`resolve()` translates credentials per provider.** It runs the
   outbound-HTTP auth resolver, then branches on `aws_signing` / `azure_signing`
   to build a bespoke JSON shape. Any other connection type fails with
   `TRUSTED_CONNECTION_TYPE`.
2. **`validate_input()` switches on the capability name.** It string-matches
   `storage-generate-presigned-url` and applies bucket/key/endpoint policy. The
   guest already enforces the same rule through
   `runtara_agent_trusted::object_url`.

Channel handling has the same problem at a larger scale: `channels/` holds about
4.3k lines of per-provider parsing, signature verification and reply code, plus
`api/services/webhook_verification.rs`.

## Target

The host authorizes and supplies inputs. The capability owns provider
semantics.

- **Context carries raw connection parameters,** typed by the connection
  schema, together with `integration_id` and `now_ms`. The capability interprets
  them. The host resolves only what needs host I/O, generically:
  - OAuth access tokens (refresh stays host-side);
  - public key material declared by the connection type, such as a JWKS or OIDC
    discovery URL, fetched and cached by the host.
- **Input policy belongs to the capability.** The guest validates its own input.
  A host pre-check, if kept, is declared in capability metadata (for example
  "path must stay under `base_url`") rather than matched by capability name.
- **Adding a trusted capability needs no server change.** The acceptance test
  for the refactor: S3 presign, Azure SAS and one channel verifier all run
  through the same host code with no per-provider or per-capability branches.

## Inbound channel verification

This is the first new use of the target model.

A trusted capability, for example `channel-verify-inbound`, exported by each
channel agent:

- **Input:** method, selected headers, body (or the signed subset of it).
- **Context:** the connection's parameters (signing secret, bot token, app id)
  and the host-fetched key set where the provider uses JWTs.
- **Output:** one of
  - `message { identity, conversation, sender, content }`
  - `respond { status, headers, body }` for handshakes such as Slack
    `url_verification`
  - `reject { reason }`

The host webhook endpoint becomes a single generic route. It calls the
capability, persists intake keyed on `(connection_id, identity)`, and acks.
Durable deduplication stays in the host because trusted instances are
stateless; the capability only supplies `identity`.

Outbound replies (`channels/channel.rs`, the Teams `serviceUrl` handling) are
also provider-specific native code. They should move to the existing channel
agents' ordinary (non-trusted) capabilities in a later slice.

## Constraints and open questions

- **Teams JWT validation** needs Microsoft's JWKS, and trusted instances cannot
  fetch it. The generic host key-set fetch above covers this. The caching and
  refetch policy in `channels/teams_auth.rs` moves with it.
- **Input size.** Mailgun multipart posts with attachments can exceed 1 MB.
  Pass headers plus signed fields only, and let the host stream attachments to
  storage, or raise the cap per capability through metadata.
- **Telegram** has no agent crate yet. It needs one before its handler can
  move.
- **Credential exposure scope.** Passing raw parameters widens what a trusted
  guest sees compared with today's pre-digested fields. Decide whether the
  connection schema marks which fields a trusted capability may receive.
- **Host pre-validation.** Trusted guests are operator-installed, digest-pinned
  built-ins. Decide whether host-side input checks are still required as
  defence in depth or can be dropped in favour of guest enforcement.

## Order

1. Generic context: raw schema-typed parameters, generic OAuth and key-set
   resolution. Migrate S3 and Azure presign to it and remove both branches from
   `resolve()`.
2. Replace `validate_input()`'s name match with guest enforcement or a
   metadata-declared policy.
3. Add `channel-verify-inbound` to the Slack, Mailgun and Teams agents. Create a
   Telegram agent. Switch to the generic webhook route and delete the
   per-provider handlers and `webhook_verification.rs` branches.
4. Move outbound replies into the channel agents.

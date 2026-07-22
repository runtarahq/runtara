# Microsoft Teams Messaging Planning Brief

## Objective

Bring Microsoft Teams messaging closer to the existing Slack experience while preserving Runtara's agent and generic credential-proxy architecture.

The target is to support production-safe Teams channel triggers and first-class workflow messaging actions without introducing a Teams-specific server gateway.

## Current State

Runtara already has:

- A `teams_bot` connection type with app ID, client secret, and optional tenant ID.
- A Teams webhook that accepts Bot Framework activities, extracts text, and starts or resumes channel sessions.
- A `TeamsChannel` adapter that sends text to the active Bot Connector conversation.
- Trigger-form support for selecting Teams connections.

Unlike Slack, Teams does not yet have a standalone WASM agent. Slack currently exposes `send-message`, `add-reaction`, and `upload-file` workflow capabilities.

The existing Teams implementation should be treated as a prototype: authentication validation, conversation-reference handling, event coverage, tests, and operational setup are incomplete.

## Architectural Constraint

Do not add a Teams-specific gateway or privileged Teams messaging endpoint to `runtara-server`.

Teams-specific behavior should remain in:

- The Teams webhook adapter for inbound activity normalization.
- A new `runtara-agent-teams` component for Bot Connector operations and provider-specific response handling.
- Connection authentication metadata for acquiring Bot Connector tokens.

Credential injection, destination enforcement, HTTP execution, rate limiting, and tenant isolation must remain generic proxy responsibilities.

## Proposed Architecture

### Generic connection-bound endpoints

Extend the generic credential proxy to support dynamic endpoints that are bound to a tenant and connection:

```text
tenant_id + connection_id + endpoint_ref
    -> exact validated base URL
    -> optional safe metadata and expiry
```

This is needed because a Teams connection can have multiple conversation-specific or regional `serviceUrl` values, while the current proxy assumes one static base URL per connection.

The endpoint reference must be opaque to workflows. A credentialed request may use it only when it belongs to the current tenant and selected connection. The proxy should then apply its normal HTTPS, path-containment, DNS, redirect, and SSRF controls.

This facility must be integration-neutral and reusable by other providers with validated dynamic endpoints.

### Conversation references

After full Bot Connector authentication succeeds, the Teams webhook should persist or refresh a conversation target containing at least:

- Tenant and connection IDs.
- Opaque endpoint reference.
- Conversation ID and type.
- Bot Framework service URL.
- Microsoft tenant ID.
- User, team, and channel identifiers when present.
- Current activity ID and reply target.
- Last-seen timestamp.

Workflow trigger data should expose an opaque reply target plus non-sensitive identifiers required for mappings. It should not expose credentials.

### Authentication

Add `teams_bot` support to the existing connection-auth resolver using deferred OAuth2 client credentials:

- Scope: `https://api.botframework.com/.default`.
- Single-tenant authority: the configured Microsoft tenant.
- Legacy multi-tenant authority only when the connection explicitly declares that app type.
- Cache and refresh tokens through the existing generic token-cache path.

The current token acquisition inside `TeamsChannel` should be removed or refactored to use the same generic credentialed-egress path as the Teams agent.

### Teams agent

Add `crates/agents/runtara-agent-teams` and register it in the workspace and agent bundle.

Initial capability:

`send-message`

- Inputs: target reference, text, optional Adaptive Card, and optional reply activity ID.
- Operation: Bot Connector `send to conversation` or `reply to activity` through the generic proxy.
- Output: success, conversation ID, and returned activity ID.
- Errors: structured authentication, permission, target-not-found, throttling, validation, and upstream-service errors.

Follow-up capabilities should be planned separately:

- Update a bot message.
- Delete a bot message.
- Add or remove a supported Teams reaction.
- Send or share a file where the Teams scope permits it.

## Required Hardening

Before enabling Teams in production:

- Require the Bot Connector's advertised signing algorithm rather than trusting the JWT header.
- Validate issuer, audience, lifetime, signature, `serviceUrl` claim, and channel endorsement.
- Validate the activity's Microsoft tenant against the connection when applicable.
- Store a service URL only after all authentication and destination checks pass.
- Scope all cached and persisted targets by tenant and connection.
- Disable or safely constrain redirects on credentialed requests.
- Propagate non-success Bot Connector responses instead of logging and returning success.
- Add idempotency or deduplication using the incoming activity ID.
- Verify the public webhook route/rewrite used by deployed installations.

## Inbound Parity Work

The webhook currently handles only non-empty text messages. Planning should cover:

- Message and mention normalization across personal, group-chat, and team scopes.
- Thread/reply activity identifiers.
- `messageReaction` activities.
- Message update and delete activities if required by product behavior.
- `conversationUpdate` and installation events needed for proactive targets.
- File-only activities and inline images.
- Adaptive Card invoke submissions.
- Loop prevention for bot-originated events.
- Optional RSC-based receipt of channel or chat messages that do not mention the bot.

## Known Product Differences from Slack

Teams parity is behavioral, not a one-to-one API port:

- Adaptive Cards replace Slack Block Kit.
- Teams reactions use a narrower supported vocabulary than Slack custom emoji.
- Bot file send/receive behavior depends on conversation scope; channel files generally involve SharePoint.
- Proactive messaging requires the app to be installed in the target user, chat, or team context.
- A bot cannot create every kind of Teams chat or channel proactively.
- Reading historical chats or channel messages is a Microsoft Graph/RSC concern and should not be coupled to the Bot Connector send-message MVP.

## Workstreams

1. **Security and identity**
   Harden inbound JWT validation and implement tenant-aware Bot Connector token acquisition.

2. **Generic endpoint binding**
   Design storage, lifecycle, proxy request contract, destination pinning, and tenant isolation for connection-bound dynamic endpoints.

3. **Conversation targets**
   Persist validated Teams conversation references and define the opaque workflow target contract.

4. **Teams agent**
   Implement and bundle `send-message`, Adaptive Card support, structured errors, and activity-ID outputs.

5. **Channel-session integration**
   Move existing Teams replies onto the generic credentialed-egress path and preserve precise reply targets.

6. **Inbound features**
   Add reactions, attachments, installation events, and other agreed activity types.

7. **Operations and UX**
   Provide a Teams app manifest, connection validation, setup documentation, UI branding, and webhook diagnostics.

8. **Verification**
   Add unit, proxy-policy, mocked Bot Connector, restart/recovery, and real Teams development-tenant tests.

## Planning Decisions Still Needed

- Is the first release reactive-only, or must scheduled/proactive sends be included?
- Should one opaque target identify a conversation, a reply chain, or a specific message?
- What retention and expiry policy should apply to stored conversation targets?
- Should generic endpoint bindings be persisted in PostgreSQL, cached in Valkey, or use both?
- Must endpoint references survive connection secret rotation, app reinstall, and runtime restart?
- Which Teams scopes are required initially: personal, group chat, team channel, or all three?
- Is Adaptive Card support required in the first `send-message` capability?
- Are incoming and outgoing files part of parity, or a later SharePoint-backed feature?
- Is all-message ingestion through RSC required, or are direct messages and mentions sufficient?
- Do historical chat/message reads belong in this integration or a separate Microsoft Graph agent?

## Suggested MVP Boundary

The smallest production-worthy release is:

- Single-tenant Teams bot connection.
- Fully validated inbound text activities in personal, group-chat, and team scopes where the bot is addressed.
- Durable, tenant-scoped conversation targets.
- Generic connection-bound endpoint enforcement in the credential proxy.
- A Teams `send-message` capability supporting text, an optional reply target, and an activity-ID result.
- Existing channel-session text replies routed through the same authentication and proxy path.
- App manifest/setup documentation and automated authentication, target-isolation, restart, and send-message tests.

Reactions, files, historical reads, all-message RSC ingestion, and advanced Adaptive Card interactions can follow after the core target and proxy contracts are proven.

## Exit Criteria for Detailed Design

Detailed implementation planning can begin once the team agrees on:

- The MVP scope and supported Teams conversation contexts.
- The generic endpoint-binding contract and persistence model.
- The opaque conversation-target schema exposed to workflows.
- The single-tenant connection/authentication contract.
- Whether Adaptive Cards are included in the initial capability.
- The boundary between Bot Connector messaging and future Microsoft Graph functionality.

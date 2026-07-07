# AWS SQS Agent — Implementation Plan

_Status: draft plan (2026-07-07). Every code-path claim below was checked against the real runtara source (file:line references are live). Design decisions are called out explicitly; open questions are collected in §11._

---

## 1. Summary & feasibility

An SQS agent is **a near-straight clone of the S3 agent** ([crates/agents/runtara-agent-s3-storage](../crates/agents/runtara-agent-s3-storage)) plus one small, reusable server-side change. It is very cheap because the hard parts already exist and are service-agnostic:

- **The WASM component never signs and never sees credentials.** It makes an HTTP call through `runtara-http` with an `X-Runtara-Connection-Id` header; the server-side proxy resolves the connection, injects credentials, and performs SigV4. See [internal_proxy.rs:430-448](../crates/runtara-server/src/api/handlers/internal_proxy.rs#L430) calling `sign_request_v4(…, service, …)`.
- **SigV4 is already generic over service name.** [aws_signing.rs](../crates/runtara-connections/src/auth/aws_signing.rs) signs whatever `service` string it is handed — `"bedrock-runtime"`, `"s3"`, and `"sqs"` are identical to it. **No new signing code.**
- **The outbound HTTP host import already exists** (`wasi:http/outgoing-handler`, wrapped by `runtara-http`). SQS bodies are small; no streaming concerns. **No WIT changes.**

What is **net-new**:

1. A new WASM agent crate `runtara-agent-sqs` (the bulk of the work, but it's boilerplate-shaped).
2. A **generic AWS-service mechanism**: the agent declares the AWS service via an `X-Runtara-Aws-Service` header, and the proxy signs + routes for that service. This replaces the current model where `service` is read from the *connection* (§4). This is the one design change and it is what makes "hundreds of AWS services" tractable — each future AWS agent is a new crate + a header, with **zero** new connection types or signing code.
3. One new optional `endpoint` field on the existing `aws_credentials` connection (for LocalStack / VPC endpoints / testing).

No schema migrations (connection params are JSONB), no new auth flow, no `static_registry.rs` change (SigV4 connections don't use an `HttpConnectionExtractor` — see [connection_types.rs:372-374](../crates/runtara-agents/src/agents/extractors/connection_types.rs#L372)).

---

## 2. Architecture — how a call flows

```
workflow step (SQS agent, WASM)
  └─ runtara_http::HttpClient
       POST "/"                                  ← relative path; JSON protocol
       header X-Runtara-Connection-Id: <conn>    ← lifted to envelope field, stripped from upstream
       header X-Runtara-Aws-Service: sqs         ← NEW: lifted to envelope field, stripped from upstream
       header X-Amz-Target: AmazonSQS.SendMessage
       header Content-Type: application/x-amz-json-1.0
       body {"QueueUrl":"…","MessageBody":"…"}
          │
          ▼  RUNTARA_HTTP_PROXY_URL  (POST /api/internal/proxy)
server proxy (internal_proxy.rs)
  1. resolve_connection_auth(conn) → AwsSigningParams{key,secret,region,service,token}, base_url
  2. NEW: if request.aws_service present → override service, derive base_url = https://{service}.{region}.amazonaws.com (unless connection has an explicit endpoint)
  3. pin_url_to_base("/", base_url)           → https://sqs.us-east-1.amazonaws.com/
  4. reject_private_url()                       (SSRF guard — see §9)
  5. sign_request_v4(service="sqs", region, …) → adds Authorization, X-Amz-Date, X-Amz-Content-Sha256, (X-Amz-Security-Token)
  6. forward to AWS, return {status, headers, body}
```

Key precedent: `runtara-http` **already** lifts special headers into named envelope fields and strips every `x-runtara-*` header before forwarding — see [runtara-http/src/lib.rs:189-205](../crates/runtara-http/src/lib.rs#L189) for `X-Runtara-Connection-Id` and `X-Runtara-Ai-Provider`. `X-Runtara-Aws-Service` follows the identical pattern, so the service declaration can never leak into the signed AWS request.

---

## 3. Protocol decision: AWS JSON, not the legacy Query/XML

Use the **AWS JSON protocol (JSON 1.0)** that modern SQS supports:

- `POST /` with header `X-Amz-Target: AmazonSQS.<Operation>`
- `Content-Type: application/x-amz-json-1.0`
- JSON request body, **JSON response body**

Rationale: no XML parser is needed inside WASM (the legacy Query protocol returns XML). The signer already folds `x-amz-*` and `content-type` headers into the canonical request, so JSON-protocol requests sign correctly with no changes. Every operation targets the same URL (`/`) with a different `X-Amz-Target`, which keeps the agent's HTTP helper trivial and uniform.

**Uniform routing:** all operations — including `CreateQueue`/`ListQueues` that have no queue URL — POST to `/` on the regional endpoint `https://sqs.{region}.amazonaws.com`. The `QueueUrl` (for message ops) travels **in the JSON body**, per the API. This means a connection is scoped to one region and its queues, which matches how AWS SDK clients already behave. One code path for all 18 operations.

---

## 4. The one server-side change: agent-declared service

### 4.1 Why

Today [provider_auth.rs:268-308](../crates/runtara-connections/src/auth/provider_auth.rs#L268) reads the service from the **connection** (`params["service"]`, defaulting to `"bedrock"`). That forces either a per-service connection type (as `s3_compatible` does) or a hand-typed `service` param per connection — neither scales across AWS's service surface. Credentials (key id, secret, session token, region, optional endpoint) are **generic**; the service is a property of the **call**.

### 4.2 The change (backward compatible)

Add an `aws_service` field to the proxy envelope, populated from `X-Runtara-Aws-Service`, and let it override the resolved signing service + derive the default endpoint host.

**(a) `runtara-http` — lift & strip the header** (mirror the `ai_provider` block at [lib.rs:195-199](../crates/runtara-http/src/lib.rs#L195)):

```rust
let aws_service = self.headers.iter()
    .find(|(k, _)| k.eq_ignore_ascii_case("x-runtara-aws-service"))
    .map(|(_, v)| v.clone());
// … add to proxy_body: "aws_service": aws_service,
```

The existing `x-runtara-*` strip at [lib.rs:201-205](../crates/runtara-http/src/lib.rs#L201) already removes it from forwarded headers — nothing else to do there.

**(b) `ProxyRequest` — new field** (after `ai_provider`, [internal_proxy.rs:195](../crates/runtara-server/src/api/handlers/internal_proxy.rs#L195)):

```rust
/// AWS service the caller is signing for (e.g. "sqs", "dynamodb"). When set,
/// it overrides the connection's service and selects the regional endpoint.
#[serde(skip_serializing_if = "Option::is_none")]
pub aws_service: Option<String>,
```

**(c) `execute_proxy_request` — override right after `resolve_connection_auth`** (before the base-URL pin at [internal_proxy.rs:316](../crates/runtara-server/src/api/handlers/internal_proxy.rs#L316)):

```rust
if let (Some(svc), Some(aws)) = (request.aws_service.as_deref(), resolved.aws_signing.as_mut()) {
    aws.service = svc.to_string();
    // Only synthesize the default host when the connection has no explicit endpoint.
    if resolved.base_url.is_none() {
        resolved.base_url = Some(aws_default_endpoint(svc, &aws.region)); // https://{svc}.{region}.amazonaws.com
    }
}
```

Add a small shared helper `aws_default_endpoint(service, region)` in `provider_auth.rs` (next to `normalize_endpoint`) so host derivation lives in one place. The uniform `{service}.{region}.amazonaws.com` pattern is exact for SQS, SNS, DynamoDB, Lambda, Kinesis, STS-regional, etc. Irregular hosts (bedrock-runtime, S3 virtual-host, global services like IAM, FIPS/dualstack) are handled by the connection's `endpoint` override or, later, a per-service host map — **out of scope for SQS**, whose host is regular.

**Backward compatibility:** existing agents send no `X-Runtara-Aws-Service` header, so `aws_service` is `None`, the override is skipped, and Bedrock (service from connection → `bedrock`, host `bedrock-runtime.{region}`) and S3 (`s3_compatible` → `s3`) behave exactly as today.

### 4.3 Connection type: reuse `aws_credentials`, add `endpoint`

No new connection type. Extend the existing struct at [connection_types.rs:331](../crates/runtara-agents/src/agents/extractors/connection_types.rs#L331):

```rust
/// Optional custom endpoint (LocalStack, VPC endpoint, GovCloud, testing).
/// Empty → default AWS regional endpoint for the calling agent's service.
#[serde(default)]
#[field(display_name = "Endpoint", description = "Custom endpoint URL; leave blank for AWS defaults", placeholder = "https://localhost:4566")]
pub endpoint: Option<String>,
```

`provider_auth.rs` already reads `params["endpoint"]` for the non-s3 branch ([provider_auth.rs:286](../crates/runtara-connections/src/auth/provider_auth.rs#L286)) and already aliases `region`/`aws_region`, `access_key_id`/`aws_access_key_id`, etc. ([provider_auth.rs:269-276](../crates/runtara-connections/src/auth/provider_auth.rs#L269)) — so an SQS connection needs only key + secret + region (+ optional session token + optional endpoint). Consider broadening the connection's `category` from `"llm"` to a generic value (cosmetic UI grouping only).

---

## 5. The agent crate `runtara-agent-sqs`

Clone the S3 crate layout verbatim; only request bodies differ.

- `Cargo.toml` — copy S3's: `crate-type = ["cdylib", "rlib"]`; `wit-bindgen-rt`, `serde`, `serde_json`, `base64`, `runtara-agent-macro`, `runtara-dsl (default-features = false)`; conditional `runtara-http` (`native` for host, `wasi` for wasm).
- `build.rs` — copy verbatim (auto-generates `wit/agent.wit` from the crate name).
- `src/lib.rs` — capability input/output structs (`#[derive(CapabilityInput/CapabilityOutput)]` + `#[field(...)]`), `#[capability]` fns, the `#[cfg(target_arch = "wasm32")] impl Guest` dispatcher, and the host-only `agent_info()` collector. Model each on S3's `storage_upload_file` / `s3_request`.

A single shared request helper does all the work (JSON protocol):

```rust
fn sqs_request(target: &str, connection_id: &str, body: &serde_json::Value)
    -> Result<serde_json::Value, AgentError>
{
    let client = runtara_http::HttpClient::with_timeout(SQS_TIMEOUT);
    let resp = client.request("POST", "/")
        .header("X-Runtara-Connection-Id", connection_id)
        .header("X-Runtara-Aws-Service", "sqs")
        .header("X-Amz-Target", target)                     // "AmazonSQS.SendMessage"
        .header("Content-Type", "application/x-amz-json-1.0")
        .body_bytes(serde_json::to_vec(body).unwrap().as_slice())
        .call_agent()
        .map_err(|e| AgentError::transient("SQS_NETWORK_ERROR", format!("{target} failed: {e}")))?;

    let parsed: serde_json::Value = serde_json::from_slice(&resp.body).unwrap_or(json!({}));
    if (200..300).contains(&resp.status) { Ok(parsed) }
    else { Err(map_sqs_error(resp.status, &parsed)) }   // {__type, message} → AgentError
}
```

`module_secure = true`, `module_supports_connections = true`, `module_integration_ids = "aws_credentials"` on the module (as S3 sets `s3_compatible`).

---

## 6. Capability catalog (full set)

Kebab ids, `module = "sqs"`. All target the same `/` with a per-op `X-Amz-Target`.

### Messages (read / write — the core ask)
| Capability id | X-Amz-Target | Key inputs | Key outputs |
|---|---|---|---|
| `queue-send-message` | `AmazonSQS.SendMessage` | queueUrl, messageBody, delaySeconds?, messageAttributes?, messageGroupId?, messageDeduplicationId? | messageId, sequenceNumber?, md5OfMessageBody |
| `queue-send-message-batch` | `AmazonSQS.SendMessageBatch` | queueUrl, entries[] (id, messageBody, …) | successful[], failed[] |
| `queue-receive-messages` | `AmazonSQS.ReceiveMessage` | queueUrl, maxNumberOfMessages?(1–10), waitTimeSeconds?(0–20 long poll), visibilityTimeout?, messageSystemAttributeNames?, messageAttributeNames? | messages[] (messageId, receiptHandle, body, attributes, messageAttributes) |
| `queue-delete-message` | `AmazonSQS.DeleteMessage` | queueUrl, receiptHandle | success |
| `queue-delete-message-batch` | `AmazonSQS.DeleteMessageBatch` | queueUrl, entries[] (id, receiptHandle) | successful[], failed[] |
| `queue-change-message-visibility` | `AmazonSQS.ChangeMessageVisibility` | queueUrl, receiptHandle, visibilityTimeout | success |
| `queue-change-message-visibility-batch` | `AmazonSQS.ChangeMessageVisibilityBatch` | queueUrl, entries[] | successful[], failed[] |

### Queue management (incl. custom KMS — §7)
| Capability id | X-Amz-Target | Key inputs | Key outputs |
|---|---|---|---|
| `queue-create-queue` | `AmazonSQS.CreateQueue` | queueName, attributes{…, KmsMasterKeyId, …}, tags? | queueUrl |
| `queue-delete-queue` | `AmazonSQS.DeleteQueue` | queueUrl | success |
| `queue-list-queues` | `AmazonSQS.ListQueues` | queueNamePrefix?, maxResults?, nextToken? | queueUrls[], nextToken? |
| `queue-get-queue-url` | `AmazonSQS.GetQueueUrl` | queueName, queueOwnerAWSAccountId? | queueUrl |
| `queue-get-queue-attributes` | `AmazonSQS.GetQueueAttributes` | queueUrl, attributeNames[] | attributes{} |
| `queue-set-queue-attributes` | `AmazonSQS.SetQueueAttributes` | queueUrl, attributes{…, KmsMasterKeyId, …} | success |
| `queue-purge-queue` | `AmazonSQS.PurgeQueue` | queueUrl | success |
| `queue-list-queue-tags` | `AmazonSQS.ListQueueTags` | queueUrl | tags{} |
| `queue-tag-queue` | `AmazonSQS.TagQueue` | queueUrl, tags{} | success |
| `queue-untag-queue` | `AmazonSQS.UntagQueue` | queueUrl, tagKeys[] | success |

`AddPermission`/`RemovePermission` and `ListDeadLetterSourceQueues` are deferred (rarely used; add later if needed).

**Field naming:** SQS JSON wire names are PascalCase (`QueueUrl`, `MessageBody`, `ReceiptHandle`). The agent's Rust structs stay snake_case with `#[serde(rename = "QueueUrl")]` per field (or a `rename_all = "PascalCase"` container) so the JSON sent to AWS is exact, while the capability metadata (Step Picker field names, `steps.X.outputs.*` refs) is controlled independently by the `#[field]` display names. Message bodies are UTF-8 strings capped at 256 KB by SQS; base64 for binary payloads is the caller's responsibility (documented on the field).

---

## 7. KMS / server-side encryption mapping

**Correction to the initial framing:** unlike S3 (per-object SSE headers), SQS encryption is a **queue attribute**, set at create/update time — never per message. So "custom KMS key ids" live only on `queue-create-queue` and `queue-set-queue-attributes`, via the `attributes` map:

- `KmsMasterKeyId` — customer CMK id, ARN, or alias (e.g. `alias/my-key`)
- `KmsDataKeyReusePeriodSeconds` — 60–86400
- `SqsManagedSseEnabled` — `"true"` for SSE-SQS (mutually exclusive with SSE-KMS)

The message path (`send`/`receive`/`delete`) needs **nothing** to talk to an encrypted queue — SQS transparently encrypts/decrypts; the caller only needs `kms:GenerateDataKey`/`kms:Decrypt` on the CMK, which is an IAM/KMS-policy concern outside the agent.

Design choice for ergonomics: model these as **first-class optional typed inputs** on the two queue-config capabilities (e.g. `kms_master_key_id: Option<String>`, `kms_data_key_reuse_period_seconds: Option<u32>`, `sqs_managed_sse_enabled: Option<bool>`) that the agent folds into the `Attributes` map before sending — rather than making users hand-assemble the raw attribute map. Keep a passthrough `attributes: Option<Map<String,String>>` too for the long tail (VisibilityTimeout, RedrivePolicy, FifoQueue, ContentBasedDeduplication, …).

---

## 8. Registration & build

1. Add `crates/agents/runtara-agent-sqs` to the workspace `members` in [Cargo.toml](../Cargo.toml).
2. Add `("sqs", runtara_agent_sqs::agent_info())` to the `agents()` vec in [runtara-agent-bundle-emit/src/main.rs](../crates/runtara-agent-bundle-emit/src/main.rs).
3. `scripts/build-agent-components.sh` compiles the component and emits `runtara_agent_sqs.wasm` + `.meta.json` into `RUNTARA_AGENT_COMPONENTS_DIR`. The CI component-count check **derives the expected count from the directory** (`ls -d crates/agents/runtara-agent-*/ | wc -l`), so adding the crate needs **no CI edit** — it self-verifies the `.wasm`/`.meta.json` pair exists.
4. `regen-frontend-api` so the new capabilities (Step Picker) and the `endpoint` field on the AWS connection appear in the UI.

No `.meta.json` is hand-authored — it's derived from the `#[capability]` macros at emit time.

---

## 9. Testing / e2e

- **Unit:** body-builder tests (op inputs → exact JSON), error-mapper tests (`{__type, message}` → `AgentError` category/severity), attribute-folding tests (KMS inputs → `Attributes` map).
- **e2e (required per repo policy):** compile → register → execute a workflow that creates a queue, sends, receives, deletes, and reads back attributes. Use **LocalStack** or **ElasticMQ** as a mock SQS endpoint (set on the connection's new `endpoint` field).
- **SSRF caveat:** the proxy calls `reject_private_url()` ([internal_proxy.rs:392](../crates/runtara-server/src/api/handlers/internal_proxy.rs#L392)); a loopback LocalStack endpoint will be rejected under the connection-proxy hardening rules. e2e must either point at a non-loopback mock (bind LocalStack to a routable test-network address, mirroring the eval-image socat sidecar approach) or run with the SSRF guard relaxed for the test env. Decide this before wiring e2e.
- Real-AWS smoke test against a throwaway queue in one region as a final gate.

---

## 10. Phasing

- **P0 — server plumbing:** `X-Runtara-Aws-Service` lift/strip in `runtara-http`, `ProxyRequest.aws_service`, the override + `aws_default_endpoint` helper, `endpoint` field on `AwsCredentialsParams`. Small, reusable, unblocks every future AWS agent.
- **P1 — message path:** `send-message`, `receive-messages`, `delete-message` (+ batch variants), `change-message-visibility`. The core read/write ask; e2e here.
- **P2 — queue management + KMS:** create/delete/list/get-url/get-attrs/set-attrs/purge + tags, with the typed KMS inputs.
- **P3 — polish:** long-poll defaults, FIFO fields, DLQ redrive helpers, docs/README (signpost length per repo convention).

---

## 11. Open questions / decisions

1. **`aws_service` override — trust boundary.** The header lets the agent pick the signing service against a generic AWS connection. That's the intended generalization, but confirm we're comfortable that any agent holding a connection_id can sign for any service the connection's IAM principal permits (this is already true of the connection's IAM grant; the header doesn't widen IAM, only which endpoint we hit). No secret exposure — creds never leave the proxy.
2. **Connection `category`.** Leave `aws_credentials` as `"llm"` (it's shared with Bedrock) or broaden to a generic bucket? Cosmetic UI grouping only.
3. **Region-per-connection.** Confirmed acceptable: one connection = one region (matches AWS SDK client semantics). Cross-region use = multiple connections.
4. **LocalStack SSRF handling for e2e** (see §9) — pick the mock-binding strategy before P1 e2e.
5. **FIFO ergonomics.** Expose `messageGroupId`/`messageDeduplicationId` as top-level optional inputs on send ops (recommended) vs. requiring them inside the attributes map.

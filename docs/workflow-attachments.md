# Workflow-owned channel attachments

Slack and Mailgun ingestion verifies and parses the webhook, deduplicates it,
and delivers it to the channel workflow. It does not fetch messages, download
files, create buckets, or upload attachments. File-only messages and Mailgun
stored-message notifications can start workflows with an empty `userMessage`.

## Inputs

The existing `data` envelope contains:

- `originalMessage`: the Slack callback or parsed Mailgun JSON/form fields,
  preserving email bodies, nested JSON, and provider-specific references.
- `sourceConnectionId`: the opaque connection ID which received the event.
  Select a connection on agent steps using the usual workflow configuration.
- `attachments`: metadata (`name`, `type`, `size`) and either a provider `url`,
  a Slack file `id`, or already-received base64 `data` for Mailgun multipart.
- `userMessage`: text already present in the event, without fetching missing bodies.

Follow-up session signals carry the same `attachments`, `originalMessage`, and
`sourceConnectionId` fields. Multipart file bytes live only in `attachments`,
not a second copy in `originalMessage`.

## Explicit workflow operations

| Agent | Capability | Inputs | Output |
|---|---|---|---|
| Slack | `get-file-info` | `file_id` | `file`: provider metadata, without content |
| Slack | `download-file` | `file_id`, optional `max_bytes` | `content`, `filename`, `content_type`, `size` |
| Mailgun | `get-message` | `url`, optional `max_bytes` | `message`: complete JSON; `attachments`: metadata array |
| Mailgun | `download-attachment` | `url`, optional `filename`, `max_bytes` | `content`, `filename`, `content_type`, `size` |

Download `content` is base64. The default and maximum size is 5 MiB (5,242,880
bytes), allowing room for the proxy envelope inside the component transport's
8 MiB limit. Lower `max_bytes` to reject oversized files. Limits are enforced
while reading the upstream body and after the response reaches the agent.

Slack downloads resolve the file ID with `files.info` first. Configure the bot
with `files:read` and access to the file's conversation. A metadata check can
decide whether to invoke `download-file` at all. The file must still be available
and accessible when the step executes.

For Mailgun stored-message notifications, map `originalMessage["message-url"]`
to `get-message.url`. Older `storage-url` and nested `storage.url` values are
preserved too. Iterate its returned attachment metadata and invoke
`download-attachment` only for selected files. The message lookup does not fetch
attachment bytes. For multipart webhook delivery, use `attachments[i].data`
directly: those bytes have already arrived, so there is nothing to download.

To defer receipt of Mailgun content, configure a `store(notify=...)` route.
Mailgun documents this storage as temporary, up to three days. A workflow needing
retention must download and save the content before it expires.

Examples (replace the connection ID before compiling):

- [Download the first Slack attachment](examples/attachments/slack-download.json).
- [Download the first URL-based Mailgun attachment](examples/attachments/mailgun-download.json).

Add a condition or loop before these steps for filtering or multiple files. To
persist a downloaded file, add `s3-storage.storage-upload-file` with an explicitly chosen
connection, bucket, and key. Map `steps.download.outputs.content` to `content`
and `steps.download.outputs.content_type` to `content_type`; leave `is_base64`
enabled. Azure storage is another explicit workflow choice. Neither download
agent assumes a destination.

## Authentication, errors, and supported endpoints

Provider credentials remain in the host connection/proxy layer; components only
receive connection references. Slack file traffic selects the declared `files`
endpoint. Mailgun selects a declared regional message/storage endpoint, scoped
to the connection's domain. Unknown hosts and escaped paths are refused; the
proxy never follows redirects automatically. A redirect surfaces as an error.

Mailgun supports `api.mailgun.net`, `api.eu.mailgun.net`,
`storage-us-west1.api.mailgun.net`, `storage-us-east4.api.mailgun.net`,
`storage-europe-west1.api.mailgun.net`, `storage.api.mailgun.net`, and
`storage.eu.mailgun.net`, under `/v3/domains/{domain}/messages`. New provider hosts
require a declared endpoint update, not a workflow-supplied credential header.

Rate limits and network/server failures produce transient errors, preserving
numeric `Retry-After`. Authentication/access errors, missing files, redirects,
invalid URLs, and size-limit failures are permanent errors. Workflows can handle
those outcomes using normal step error handling.

## Migration

Existing workflows using automatically assigned `storage_bucket` / `storage_key`
must add explicit download/upload steps. New events no longer contain those
fields. Existing stored objects are untouched.

The old “Use as default file storage” setting and the native S3 client are
removed. The `object_storage` connection default remains available for workflow
agents. The legacy `isDefaultFileStorage` API/database field remains a compatibility
alias for that default; it no longer causes webhook attachment persistence.

## Verification

Build with `scripts/build-agent-components.sh`. The component-host
`attachment_downloads` integration test loads the actual WASM agents against a
local proxy fixture. Run the isolated native-server E2E with:

```sh
cargo build -p runtara-server --bin runtara-server
python3 e2e/test_workflow_attachments.py
```

It requires Docker with `pgvector/pgvector:pg18` and `redis-server`, creates its
own services and synthetic connections, and checks raw ingestion, no automatic
downloads even with a storage default, multipart bytes, deduplication, and
compiled workflow downloads/uploads. It stops only its own resources and keeps
test logs in the printed temporary directory. Provider traffic uses a local mock;
this does not claim a live Slack/Mailgun account test.

Provider references: [Slack file authentication](https://docs.slack.dev/reference/objects/file-object/),
[Mailgun retrieval](https://documentation.mailgun.com/docs/mailgun/user-manual/receive-forward-store/storing-and-retrieving-messages),
[Mailgun retention](https://documentation.mailgun.com/docs/mailgun/user-manual/receive-forward-store/route-actions).

# DataSource — a self-describing content envelope for agents

> **Status:** Proposed (Option A from the csv/xml input-decoding design discussion)
> **Author:** _draft_
> **Scope:** all agents that accept or emit byte/text payloads (csv, xml, crypto,
> xlsx, compression) and the storage producers (s3, azure-blob, sharepoint, http).
> **Supersedes:** the per-agent `*DataInput` untagged enums and the "try base64,
> fall back to raw bytes" heuristic currently in `csv` and `xml`.
>
> **Decisions (resolved):** (1) the one input behavior change — bare-string-base64
> now means **text** — is **accepted**; (2) `file` is a **first-class variant**
> (not a base64 alias), because the UI wants a file picker and to request files
> from users; (3) the Step Picker macro/metadata enhancement is **committed**, not
> optional.

---

## 1. Motivation

### 1.1 The ambiguity

Agents pass byte payloads to each other as JSON. A payload can legitimately be:

- **raw text** (an http response body, a hand-authored XML/CSV string),
- **base64** (a storage download, an upload body, an image),
- **raw bytes** (an inline `[u8]` array),
- **a file** (base64 content + filename + mime type).

Today each consuming agent declares an *untagged* `*DataInput` enum
(`Bytes | File | Base64String`). Because the bare-string arm matches **any**
JSON string, the agent cannot tell base64 from raw text and must **guess**. The
shipped stopgap (in `crates/agents/runtara-agent-csv/src/lib.rs` and
`runtara-agent-xml/src/lib.rs`) is:

```rust
// try base64, fall back to raw UTF-8 bytes when it fails
Ok(BASE64.decode(s).unwrap_or_else(|_| s.as_bytes().to_vec()))
```

### 1.2 Why guessing is wrong (not just inelegant)

Two *legitimate* upstream sources arrive as a bare string but mean the **opposite**:

| Producer | Default `content` shape | Correct handling |
|---|---|---|
| `s3` / `azure-blob` / `sharepoint` download | **base64** (unless `as_text=true`) | base64-**decode** |
| `http` response body (`text`) / hand-authored mapping | **raw text** | use **as-is** |

A heuristic cannot separate these reliably. The footgun: a raw-text payload that
*happens to be valid base64* — e.g. a CSV whose entire content is the word
`test`, or the literal `dGVzdA==` — is silently base64-decoded into garbage
instead of being parsed. Silent corruption is strictly worse than a loud error.

The fix is not a better heuristic. It is to make the **producer's knowledge of
the format travel with the bytes**, so consumers never guess.

### 1.3 Why now / why a shared type

- `FileData` is **duplicated in 8 places** (6 agents + `runtara-dsl` +
  `runtara-agents`). Each agent re-derives format handling.
- `runtara-agent-encoding` already exists as the *one* shared encoding
  vocabulary — the natural home for a *one* shared payload vocabulary.
- The multi-source reality (s3/azure/sharepoint/http/file) and the existing
  **50 MB inline-base64 ceiling** mean the extensibility is already latent: a
  future `ref` variant (an object-store handle instead of inline base64) is the
  obvious next step and is only possible with a tagged, self-describing design.

---

## 2. Goals / Non-goals

**Goals**

1. A single, self-describing payload envelope (`DataSource`) shared by every
   byte/text agent, living in `runtara-agent-encoding`.
2. **No guessing, ever.** Format is explicit. A malformed payload is a hard,
   structured error — never silent corruption.
3. **Encoding is selectable** and carried in-band for text.
4. **Backward compatible on input** — existing workflows that pass a bare
   string / byte array / `FileData` object keep working via a documented,
   lossless normalization (default-to-**text**, never default-to-base64).
5. Producers (s3/azure/sharepoint/http) **emit** the envelope so download→parse
   pipelines are unambiguous end-to-end.
6. Collapse the 8× `FileData` duplication.

**Non-goals**

- Changing how bytes move on the wire (still inline JSON; a `ref`/handle
  transport is future work, designed-for but not built here).
- Reworking the encoding detection itself (`runtara-agent-encoding::decode`
  is unchanged).
- A big-bang break. Migration is strangler-fig; legacy shapes are accepted
  throughout the deprecation window.

---

## 3. The `DataSource` envelope

New module `crates/runtara-agent-encoding/src/data_source.rs`, re-exported from
the crate root.

```rust
use serde::{Deserialize, Serialize};
use crate::Encoding;

/// A self-describing content payload. The `format` tag states what the bytes
/// are, so consumers never have to guess.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "format", rename_all = "snake_case")]
pub enum DataSource {
    /// Already-decoded text. `encoding` records how to re-serialize it to bytes
    /// (default UTF-8); it is *not* re-decoded by text consumers.
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Encoding::is_utf8")]
        encoding: Encoding,
    },

    /// A file: base64 content plus provenance. This is the variant the UI's file
    /// picker produces and "request a file from the user" collects, so `filename`
    /// is first-class. Legacy `FileData` (`{content, filename?, mime_type?}`)
    /// normalizes here.
    File {
        /// Base64 (standard alphabet) file content.
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
    },

    /// Bare base64 (standard alphabet) bytes — no file framing. For inline blobs
    /// and binary bodies without a filename.
    Base64 { data: String },

    /// Inline raw bytes (JSON array of u8). For small payloads / round-tripping;
    /// large binary should use `file`/`base64`.
    Bytes { bytes: Vec<u8> },

    // FUTURE (not in this spec): a content-addressed handle for large payloads.
    // Ref { uri: String, etag: Option<String>, size: Option<u64>, ... },
}
```

**Why internal tagging.** `{"format":"base64","data":"…"}` reads naturally, is
self-describing for tooling/LLM authors, lets each variant carry its own named
fields, and is *additively extensible* (a new variant is one enum arm, not a new
flag on every consumer). It matches the `{format, value, …}` sketch from the
design discussion.

**Format values:** `text`, `file`, `base64`, `bytes`. `file` and `base64` both
carry base64 bytes; `file` adds filename/mime provenance and drives the UI file
picker (§8). Unknown `format` is a deserialization error (explicit failure).

### 3.1 Encoding semantics

`Encoding` (existing, in the same crate) must gain `Serialize` so producers can
emit `Text`:

```rust
impl Serialize for Encoding {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error> {
        // canonical name, or "Auto"
        s.serialize_str(match self.resolve() {
            Some(enc) => enc.name(),
            None => "Auto",
        })
    }
}
// plus a helper used by skip_serializing_if and into_bytes:
impl Encoding { pub fn is_utf8(&self) -> bool { self.resolve() == Some(encoding_rs::UTF_8) } }
```

Encoding is relevant in **both directions**:

- **Text consumer** (csv, xml): a `Text` source is already decoded → used as-is.
  A `Base64`/`Bytes` source is decoded with the consumer's own `encoding` field
  (the existing `FromCsvInput.encoding` / `FromXmlInput.encoding`, `Auto`-capable).
- **Byte consumer** (crypto hash, xlsx, compression, storage upload): a `Text`
  source is encoded to bytes using the `Text.encoding` (UTF-8 by default); a
  `Base64`/`Bytes` source is already bytes.

### 3.2 Helpers

```rust
impl DataSource {
    /// Bytes for byte-consumers. Base64 decode failure is a hard error.
    pub fn into_bytes(self) -> Result<Vec<u8>, DataSourceError> {
        match self {
            DataSource::Bytes { bytes } => Ok(bytes),
            DataSource::Base64 { data } => BASE64.decode(&data)
                .map_err(DataSourceError::invalid_base64),
            DataSource::File { content, .. } => BASE64.decode(&content)
                .map_err(DataSourceError::invalid_base64),
            DataSource::Text { text, encoding } => Ok(crate::encode(&text, encoding)),
        }
    }

    /// Decoded text for text-consumers. `fallback` decodes byte-bearing
    /// variants; ignored for `Text` (already decoded).
    pub fn into_text(self, fallback: Encoding) -> Result<crate::DecodeOutcome, DataSourceError> {
        match self {
            DataSource::Text { text, encoding } =>
                Ok(crate::DecodeOutcome { encoding_name: encoding_name(encoding), text, had_errors: false }),
            other => {
                let bytes = other.into_bytes()?;          // base64 may hard-error
                Ok(crate::decode(&bytes, fallback))       // lossy, never fails
            }
        }
    }
}
```

New free function in the encoding crate (mirrors `decode`):

```rust
/// Text → bytes for a chosen encoding. UTF-8 (and Auto) is `text.as_bytes()`;
/// other charsets go through `encoding_rs::Encoding::encode` (lossy for
/// unrepresentable chars, never fails).
pub fn encode(text: &str, encoding: Encoding) -> Vec<u8> { … }
```

### 3.3 Error type

```rust
#[derive(Debug, Clone)]
pub struct DataSourceError { pub code: &'static str, pub message: String }
impl DataSourceError {
    pub fn invalid_base64(e: base64::DecodeError) -> Self {
        Self { code: "DATA_INVALID_BASE64", message: format!("content is not valid base64: {e}") }
    }
}
```

Each agent maps `DataSourceError` into its existing error channel (csv's
`err_json`, xml's `AgentError::permanent`, etc.) — codes stay agent-namespaced
(`XML_DECODE_ERROR` wraps `DATA_INVALID_BASE64`) so existing `knownErrors`
metadata and `onError` routing are unaffected.

---

## 4. Backward-compatible input wire format

Inputs accept the canonical envelope **and** the three legacy shapes, via a thin
wrapper whose only job is normalization:

```rust
/// What capability inputs declare for a payload field (`data: DataInput`).
/// Deserializes the canonical `DataSource` *or* a legacy shape, normalizing to
/// `DataSource`. Serialization always emits the canonical form.
#[derive(Debug, Clone)]
pub struct DataInput(pub DataSource);

#[derive(Deserialize)]
#[serde(untagged)]
enum Wire {
    Canonical(DataSource),     // {"format": …}
    LegacyFile(LegacyFileData),// {"content": …, filename?, mime_type?}
    LegacyBytes(Vec<u8>),      // [104, …]
    LegacyString(String),      // "…"
}
```

Normalization (`From<Wire> for DataSource`):

| Legacy input | Normalized to | Note |
|---|---|---|
| `{"format": …}` | the canonical variant | preferred form |
| `{"content": b64, filename?, mime_type?}` | `File { content, filename, mime_type }` | legacy `FileData` → `file` |
| `[u8, …]` (JSON array) | `Bytes { bytes }` | |
| `"…"` (bare string) | **`Text { text, encoding: UTF-8 }`** | **default-to-text** |

**The default-to-text rule is the crux.** Text→bytes is total and lossless;
base64-guessing is the lossy, corrupting direction. So a bare string is treated
as text, *never* speculatively base64-decoded. Producers that mean base64 must
say so (canonical `Base64`, or the legacy `{"content": …}` object).

**Untagged ordering** (serde tries variants top-down): `Canonical` first (requires
the `format` tag), then `LegacyFile` (object with `content`, no `format`), then
`Bytes` (array), then `String`. The shapes are disjoint by JSON type + required
fields, so there is no accidental match.

> **Behavior change to call out in release notes.** A workflow that today maps a
> known-**base64** *bare string* (e.g. an s3 download's `outputs.content`) into a
> text consumer's `data` field currently gets auto-base64-decoded by the shipped
> fallback. After this change a bare string is **text**. The fix is to map the
> producer's new self-describing `outputs.data` field instead (§7). This is the
> single intentional break, and it trades silent ambiguity for an explicit,
> correct pipeline.

---

## 5. Crate layout & shared-type API

`runtara-agent-encoding` becomes the home of the payload vocabulary:

- **add** `data_source.rs`: `DataSource`, `DataInput`, `DataSourceError`,
  `encode()`, and a canonical `FileData` re-export (`pub type FileData = …` or a
  thin struct) to replace the 8 duplicates.
- **add** `Serialize for Encoding` + `Encoding::is_utf8`.
- crate stays wasm-safe (deps are `serde`, `encoding_rs`, `chardetng`,
  `runtara-dsl` default-features=false, `base64`) — `base64` is the only new dep
  and is already used by every consumer.

`runtara-agents` (host) gains a dependency on `runtara-agent-encoding` so the
native xlsx/compression handlers (§7.3) share the same type.

---

## 6. Consumer changes

| Agent | Today | After |
|---|---|---|
| `csv` | `CsvDataInput` (Bytes\|File\|Base64String) + fallback | `data: DataInput`; `from_csv`/`get_header` call `data.0.into_text(input.encoding)?` |
| `xml` | `XmlDataInput` + fallback | `data: DataInput`; `from_xml` calls `data.0.into_text(input.encoding)?` |
| `crypto` | `HashDataInput` (Text\|File) — already explicit | `data: DataInput`; `into_bytes()?` (its `Text\|File` is a strict subset, zero behavior change) |
| `xlsx` | `XlsxDataInput`, forwards to native handler | `data: DataInput` **+ host handler update** (§7.3); `into_bytes()?` on the host. Binary format → only `base64`/`bytes`/`file` are meaningful |
| `compression` | `ArchiveDataInput` (FileData\|Base64String) | `file: DataInput`; `into_bytes()?` on host |

Each consumer:

1. replaces its local `*DataInput` enum + `FileData` + `to_bytes()` with
   `data: DataInput` and a one-line `into_text`/`into_bytes` call;
2. deletes its local `FileData`;
3. keeps its existing sibling `encoding` field (csv/xml) — now passed as the
   `fallback` to `into_text`.

### 6.3 xlsx / compression native-handler constraint

`xlsx` and `compression` are *forwarding stubs*: the wasm component POSTs the
typed input JSON to the host native endpoint, and
`crates/runtara-agents/src/agents/xlsx.rs` deserializes it with its **own**
`XlsxDataInput`. Both sides must change in lockstep:

- wasm side: `data: DataInput` (serializes canonical on forward).
- host side: `data: DataInput` (accepts canonical + legacy). Since the host now
  deps `runtara-agent-encoding`, both literally share the type.

These are binary formats, so a `Text` source is nonsensical; the host handler
should reject `format: "text"` with a clear `XLSX_TEXT_UNSUPPORTED` error rather
than silently encoding it. (This is exactly the kind of explicitness the whole
change buys.)

---

## 7. Producer changes (the centerpiece)

Producers must **emit** the envelope so a download→parse pipeline is
unambiguous. Strategy is **additive**: add a self-describing `data: DataSource`
output, keep the legacy `content: String` for the deprecation window.

### 7.1 s3 / azure-blob (identical shapes)

`DownloadFileOutput` (`runtara-agent-s3-storage`, `runtara-agent-azure-blob-storage`):

```rust
pub struct DownloadFileOutput {
    pub success: bool,
    // NEW — preferred, self-describing:
    #[field(display_name = "Data",
            description = "Self-describing content envelope. format=base64 (binary) or text.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<DataSource>,
    // DEPRECATED — kept for one release window:
    #[field(description = "DEPRECATED: use `data`. Base64 by default, or UTF-8 when as_text=true.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    pub content_type: Option<String>,
    pub size: Option<u64>,
    pub error: Option<String>,
}
```

Population in `storage_download_file`:

```rust
let data = if input.as_text.unwrap_or(false) {
    // decode bytes → text via the encoding crate (replaces the awkward
    // String::from_utf8(..).unwrap_or_else(base64-encode) fallback)
    let text = runtara_agent_encoding::decode(&body, Encoding::Auto).text;
    DataSource::Text { text, encoding: Encoding::default() }
} else {
    // a download IS a file: it has a name (the key) and a mime type.
    DataSource::File {
        content: BASE64.encode(&body),
        filename: basename(&input.key),
        mime_type: content_type.clone(),
    }
};
// emit BOTH for the deprecation window:
DownloadFileOutput { data: Some(data), content: Some(legacy_string), .. }
```

`UploadFileInput` — accept the envelope as the preferred content source:

```rust
pub struct UploadFileInput {
    // NEW — supersedes content/is_base64 when present:
    #[serde(default)] pub source: Option<DataInput>,
    // DEPRECATED pair, kept for the window:
    pub content: String,
    #[serde(default = "default_true_opt")] pub is_base64: Option<bool>,
    pub content_type: Option<String>,
    ..
}
// handler:
let data = match input.source {
    Some(s) => s.0.into_bytes().map_err(to_agent_err)?,
    None => if input.is_base64.unwrap_or(true) { BASE64.decode(&input.content)? }
            else { input.content.into_bytes() },
};
```

This makes `download.outputs.data → upload.source` a clean, lossless pipe.

### 7.2 sharepoint

Same shape as s3 (`DownloadFileOutput.content`, `UploadFileInput.content` +
`is_base64`, helper `decode_content`). Apply §7.1 verbatim: add
`DownloadFileOutput.data: Option<DataSource>`, add `UploadFileInput.source`,
route `decode_content` through `DataInput::into_bytes`.

### 7.3 http

`http`'s `HttpResponseBody` is **already** self-describing
(`Json(Value) | Text(String) | Binary { base64 }`). Rather than churn it, add a
total conversion and (optionally) mirror it as a `data` field:

```rust
impl From<HttpResponseBody> for DataSource {
    fn from(b: HttpResponseBody) -> Self {
        match b {
            HttpResponseBody::Text(t)        => DataSource::Text { text: t, encoding: Encoding::default() },
            HttpResponseBody::Binary { base64 } => DataSource::Base64 { data: base64 },
            HttpResponseBody::Json(v)        => DataSource::Text { text: v.to_string(), encoding: Encoding::default() },
        }
    }
}
```

Lower priority than storage because http's body is already distinguishable, but
aligning it means *all* producers speak one vocabulary.

---

## 8. Metadata, Step Picker & authoring schema

The capability macro renders a `*DataInput` field as `type: "string"` today
(confirmed via `get_capability xml from-xml` → `data` is `"type":"string"`),
because the field's enum type isn't registered as a nested type. We make
`DataSource` a **first-class, UI-aware type** so the Step Picker can render a
format selector — and, for `file`, a real **file picker**.

**Mechanism (committed).** Rather than deriving metadata for arbitrary tagged
enums, register **one** descriptor for `DataSource` and link fields to it by
name — mirroring how scalar fields already link to `Encoding` via
`#[field(enum_type = "Encoding")]` + the `EnumVariants` trait. Concretely:

- Add a richer descriptor to `runtara-dsl::agent_meta` — a `VariantTypeMeta`
  (discriminator `format`; per-variant `{ value, display_name, fields:
  &[InputFieldMeta], widget }`) alongside the existing `InputTypeMeta`.
- Provide a hand-written `DataSource` descriptor: its four variants, their
  fields, and a `widget` hint per variant — `text` → textarea + encoding
  dropdown, `file` → **file upload**, `base64` → text, `bytes` → advanced/hidden.
- Extend the macro so `#[field(envelope = "DataSource")]` on a `data: DataInput`
  field links to that descriptor, and `capability_to_api` surfaces it (instead of
  collapsing to `string`).
- This stays small (one descriptor + one link attribute) and **generalizes
  later** into full tagged-enum derivation if other envelopes appear.

**Frontend.** The generated client gains the `DataSource` variant metadata; the
Step Picker:

- renders a **format selector**; per-format fields follow the descriptor;
- for `format: "file"` renders a **file upload** widget that base64-encodes the
  chosen file into `{ "format": "file", "content": <b64>, "filename", "mime_type" }`;
- this is also how a workflow **requests a file from the user** at trigger time —
  a `DataSource` workflow input with the `file` widget becomes a file drop-zone
  in the run dialog. (The producer→consumer pipe in §7 and this human-upload path
  produce the *same* `file` envelope, so downstream steps don't care which.)

**Authoring schema (LLM/MCP authors).** Add a `dataSource` section to
`get_workflow_authoring_schema` with the canonical shapes + one example per
`format`, so non-UI authors emit valid envelopes (see Appendix B).

`regen-frontend-api` must run after both the producer output structs and the
`DataSource` descriptor land, so the generated client picks up the new `data`
fields and the variant metadata.

---

## 9. Rollout phases (strangler-fig)

| Phase | Content | Breaking? |
|---|---|---|
| **0** | Add `DataSource`/`DataInput`/`encode`/`Encoding: Serialize` + canonical `FileData` to `runtara-agent-encoding`. Unit tests only. | no |
| **1** | Migrate `csv`, `xml` consumers to `data: DataInput`. Keep behavior identical to the shipped fallback **except** bare-string→text (the intended change). | input-compatible; one documented behavior change |
| **2** | Migrate `crypto` (subset, no behavior change). Add the `DataSource` descriptor (`agent_meta`) + the `dataSource` authoring-schema section (§8). | no |
| **2b** | Macro `#[field(envelope)]` link + `capability_to_api` surfacing; frontend Step Picker **format selector & file picker**, `DataSource` workflow inputs as file drop-zones (§8). `regen-frontend-api`. | no (additive UI) |
| **3** | **Producers**: add `data: DataSource` outputs (`file`/`text`) + `source` inputs to s3, azure-blob, sharepoint; add http `From` conversion. Keep legacy `content`. Run `regen-frontend-api`. | additive |
| **4** | Migrate `xlsx`, `compression` + host native handlers in lockstep (§7.3). | input-compatible |
| **5** | Delete the 8 duplicate `FileData` defs; everything imports the shared type. | internal only |
| **6** (later release) | Remove deprecated `content: String` outputs / `is_base64` inputs once dashboards show no workflow references them. | breaking — gated on telemetry |
| **7** (future) | `DataSource::Ref` for externalized large payloads (object-store handle). | additive |

Each phase is independently shippable and `e2e-verify`-able.

---

## 10. Validation, errors, observability

- **No silent fallback.** `into_bytes` on a malformed `Base64` returns
  `DATA_INVALID_BASE64` (hard, structured). Unknown `format` → deserialization
  error at input parse time.
- **Lossless text.** `into_text` on byte sources uses `runtara-agent-encoding::decode`
  (lossy U+FFFD substitution, `had_errors` flag) — never a hard failure, matching
  today's text-agent behavior; the `had_errors`/`encoding_name` can be surfaced.
- **Deprecation signal.** When a producer still populates legacy `content` or a
  consumer normalizes a legacy shape, emit a `tracing` debug/`deprecated` marker
  so we can measure remaining legacy usage before Phase 6.

---

## 11. Testing

- **Unit (encoding crate):** round-trip each `DataSource` variant through
  serde (canonical), each legacy shape through `DataInput` normalization
  (incl. the `test`/`dGVzdA==` footgun → now `Text`, not corrupted),
  `into_bytes`/`into_text` matrices, base64-error is hard, non-UTF-8 `encode`.
- **Per-agent unit:** keep existing parse tests; add the production untagged
  path (`serde_json::from_value::<DataInput>`) for raw/base64/file/bytes — this
  is the path the old hand-constructed tests bypassed.
- **e2e (per `e2e-verify`):** rebuild components + emit-meta, then:
  1. `http GET (text) → xml from-xml` — raw text body parses.
  2. `s3 download (default base64) → outputs.data → csv from-csv` — base64
     decodes correctly with **no `as_text` and no guessing**.
  3. `s3 download → data → s3 upload source` round-trips bytes unchanged.
  4. malformed base64 `{"format":"base64","data":"!!!"}` → hard
     `DATA_INVALID_BASE64`, not silent garbage.

---

## 12. Risks, tradeoffs, open questions

- **Output break risk (Phase 3/6).** Adding `data` is additive; *removing*
  `content` is the real break. Gate Phase 6 on reference telemetry
  (`find_references` / instance dashboards), not a fixed date.
- **The one input behavior change — accepted.** Bare-string-base64 → now text
  (§4). Loud in release notes; the migration is "map `.outputs.data`".
- **`bytes` variant size.** A JSON `[u8]` array is large/inefficient; document
  that big binary uses `base64`. (Motivates the future `ref` variant.)
- **Macro scope — committed.** The `DataSource` descriptor + `#[field(envelope)]`
  link is the only change touching `runtara-agent-macro` / `runtara-dsl::agent_meta`;
  everything else is library + agent edits. It drives the Step Picker format +
  **file** widgets (§8) but is cleanly separable — Phases 0–1 function with `data`
  rendering as `string` if the UI work slips.
- **`file` is first-class — decided.** A distinct `file` variant (not a `base64`
  alias) so the UI renders a file picker and workflows can request files from
  users (§8). `base64` remains for bare/inline blobs without file framing.
- **Open:** should `Json` http bodies become `Text(stringified)` or a future
  structured `DataSource::Json(Value)` variant? Deferred; `Text` is safe now.

---

## Appendix A — file-by-file change inventory

**New / shared**
- `crates/runtara-agent-encoding/src/data_source.rs` — `DataSource`, `DataInput`,
  `DataSourceError`, `encode()`, shared `FileData`. *(new)*
- `crates/runtara-agent-encoding/src/lib.rs` — `Serialize for Encoding`,
  `Encoding::is_utf8`, `pub mod data_source` + re-exports.
- `crates/runtara-agent-encoding/Cargo.toml` — add `base64`.
- `crates/runtara-agents/Cargo.toml` — add `runtara-agent-encoding`.

**Consumers**
- `crates/agents/runtara-agent-csv/src/lib.rs` — drop `CsvDataInput`/`FileData`/
  `to_bytes`; `data: DataInput`; `into_text(encoding)`.
- `crates/agents/runtara-agent-xml/src/lib.rs` — same (replaces the current fix).
- `crates/agents/runtara-agent-crypto/src/lib.rs` — `HashDataInput` → `DataInput`;
  `into_bytes`.
- `crates/agents/runtara-agent-xlsx/src/lib.rs` + `crates/runtara-agents/src/agents/xlsx.rs`
  — `XlsxDataInput` → `DataInput` (both sides); reject `text`.
- `crates/agents/runtara-agent-compression/src/lib.rs` (+ host handler) —
  `ArchiveDataInput` → `DataInput`.

**Producers**
- `crates/agents/runtara-agent-s3-storage/src/lib.rs` — `DownloadFileOutput.data`,
  `UploadFileInput.source`; populate envelope; `regen-frontend-api`.
- `crates/agents/runtara-agent-azure-blob-storage/src/lib.rs` — mirror of s3.
- `crates/agents/runtara-agent-sharepoint/src/lib.rs` — mirror; route
  `decode_content` through `DataInput`.
- `crates/agents/runtara-agent-http/src/lib.rs` — `From<HttpResponseBody> for DataSource`.

**Metadata / authoring**
- `crates/runtara-dsl/src/agent_meta.rs` — add `VariantTypeMeta` (discriminator +
  per-variant fields + `widget`); the hand-written `DataSource` descriptor; surface
  it from `capability_to_api`.
- `crates/runtara-agent-macro/src/lib.rs` — `#[field(envelope = "DataSource")]`
  link from a `data: DataInput` field to the descriptor.
- frontend (`regen-frontend-api` + Step Picker) — format selector + **file picker**;
  `DataSource` workflow inputs become file drop-zones.
- `get_workflow_authoring_schema` source — add `dataSource` section + examples.
- Delete 8× `FileData` (Phase 5): the 6 agents above + `runtara-dsl/src/schema_types.rs`
  + `runtara-agents/src/types.rs` (re-export the shared type to preserve paths).

## Appendix B — example envelopes

```jsonc
// raw text (http body, hand-authored)
{ "format": "text", "text": "<root><name>Alice</name></root>" }
{ "format": "text", "text": "naïve", "encoding": "windows-1252" }

// file (UI file picker / "request a file from user" / storage download produce this)
{ "format": "file", "content": "PHJvb3QvPg==", "filename": "a.xml", "mime_type": "application/xml" }

// bare base64 (no file framing)
{ "format": "base64", "data": "PHJvb3QvPg==" }

// inline bytes
{ "format": "bytes", "bytes": [60, 114, 111, 111, 116, 47, 62] }

// legacy shapes still accepted on input (normalized):
"<root/>"                                  // → text  (NOT base64-guessed)
{ "content": "PHJvb3QvPg==" }              // → file  (legacy FileData)
[60, 114, 111, 111, 116, 47, 62]           // → bytes
```

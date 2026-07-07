# QuickBooks Agent — Thin Typed Layer Plan

Implementation-ready plan for a small set of **typed preset capabilities** layered on
top of the already-committed generic core in `crates/agents/runtara-agent-quickbooks/src/lib.rs`.

Status of the base agent (P2, committed): generic `query`, `read`, `create`, `update`,
`delete`, `report` capabilities plus the reusable helper set (`qbo_get`, `qbo_post`,
`require_connection`, `write_path`, `read_path`, `query_path`, `minor`, `entity_output`,
`extract_entity_object`, `extract_query`, `str_field`, `url_encode`). All HTTP, base-URL
pinning (`.../v3/company/{realmId}`), and Bearer-token injection happen in the proxy; the
component sends **relative paths only** and never sees host, realmId, or secrets.

---

## 1. Summary & principle

**Goal.** Add at most six *friendly, typed* capabilities that build the QuickBooks Online
(QBO) JSON body from named fields and delegate to the existing `qbo_post` / `qbo_get`
helpers. **No new HTTP code, no new routing, no new auth.**

Planned capabilities (5, with room to stop at the first four):

| fn name              | capability id (kebab) | verb        | QBO entity | side_effects |
|----------------------|-----------------------|-------------|------------|--------------|
| `create_invoice`     | `create-invoice`      | POST        | Invoice    | yes          |
| `upsert_customer`    | `upsert-customer`     | GET + POST  | Customer   | yes          |
| `record_payment`     | `record-payment`      | POST        | Payment    | yes          |
| `create_bill`        | `create-bill`         | POST        | Bill       | yes          |
| `get_customer`       | `get-customer`        | GET (query) | Customer   | no (optional)|

**Principle — thin presets, not per-entity completeness.**

- Each typed capability is a *body builder* over the generic core. It maps a small,
  ergonomic input struct to the QBO wire shape, calls `qbo_post(write_path(entity, mv), body)`
  (or `qbo_get`), and returns a flattened output. It must add **zero** new networking.
- We deliberately do **not** grow toward one-capability-per-entity. The generic
  `create` / `update` / `query` already cover the long tail (raw `body: Value`). Typed
  presets exist only for the highest-value, easiest-to-get-wrong flows where a raw JSON
  body is a footgun (nested `Line[]` / `LinkedTxn` / `Ref` objects, the upsert
  query-then-branch dance).

**When to prefer typed vs generic.**

- **Typed** when the caller thinks in domain terms ("invoice this customer for these line
  items") and the QBO body has fiddly nested/ref structure that is easy to get wrong.
- **Generic** (`create`/`update`/`query` with a raw `body`) when the caller already has a
  QBO-shaped object, needs a field the preset does not surface, or is touching an entity
  we did not preset. The typed layer never blocks the generic escape hatch.

---

## 1a. ⚠️ Input-metadata constraint (verified — governs the struct types below)

The `CapabilityInput` derive does **not** resolve nested custom types for *inputs*.
`InputFieldMeta` has **no** `nested_type_name` (only `OutputFieldMeta` does —
`crates/runtara-dsl/src/agent_meta.rs:186‑204`), and input type-conversion sends any
non-primitive through a catch-all: `Vec<InvoiceLine>` → **array of string**, and a bare
`BillAddr` struct → **string** (`agent_meta.rs:1026‑1055`). So a field typed as
`Vec<CustomStruct>` or `CustomStruct` renders as **array-of-string / string** in the Step
Picker — the sub-fields never surface. This is not a compile error (nothing looks the
sub-type up), so a naive typed model would silently ship an unusable form. Outputs don't
have this problem; the only input escape hatch is a hardcoded per-type JSON-Schema arm, as
used for `ConditionExpression` (`agent_meta.rs:1020‑1024`).

**Decision for this layer.** Declare the *repeated/nested* fields — `line_items`,
`apply_to`, `bill_addr` — as **`serde_json::Value`**, with the element shape written into
`#[field(description = ...)]`. Keep every **scalar** field (`customer_ref`, `total_amt`,
`txn_date`, `doc_number`, `match_by`, `currency`, …) as a proper typed field — those *do*
surface correctly. The per-capability tables below give the **logical mapping the body
builder applies**; the actual Rust struct types each nested container as `Value` and the
builder iterates/reads it.

The preset's value is unchanged: it still hides the verbose `Line[].SalesItemLineDetail` /
`LinkedTxn` / `DetailType` wrapping and the upsert query-then-branch dance — the caller
passes a compact `[{item_ref, amount, qty}]` array (documented in the field description),
not raw QBO line objects, and gets `id`/`sync_token` flattened back.

**Future option (out of scope):** fully-typed nested inputs with rich Step-Picker rendering
would be a *macro enhancement* — add `nested_type_name` to `InputFieldMeta` and resolve it
in `input_field_to_api` the way outputs already do. Prefer that over the per-type hardcoded
schema arm if this ever becomes a requirement.

---

## 2. Capabilities

For every capability below: `_connection` is the standard skipped field
(`#[field(skip)]` + `#[serde(default, skip_serializing_if = "Option::is_none")]` on
`_connection: Option<RawConnection>`), and `minor_version: Option<String>` is surfaced and
passed through `minor(&input.minor_version)` (defaults to `"75"`). Optional input fields are
`Option<T>` with `#[serde(default, skip_serializing_if = "Option::is_none")]`; when `None`
they are simply omitted from the QBO body (let QBO apply its own defaults).

All money fields are JSON numbers (`f64`), never strings. All ids are strings. All dates are
ISO `yyyy-MM-dd` strings passed through verbatim.

### 2.1 `create_invoice` → `create-invoice`

Builds an Invoice and POSTs it. Verified required QBO fields: `CustomerRef.value`, and a
non-empty `Line[]` where each item line carries `Amount`, `DetailType =
"SalesItemLineDetail"`, and `SalesItemLineDetail.ItemRef.value`. **`Amount` is NOT derived
from `Qty*UnitPrice` on create** — it must be sent.

**Typed input — `CreateInvoiceInput`:**

| field                     | type              | req | → QBO path                                       |
|---------------------------|-------------------|-----|--------------------------------------------------|
| `customer_ref`            | `String`          | yes | `CustomerRef.value`                              |
| `line_items`              | `Value` (array)   | yes | `Line[]` — a JSON array `[{item_ref, amount?, qty?, unit_price?, description?}]` (shape documented in `#[field(description)]`; see §1a); the builder wraps each into a `SalesItemLineDetail` line |
| `line_items[].item_ref`   | `String`          | yes | `Line[].SalesItemLineDetail.ItemRef.value`       |
| `line_items[].amount`     | `Option<f64>`     | *   | `Line[].Amount` (see rule)                        |
| `line_items[].qty`        | `Option<f64>`     | no  | `Line[].SalesItemLineDetail.Qty`                 |
| `line_items[].unit_price` | `Option<f64>`     | no  | `Line[].SalesItemLineDetail.UnitPrice`           |
| `line_items[].description`| `Option<String>`  | no  | `Line[].Description`                              |
| `txn_date`                | `Option<String>`  | no  | `TxnDate`                                         |
| `due_date`                | `Option<String>`  | no  | `DueDate`                                         |
| `currency`                | `Option<String>`  | no  | `CurrencyRef.value`                              |
| `doc_number`              | `Option<String>`  | no  | `DocNumber`                                       |
| `bill_email`              | `Option<String>`  | no  | `BillEmail.Address` (object, **not** a ref)       |
| `customer_memo`           | `Option<String>`  | no  | `CustomerMemo.value`                              |

\* `amount` rule: if `amount` is provided, use it. If omitted but both `qty` and
`unit_price` are present, compute `amount = qty * unit_price` in the body builder. If
omitted and we cannot compute, **fail input validation** (`AgentError`) before calling QBO —
QBO would otherwise reject or mis-total the line.

**Body it builds** (mirrors the verified `createBodyExample`):

```json
{
  "CustomerRef": { "value": "58" },
  "Line": [
    {
      "Amount": 150.00,
      "DetailType": "SalesItemLineDetail",
      "Description": "Strategy consulting - 3 hours",
      "SalesItemLineDetail": {
        "ItemRef": { "value": "1" },
        "Qty": 3,
        "UnitPrice": 50.00
      }
    }
  ],
  "TxnDate": "2026-07-07",
  "DueDate": "2026-07-21",
  "DocNumber": "INV-1001",
  "BillEmail": { "Address": "customer@example.com" },
  "CustomerMemo": { "value": "Thank you for your business" },
  "CurrencyRef": { "value": "USD" }
}
```

**Helper calls:**

```rust
let connection = require_connection(&input._connection)?;
let mv = minor(&input.minor_version);
let body = build_invoice_body(&input)?;                 // pure Value builder, unit-tested
let response = qbo_post(connection, &write_path("Invoice", &mv), body)?;
Ok(entity_output(&response, "Invoice"))                 // → { entity, id, sync_token, object }
```

**Output — reuse `EntityOutput`** (`entity`, `id`, `sync_token`, `object`). Callers read
`TotalAmt` / `Balance` / `DocNumber` / `DueDate` off `object`. (If we want them top-level we
would define `CreateInvoiceOutput`, but reusing `EntityOutput` keeps the layer thin and
needs no new `output_types` entry — recommended.)

**Edge cases.**

- `ItemRef.value` / `CustomerRef.value` must be real ids in that realm; a bad id returns a
  QBO validation fault surfaced as-is by `qbo_post`.
- `BillEmail` is `{ "Address": ... }`; sending a bare string fails.
- `CurrencyRef` only settable when multicurrency is enabled — leave it out unless the caller
  passes `currency`.
- Create response wraps as `{ "Invoice": {...}, "time": ... }`; `entity_output` unwraps it.
- Update/delete of an invoice stay on the **generic** `update`/`delete` capability
  (they need `Id` + current `SyncToken`; a stale token → error 5010).

### 2.2 `upsert_customer` → `upsert-customer`

QBO has **no native upsert**. This capability performs the query-first, then-branch pattern.
Only `DisplayName` is truly required to create a Customer, and it must be **unique across
Customers, Vendors, and Employees** (collision → fault **6240 "Duplicate Name Exists"**),
which is exactly what the query-first flow avoids.

**Typed input — `UpsertCustomerInput`:**

| field                    | type             | req | notes                                                        |
|--------------------------|------------------|-----|--------------------------------------------------------------|
| `match_by`               | enum `display_name` \| `email` \| `id` | no (default `display_name`) | lookup strategy |
| `match_value`            | `Option<String>` | *   | value to look up; defaults per strategy (see algorithm)      |
| `customer.display_name`  | `String`         | yes | `DisplayName` (also default `match_value` when `display_name`)|
| `customer.email`         | `Option<String>` | no  | `PrimaryEmailAddr.Address` (also `match_value` when `email`) |
| `customer.given_name`    | `Option<String>` | no  | `GivenName`                                                  |
| `customer.family_name`   | `Option<String>` | no  | `FamilyName`                                                 |
| `customer.company_name`  | `Option<String>` | no  | `CompanyName`                                                |
| `customer.phone`         | `Option<String>` | no  | `PrimaryPhone.FreeFormNumber`                                |
| `customer.bill_addr`     | `Value` (object) | no  | `BillAddr` — a JSON object `{line1, line2?, city?, state?, postal_code?, country?}` (shape in `#[field(description)]`; see §1a); mapped to `Line1/Line2/City/CountrySubDivisionCode/PostalCode/Country` |

\* `match_value`: required for `match_by=id`; for `display_name` defaults to
`customer.display_name`; for `email` defaults to `customer.email`.

`BillAddr` typed sub-struct → QBO `PhysicalAddress`:
`line1→Line1, line2→Line2, city→City, state→CountrySubDivisionCode, postal_code→PostalCode,
country→Country`.

**Upsert algorithm (explicit):**

1. **Build the lookup query** from `match_by`:
   - `display_name` → `SELECT * FROM Customer WHERE DisplayName = '<escaped match_value>'`
   - `id`           → `SELECT * FROM Customer WHERE Id = '<match_value>'`
   - `email`        → `SELECT * FROM Customer` (paged) then **client-side** filter on
     `PrimaryEmailAddr.Address` (email is generally **not** a filterable QBO field).
     Document as slow / small-book-only; prefer `display_name`.
2. **Run it** via `qbo_get(connection, &query_path(&query, &mv))` and unpack with
   `extract_query(&response)` → `(items, count, _)`.
3. **Branch:**
   - **Match found** (`count >= 1`, take the first row): read its `Id` and `SyncToken` via
     `str_field`. Build a **sparse update** body:
     `{ "Id": <id>, "SyncToken": <token>, "sparse": true, ...changed fields }` and
     `qbo_post(connection, &write_path("Customer", &mv), body)`. `sparse: true` is
     mandatory — a full update **replaces** the object and wipes unsent writable fields.
   - **No match** (`count == 0`): build a create body (no `Id`/`SyncToken`) and
     `qbo_post` the same `write_path("Customer", &mv)`.
4. **Return** the resulting customer flattened (see output), with `created` reflecting the
   branch taken.

**Query-string escaping (in the query builder helper):** wrap literals in single quotes;
escape an embedded apostrophe as `\'`. `query_path` already `url_encode`s the whole string,
so the builder produces the logical SQL and lets `query_path` handle URL encoding. QBO query
language allows only `AND` (no `OR`/`JOIN`/`GROUP BY`); we only ever emit single-predicate
equality, so this is safe.

**Body it builds** (create branch):

```json
{
  "DisplayName": "Acme Widgets LLC",
  "GivenName": "Jane",
  "FamilyName": "Doe",
  "CompanyName": "Acme Widgets LLC",
  "PrimaryEmailAddr": { "Address": "ap@acme.example" },
  "PrimaryPhone": { "FreeFormNumber": "+1 (415) 555-0142" },
  "BillAddr": {
    "Line1": "500 Market St",
    "City": "San Francisco",
    "CountrySubDivisionCode": "CA",
    "PostalCode": "94105",
    "Country": "USA"
  }
}
```

**Output — `UpsertCustomerOutput`** (a small custom struct is justified here because
`created`/`matched_by` are not on `EntityOutput`):

| field          | type             | source                                              |
|----------------|------------------|-----------------------------------------------------|
| `id`           | `String`         | resulting `Customer.Id`                             |
| `sync_token`   | `String`         | resulting `Customer.SyncToken`                      |
| `created`      | `bool`           | `true` if inserted, `false` if matched + updated    |
| `display_name` | `String`         | echoed `DisplayName`                                |
| `matched_by`   | `Option<String>` | strategy that located the record (`None` when created)|
| `object`       | `Value`          | full returned `Customer` for downstream steps       |

**Edge cases.**

- **6240** is a business fault (HTTP 400, `Fault.Error[].code == "6240"`); the query-first
  flow avoids it. If it still fires (race — two creators between query and POST), surface it;
  optionally re-query and retry once (see §6 risks).
- **5010 stale object**: the update branch must use the `SyncToken` from the *just-run
  query*; if stale, re-query and retry once.
- No hard delete for Customer — soft-delete is a sparse update `{Id, SyncToken, sparse:true,
  Active:false}` (out of scope here; note for callers).

### 2.3 `record_payment` → `record-payment`

Records a customer Payment, optionally applying it to invoices via `LinkedTxn`. Required:
`CustomerRef.value` and `TotalAmt`. **Payment lines are `{ Amount, LinkedTxn:[{TxnId,
TxnType}] }` — there is NO `DetailType` on payment lines** (unlike Invoice lines).

**Typed input — `RecordPaymentInput`:**

| field                     | type                | req | → QBO path                                                    |
|---------------------------|---------------------|-----|---------------------------------------------------------------|
| `customer_ref`            | `String`            | yes | `CustomerRef.value`                                           |
| `total_amt`               | `f64`               | yes | `TotalAmt`                                                    |
| `apply_to`                | `Value` (array)     | no  | JSON array `[{invoice_id, amount}]` (shape in `#[field(description)]`; see §1a); one `Line[]` element per entry; empty/absent → fully unapplied |
| `apply_to[].invoice_id`   | `String`            | yes*| `Line[].LinkedTxn[0].TxnId` (`TxnType` fixed = `"Invoice"`)   |
| `apply_to[].amount`       | `f64`               | yes*| `Line[].Amount`                                               |
| `payment_ref_num`         | `Option<String>`    | no  | `PaymentRefNum`                                               |
| `deposit_to_account_ref`  | `Option<String>`    | no  | `DepositToAccountRef.value` (omit → Undeposited Funds)        |
| `currency_ref`            | `Option<String>`    | no  | `CurrencyRef.value` (only when multicurrency + non-home)      |
| `txn_date`                | `Option<String>`    | no  | `TxnDate` (defaults to today)                                 |
| `private_note`            | `Option<String>`    | no  | `PrivateNote`                                                 |

\* required within each `apply_to` element.

**Body it builds:**

```json
{
  "CustomerRef": { "value": "20" },
  "TotalAmt": 100.00,
  "PaymentRefNum": "1234",
  "Line": [
    { "Amount": 100.00, "LinkedTxn": [ { "TxnId": "96", "TxnType": "Invoice" } ] }
  ]
}
```

**Helper calls:** `qbo_post(connection, &write_path("Payment", &mv), body)` →
`entity_output(&response, "Payment")` (reuse `EntityOutput`; callers read
`UnappliedAmt`/`Line`/`DepositToAccountRef` off `object`).

**Edge cases (enforce in the body builder / document):**

- `TotalAmt` must be `>=` sum of `apply_to[].amount`. Equal = fully applied
  (`UnappliedAmt == 0`); greater = partial (positive `UnappliedAmt`, a customer credit). We
  do **not** set `UnappliedAmt` — QBO computes it.
- A single `apply_to[].amount` greater than the invoice's open balance is rejected — apply
  at most the remaining balance per link.
- Empty/omitted `apply_to` → no `Line[]` → a pure unapplied (credit) payment.
- Each linked invoice's customer must match `CustomerRef` (cross-customer application is
  rejected).
- `TxnType` is always the literal `"Invoice"`; do not surface it as input.

### 2.4 `create_bill` → `create-bill`

Creates an AP Bill. Required: `VendorRef.value` and a non-empty `Line[]`, where each line is
either **account-based** (`AccountBasedExpenseLineDetail.AccountRef`) or **item-based**
(`ItemBasedExpenseLineDetail.ItemRef`), with `Amount` and the matching `DetailType`.

**Typed input — `CreateBillInput`:**

| field                     | type            | req | → QBO path                                                    |
|---------------------------|-----------------|-----|---------------------------------------------------------------|
| `vendor_ref`              | `String`        | yes | `VendorRef.value`                                             |
| `line_items`              | `Value` (array) | yes | JSON array `[{amount, account_ref?, item_ref?, description?}]` (non-empty; shape in `#[field(description)]`; see §1a); builder picks the detail type per line |
| `line_items[].amount`     | `f64`           | yes | `Line[].Amount`                                               |
| `line_items[].account_ref`| `Option<String>`| **†**| account-based: `AccountBasedExpenseLineDetail.AccountRef.value`|
| `line_items[].item_ref`   | `Option<String>`| **†**| item-based: `ItemBasedExpenseLineDetail.ItemRef.value`        |
| `line_items[].description`| `Option<String>`| no  | `Line[].Description`                                          |
| `txn_date`                | `Option<String>`| no  | `TxnDate`                                                     |
| `due_date`                | `Option<String>`| no  | `DueDate` (derived from terms if omitted)                    |
| `doc_number`              | `Option<String>`| no  | `DocNumber` (vendor's bill/reference number)                 |
| `currency`                | `Option<String>`| no  | `CurrencyRef.value` (only with multicurrency)                |

**† Detail-type selection rule (per line, enforced in builder):**

- `account_ref` set → **account-based** line (`DetailType =
  "AccountBasedExpenseLineDetail"`).
- else `item_ref` set → **item-based** line (`DetailType =
  "ItemBasedExpenseLineDetail"`).
- **both set → `account_ref` wins** (documented, deterministic).
- **neither set → fail input validation** before calling QBO.

**Body it builds** (account-based line):

```json
{
  "VendorRef": { "value": "56" },
  "Line": [
    {
      "DetailType": "AccountBasedExpenseLineDetail",
      "Amount": 100.00,
      "AccountBasedExpenseLineDetail": { "AccountRef": { "value": "7" } }
    }
  ]
}
```

**Helper calls:** `qbo_post(connection, &write_path("Bill", &mv), body)` →
`entity_output(&response, "Bill")` (reuse `EntityOutput`; `TotalAmt`/`Balance`/`DueDate`
readable off `object`).

**Edge cases.**

- The `<DetailType>` object key must match `DetailType` exactly; a stray key from the other
  variant → validation error. The builder only ever emits the one matching key.
- `TotalAmt`/`Balance` are system-computed — never send them (ignored if sent).
- `CurrencyRef` only meaningful with multicurrency; omit otherwise.
- Lines may mix account-based and item-based within one Bill.

### 2.5 `get_customer` → `get-customer` (optional)

Thin read-by-key over `query`, so callers can resolve a Customer id without hand-writing QBO
SQL. Skip if we want to stay at four capabilities — the generic `query` already covers it.

**Typed input — `GetCustomerInput`:** `match_by` (`display_name`|`email`|`id`) + `match_value`.

**Body/call:** builds the same lookup query as §2.2 step 1, calls
`qbo_get(connection, &query_path(&query, &mv))`, unpacks with `extract_query`.

**Output — `GetCustomerOutput`:** `found: bool`, `id: Option<String>`,
`sync_token: Option<String>`, `object: Option<Value>`. No side effects (**omit
`side_effects`** in the `#[capability]` attribute).

---

## 3. Shared helpers to add

Keep these tiny and pure (`Value`-in/`Value`-out) so they unit-test without a connection.
Per §1a, the builders read the nested `line_items` / `apply_to` / `bill_addr` fields as
`serde_json::Value` — read each element key defensively (a missing/mistyped required key →
a validation `AgentError` before calling QBO, never a panic). A `Value` input field renders
in the Step Picker as free-form (same as the generic `create`'s `body: Value`), with the
expected shape carried in the field description.

- **`ref_obj(value: &str) -> Value`** → `{ "value": <value> }`. The one QBO reference
  shape (`CustomerRef`, `ItemRef`, `AccountRef`, `VendorRef`, `DepositToAccountRef`,
  `CurrencyRef`, …). We never emit `name` on write (it is echo-only/ignored), which keeps the
  helper single-arg. (If a call site ever needs `name`, add `ref_obj_named(value, name)`
  rather than complicating the common path.)
- **`build_invoice_body(input: &CreateInvoiceInput) -> Result<Value, AgentError>`** —
  assembles `CustomerRef`, the `Line[]` (with the `Amount` compute/validate rule),
  `BillEmail`/`CustomerMemo` objects, and the optional scalars.
- **`build_payment_body(input: &RecordPaymentInput) -> Result<Value, AgentError>`** —
  assembles `CustomerRef`, `TotalAmt`, and one `Line` per `apply_to` entry
  (`{ Amount, LinkedTxn: [ ref-less { TxnId, TxnType:"Invoice" } ] }`); validates
  `TotalAmt >= sum(amounts)`.
- **`build_bill_body(input: &CreateBillInput) -> Result<Value, AgentError>`** — assembles
  `VendorRef` and per-line account-vs-item detail (with the selection rule + neither-set
  error).
- **`build_customer_body(input: &UpsertCustomerInput, existing: Option<(&str,&str)>) ->
  Value`** — when `existing = Some((id, sync_token))`, prepends `Id`, `SyncToken`,
  `"sparse": true`; otherwise a plain create body. Maps `BillAddr` fields.
- **`customer_lookup_query(match_by, match_value) -> String`** — logical QBO SQL with
  single-quote escaping (`\'`), consumed by `query_path` (which URL-encodes). Shared by
  `upsert_customer` and `get_customer`.

These reuse the existing helpers unchanged: `qbo_get`, `qbo_post`, `require_connection`,
`write_path`, `query_path`, `minor`, `entity_output`, `extract_query`, `str_field`,
`url_encode`. **No new HTTP is introduced anywhere.**

---

## 4. Wiring checklist (per capability)

All edits are in `crates/agents/runtara-agent-quickbooks/src/lib.rs`. The
`#[capability(...)]` macro auto-generates the executor and metadata statics; the WASM
plumbing (dispatcher + `agent_info()`) is hand-written and must be extended for each new
capability.

**Capability id is kebab-case, derived by the macro** as
`fn_name.replace('_', "-")` (`runtara-agent-macro/src/lib.rs:305`). So `fn create_invoice`
→ id `"create-invoice"`, and the **dispatcher match arm uses the kebab string**. (This
matches the committed pattern in `runtara-agent-compression`:
`"create-archive" => __executor_create_archive(value)`.)

For each new capability `foo_bar`:

1. **Define** `FooBarInput` (`derive(CapabilityInput)`), optionally `FooBarOutput`
   (`derive(CapabilityOutput)`) — or reuse `EntityOutput` — and
   `#[capability(module = "quickbooks", display_name = ..., description = ...,
   side_effects = true)] pub fn foo_bar(input: FooBarInput) -> Result<_, AgentError>`.
   (Omit `side_effects` for the read-only `get_customer`.)

2. **Dispatcher match arm** in `impl Guest for Component::invoke` (currently
   `lib.rs:813–819`), add:
   ```rust
   "foo-bar" => __executor_foo_bar(value),
   ```

3. **`agent_info()` caps array** (`lib.rs:706–713`), add:
   ```rust
   &__CAPABILITY_META_FOO_BAR,
   ```

4. **`agent_info()` `input_types`** (`lib.rs:715–724`), add:
   ```rust
   ("FooBarInput", &__INPUT_META_FooBarInput as &InputTypeMeta),
   ```

5. **`agent_info()` `output_types`** (`lib.rs:726–742`), add **only if a NEW output type**:
   ```rust
   ("FooBarOutput", &__OUTPUT_META_FooBarOutput as &OutputTypeMeta),
   ```
   Reusing `EntityOutput` (invoice/payment/bill) → **no** `output_types` edit needed.
   `UpsertCustomerOutput` and `GetCustomerOutput` are new → add them.

Concrete per-capability additions:

| capability        | match arm                                      | caps entry                        | input_types                          | new output_types                       |
|-------------------|------------------------------------------------|-----------------------------------|--------------------------------------|----------------------------------------|
| `create_invoice`  | `"create-invoice" => __executor_create_invoice`| `&__CAPABILITY_META_CREATE_INVOICE`| `("CreateInvoiceInput", …)`         | none (reuse `EntityOutput`)            |
| `upsert_customer` | `"upsert-customer" => __executor_upsert_customer`| `&__CAPABILITY_META_UPSERT_CUSTOMER`| `("UpsertCustomerInput", …)`      | `("UpsertCustomerOutput", …)`          |
| `record_payment`  | `"record-payment" => __executor_record_payment`| `&__CAPABILITY_META_RECORD_PAYMENT`| `("RecordPaymentInput", …)`         | none (reuse `EntityOutput`)            |
| `create_bill`     | `"create-bill" => __executor_create_bill`      | `&__CAPABILITY_META_CREATE_BILL`  | `("CreateBillInput", …)`             | none (reuse `EntityOutput`)            |
| `get_customer`    | `"get-customer" => __executor_get_customer`    | `&__CAPABILITY_META_GET_CUSTOMER` | `("GetCustomerInput", …)`            | `("GetCustomerOutput", …)`             |

`crates/runtara-agent-bundle-emit/src/main.rs` already registers
`("quickbooks", runtara_agent_quickbooks::agent_info())` — no edit there; `agent_info()`
auto-discovers the new `#[capability]` fns once the four/five edits above are in place.

---

## 5. Testing

**Host unit tests** (append to the `#[cfg(test)]` block, currently `lib.rs:~907–1009`). These
are pure and need no connection or network:

- `build_invoice_body`: (a) `amount` provided → echoed; (b) `amount` omitted but `qty` +
  `unit_price` present → computed `qty*unit_price`; (c) neither → `Err`; (d) `BillEmail` is
  `{ "Address": ... }` not a string; (e) `DetailType == "SalesItemLineDetail"` on every line.
- `build_payment_body`: (a) one `Line` per `apply_to` with `LinkedTxn[0].TxnType ==
  "Invoice"` and **no `DetailType`**; (b) empty `apply_to` → no `Line` key; (c)
  `sum(amounts) > total_amt` → `Err`.
- `build_bill_body`: (a) `account_ref` → `AccountBasedExpenseLineDetail` only; (b) `item_ref`
  → `ItemBasedExpenseLineDetail` only; (c) both → account wins; (d) neither → `Err`.
- `build_customer_body`: create branch has no `Id`/`SyncToken`/`sparse`; update branch has
  all three plus `"sparse": true`; `BillAddr.state` → `CountrySubDivisionCode`.
- `customer_lookup_query`: `display_name` → `... WHERE DisplayName = '...'`; embedded
  apostrophe escaped as `\'`; `id` → `WHERE Id = '...'`; verify it round-trips through
  `query_path` with `minorversion` pinned (extend the existing
  `query_path_encodes_sql_and_pins_minorversion` test pattern).
- `ref_obj`: emits exactly `{ "value": ... }` with no `name`.

Run: `cargo test -p runtara-agent-quickbooks`.

**Build + metadata verification:**

```bash
scripts/build-agent-components.sh
jq '.capabilities[] | select(.id | startswith("create-") or . == "upsert-customer")' \
  target/wasm32-wasip2/release/runtara_agent_quickbooks.meta.json
```

Confirm each new capability appears with linked input/output types and `side_effects`.
Also assert the **input metadata** matches §1a: the scalar fields (`customer_ref`,
`total_amt`, `txn_date`, `doc_number`, `match_by`, …) surface as typed fields, and the
nested containers (`line_items` / `apply_to` / `bill_addr`) surface as free-form `Value`
fields carrying the documented shape in their description — *not* silently absent. (This
catches a regression if someone later re-types a nested field as `Vec<CustomStruct>`, which
would render as array-of-string.)

**Live e2e (Intuit sandbox).** The create/upsert paths mutate real QBO data, so a genuine
run needs a real connection: an OAuth2 token + realmId configured on a QuickBooks connection,
against the Intuit **sandbox** company. As in P2, the component only ever emits relative
paths; the proxy pins `.../v3/company/{realmId}` and injects the Bearer token — the
fake-token path still reaches Intuit (and returns 401), so the e2e harness exercises routing
end-to-end even without valid creds. Follow the `e2e-verify` skill: compile a tiny workflow
that calls `create-invoice`/`upsert-customer`, register, execute, and assert the returned
`id`/`sync_token` (or the surfaced 401/validation fault) rather than trusting unit tests
alone.

---

## 6. Phasing & risks

**Suggested order (each ships independently once wired + tested):**

1. `create_invoice` — highest value, self-contained, reuses `EntityOutput`. Proves the
   preset pattern end to end.
2. `create_bill` — same single-POST shape as invoice; adds the account-vs-item selection
   rule (good second exercise of the body-builder pattern).
3. `record_payment` — introduces `LinkedTxn` and the `TotalAmt >= sum` guard.
4. `upsert_customer` — most complex (query-then-branch, sparse update, new output struct);
   do it last so the simpler presets are already proven.
5. `get_customer` — optional; only if callers need a typed lookup separate from `query`.

**Risks & mitigations.**

- **QBO `minorversion` drift.** All presets thread `minor()` (default `"75"`); the field
  shapes here are verified against current QBO. If Intuit bumps required minorversion, the
  default is the single place to change — keep it aligned with the generic core's default.
- **Ref `name` vs `value`.** `name` is echo-only and ignored on write; `ref_obj` emits only
  `value` to avoid callers thinking `name` is authoritative. A wrong `value` id is a QBO
  validation fault surfaced verbatim.
- **Sparse-update pitfalls.** A full (non-sparse) Customer update **replaces** the object and
  wipes unsent fields; the upsert update branch must always send `"sparse": true`. Covered by
  a `build_customer_body` unit test asserting the flag on the update branch.
- **Upsert race / stale token.** Between the lookup query and the POST another actor can
  create the same name (→ 6240) or bump the `SyncToken` (→ 5010). Mitigation: on 5010,
  re-query and retry the sparse update once; on 6240 in the create branch, re-query and fall
  through to the update branch once. Keep retries bounded (single retry) to avoid loops; rate
  limits (429/throttle) are handled by the proxy/backoff layer, not the preset.
- **Scope creep.** The temptation is to keep adding presets. Hold the line at ≤6; anything
  else stays on the generic `create`/`update`/`query` with a raw `body`.

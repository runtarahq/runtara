# connector_gen

Generates a workflow agent's capabilities from the vendor's OpenAPI spec.
[openapi-generator](https://openapi-generator.tech) parses the spec; our
templates in `templates/` render every model and operation into one file,
`src/generated.rs`, that uses the same `#[capability]`, `CapabilityInput` and
`CapabilityOutput` macros as hand-written agents.

This README covers the tool. Wiring an agent into the platform (connection
types, registrations, verification) is in
[docs/adding-an-agent.md](../../docs/adding-an-agent.md).

Run every command below from the repository root.

## Setup

The pip package bundles a JDK through the `jdk4py` extra, so no system Java is
needed. It needs Python 3.10 or newer; macOS's own `python3` is 3.9, so name a
newer interpreter explicitly (Homebrew's, for example):

```bash
/opt/homebrew/bin/python3.14 -m venv scripts/connector_gen/.venv
scripts/connector_gen/.venv/bin/pip install -r scripts/connector_gen/requirements.txt
```

The venv is git-ignored. `connector_gen.py` uses
`scripts/connector_gen/.venv/bin/openapi-generator-cli` when it exists,
otherwise `$OPENAPI_GENERATOR_CLI` or `openapi-generator-cli` on `PATH`.

## Walkthrough: Pipedrive

`crates/agents/runtara-agent-pipedrive` was produced with these steps.

**1. Pin the spec.** Find the vendor's OpenAPI 3 spec URL and its sha256:

```bash
curl -sSL https://developers.pipedrive.com/docs/api/v1/openapi-v2.yaml | shasum -a 256
```

**2. Pick the operations.** List the spec's operationIds and choose the ones
users need; a spec can have hundreds.

```bash
curl -sSL https://developers.pipedrive.com/docs/api/v1/openapi-v2.yaml | grep "operationId:"
```

**3. Create the crate.** Copy `crates/agents/runtara-agent-pipedrive/` to
`runtara-agent-<id>/`, keep `Cargo.toml` (renamed), `src/lib.rs` and
`src/kit.rs`, and write `connector.yaml`:

```yaml
agent:
  id: pipedrive                      # agent id and capability module
  name: Pipedrive
  description: Pipedrive CRM — deals.
  integration_id: pipedrive_api_token  # connection type the agent binds to (or a list)
  error_prefix: PIPEDRIVE            # error codes: PIPEDRIVE_RATE_LIMITED, …
  rate_limited: true                 # optional: the vendor limits request rates, so a step
                                     # of any capability is retried 5 times by default, not 3
spec:
  url: https://developers.pipedrive.com/docs/api/v1/openapi-v2.yaml
  sha256: 2b0fb0de3cad7a37158528034f85942e6b22a0a0649d9c811e0676e7c040955d
labels:                              # optional: field name -> label shown in the step editor
  org_id: Organization
operations:                          # operationIds to generate
  - getDeals
  - getDeal
  - addDeal
  - updateDeal
  - deleteDeal
```

**4. Fetch.** Downloads the spec, refuses to continue if its sha256 differs from
the pin, and keeps the listed operations plus every component they reference.
Writes `spec.slice.json` and `tests/fixtures/<operationId>.json` (the spec's own
2xx response examples).

```bash
scripts/connector_gen/.venv/bin/python scripts/connector_gen/connector_gen.py fetch crates/agents/runtara-agent-pipedrive
```

**5. Generate.** Renders `src/generated.rs`, formatted with the repository's
`rustfmt`. Offline and deterministic: the same pin, slice and templates always
give the same file.

```bash
scripts/connector_gen/.venv/bin/python scripts/connector_gen/connector_gen.py generate crates/agents/runtara-agent-pipedrive
```

For Pipedrive this gives five capabilities: `get-deals` and `get-deal` (reads;
inputs such as `id`, `include_fields`, `limit`, `cursor`), and `add-deal`,
`update-deal`, `delete-deal` (side effects; `add-deal` takes the deal's fields
`title`, `value`, `currency`, `person_id`, … as inputs). Outputs are typed, e.g.
`get-deal` returns `data` with the deal's fields.

**6. Register the agent** in `crates/runtara-agent-bundle-emit`: a path
dependency in `Cargo.toml` and an `agents()` entry in `src/main.rs`. Without it
`scripts/build-agent-components.sh` fails and the server will not boot.

```toml
runtara-agent-pipedrive = { path = "../agents/runtara-agent-pipedrive" }
```

```rust
("pipedrive", runtara_agent_pipedrive::agent_info()),
```

**7. Test and verify.** Add tests like `runtara-agent-pipedrive/tests/examples.rs`
(the response examples round-trip; the metadata lists the expected
capabilities and inputs), then:

```bash
cargo test -p runtara-agent-pipedrive
cargo clippy -p runtara-agent-pipedrive --all-targets -- -D warnings
cargo build -p runtara-agent-pipedrive --target wasm32-wasip2 --release
```

A step can only use the agent through a connection of its `integration_id`.
Pipedrive's is `pipedrive_api_token` (an API token sent as `x-api-token` to
`https://api.pipedrive.com`); a new vendor needs one too, see
[docs/adding-an-agent.md](../../docs/adding-an-agent.md#connection-type).

## Everyday changes

**Add operations.** Append operationIds to `connector.yaml`, e.g. Pipedrive
persons:

```yaml
  - getPersons
  - getPerson
  - addPerson
  - updatePerson
```

Run `fetch` and `generate` again and review `git diff`: new capabilities
`get-persons`, `get-person`, `add-person`, `update-person`. Capabilities are
grouped by the operation's (first) tag, then ordered by operationId, whatever
the order of the list.

**The vendor changed the spec.** `fetch` stops with
`… changed: sha256 is <new>, connector.yaml pins <old>`. Put the new hash in
`connector.yaml`, run `fetch` and `generate`, and review the diff of
`spec.slice.json` and `src/generated.rs`: it is exactly the vendor's change to
the operations you use.

**Before committing**, confirm everything derived from the spec is current:

```bash
scripts/connector_gen/.venv/bin/python scripts/connector_gen/connector_gen.py check crates/agents/runtara-agent-pipedrive
```

`check` takes several crates at once and fails when `src/generated.rs` is stale
or edited by hand (printing the diff), when `tests/fixtures` no longer match the
examples in `spec.slice.json`, or when `connector.yaml`'s operations or labels
are not in `spec.slice.json` (edited without running `fetch`). It does not
download the spec, so a hand edit to `spec.slice.json` itself shows only in the
diff under review; `fetch` re-derives it.

**A schema name collides with the generated code** (`Value`, `Result`, or
`<OperationId>Input`): `fetch` stops and asks for a rename in `connector.yaml`:

```yaml
model_names:
  Value: Amount
```

**The vendor wraps request bodies** in one object named after the resource,
as Zendesk does (`{"ticket": {"subject": …}}`). List the wrapper names, and
the wrapped fields become the step's inputs instead of one `ticket` object:

```yaml
body_envelopes:
  - ticket
  - user
```

`fetch` notes what it did per operation: other optional body fields beside the
envelope are dropped (a required one is an error), and an envelope that is one
of several shapes (`anyOf` of create and merge fields, say) offers the fields
of all of them, all optional. Shapes beside the envelope's own fields add
theirs, as they do on a body, and an envelope whose shapes cannot be flattened
stays one input. Each operation unwraps its own copy of a shared
`#/components/requestBodies` entry. A listed name that wraps no request body
is an error.

**The vendor publishes several specs** for one API, one per product, as UPS
does. List them under `specs:` instead of `spec:` and `operations:`, each
with a `name`, its own pin and its own operations:

```yaml
specs:
  - name: Tracking
    url: https://raw.githubusercontent.com/UPS-API/api-documentation/main/Tracking.yaml
    sha256: e4eb800e…
    operations:
      - getSingleTrackResponseUsingGET
  - name: Shipping
    url: https://raw.githubusercontent.com/UPS-API/api-documentation/main/Shipping.yaml
    sha256: d84b53e5…
    operations:
      - Shipment
```

`fetch` slices each spec and merges the slices into one `spec.slice.json`.
The specs must list the same `servers` (URLs and variable values; their
descriptions may differ, as Zoho's do), or first servers on one host whose
paths differ, as eBay's `https://api.ebay.com{basePath}` do with a base path
per API. Then each spec's path (its variables at their defaults) goes in front
of its own paths (`/sell/inventory/v1/offer`), and the merged server is the
host alone; a spec's other servers are dropped. Specs that list servers on
different hosts (PayPal's name the sandbox first in some, only production in
others) take `server`, the one URL their paths are relative to; the host a
request goes to is the connection's either way:

```yaml
server: https://api-m.paypal.com
```

No path and method may be in two of the specs. A schema that two specs define differently under one generated name
gets the spec's name in front in each of them (`ErrorResponse` becomes
`TrackingErrorResponse` and `ShippingErrorResponse`); one they define the same
way is kept once. So does any other component, such as a shared parameter
(Zoho's `Module` becomes `RecordsModule` and `SearchModule`), which the
generated code never shows. A `body_envelopes`, `connection_headers`,
`connection_parameters`, `pinned_path_parameters`, `omit_parameters` or
`read_operations` name has to match in one of the specs.

**The connection sends a header** every operation declares, such as UPS's
`transId` (unique per request) and `transactionSrc`. List it, and it is no
step input; the connection type's auth arm sends it instead:

```yaml
connection_headers:
  - transId
  - transactionSrc
```

**Every operation takes the credential as a parameter**, as Slack's `token`
is a header on its POST methods and a query parameter on its GET ones. List it,
and it is no step input in either place; the connection sends the credential
its own way (Slack's as a Bearer header):

```yaml
connection_parameters:
  - token
```

**The vendor publishes no spec.** Write one from the vendor's API reference, keep
it in the crate, and name it by `path` (relative to the crate) instead of `url`.
It is pinned the same way, so a change to it needs a new `sha256`:

```yaml
spec:
  path: openapi.yaml
  sha256: 2b0fb0de…
```

**An input cannot take the wire name.** The platform sets `connection_id` on every
call, so `fetch` refuses an input with that name; and a name like `status[]` reads
badly in the step editor. In the spec, give the parameter or body field its input
name, which is also the generated Rust field's. The request still sends the wire
name:

```yaml
parameters:
  - name: status[]
    in: query
    x-runtara-input-name: statuses
    schema: {type: array, items: {type: string}}
```

**A query parameter's keys are the caller's to choose**, such as a filter on custom
fields. Declare a `deepObject` with `additionalProperties`. It stays one input, an
object sent as `filter[Colour]=red` and `filter[Size][0]=S`. Its generated code
calls `kit.rs`'s `query_object`, which `runtara-agent-pipedrive`'s copy has; an
agent with an older copy needs those methods added.

```yaml
parameters:
  - name: filter
    in: query
    style: deepObject
    schema: {type: object, additionalProperties: true}
```

**The API version is a path parameter**, such as UPS's
`/shipments/{version}/ship`. Pin it, and `fetch` writes the spec's default into
the path (`/shipments/v2409/ship`, the version the generated types describe)
and drops the input. A pinned parameter without a default is an error.

```yaml
pinned_path_parameters:
  - version
```

**Every path starts with the account**, as BigCommerce's start with
`/stores/{store_hash}`. Name that start, and `fetch` cuts it from every path
(`/v2/orders`) and drops the path parameters it names; the connection type's
base URL ends in it with the account's value
(`https://api.bigcommerce.com/stores/<hash>`), and the proxy keeps every
request under it. A listed operation whose path does not start with it is an
error.

```yaml
connection_path_prefix: /stores/{store_hash}
```

**The spec requires a field the API does not.** UPS's label recovery spec
requires `TrackingNumbers` and `ReferenceValues`, while the API needs one of
several keys. Name the schema (as it is in `spec.slice.json`) and the fields;
they become optional inputs:

```yaml
optional_fields:
  LabelRecoveryRequest:
    - TrackingNumbers
    - ReferenceValues
```

**The spec types an input wrongly.** Slack's spec types some message
timestamps (`ts`) as numbers; sent as one, `1712345678.000100` becomes
`1712345678.0001`, another message's id. Give the input name the type (or the
whole schema) it should have; it applies to that parameter or request-body
field in every listed operation, keeping its description unless the schema
gives one. A name no listed operation takes is an error.

```yaml
input_types:
  ts: string
  thread_ts: string
```

**The spec types a field inside a schema wrongly**, out of `input_types`'
reach, such as a field of the records in a request body's array. Zoho's spec
types a record's `$append_values` as a string; its API takes an object of
multi-select picklist fields, each mapped to `true` or `false`. Name the schema
(as it is in `spec.slice.json`) and give each property the type or schema it
should have, as for `input_types`. A schema or property the slice lacks is an
error.

```yaml
field_types:
  RecordPutWriteDataItem:
    $append_values:
      type: object
      additionalProperties: {type: boolean}
```

A schema written inside another, with no name of its own, is named by the
path to it from a component: a property at each step, but for `items` where
the schema there is an array, which steps into its items (a property called
`items` is reached where the schema is no array). A key that is a component's
whole name, dots and all, names that component. WooCommerce's schema types the
`id` of a line item's image an integer, and the store sends 37, `"50"` or 0;
a schema without a type takes any value:

```yaml
field_types:
  shop_order.line_items.items.image:
    id: {description: "The image's ID, a number or a string."}
```

**The spec names a type in a title.** BigCommerce's specs write a number as
`type: string` with the title `float` (a product's `price`, a transaction's
`amount`), and its API sends and takes numbers there. Give the title the type
or schema it means; every string schema with that title gets it, and loses
the title. A title no string schema has is an error.

```yaml
title_types:
  float: {type: number, format: double}
```

**The API reads a list in the query comma-separated**, as BigCommerce reads
`id:in=4,5,6`, though the spec leaves its array parameters at OpenAPI's
default (`id:in=4&id:in=5`). Every array query parameter in the `form` style
is then sent `explode: false`:

```yaml
comma_separated_query_arrays: true
```

**The descriptions are HTML**, as eBay's are (`<br>`, `<strong>`, links to
its own site). With `html_descriptions`, `fetch` writes every description and
summary as the plain text the step editor shows: line breaks for `<br>`,
paragraphs and list items (`- `), backticks for `<code>`, a link's text
without its URL, entities decoded, and a `<pre>` block laid out as it is. A
`<` that starts no HTML element (a `<start>` placeholder) stays text.
Examples, defaults and enums keep their text.

```yaml
html_descriptions: true
```

**No step input can carry a parameter**, such as Zoho's `filters`, a nested
filter object `fetch` refuses. List it, and the operations that take it are
generated without it; another operation (a search, a query) covers the need:

```yaml
omit_parameters:
  - filters
  - cross_filters
```

**The vendor's operationIds are not names**, as Magento's are derived from the
path (`GetV1OrdersId`) and it writes no summaries, so the steps would read
"Get V1 Orders ID". Name each one; `getOrder` is the capability `get-order`,
labelled "Get Order" unless the vendor gives a summary. `operations:` and every
other option keep naming the vendor's ids, as the spec does:

```yaml
operation_names:
  GetV1OrdersId: getOrder
  GetV1Orders: searchOrders
```

Where the vendor's description says something else (Magento's update of a
product reads "Create product"), is written for its own developers, or is
missing, the entry gives the capability's description too:

```yaml
operation_names:
  PutV1ProductsSku:
    name: updateProduct
    description: Updates the product that has `current_sku` with the fields given.
```

Where the vendor's summary is a sentence of its reference, as Shopware's are
("Create a new Product resources."), the entry gives the step's label, which
then wins over the summary:

```yaml
operation_names:
  searchProduct:
    name: searchProducts
    label: Search Products
```

**One object is declared as a query parameter per key**, at fixed indices, as
Magento's search criteria are (`searchCriteria[filterGroups][0][filters][0][field]`,
`searchCriteria[pageSize]`, …), which allow one filter and one sort order. Name
the object, and its parameters become one free-form `deepObject` input under
its name (`searchCriteria`), sent with any keys and indices by `kit.rs`'s
`query_object`; give its description, and `required` when the API refuses a
request without it though the spec leaves every key optional. Nested keys need
a `query_object` that recurses, as `runtara-agent-magento`'s does:

```yaml
query_objects:
  searchCriteria:
    required: true
    description: Magento's search criteria, as its REST API documents them …
```

**An operation only reads, though its method is not GET**, such as a query sent
as a POST (Zoho's COQL). List it, and the step is not marked as having side
effects:

```yaml
read_operations:
  - executeCOQLQuery
```

**Records refer to one another, and back**, as Shopware's entities do: an
order has its deliveries, each delivery its order, and every record a dozen
others. Typed, they are infinite (`E0072`), and hundreds of types. With
`opaque_records`, a property of one schema that refers to another record (a
component schema with an `id` of its own), directly, as the one part of an
`allOf`, as the one part of an `anyOf` or `oneOf` beside `{type: "null"}` (a
nullable reference, which stays nullable), or as an array's items or a map's
values, takes any JSON instead.
What an operation refers to itself (a search's `data: [Order]`, a create's
body) stays typed, and so does a schema without an `id` (a price), whatever it
refers to. A record that nothing typed refers to any more is not sliced.

```yaml
opaque_records: true
```

**The body repeats a path parameter**, as Shopware's `PATCH /product/{id}`
takes the product's `id` in its path and in its body, and keeps the path's.
Without an option the two inputs clash (`two inputs are named id`). With
`path_parameters_win`, the body field is no input and is not sent; the path
parameter is the input. The operation gets its own copy of the body schema
without the field, the component `<OperationId>Body` (`UpdateProductBody`),
because the schema it copies may be the body of an operation without that path
parameter, a create whose `id` is an input. `optional_fields` names the copy
to make an update's fields optional.

```yaml
path_parameters_win: true
```

**The spec is OpenAPI 3.2**, as Shopware's is. openapi-generator reads no 3.2
document, so `generate` hands it the slice labelled 3.1; the slice keeps the
vendor's version. `fetch` refuses, naming them, what 3.2 added that the listed
operations use (`itemSchema`, `in: querystring`, `dataValue`, …); what they do
not use (a tag's `parent`, the document's `$self`) does not matter.

**The agent accepts several connection types** for the same API (an OAuth
client and an API token, say): list them all.

```yaml
agent:
  integration_id:
    - hubspot_private_app
    - hubspot_access_token
```

## WordPress REST APIs

WordPress publishes no OpenAPI, but every site describes its own REST API, and
a plugin's (WooCommerce's `wc/v3`) the same way: `GET
/wp-json/<namespace>?context=help` answers each route's regex path, the
methods of its endpoints with their arguments, and the JSON schema of what it
answers. Save that from a site that runs the plugin's version you want, keep it
in the crate, and name it with `format: wordpress`:

```yaml
spec:
  path: woocommerce-11.3.1-schema.json
  sha256: 15fdf88a…
  format: wordpress
```

```bash
curl -sS 'https://shop.example.com/wp-json/wc/v3?context=help' | python3 -m json.tool --indent 2 --no-ensure-ascii > woocommerce-11.3.1-schema.json
```

`fetch` turns it into OpenAPI 3.1 (`wordpress_source.py`) before slicing:

- `(?P<id>[\d]+)` in a route is the path parameter `{id}`: an integer when its
  pattern is digits only, a string otherwise. A route with a group that has no
  name is skipped, with a note.
- A GET's or DELETE's arguments are query parameters, a POST's or PUT's a JSON
  body; an argument's `required: true` is the body's `required`. Of POST, PUT
  and PATCH, which WordPress lists for an update, only PUT is generated (POST
  for a route ending in `/batch`).
- WordPress's types are JSON Schema's: `date-time` is a string of that format,
  `bool` boolean, `mixed` and a union of several types any value, `[T, "null"]`
  nullable. `context` and callbacks are dropped, `readonly` is `readOnly`, and
  a `default` or an `enum` value that does not fit its type is dropped.
- A request body has no defaults: some are the site's own settings
  (WooCommerce's order `currency` is the store's), and the site applies them to
  a field left out anyway.
- Each route's schema is a component named by its `title` (`shop_order`). A GET
  of a route that does not end in a path parameter answers an array of it, and
  a POST to one answers 201.
- operationIds come from the path: `/orders` gives `listOrders` and
  `createOrder`; `/orders/{id}` gives `getOrder`, `updateOrder` and
  `deleteOrder`; `/orders/{order_id}/refunds` gives `listOrderRefunds` and
  `createOrderRefund`; `/orders/batch` gives `batchOrders`. Of two that would
  share a name, one with path parameters is named after them too
  (`batchSettingsByGroupId`). WordPress writes no summaries: give the
  capabilities' descriptions in `operation_names`.
- The server is `/wp-json/<namespace>`: requests go to the connection's base
  URL, the site's address, followed by it.

What a route answers otherwise than its schema says (a schema bug of the
plugin's) is fixed with `field_types`; an argument the site takes but does not
declare is added to the saved index by hand, and connector.yaml says so, as
`runtara-agent-woocommerce`'s does for a refund's `line_items`.

## GraphQL APIs

A GraphQL API (Shopify's Admin API, Braintree, Wave) publishes a schema, not
operations: it has types and top-level fields, and what a request asks for is
up to the caller. So each capability is an operation we write, one file per
operation in `operations/`, named as the operation. For Shopify's Admin API
(the whole agent is `crates/agents/runtara-agent-shopify`):

```yaml
agent:
  id: shopify
  name: Shopify
  description: Shopify Admin GraphQL API — products.
  integration_id: [shopify_access_token, shopify_client_credentials]
  error_prefix: SHOPIFY
graphql:
  schema:
    introspect: https://shopify.dev/admin-graphql-direct-proxy/2026-10
    sha256: 81ea7bbb…
  path: /admin/api/2026-10/graphql.json  # the endpoint, relative to the connection
  user_errors: userErrors                # optional, see below
  scalars:                               # the vendor's own scalars
    DateTime: {type: string, format: date-time}
    Money: string
operations:
  - getProduct      # operations/getProduct.graphql
  - listProducts
  - createProduct
```

```graphql
mutation createProduct($title: String!, $vendor: String, $status: ProductStatus) {
  productCreate(product: {title: $title, vendor: $vendor, status: $status}) {
    product { id title status }
    userErrors { field message }
  }
}
```

`fetch` loads the schema and turns each operation into an OpenAPI operation,
which is then sliced, labelled and rendered like any other:

- **The schema** comes from `introspect:` (an endpoint it sends the
  introspection query to), `url:` (an SDL file or introspection result to
  download) or `path:` (one in the crate). An introspection result's order is
  the server's, so the pin is the sha256 of the schema printed as sorted SDL;
  a change fails `fetch` like a changed spec. Introspection asks for
  deprecated arguments and input fields too, which a server otherwise leaves
  out, so the schema (and its pin) is the same whichever way it is read. The
  schema is taken as valid: a vendor's can break rules newer than it
  (Shopify's deprecates fields of types whose interface does not), and only
  the operations are checked against it.
- **An operation** must be valid against the schema, be the file's only
  operation (fragments may sit beside it), carry the file's name, and select
  exactly one of the API's top-level fields (not `__type` or `__schema`).
  Subscriptions are refused. `fetch` notes each deprecated field, argument and
  input field an operation uses.
- **The step's inputs are its variables**, labelled after the variable and
  described by the argument or input field it is passed to. A variable
  without `!`, or with a default, is optional; the default is the input's.
  Putting variables inside an input object (`product: {title: $title}`) gives
  the step one input per field, where a variable of the input object's type
  would be one object input. An unset input is not sent, so the operation's
  default applies (or, for an input object's field, the vendor leaves the
  value as it is). An input whose type is not non-null (no `!`), an input
  object's field of such a type and a list's item may also be set to `null`,
  which is sent as `null`: it overrides the default, or clears the value.
  These are `Option<Option<T>>` (`kit::nullable`), as a nullable body field
  of an OpenAPI operation is. A field the vendor wants sent even when it is
  `null` cannot be guaranteed this way: an unset item field is left out.
  Shopify's `changeFromQuantity`, in every inventory quantity it sets or
  adjusts, is one; its agent sends those two mutations from hand-written
  capabilities with documents of their own (see below).
- **The output** is what the top-level field returns, built from the fields
  the operation selects; aliases, inline fragments and fragment spreads are
  merged, each fragment's fields read from its own type (an `owner` that is a
  `User` in one fragment and a `Customer` in another is one object with the
  fields of both). The returned object is `<Operation>Result`, and the objects
  inside it are named after the operation and their response keys
  (`ListProductsPageInfo`, `GetProductVariantsNodes`). A list whose items the
  schema does not declare non-null (`[Node]`, as Shopify's `nodes(ids:)`
  returns, with `null` for an unknown id) is a `Vec<Option<T>>`.
- **Types:** `ID` and `String` are strings, `Int` an `i64`, `Float` an `f64`,
  `Boolean` a `bool`, and an enum a string listing its values. Every other
  scalar is the vendor's and needs a type in `graphql.scalars` (`string`, or a
  schema); `fetch` names the ones missing.
- **Queries are reads; mutations have side effects.**
- **`user_errors`** names the field in which the vendor's mutations report
  input they refused (Shopify's `userErrors`). A mutation whose result has
  that field must select it so that it is always in the answer: directly or
  through a fragment, not aliased, and with no `@skip` or `@include` on it or
  on a fragment around it. Otherwise a refused write could read as done.
- Descriptions are Markdown: links read as their text, and the lines of a
  paragraph are joined. Code, fenced or indented, is kept as written.

The generated request is a POST to `graphql.path` of `{"query": …,
"variables": …}`, which the crate's `kit.rs` builds:

```rust
kit::Request::new("POST", path).graphql(
    DOCUMENT,        // the operation as printed
    "productCreate", // the top-level field's response key
    false,           // a query (true) or a mutation (false)
    variables,       // a kit::Body of the set inputs
    "SHOPIFY",       // the error prefix
)
```

So a GraphQL agent's kit has `Request::graphql` and `nullable`, and its
`send` reads the answer, which a GraphQL API sends with HTTP 200 whether it
worked or not: `errors` (a rate limit among them, as Shopify's `THROTTLED`)
and the mutation's `user_errors` field. How each maps to an error is the
vendor's to define. A query can be sent again when it gets no answer; a
mutation, like any other write, is retried only if sending it twice changes
nothing.

**A `null` top-level field is an error, not a result.** `product(id:)` answers
`null` for an id it does not know, and a nullable mutation payload can be
`null` too. The kit reports that as an error: nothing found
(`<PREFIX>_NOT_FOUND`) for a query, and an unknown outcome for a mutation,
which may or may not have been carried out. So the capability returns the
field's non-null type (`GetProductResult`, not `Option<GetProductResult>`),
and a workflow branches on the error rather than checking every step's output
for `null`. Items of a list are different: a `null` item is part of the
answer (`nodes(ids:)` keeps a place for each id), so they are
`Option<T>`.

`check` also fails when an operation file no longer matches the operation in
`spec.slice.json`, or `graphql.path` (where an API version usually is) no
longer matches its endpoint: edited without `fetch`. Layout and comments do
not count.

## Hand-written capabilities

A generated capability is one API call. Work that takes several in one step
(read every page of a list, look an item up and then change it, send a file
in parts) is a capability written by hand in the crate's `src/custom.rs`,
which the agent carries beside the generated ones. List it in
`connector.yaml` with its input struct and every output struct `custom.rs`
declares that its output uses, nested ones included (a generated model it
returns is registered already). An output struct left out compiles, but the
step editor then shows that output without its fields.

```yaml
custom_capabilities:
  - function: list_all_products
    input: ListAllProductsInput
    outputs: [ListAllProductsOutput]
```

`generate` adds each one to the agent's metadata (`agent_info()`) and to its
component export (`agent_component!`), so it shows in the step editor and runs
like a generated capability. In `custom.rs`:

- Use the same macros as the generated code: `#[capability]` with
  `module = "<agent id>"`, `CapabilityInput` (with the `_connection` field the
  generated inputs have) and `CapabilityOutput`. The capability takes an input
  struct of its own, declared in `custom.rs`, not a generated one.
- Give every struct in `custom.rs` a name no generated type has. The agent's
  metadata looks input and output types up by name, so a `ListProductsInput`
  or `Product` of your own would replace the generated one's fields in the
  step editor; `generate` refuses an input or output listed under a generated
  type's name.
- The capability's id is its function's name with `-` for `_`; set no `id` of
  your own. `generate` refuses a function whose id a generated operation
  already has, and `agent_component!` refuses two capabilities with one id
  when the crate compiles.
- A hand-written capability cannot suspend (`suspends = true`) or be
  `trusted`: the generated agent declares neither.
- Make calls through the generated operations' requests and the crate's
  `kit.rs`, so errors read the same as everywhere else in the agent:
  `ListProductsInput { first: Some(Some(250)), … }.request()?.send(connection, "SHOPIFY").await?`.
  Where no generated request fits (a field the vendor wants sent even when
  `null`), build one with `kit::Request::new(…).graphql(DOCUMENT, …)`; `fetch`
  does not see that document, so check it against the schema yourself and
  test the request it builds.
- To name fields on the wire as the generated types do (a GraphQL API's
  camelCase), use `#[serde(rename_all = "camelCase")]`, which the metadata
  follows, and keep the connection's key with
  `#[serde(rename = "_connection")]`: camelCase would make it `connection`.
- Declare the module in `src/lib.rs`: `pub mod custom;` and `pub use custom::*;`.

## A generated agent crate

```
crates/agents/runtara-agent-<id>/
  connector.yaml      # spec source and operation allowlist (hand-written)
  operations/         # a GraphQL API's operations, one .graphql file each (hand-written)
  spec.slice.json     # the spec trimmed to the listed operations (fetched)
  tests/fixtures/     # the spec's 2xx response examples (fetched)
  tests/*.rs          # hand-written tests
  src/generated.rs    # rendered by `generate`; never edit by hand
  src/kit.rs          # request builder and error envelope the generated code calls
  src/custom.rs       # hand-written capabilities, if `custom_capabilities` lists any
  src/lib.rs          # hand-written: `pub mod kit; mod generated; pub use generated::*;`
                      # (and `pub mod custom; pub use custom::*;` with custom.rs)
```

## How the templates map the spec

- Each operation becomes a `<OperationId>Input` struct and an async
  `#[capability]` function. Path, query and header parameters are inputs, and
  so is every writable field of an object request body: the step takes `title`,
  `value`, … rather than one JSON `body` (that is what the generic HTTP agent is
  for). The function rebuilds the body from those fields. A body composed with
  `allOf` is merged into one object first, and a `oneOf`/`anyOf` of objects
  (Shippo's label purchase: from a rate, or instant) offers the fields of every
  shape, optional unless every shape requires them, for the vendor to validate;
  `fetch` warns when shapes disagree on a field. Shapes beside declared fields
  add theirs to those, and leave the declared ones as they are. A body that is
  not an object, is composed of anything but objects, or requires a field it
  does not declare stays a single `body` input; an array of models (PayPal's
  JSON Patch list) is a `Vec` of their structs. Non-GET operations are marked
  `side_effects`, except those listed in `read_operations`.
- A field that is an `allOf` of a scalar schema and parts that only describe
  or constrain it (PayPal's `cancel_url`: a `url` string plus `format: uri`)
  is that scalar. Schema keywords a spec writes on a parameter beside its
  `schema` (`enum`, `format`, …) go into the schema.
- A model is a struct named after its schema in PascalCase; an inline schema is
  named after its operation, acronyms kept (`executeCOQLQuery`'s response is
  `ExecuteCOQLQuery200Response`). A field takes its property's name in snake
  case, with runs of `_` made one (Zoho's `status_reason__s` is the field
  `status_reason_s`); the wire name stays.
- An exploded array query parameter (the OpenAPI default) is sent as
  `ids=1&ids=2`; with `explode: false` it is sent as `ids=1,2`. Items typed as
  an object with no declared fields (Jira's `StringList`) are sent as strings.
  Optional cookie parameters are dropped by `fetch`.
- An object query parameter is one input per field, named as its style sends
  it: with `style: deepObject`, `page` with `size` and `after` becomes
  `page[size]` and `page[after]` (Rust fields `page_size`, `page_after`); with
  the default `style: form`, the fields go unprefixed (`size`, `after`). Of a
  `oneOf` such as Zendesk's "page number or cursor", the object's fields are
  inputs and the scalar alternative stays an input under the parameter's own
  name (`page`).
- A parameter whose schema is a `oneOf`/`anyOf` of scalars is a string input.
- An optional body field the spec marks `nullable` is an `Option<Option<T>>`
  (deserialized with `kit::nullable`): omitted is not sent, an explicit `null`
  is sent as `null` (to clear the vendor's value), and a value is sent as is.
  So is a nullable field of a model, as in a body that is an array of records
  (BigCommerce's batch of variants, where `price: null` falls back to the
  product's price); a model read from a response keeps a `null` the same way.
  openapi-generator ignores `nullable` in a 3.1 document, so a 3.1 slice is
  rendered with each nullable field that has a `type` written as
  `type: [<type>, "null"]`; one that is only a `$ref` or an `allOf` has no
  type to list and stays an `Option<T>` there. Where the vendor's spec does
  not mark a field nullable that takes `null`, mark it with `field_types`
  (`nullable: true`).
- `<OperationId>Input::request()` builds the exact request without sending it,
  so tests can check the method, path, query and body; the capability function
  only adds the connection.
- The capability returns the full response type: `Vec<T>` for an array,
  `HashMap<String, T>` for a map, the primitive or model otherwise, and `Value`
  when the operation declares no JSON response. A 204 (No Content) declares
  none, whatever schema the spec gives it: eBay's give theirs the error
  schema. At runtime an empty success body (e.g. 204) decodes as `null`, and a
  non-JSON one as its text.
- Enums keep their wire type (`String`, `i64`) in inputs and outputs, so a value
  the spec does not list passes through unchanged; the allowed values are listed
  in the field's description. A `$ref` to an enum schema is inlined by `fetch`,
  as is one to an enum written as an `allOf` of a scalar and its values
  (Shippo's `CustomsDeclarationIncotermEnum`).
- `integer` without a `format` is generated as `i64` (`fetch` adds
  `format: int64`), so large ids and amounts fit.
- `number` is generated as `f64`, `format: float` too: as an `f32`, Shopware's
  amount 2396.8 would be read as 2396.800048828125.
- Every input and output field gets a label for the step editor, chosen by
  `fetch` in this order: the `labels:` override in `connector.yaml`; the
  vendor's `title` when it is a phrase (one-word titles name models); the field
  name humanized (`person_id`, `personId` → "Person ID", with known acronyms
  such as ID, URL, HTML, VAT kept upper-case). openapi-generator drops keys
  beside a `$ref`, so `fetch` writes a field that is a bare `$ref` as `allOf`
  of that one `$ref`: the same type, with its label. Scalar `example`s become the field's placeholder and scalar
  `default`s the default value of optional inputs. Descriptions, summaries and
  the agent's name are printed as written; an operation without a summary is
  named after its operationId.
- Each schema becomes a `CapabilityOutput` struct. Every field is an `Option`,
  and a flattened `__extra` map (hidden from the metadata) keeps fields the
  vendor returns but does not declare.
- Type names are written bare (no `models::` prefix) so the output macro can
  resolve nested types; the `pascalcase` lambda turns raw inline schema names
  into the generated struct names. The Rust generator spells runs of capitals
  differently (`EmailCCObject` → `EmailCcObject`), so `fetch` renames such
  schemas in the slice to the generated spelling.
- `fetch` also rewrites what openapi-generator would render wrongly: examples
  given as `$ref`s to `#/components/examples` are inlined,
  `additionalProperties: true` beside declared fields is dropped (`__extra`
  keeps undeclared fields anyway), a `required` written on a field instead of
  a list on its object is dropped, and a one-word `title` naming an inline
  object is written in the generated struct's spelling (`POD` → `Pod`).
  A title with spaces names the inline object too; a word in it spelled with a
  run of capitals is respelled the same way (Slack's "Conversation IM Channel
  object" → "Conversation Im Channel object"), and the phrase as written stays
  the field's label.
- JSON Schema forms OpenAPI 3.0 lacks are rewritten by `fetch`: `items: [A, B]`
  (Slack's "A or B") becomes `anyOf: [A, B]`, `type: [string, "null"]` a
  nullable string, a list of several types an `anyOf` (each alternative with
  the keywords of its type, `items` with `array`), `type: "null"`, alone
  or as an alternative, `nullable`, and `const: X` the enum `[X]` (a boolean's
  `const` is dropped).
- Tabs after a value (`default: <tab>`) are valid YAML 1.2 that PyYAML cannot
  read; `fetch` drops trailing whitespace before parsing.
- A Swagger 2.0 spec (Slack's) is converted to OpenAPI 3 before it is sliced:
  `host`, `basePath` and `schemes` become `servers`; `definitions` become
  `components.schemas`; a `body` parameter becomes the request body; `formData`
  parameters become an object request body offered in every media type the
  operation `consumes`, so an API that also takes JSON is sent JSON; an array
  query parameter is comma-separated unless `collectionFormat: multi`; a
  response's `schema` and `examples` become its `content`; and a parameter's
  vendor extensions (`x-runtara-input-name`) stay on it.
- An operation that declares no 2xx response (Magento's save of source items
  declares only 400, 401 and `default`) is given a `200` without a body, so its
  output is `Value`; openapi-generator would otherwise take `default`'s, an
  error's shape. `fetch` notes each one: a spec that declares its success only
  as `default` would lose its output's type the same way.
- `fetch` marks operations with `x-runtara-*` vendor extensions (side effects,
  summaries, which capability carries the agent's module attributes) because
  the templates cannot compute them. An operation with several tags keeps its
  first, so it is rendered once.
- `oneOf` and `anyOf` schemas are kept as raw JSON (`Value`).

## Limits

- OpenAPI 3.0, 3.1 (its features such as `type: "null"` are rewritten where
  listed above) and 3.2 (where the listed operations use nothing 3.2 added),
  Swagger 2.0, GraphQL schemas with operations of ours, and WordPress REST
  namespaces as a site describes them.
- A GraphQL operation selects one top-level field and is no subscription. The
  objects it returns are typed from what it selects, with every field
  optional. A top-level field that answers `null` is an error the kit reports,
  never a `null` result.
- `fetch` refuses, naming the operation, what the templates cannot send yet:
  request bodies that are not JSON (form-encoded, multipart), object query
  parameters without declared scalar fields (unless free-form: a `deepObject`
  with `additionalProperties`) or in a style other than `deepObject` or exploded
  `form`, array query parameters of objects or in a
  style other than `form`, and required cookie parameters. A Swagger 2.0 operation that
  `consumes` only form data is such a form body.
- An input typed by a schema (a ticket's `comment`, say) is described to the step
  editor as `any`, without its fields, and an explicit `null` inside it is dropped.
  Where that `null` means something, as in Jira's `update` (`{"set": null}`
  clears a field), type the input as a plain object with `input_types`.
- A generated capability is one API call; one that takes several is written by
  hand (see Hand-written capabilities).
- `fetch` refuses a request-body field named like a query or header parameter
  of the same operation (`two inputs are named …`), and one named like a path
  parameter unless `path_parameters_win` is set.

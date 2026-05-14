# Reporting Module Spec

## Objective

Add a first-class Reports area to the application where users can create,
configure, view, and share live reports over the Object Model.

The first version is intentionally object-model-only:

- reports render existing Object Model data
- reports do not launch workflows
- reports do not depend on workflow instances, step events, or step outputs
- reports are live views over the current state of one or more object schemas
- report authors describe the report using markdown plus typed, declarative
  report blocks

The end-user outcome is a `Reports` tab in the main UI. A user opens the tab,
selects a report, adjusts filter presets, and sees markdown, tables, metrics,
and charts rendered from Object Model data with server-side pagination and lazy
loading where needed.

## Non-Goals For The First Version

- No arbitrary SQL editor.
- No arbitrary JavaScript execution in report definitions.
- No workflow execution or workflow triggering from reports.
- No dependency on workflow instance retention.
- No support for per-workflow-invocation reports in the MVP.
- No report scheduler or email delivery in the MVP.
- No full BI semantic layer in the MVP.

These can be added later, but the first version should establish the report
definition, object-model query, and UI rendering boundaries cleanly.

## Existing Capabilities To Reuse

The implementation should build on capabilities already present in the repo:

- Object Model schemas and instances.
- Object Model `filter` endpoint semantics for table-style data.
- Object Model `aggregate` endpoint semantics for chart and metric-style data.
- Connection-backed object stores through existing Object Model connection
  resolution.
- `react-markdown` and `remark-gfm` for markdown rendering.
- `recharts` for chart rendering.
- Existing frontend routing, navigation, feature module, and API client patterns.

Reports should not bypass Object Model services to read database tables directly.
The backend report runner should use the same tenant-aware Object Model service
path as the rest of the application.

## Product Model

### Report

A report is a saved definition with:

- metadata such as name, slug, description, tags, owner, timestamps
- a structured layout used as the report body
- report-level filters and presets
- typed blocks referenced from the layout, including markdown narrative blocks
- optional block-specific filters
- optional default viewer state
- a definition version used for validation and migration

### Report Viewer

A report viewer is the runtime page that:

- loads the saved report definition
- initializes filter state from defaults and URL params
- renders layout nodes and typed blocks
- fetches data for visible blocks
- handles table pagination, table sorting, and chart/table filter changes
- keeps user-selected filter state URL-addressable

### Report Author

A report author can:

- create a report
- edit report metadata
- edit markdown blocks
- add and configure blocks
- configure report-level and block-level filters
- preview the report against live object data
- save a draft or publish the current definition

The first implementation can ship with a pragmatic editor instead of a polished
drag-and-drop builder. The important part is that the saved definition is stable
and typed.

## Data Fetching

### Fetching Principles

Report data fetching should follow these rules:

1. The browser does not execute arbitrary report queries directly.
2. The browser sends report ID, filter state, pagination state, sorting state,
   and requested block IDs to the backend.
3. The backend loads the report definition, validates viewer-supplied state, and
   converts block definitions into Object Model filter or aggregate requests.
4. Object Model services execute the queries under the current tenant and
   connection context.
5. The backend returns normalized block data that is easy for the frontend to
   render.

This keeps authorization, validation, schema compatibility, and query limits on
the server.

### Backend Endpoints

Suggested route group:

```text
GET    /api/reports
POST   /api/reports
GET    /api/reports/{reportId}
PUT    /api/reports/{reportId}
DELETE /api/reports/{reportId}

POST   /api/reports/{reportId}/validate
POST   /api/reports/{reportId}/render
POST   /api/reports/{reportId}/blocks/{blockId}/data
```

The management endpoints persist report definitions. The render and block-data
endpoints execute read-only data queries.

### Report List Fetch

`GET /api/reports` returns the data needed for the Reports tab list:

```json
{
  "reports": [
    {
      "id": "rep_sales_overview",
      "name": "Sales Overview",
      "slug": "sales-overview",
      "description": "Revenue, orders, and active customers",
      "updatedAt": "2026-04-28T10:00:00Z",
      "createdBy": "user_123",
      "tags": ["sales"]
    }
  ]
}
```

### Report Definition Fetch

`GET /api/reports/{reportId}` returns the saved definition but does not execute
block queries.

The frontend uses this response to:

- render the report shell
- build filter controls
- determine which block data requests are needed
- construct the editor form

### Schema Metadata Fetch

The editor needs Object Model schema metadata before it can build type-aware
filters, table columns, chart groupings, and aggregate controls.

The frontend should reuse the existing Object Model schema endpoints:

```text
GET /api/runtime/object-model/schemas
GET /api/runtime/object-model/schemas/{id}
GET /api/runtime/object-model/schemas/name/{name}
```

The report editor uses schema metadata to determine:

- available schemas
- field names
- field labels where available
- field types
- nullable fields
- enum-like values where available
- sortable and filterable fields
- aggregate-compatible fields
- default formatter suggestions

The report viewer should not need to fetch schema metadata for normal rendering.
The backend render response should already include normalized block metadata
such as table columns, chart output columns, and value types. This keeps the
viewer lightweight and avoids duplicating server validation in the browser.

### Report Render Fetch

`POST /api/reports/{reportId}/render` is the primary viewer endpoint.

Request:

```json
{
  "filters": {
    "date_range": {
      "from": "2026-03-29T00:00:00Z",
      "to": "2026-04-28T23:59:59Z"
    },
    "status": ["active", "paused"]
  },
  "blocks": [
    {
      "id": "revenue_by_day"
    },
    {
      "id": "recent_orders",
      "page": {
        "size": 50,
        "offset": 0
      },
      "sort": [
        {
          "field": "created_at",
          "direction": "desc"
        }
      ]
    }
  ],
  "timezone": "Europe/Warsaw"
}
```

Response:

```json
{
  "report": {
    "id": "rep_sales_overview",
    "definitionVersion": 1
  },
  "resolvedFilters": {
    "date_range": {
      "from": "2026-03-29T00:00:00Z",
      "to": "2026-04-28T23:59:59Z",
      "label": "Last 30 days"
    },
    "status": ["active", "paused"]
  },
  "blocks": {
    "revenue_by_day": {
      "type": "chart",
      "status": "ready",
      "data": {
        "columns": ["day", "revenue"],
        "rows": [
          ["2026-04-01", 12000],
          ["2026-04-02", 14000]
        ]
      }
    },
    "recent_orders": {
      "type": "table",
      "status": "ready",
      "data": {
        "columns": [
          {
            "key": "order_id",
            "label": "Order ID",
            "type": "string"
          },
          {
            "key": "total_amount",
            "label": "Total",
            "type": "number"
          }
        ],
        "rows": [
          {
            "id": "ord_1",
            "order_id": "10001",
            "total_amount": 199.95
          }
        ],
        "page": {
          "size": 50,
          "offset": 0,
          "hasNextPage": true
        }
      }
    }
  },
  "errors": []
}
```

`POST /api/reports/{reportId}/blocks/{blockId}/data` can be used by the
frontend for lazy blocks, table pagination, chart refresh, and retry. Its request
shape should match the block entry in the render request.

### Data Query Execution

Each data block declares a source query. The report runner converts the source
query into one of two Object Model operations.

The runner should use the existing runtime Object Model operations internally:

```text
POST /api/runtime/object-model/instances/schema/{name}/filter
POST /api/runtime/object-model/instances/schema/{name}/aggregate
```

The report API can call the underlying services directly rather than issuing
HTTP requests to itself. The important boundary is semantic: reports execute
through the same validated Object Model filter and aggregate path as other app
features.

### Data Sources And Connections

A report block can optionally specify an Object Model connection context when
the schema is backed by an external store.

Example:

```json
{
  "source": {
    "schema": "Order",
    "connectionId": "conn_reporting_warehouse",
    "mode": "filter"
  }
}
```

If omitted, the block uses the default Object Model store for the tenant.

Connection handling rules:

- the backend validates that the current tenant can use the connection
- the backend resolves the connection through existing Object Model connection
  resolution
- the connection ID participates in cache keys
- viewers never receive connection credentials
- report definitions store only connection IDs, not connection secrets
- if a connection is deleted or becomes invalid, affected blocks render with
  block-scoped errors

#### Table Blocks Use Filter Queries

Table blocks use Object Model filtering:

```json
{
  "schema": "Order",
  "mode": "filter",
  "condition": {
    "op": "AND",
    "arguments": [
      {
        "op": "GTE",
        "arguments": ["created_at", "2026-03-29T00:00:00Z"]
      },
      {
        "op": "LTE",
        "arguments": ["created_at", "2026-04-28T23:59:59Z"]
      },
      {
        "op": "IN",
        "arguments": ["status", ["active", "paused"]]
      }
    ]
  },
  "sortBy": "created_at",
  "sortOrder": "desc",
  "offset": 0,
  "limit": 50
}
```

The backend combines:

- fixed block base conditions
- report-level filter conditions
- block-level filter conditions
- table search conditions
- table sort state
- table pagination state

The combination must be deterministic and validated against schema field types.

#### Chart And Metric Blocks Use Aggregate Queries

Chart and metric blocks use Object Model aggregation:

```json
{
  "schema": "Order",
  "mode": "aggregate",
  "groupBy": [
    {
      "field": "created_at",
      "bucket": "day",
      "alias": "day"
    }
  ],
  "aggregates": [
    {
      "alias": "revenue",
      "op": "SUM",
      "field": "total_amount"
    }
  ],
  "condition": {
    "op": "AND",
    "arguments": [
      {
        "op": "GTE",
        "arguments": ["created_at", "2026-03-29T00:00:00Z"]
      },
      {
        "op": "LTE",
        "arguments": ["created_at", "2026-04-28T23:59:59Z"]
      }
    ]
  },
  "orderBy": [
    {
      "field": "day",
      "direction": "asc"
    }
  ],
  "limit": 500
}
```

Supported aggregate operations in the report definition should initially mirror
the existing Object Model aggregate surface:

- `count`
- `sum`
- `min`
- `max`
- `first_value`
- `last_value`

Expression aggregates should be treated carefully. If the existing Object Model
aggregate API supports expressions, report definitions should allow only the
already-supported expression syntax and validate it server-side.

### Filter Preset Data

Filters can have static or dynamic options.

Static options are stored directly in the report definition:

```json
{
  "id": "status",
  "type": "multi_select",
  "label": "Status",
  "options": {
    "source": "static",
    "values": [
      {
        "label": "Active",
        "value": "active"
      },
      {
        "label": "Paused",
        "value": "paused"
      }
    ]
  }
}
```

Dynamic options are fetched from Object Model data through the report backend:

```json
{
  "id": "customer",
  "type": "select",
  "label": "Customer",
  "options": {
    "source": "object_model",
    "schema": "Customer",
    "labelField": "name",
    "valueField": "id",
    "searchable": true,
    "limit": 50,
    "baseCondition": {
      "op": "EQ",
      "arguments": ["status", "active"]
    }
  }
}
```

Dynamic option requests should be server-side, paginated, and search-aware:

```text
GET /api/reports/{reportId}/filters/{filterId}/options?search=acme&limit=50
```

For simple distinct value lists, the backend can implement options by running an
aggregate grouped by the selected field.

### Time Range Presets

Time range filters should support both absolute values and presets.

Suggested built-in presets:

- today
- yesterday
- last_7_days
- last_30_days
- this_month
- last_month
- this_quarter
- year_to_date
- custom

The backend resolves presets into absolute instants using the request timezone.
The resolved values are returned in `resolvedFilters` so the UI and backend agree
on the exact interval.

Time ranges should be modeled as half-open intervals internally:

```text
from <= value < to
```

This avoids end-of-day precision bugs. The UI can still show friendly inclusive
labels.

### Pagination And Lazy Loading

Tables should use server-side pagination from the beginning.

Initial MVP pagination:

- `offset`
- `limit`
- `hasNextPage`

Later pagination:

- cursor-based pagination for very large or frequently changing datasets
- total row count only when requested, because counts can be expensive

Block lazy-loading rules:

- `lazy: false` blocks are fetched during the first render request
- `lazy: true` blocks are fetched when they enter the viewport
- table next-page requests fetch only that table block
- chart retry requests fetch only that chart block
- changing a report-level filter invalidates all dependent blocks
- changing a block-level filter invalidates only that block

The report definition should let authors configure:

```json
{
  "pagination": {
    "mode": "server",
    "defaultPageSize": 50,
    "allowedPageSizes": [25, 50, 100]
  },
  "lazy": true
}
```

### Data Limits

The report runner should enforce limits independent of frontend behavior:

- maximum table page size
- maximum aggregate rows
- maximum dynamic option rows
- maximum number of blocks per render request
- maximum markdown size
- maximum report definition size
- request timeout for block data

The response should return a structured error when a block exceeds limits rather
than failing the whole report where possible.

Example:

```json
{
  "blocks": {
    "large_table": {
      "type": "table",
      "status": "error",
      "error": {
        "code": "PAGE_SIZE_TOO_LARGE",
        "message": "The requested page size exceeds the allowed limit."
      }
    }
  }
}
```

### Caching

MVP caching can be conservative:

- report definitions can be cached client-side by ID and updatedAt
- block data should not be globally cached initially unless the Object Model data
  freshness rules are explicit
- the frontend can keep per-view block responses in memory while the user stays
  on the report

Later, the backend can add short-lived block query caching keyed by:

- tenant ID
- report ID
- report definition version
- block ID
- resolved filters
- pagination state
- sort state
- connection ID

## Report Description

### Canonical Definition

Reports should be stored as canonical JSON. Layout is expressed as typed JSON
nodes, and markdown is a typed block primitive rather than a top-level report
body.

This gives us:

- validation before save
- migration across definition versions
- stable API contracts
- editor support without parsing arbitrary markdown every time
- a safe renderer that can identify known report blocks

Suggested persisted shape:

```json
{
  "definitionVersion": 1,
  "name": "Sales Overview",
  "slug": "sales-overview",
  "description": "Revenue, orders, and active customers",
  "tags": ["sales"],
  "layout": [
    {
      "id": "intro_node",
      "type": "block",
      "blockId": "intro"
    },
    {
      "id": "revenue_node",
      "type": "block",
      "blockId": "revenue_by_day"
    },
    {
      "id": "orders_node",
      "type": "block",
      "blockId": "recent_orders"
    }
  ],
  "filters": [
    {
      "id": "date_range",
      "label": "Period",
      "type": "time_range",
      "default": {
        "preset": "last_30_days"
      },
      "required": true
    },
    {
      "id": "status",
      "label": "Status",
      "type": "multi_select",
      "default": ["active"],
      "options": {
        "source": "static",
        "values": [
          {
            "label": "Active",
            "value": "active"
          },
          {
            "label": "Paused",
            "value": "paused"
          }
        ]
      }
    }
  ],
  "blocks": [
    {
      "id": "intro",
      "type": "markdown",
      "markdown": {
        "content": "# Sales Overview\n\nRevenue and order activity for the selected period."
      }
    },
    {
      "id": "revenue_by_day",
      "type": "chart",
      "title": "Revenue by day",
      "lazy": false,
      "source": {
        "schema": "Order",
        "mode": "aggregate",
        "condition": {
          "and": [
            {
              "field": "created_at",
              "op": "between",
              "valueFrom": "filters.date_range"
            },
            {
              "field": "status",
              "op": "in",
              "valueFrom": "filters.status"
            }
          ]
        },
        "groupBy": [
          {
            "field": "created_at",
            "bucket": "day",
            "alias": "day"
          }
        ],
        "aggregates": [
          {
            "alias": "revenue",
            "op": "sum",
            "field": "total_amount"
          }
        ],
        "orderBy": [
          {
            "field": "day",
            "direction": "asc"
          }
        ],
        "limit": 500
      },
      "chart": {
        "kind": "line",
        "x": "day",
        "series": [
          {
            "field": "revenue",
            "label": "Revenue"
          }
        ]
      }
    },
    {
      "id": "recent_orders",
      "type": "table",
      "title": "Recent orders",
      "lazy": true,
      "source": {
        "schema": "Order",
        "mode": "filter",
        "condition": {
          "and": [
            {
              "field": "created_at",
              "op": "between",
              "valueFrom": "filters.date_range"
            }
          ]
        }
      },
      "table": {
        "columns": [
          {
            "field": "order_id",
            "label": "Order ID"
          },
          {
            "field": "customer_name",
            "label": "Customer"
          },
          {
            "field": "status",
            "label": "Status"
          },
          {
            "field": "total_amount",
            "label": "Total",
            "format": "currency"
          },
          {
            "field": "created_at",
            "label": "Created",
            "format": "datetime"
          }
        ],
        "defaultSort": [
          {
            "field": "created_at",
            "direction": "desc"
          }
        ],
        "pagination": {
          "mode": "server",
          "defaultPageSize": 50,
          "allowedPageSizes": [25, 50, 100]
        }
      }
    }
  ]
}
```

### Markdown Blocks

Markdown should stay familiar to users, but it lives inside a typed markdown
block. Layout placement still uses normal block layout nodes:

```json
{
  "id": "intro",
  "type": "markdown",
  "source": {
    "schema": "Order",
    "mode": "aggregate",
    "aggregates": [{ "alias": "revenue", "op": "sum", "field": "total_amount" }]
  },
  "markdown": {
    "content": "# Sales Overview\n\nRevenue for the selected period: {{source.revenue}}"
  }
}
```

Markdown blocks support normal GFM markdown plus source interpolation from the
same block only:

```markdown
{{source.field_name}}
{{source[0].field_name}}
```

This keeps markdown readable while keeping layout and data configuration in
typed JSON. Block placement is handled by `definition.layout`, not by embedding
other report blocks in markdown.

Alternative authoring syntax can be added later using fenced code blocks:

````markdown
```runtara-report-table
id: recent_orders
schema: Order
columns:
  - order_id
  - customer_name
  - total_amount
```
````

If fenced report blocks are supported, they should compile into the same
canonical JSON definition. The renderer should never execute arbitrary code
blocks.

### Filter Types

Supported MVP filter controls:

| Type | Value shape | Typical field types |
| --- | --- | --- |
| `select` | single scalar | string, enum, id |
| `multi_select` | scalar array | string, enum, id |
| `radio` | single scalar | small enums |
| `checkbox` | boolean | boolean |
| `time_range` | `{ from, to }` or preset | date, datetime |
| `number_range` | `{ min, max }` | number |
| `text` | string | string |
| `search` | string | string, text-like fields |

Each filter definition should include:

- `id`
- `label`
- `type`
- `default`
- `required`
- optional `options`
- optional `placeholder`
- optional validation
- optional visibility conditions

### Filter Scope

There are three filter layers:

1. Report-level filters.
   Shared controls visible at the top of the report.

2. Block-level filters.
   Controls attached to one chart or table. These are useful for columns,
   grouping, local status toggles, and block-specific time ranges.

3. Base filters.
   Fixed conditions authored into the block. Viewers cannot edit them.

The backend should combine them in this order:

```text
base condition AND report-level filter conditions AND block-level filter conditions
```

### Filter To Query Mapping

Filters should not be raw Object Model conditions by default. A filter is a UI
control plus a mapping to one or more query conditions.

Example:

```json
{
  "id": "status",
  "type": "multi_select",
  "label": "Status",
  "default": ["active"],
  "appliesTo": [
    {
      "blockId": "revenue_by_day",
      "field": "status",
      "op": "in"
    },
    {
      "blockId": "recent_orders",
      "field": "status",
      "op": "in"
    }
  ]
}
```

This allows one UI filter to apply differently to different blocks.

Block definitions can also reference filters inline:

```json
{
  "field": "created_at",
  "op": "between",
  "valueFrom": "filters.date_range"
}
```

The report compiler should normalize both styles into a block dependency map.
The frontend uses the map to know which blocks to invalidate when a filter
changes.

### Column And Type Customization

For each table or chart, authors should be able to customize available controls
by column and field type.

Examples:

- string or enum field: select, multi-select, radio, text search
- boolean field: checkbox, radio
- date or datetime field: time range, date preset, relative period
- number field: number range, metric threshold
- id/reference field: select with dynamic options

The editor should derive the default control options from Object Model schema
metadata but allow the author to override:

- label
- control type
- available values
- default value
- whether the control is report-level or block-level
- whether the control is visible in the viewer
- which blocks the control applies to

### Block Types

#### Markdown Block

Markdown content that does not fetch data. The main report body is markdown, but
inline markdown blocks can be useful in the editor.

#### Metric Block

A single aggregate value or small set of values.

Example:

```json
{
  "id": "total_revenue",
  "type": "metric",
  "source": {
    "schema": "Order",
    "mode": "aggregate",
    "aggregates": [
      {
        "alias": "value",
        "op": "sum",
        "field": "total_amount"
      }
    ]
  },
  "metric": {
    "valueField": "value",
    "format": "currency"
  }
}
```

#### Table Block

A paginated Object Model filter query rendered as a table.

Table blocks should support:

- visible columns
- column labels
- column formatting
- optional per-column display cutoff with `maxChars`
- default sort
- sortable columns
- server-side pagination
- row actions in later versions
- export in later versions

#### Chart Block

An Object Model aggregate query rendered as a chart.

Chart blocks should support:

- line
- bar
- area
- pie or donut
- stacked bar later
- x-axis mapping
- series mapping
- color assignment
- empty state
- legend
- tooltip

#### Key-Value Block

A compact block for object metadata or one-row aggregate data.

This can be added after table, chart, and metric blocks if needed.

### Validation

Report definitions should be validated on save and before render.

Validation should check:

- unique report slug within tenant
- unique block IDs within a report
- valid markdown block references
- valid filter IDs
- valid `valueFrom` references
- valid schema names
- valid field names for each schema
- operator compatibility with field type
- aggregate compatibility with field type
- chart field mappings exist in aggregate output
- table columns exist in schema
- pagination limits are within server caps
- dynamic option sources are valid
- no unsupported raw HTML or executable code

Validation response:

```json
{
  "valid": false,
  "errors": [
    {
      "path": "blocks[0].source.aggregates[0].field",
      "code": "FIELD_NOT_FOUND",
      "message": "Field total_amount does not exist on schema Order."
    }
  ],
  "warnings": [
    {
      "path": "blocks[1].table.pagination.defaultPageSize",
      "code": "LARGE_PAGE_SIZE",
      "message": "Large table pages may be slow to render."
    }
  ]
}
```

## Persistence

### Tables

Suggested database tables:

```sql
create table report_definitions (
  id uuid primary key,
  tenant_id uuid not null,
  slug text not null,
  name text not null,
  description text,
  tags jsonb not null default '[]',
  definition_version integer not null,
  definition jsonb not null,
  status text not null default 'published',
  created_by uuid,
  updated_by uuid,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  unique (tenant_id, slug)
);
```

Optional future versioning:

```sql
create table report_definition_versions (
  id uuid primary key,
  report_id uuid not null references report_definitions(id),
  version_number integer not null,
  definition jsonb not null,
  created_by uuid,
  created_at timestamptz not null default now(),
  unique (report_id, version_number)
);
```

MVP can store only the current definition in `report_definitions`. Add versioning
once reports become operationally important or shareable outside the immediate
tenant UI.

### Status

Suggested statuses:

- `draft`
- `published`
- `archived`

The viewer should show only `published` reports by default. Editors can access
drafts.

## Backend Construction

### Module Layout

Suggested server-side module layout:

```text
crates/runtara-server/src/api/
  dto/reports.rs
  handlers/reports.rs
  repositories/reports.rs
  services/reports/
    mod.rs
    compiler.rs
    runner.rs
    validation.rs
    filters.rs
    blocks.rs
```

Responsibilities:

- DTOs define API request and response shapes.
- Repository persists report definitions.
- Compiler converts canonical definition into an executable report plan.
- Validator checks report definitions against Object Model schema metadata.
- Runner executes block plans through Object Model services.
- Filter utilities resolve presets, normalize values, and build conditions.
- Block utilities normalize table, chart, and metric data responses.

### Report Compiler

The compiler takes a report definition and produces:

- normalized layout tree
- markdown block content with validated source placeholders
- normalized filters
- block dependency map
- per-block executable query plan
- field and output metadata for rendering

The compiled plan should not be persisted initially. It can be recomputed on
save and render. If compilation becomes expensive, cache it by report ID and
definition version.

### Report Runner

The runner receives:

- tenant context
- authenticated user context
- report ID or compiled definition
- viewer filters
- requested block IDs
- per-block pagination and sort state
- timezone

It returns:

- resolved filters
- normalized block data
- block-level errors

The runner must:

- validate viewer filter values
- resolve filter presets
- enforce query limits
- enforce tenant scoping
- call Object Model services instead of raw SQL
- avoid failing the entire report when one block fails

### Error Handling

Report render errors should be block-scoped whenever possible.

Examples:

- invalid viewer filter: whole render request returns `400`
- report not found: `404`
- unauthorized report access: `403`
- one block references a deleted field: render succeeds with that block in error
- object store timeout for one chart: render succeeds with that chart in error

## Frontend Construction

### Routes

Add Reports routes to the frontend router:

```text
/reports
/reports/new
/reports/:reportId
/reports/:reportId/edit
```

Add a `Reports` item to the main navigation.

### Feature Module Layout

Suggested frontend feature layout:

```text
crates/runtara-server/frontend/src/features/reports/
  api/
    reportsApi.ts
    reportsTypes.ts
  pages/
    ReportsListPage.tsx
    ReportViewerPage.tsx
    ReportEditorPage.tsx
  components/
    ReportRenderer.tsx
    ReportToolbar.tsx
    ReportFilterBar.tsx
    ReportBlockHost.tsx
    blocks/
      MarkdownBlock.tsx
      MetricBlock.tsx
      TableBlock.tsx
      ChartBlock.tsx
    editor/
      ReportMetadataPanel.tsx
      ReportMarkdownEditor.tsx
      ReportFiltersPanel.tsx
      ReportBlocksPanel.tsx
      BlockInspector.tsx
      QueryBuilder.tsx
      ReportPreview.tsx
  hooks/
    useReportDefinition.ts
    useReportFilters.ts
    useReportBlockData.ts
    useReportUrlState.ts
```

Use existing frontend API and state patterns in the repo. Do not introduce a new
global state library just for reports unless the existing app already uses one
for comparable server state.

### Reports List Page

The list page should provide:

- report search
- tags or simple filtering later
- create report button
- report cards or dense table matching the app style
- updated timestamp
- owner if available
- draft or published status for editors

Clicking a report opens `/reports/:reportId`.

### Report Viewer Page

The viewer page should include:

- title and description
- filter bar
- report actions menu
- markdown-rendered report body
- lazy block loading
- block-level loading, empty, and error states
- URL-synced filter state
- edit button for users with permission

The page should not show implementation instructions or schema details to normal
viewers.

### Report Editor Page

The editor should be split into practical areas:

- metadata
- layout
- markdown blocks
- filters
- blocks
- preview

The MVP editor can be form-based:

1. Choose report name and slug.
2. Add or edit markdown blocks.
3. Add filters.
4. Add blocks.
5. Arrange blocks in the layout.
6. Preview.
7. Save.

The editor should include a schema-aware query builder for blocks:

- pick object schema
- choose filter or aggregate mode
- choose fields
- choose operators based on field type
- configure grouping and aggregates
- configure table columns or chart mappings
- configure pagination/lazy loading
- configure block-level filters

### Filter Builder

The filter builder should use Object Model schema metadata to suggest controls.

For a selected schema field:

- string enum-like fields suggest select or multi-select
- boolean fields suggest checkbox or radio
- datetime fields suggest time range
- numeric fields suggest number range
- reference or id fields suggest dynamic select

The author should be able to define whether a filter is:

- global to the report
- local to one block
- local to multiple selected blocks
- hidden but fixed as a base condition

### Block Builder

The block builder should let the author configure:

- block ID
- block title
- block type
- schema
- data mode: filter or aggregate
- fields
- base condition
- filter mappings
- sorting
- pagination
- lazy loading
- table columns or chart mappings
- formatting

The block builder should validate as the user edits and surface errors near the
field that caused them.

### URL State

Viewer state should be shareable.

Example:

```text
/reports/sales-overview?date_range=last_30_days&status=active,paused&page.recent_orders=2
```

URL state should include:

- report-level filters
- block-level filters where practical
- table page
- table page size
- table sort

Do not put large values or sensitive values in the URL. For long state, add a
future saved-view model.

## Display And Rendering

### Rendering Pipeline

The frontend rendering pipeline:

1. Load report definition.
2. Build initial filter state from definition defaults and URL params.
3. Render report shell: title, toolbar, filters, and layout skeleton.
4. Render layout block nodes with `ReportBlockHost` components.
5. Fetch data for eager blocks.
6. Fetch data for lazy blocks when they enter the viewport.
7. Render each block according to its type.
8. On filter change, update URL state and refetch dependent blocks.
9. On table page or sort change, refetch only that table block.

### Markdown Block Rendering

Use `react-markdown` with GFM support. Raw HTML should remain disabled.

The renderer should recognize source placeholders inside markdown blocks:

```markdown
{{source.revenue}}
{{source[0].customer_name}}
```

During render, placeholders are replaced with values from the markdown block's
own rendered source data. Layout decides where the markdown block is mounted.

Unknown source placeholders should return an editor-visible validation error and
a viewer-visible markdown block error.

Normal fenced code blocks remain plain code blocks. Only recognized report block
syntax is converted into dynamic UI.

### Table Rendering

Table blocks should render:

- column headers
- row data
- sortable headers when configured
- formatted values
- loading state
- empty state
- error state
- pagination controls
- page size selector if multiple sizes are allowed

All table sort and pagination should be server-side. The table should not fetch
all rows and paginate in memory.

### Chart Rendering

Chart blocks should render with `recharts`.

Chart renderer responsibilities:

- map backend columns and rows into chart data objects
- render the configured chart type
- format axis labels
- format tooltips
- render legend where configured
- handle empty state
- handle block-level loading and error states
- stay responsive to the container width

The chart renderer should not understand Object Model query semantics. It should
only understand normalized chart data plus chart display configuration.

### Metric Rendering

Metric blocks should render:

- primary value
- optional label
- optional comparison value later
- configured formatter
- loading, empty, and error states

### Empty States

Every data block needs a useful empty state:

- table: no rows match current filters
- chart: no data for selected period
- metric: no value available

Empty state should be distinct from loading and error.

### Error States

Block errors should show:

- concise user-facing message
- retry action where useful
- details in editor or developer mode only

Do not expose raw SQL, credentials, internal stack traces, or tenant IDs in UI
errors.

### Responsive Behavior

Reports should work on desktop and mobile:

- filter bar collapses into a compact controls area on small screens
- tables scroll horizontally if needed
- charts maintain a stable minimum height
- markdown spacing stays readable
- block controls do not overlap chart/table content

## Security And Permissions

### Authorization

Reports should be tenant-scoped.

Minimum permission model:

- viewers can list and view published reports
- editors can create and edit reports
- admins can archive or delete reports

If the app already has more granular authorization primitives, reports should
use those instead of inventing a separate model.

### Query Safety

Reports must not permit:

- arbitrary SQL
- arbitrary JavaScript
- raw HTML execution
- unbounded table page sizes
- unbounded aggregate result sets
- cross-tenant schema or instance reads

All fields, schemas, operators, and filter values should be validated server-side
against Object Model metadata.

### Data Exposure

Report authors can accidentally expose sensitive Object Model fields. The first
implementation should at least:

- respect any existing Object Model access rules
- avoid exposing hidden/internal fields in the editor by default
- allow only permitted users to edit reports

Later, add field-level report visibility rules if Object Model supports them.

## Observability

Add structured logs and metrics around:

- report definition validation failures
- report render duration
- per-block query duration
- per-block errors
- slow Object Model filter queries
- slow Object Model aggregate queries
- report ID, block ID, schema, mode, and tenant ID

Do not log full filter values if they may contain sensitive data.

## Testing Plan

### Backend Tests

Cover:

- report definition CRUD
- slug uniqueness per tenant
- validation catches missing schema
- validation catches missing field
- validation catches invalid field/operator combinations
- time range preset resolution
- report-level filter to condition conversion
- block-level filter to condition conversion
- table pagination limit enforcement
- aggregate limit enforcement
- block-scoped error behavior
- tenant scoping

### Frontend Tests

Cover:

- Reports nav item appears
- Reports list loads
- viewer loads definition and renders markdown
- placeholders mount the correct block components
- filter changes update URL state
- filter changes refetch dependent blocks
- table pagination refetches only the table block
- lazy block fetches after becoming visible
- chart empty state
- table error state
- editor validates required block fields

### Smoke/E2E Tests

Add a smoke test for:

1. Open `/reports`.
2. Create a simple report over a seeded object schema.
3. Add a time range filter.
4. Add a table block with server pagination.
5. Add a chart block with aggregation.
6. Save.
7. Open viewer.
8. Change filter preset.
9. Verify table and chart update.

## Rollout Plan

### Phase 1: Backend Foundation

- Add `report_definitions` migration.
- Add report DTOs.
- Add repository CRUD.
- Add validation service.
- Add report runner that supports table blocks and metric blocks.
- Add API routes.

### Phase 2: Viewer UI

- Add Reports nav item and routes.
- Add Reports list page.
- Add viewer page.
- Add markdown block renderer with source interpolation.
- Add filter bar.
- Add table block renderer.
- Add metric block renderer.

At the end of this phase, a developer-created report definition should render in
the UI.

### Phase 3: Chart Blocks And Lazy Loading

- Add aggregate-backed chart blocks.
- Add chart renderer using `recharts`.
- Add lazy block loading.
- Add table pagination and sort refinement.
- Add dynamic filter options.

### Phase 4: Editor UI

- Add report create/edit page.
- Add metadata panel.
- Add markdown editor.
- Add filter builder.
- Add table block builder.
- Add chart block builder.
- Add live preview.

At the end of this phase, non-developer users should be able to create practical
reports from the UI.

### Phase 5: Hardening

- Add report definition versioning if needed.
- Add better permission controls.
- Add saved viewer states if needed.
- Add export if needed.
- Add performance metrics and slow query diagnostics.

## Acceptance Criteria

The reporting tab is considered working when:

- `Reports` appears in the main navigation.
- A user can list reports.
- A user can open a report viewer.
- The viewer renders markdown content.
- The viewer renders at least table, metric, and chart blocks.
- Report-level filters render from the report definition.
- Select, radio, and time range filters work.
- Filter changes refetch affected blocks.
- Tables use server-side pagination.
- Lazy blocks do not fetch until needed.
- Report data is fetched through backend report endpoints.
- Backend report endpoints execute through Object Model services.
- Invalid report definitions fail validation with useful messages.
- One failed block does not break the entire report.
- The implementation does not execute arbitrary SQL, JavaScript, or raw HTML.

## Future Extensions

Possible follow-up work:

- per-workflow-invocation reports
- persisted report snapshots
- scheduled report delivery
- CSV export
- PDF export
- saved report views
- role-specific report visibility
- report templates
- report folders
- dashboard layouts with draggable sections
- raw SQL blocks behind a separate, audited, read-only permission model

Workflow-instance reports should be treated as a second product track. They need
different persistence and retention rules because workflow step outputs are not a
reliable long-term data source today. The Object Model report foundation should
remain useful even if invocation-scoped reports are added later.

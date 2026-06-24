# Templating

Runtara renders templates in two places, both with the **same engine and the same
helper set**:

- **Mapping expressions** — an input mapping with `valueType: "template"`, e.g.
  `{"valueType": "template", "value": "Bearer {{ steps.auth.outputs.token }}"}`.
- **The `text` / `render-template` agent** — its `text` input is rendered against
  the `context` input.

The engine is [minijinja](https://docs.rs/minijinja) 2.x — a Jinja2-compatible
subset. It is **not** full Jinja2/Flask: only minijinja's builtin filters,
functions, and tests are available, plus `tojson`. No Runtara-specific helpers are
registered, and no template extension is enabled beyond the defaults and `json`.

## Context

Templates resolve dot-paths against the execution envelope:

| Path | What it is |
|------|------------|
| `data.*`       | the workflow/step input payload |
| `variables.*`  | declared + runtime variables (resolved to their value) |
| `steps.<id>.outputs.*` | a prior step's outputs |

Undefined paths render as an empty string (minijinja default).

## Available helpers

The full set is minijinja's builtin reference:
<https://docs.rs/minijinja/latest/minijinja/filters/index.html> (filters),
[`functions`](https://docs.rs/minijinja/latest/minijinja/functions/index.html), and
[`tests`](https://docs.rs/minijinja/latest/minijinja/tests/index.html). Commonly used:

- **Filters:** `default` (`d`), `upper`, `lower`, `title`, `capitalize`, `trim`,
  `replace`, `length` (`count`), `first`, `last`, `join`, `split`, `list`, `map`,
  `select`/`reject`, `sort`, `unique`, `reverse`, `int`, `float`, `round`, `abs`,
  `min`, `max`, `sum`, `items`, `dictsort`, `urlencode`, `indent`, `escape` (`e`),
  `safe`, and **`tojson`** (enabled via the minijinja `json` feature).
- **Functions/globals:** `range`, `dict`, `namespace`, `debug`. (That is the
  complete list — minijinja registers no others.)
- **Tests** (`{% if x is … %}`): `defined`, `none`, `boolean`, `number`, `string`,
  `sequence`, `mapping`, `iterable`, `even`, `odd`, `eq`/`==`, `ne`/`!=`,
  `lt`/`<`, `le`/`<=`, `gt`/`>`, `ge`/`>=`, `in`, `true`, `false`, `sameas`.
- **Loop variable:** `loop.index`, `loop.index0`, `loop.first`, `loop.last`,
  `loop.length`, `loop.revindex`, `loop.cycle(...)`.

## Not available (and what to use instead)

These are Jinja2/Flask idioms that **minijinja does not ship**, so they fail with
`unknown function`/`unknown filter`. A render that hits one gets an error pointing
back to this page.

### `now()` — use the datetime agent

`now()` is intentionally **not** exposed in mapping templates. Workflow execution is
durable and replayed (on suspend/resume, drain recovery, etc.); mapping templates
are re-evaluated on replay, so a wall-clock `now()` would return a different value
each replay and silently diverge from the original run.

Get the current time from the **`datetime` / `get-current-date`** agent step
instead — agent calls are checkpointed, so the timestamp is captured once and
replayed consistently. Forward it into subgraphs via a Split step's `variables`.

```jsonc
// step "ts": agent datetime / get-current-date  (inputMapping: { "include_time": true })
// then reference it:
{ "valueType": "reference", "value": "steps.ts.outputs" }
```

### `joiner()` — use a namespace flag

Use minijinja's `namespace` to track "first iteration", or `loop.last`:

```jinja
{% set ns = namespace(first=true) %}
{%- for x in items -%}
  {% if not ns.first %},{% endif %}{{ x }}{% set ns.first = false %}
{%- endfor -%}
```

or, equivalently:

```jinja
{%- for x in items -%}{{ x }}{% if not loop.last %},{% endif %}{%- endfor -%}
```

### Emitting JSON

Use the `tojson` filter (now enabled). To turn a JSON string back into a value,
render it and parse with the `transform` / `from-json-string` capability.

## Errors

An unknown filter/function/test/method produces a `Template render error: …`
message that appends a hint pointing here, so the missing helper is distinguishable
from a typo without a deploy-execute round trip.

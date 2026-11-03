#!/usr/bin/env python3
"""Generate an agent crate's `src/generated.rs` from the vendor's OpenAPI spec.

Every generated agent crate carries a `connector.yaml` (see README.md). Commands,
where CRATE is an agent crate directory such as
`crates/agents/runtara-agent-pipedrive`:

    fetch CRATE       download the spec (or specs), verify each sha256, and write
                      `spec.slice.json` plus `tests/fixtures/<operationId>.json`
    generate CRATE    render `src/generated.rs` from `spec.slice.json`
    check CRATE...    fail when `src/generated.rs`, the fixtures or the operation
                      list no longer match `spec.slice.json`
"""

import argparse
import copy
import difflib
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import urllib.parse
import urllib.request
from pathlib import Path

import yaml

HTTP_METHODS = {"get", "put", "post", "delete", "patch", "head", "options", "trace"}
TOOL_DIR = Path(__file__).resolve().parent
REPO_ROOT = TOOL_DIR.parent.parent
TEMPLATE_DIR = TOOL_DIR / "templates"

# The stock Rust generator maps these to chrono/uuid types the agents don't
# depend on; strings keep the wire value unchanged.
TYPE_MAPPINGS = {
    "UUID": "String",
    "URI": "String",
    "date": "String",
    "DateTime": "String",
    "file": "Value",
    "binary": "Value",
}

# Types the generated file defines or imports. A schema whose generated name
# is one of these (or `<OperationId>Input`) would collide with them.
RESERVED_TYPE_NAMES = {
    "AgentError", "Box", "CapabilityInput", "CapabilityOutput", "Deserialize",
    "HashMap", "Option", "RawConnection", "Result", "Serialize", "String",
    "Value", "Vec",
}

# Keys whose values are data (payload examples, defaults, enum values), not schema.
DATA_KEYS = ("example", "examples", "default", "enum")

# Keys whose values map names to schemas (`properties`) or status codes to
# responses (`responses`), whatever the names are: a response coded `default`
# is an error response, not data.
NAME_MAPS = ("properties", "responses")


def load_config(crate: Path) -> dict:
    with open(crate / "connector.yaml") as f:
        return yaml.safe_load(f)


def spec_sources(config: dict) -> list:
    """The specs a connector is generated from, each with its operations: the
    `specs:` list (an API the vendor publishes as several specs, one per
    product), or the single `spec:` with the top-level `operations:`."""
    if "specs" in config:
        return config["specs"]
    return [{**config["spec"], "operations": config["operations"]}]


def operation_ids(config: dict) -> list:
    return [op for source in spec_sources(config) for op in source["operations"]]


# ---------------------------------------------------------------------------
# fetch
# ---------------------------------------------------------------------------


class SpecLoader(yaml.SafeLoader):
    """YAML 1.1 reads `yes`/`no`/`on`/`off` as booleans, unquoted dates as
    dates and a bare `=` as its "value" tag, which PyYAML cannot even load;
    OpenAPI (YAML 1.2 / JSON) means strings by all of them."""


SpecLoader.yaml_implicit_resolvers = {
    first: [
        (tag, regexp) for tag, regexp in resolvers
        if tag not in ("tag:yaml.org,2002:bool", "tag:yaml.org,2002:timestamp", "tag:yaml.org,2002:value")
    ]
    for first, resolvers in yaml.SafeLoader.yaml_implicit_resolvers.items()
}
SpecLoader.add_implicit_resolver(
    "tag:yaml.org,2002:bool",
    re.compile(r"^(?:true|True|TRUE|false|False|FALSE)$"),
    list("tTfF"),
)


def stringify_keys(node):
    """Unquoted response codes (`200:`) load as integers; OpenAPI keys are strings."""
    if isinstance(node, dict):
        return {str(key): stringify_keys(value) for key, value in node.items()}
    if isinstance(node, list):
        return [stringify_keys(value) for value in node]
    return node


# YAML 1.2 allows a tab as separating whitespace, so a vendor's `default: <tab>`
# is valid, but PyYAML cannot even scan it. Trailing whitespace means nothing.
TRAILING_TABS = re.compile(r"[ \t]*\t[ \t]*(?=\r?$)", re.MULTILINE)


def load_spec(raw) -> dict:
    if isinstance(raw, bytes):
        raw = raw.decode("utf-8-sig")
    return stringify_keys(yaml.load(TRAILING_TABS.sub("", raw), Loader=SpecLoader))


# ---------------------------------------------------------------------------
# Swagger 2.0
# ---------------------------------------------------------------------------

# Where Swagger 2.0 keeps what OpenAPI 3 keeps under `components`.
SWAGGER_REFS = {
    "#/definitions/": "#/components/schemas/",
    "#/parameters/": "#/components/parameters/",
    "#/responses/": "#/components/responses/",
}

# The keys of a Swagger 2.0 parameter (or items object) that describe its
# value: OpenAPI 3 moves them into the parameter's `schema`.
PARAMETER_SCHEMA_KEYS = (
    "type", "format", "items", "enum", "default", "maximum", "minimum", "exclusiveMaximum",
    "exclusiveMinimum", "maxLength", "minLength", "pattern", "maxItems", "minItems",
    "uniqueItems", "multipleOf",
)

OAUTH2_FLOWS = {
    "implicit": "implicit", "password": "password",
    "application": "clientCredentials", "accessCode": "authorizationCode",
}


def is_swagger2(spec: dict) -> bool:
    return str(spec.get("swagger", "")).startswith("2.")


def convert_swagger2(spec: dict) -> dict:
    """The OpenAPI 3.0 form of a Swagger 2.0 spec (Slack publishes only one).

    `formData` parameters become a request body offered in every media type
    the operation `consumes`, so an API that also takes JSON keeps it (and
    `check_operation` refuses an operation that takes form bodies only)."""
    spec = copy.deepcopy(spec)
    rewrite_swagger_refs(spec)
    converted = {"openapi": "3.0.3", "info": spec.get("info") or {}}
    if spec.get("host") or spec.get("basePath"):
        schemes = spec.get("schemes") or ["https"]
        scheme = "https" if "https" in schemes else schemes[0]
        host = f"{scheme}://{spec['host']}" if spec.get("host") else ""
        converted["servers"] = [{"url": host + (spec.get("basePath") or "").rstrip("/")}]
    consumes, produces = spec.get("consumes"), spec.get("produces")
    shared_parameters = spec.get("parameters") or {}
    shared_responses = spec.get("responses") or {}

    paths = {}
    for path, item in (spec.get("paths") or {}).items():
        shared = [deref_swagger(spec, p) for p in item.get("parameters") or []]
        paths[path] = {}
        for method, op in item.items():
            if method not in HTTP_METHODS:
                continue
            paths[path][method] = convert_operation(spec, op, shared, consumes, produces)
    converted["paths"] = paths

    components = {}
    if spec.get("definitions"):
        components["schemas"] = spec["definitions"]
    plain = {name: p for name, p in shared_parameters.items() if p.get("in") not in ("body", "formData")}
    if plain:
        components["parameters"] = {name: convert_parameter(p) for name, p in plain.items()}
    if shared_responses:
        components["responses"] = {name: convert_response(r, produces or ["application/json"])
                                   for name, r in shared_responses.items()}
    if spec.get("securityDefinitions"):
        components["securitySchemes"] = {
            name: convert_security_scheme(scheme) for name, scheme in spec["securityDefinitions"].items()}
    converted["components"] = components
    for key in ("security", "tags", "externalDocs"):
        if key in spec:
            converted[key] = spec[key]
    convert_schemas(converted)
    return converted


def rewrite_swagger_refs(node) -> None:
    """`#/definitions/Deal` -> `#/components/schemas/Deal`, and so on, outside data."""
    if isinstance(node, list):
        for value in node:
            rewrite_swagger_refs(value)
    elif isinstance(node, dict):
        ref = node.get("$ref")
        if isinstance(ref, str):
            for old, new in SWAGGER_REFS.items():
                if ref.startswith(old):
                    node["$ref"] = new + ref[len(old):]
        for key, value in node.items():
            if key in NAME_MAPS and isinstance(value, dict):
                rewrite_swagger_refs(list(value.values()))
            elif key not in DATA_KEYS:
                rewrite_swagger_refs(value)


def deref_swagger(spec: dict, node: dict) -> dict:
    """A parameter or response `$ref`, already rewritten to `#/components/…`,
    read from where Swagger 2.0 keeps it."""
    ref = node.get("$ref")
    if not isinstance(ref, str):
        return node
    for old, new in SWAGGER_REFS.items():
        if ref.startswith(new):
            return resolve(spec, old + ref[len(new):])
    return node


def convert_operation(spec: dict, source: dict, shared: list, consumes, produces) -> dict:
    op = {key: value for key, value in source.items()
          if key not in ("consumes", "produces", "schemes", "parameters")}
    consumes = source.get("consumes") or consumes
    produces = source.get("produces") or produces or ["application/json"]
    parameters = {(p["name"], p["in"]): p for p in shared}
    # An operation's own parameter overrides a path-level one of the same name and location.
    for parameter in source.get("parameters") or []:
        parameter = deref_swagger(spec, parameter)
        parameters[(parameter["name"], parameter["in"])] = parameter

    plain, form, body = [], [], None
    for parameter in parameters.values():
        if parameter["in"] == "body":
            body = parameter
        elif parameter["in"] == "formData":
            form.append(parameter)
        else:
            plain.append(convert_parameter(parameter))
    if plain:
        op["parameters"] = plain
    if body is not None:
        op["requestBody"] = {
            **({"description": body["description"]} if body.get("description") else {}),
            "content": {media: {"schema": body.get("schema") or {}} for media in consumes or ["application/json"]},
            **({"required": True} if body.get("required") else {}),
        }
    elif form:
        files = any(p.get("type") == "file" for p in form)
        media_types = consumes or ["multipart/form-data" if files else "application/x-www-form-urlencoded"]
        schema = {"type": "object", "properties": {p["name"]: parameter_schema(p, described=True) for p in form}}
        required = [p["name"] for p in form if p.get("required")]
        if required:
            schema["required"] = required
        op["requestBody"] = {
            "content": {media: {"schema": copy.deepcopy(schema)} for media in media_types},
            **({"required": True} if required else {}),
        }
    op["responses"] = {
        code: convert_response(deref_swagger(spec, response), produces)
        for code, response in (op.get("responses") or {}).items()
    }
    return op


def parameter_schema(parameter: dict, described: bool = False) -> dict:
    """The schema of a Swagger 2.0 parameter or items object."""
    schema = {key: copy.deepcopy(parameter[key]) for key in PARAMETER_SCHEMA_KEYS if key in parameter}
    if isinstance(schema.get("items"), dict):
        schema["items"] = parameter_schema(schema["items"])
    if schema.get("type") == "file":
        schema = {"type": "string", "format": "binary"}
    if described and parameter.get("description"):
        schema["description"] = parameter["description"]
    return schema


def convert_parameter(parameter: dict) -> dict:
    converted = {key: parameter[key] for key in ("name", "in", "description", "required") if key in parameter}
    converted["schema"] = parameter_schema(parameter)
    if converted["schema"].get("type") == "array" and parameter["in"] == "query":
        # Swagger 2.0 sends arrays comma-separated unless told otherwise.
        style = {"csv": ("form", False), "multi": ("form", True), "ssv": ("spaceDelimited", False),
                 "pipes": ("pipeDelimited", False)}.get(parameter.get("collectionFormat", "csv"))
        if style is None:
            sys.exit(f"parameter `{parameter['name']}`: collectionFormat "
                     f"`{parameter['collectionFormat']}` has no OpenAPI 3 equivalent")
        converted["style"], converted["explode"] = style
    return converted


def convert_response(response: dict, produces: list) -> dict:
    converted = {"description": response.get("description") or ""}
    if "schema" in response:
        converted["content"] = {media: {"schema": copy.deepcopy(response["schema"])} for media in produces}
    for media, example in (response.get("examples") or {}).items():
        converted.setdefault("content", {}).setdefault(media, {})["example"] = example
    if response.get("headers"):
        converted["headers"] = {
            name: {**({"description": h["description"]} if h.get("description") else {}),
                   "schema": parameter_schema(h)}
            for name, h in response["headers"].items()
        }
    return converted


def convert_security_scheme(scheme: dict) -> dict:
    if scheme.get("type") == "basic":
        return {"type": "http", "scheme": "basic", **{k: v for k, v in scheme.items() if k == "description"}}
    if scheme.get("type") != "oauth2":
        return scheme
    flow = {key: scheme[key] for key in ("authorizationUrl", "tokenUrl") if key in scheme}
    flow["scopes"] = scheme.get("scopes") or {}
    return {"type": "oauth2", **{k: v for k, v in scheme.items() if k == "description"},
            "flows": {OAUTH2_FLOWS.get(scheme.get("flow"), scheme.get("flow")): flow}}


def convert_schemas(spec: dict) -> None:
    """Schema keywords Swagger 2.0 spells differently: `x-nullable`, `type:
    file` and a string `discriminator`."""
    def visit(node):
        if "x-nullable" in node:
            node["nullable"] = bool(node.pop("x-nullable"))
        if node.get("type") == "file":
            node["type"], node["format"] = "string", "binary"
        if isinstance(node.get("discriminator"), str):
            node["discriminator"] = {"propertyName": node["discriminator"]}

    walk_schema(spec, visit)


def walk_schema(node, visit):
    """Call `visit` on every mapping under `node`, skipping data such as examples.
    A `properties` or `responses` map is walked whatever its names are."""
    if isinstance(node, list):
        for value in node:
            walk_schema(value, visit)
    elif isinstance(node, dict):
        visit(node)
        for key, value in node.items():
            if key in NAME_MAPS and isinstance(value, dict):
                for prop in value.values():
                    walk_schema(prop, visit)
            elif key not in DATA_KEYS:
                walk_schema(value, visit)


def component_ref(ref: str) -> str:
    """`#/components/schemas/Deal/properties/status` -> `#/components/schemas/Deal`."""
    return "/".join(ref.split("/")[:4])


def refs_in(node, found):
    def visit(n):
        ref = n.get("$ref")
        if isinstance(ref, str) and ref.startswith("#/components/"):
            found.add(component_ref(ref))

    walk_schema(node, visit)


def resolve(spec, ref):
    node = spec
    for part in ref[2:].split("/"):
        node = node[part.replace("~1", "/").replace("~0", "~")]
    return node


def deref(spec, node):
    while isinstance(node, dict) and isinstance(node.get("$ref"), str):
        node = resolve(spec, node["$ref"])
    return node


def is_json(media_type: str) -> bool:
    return media_type.split(";")[0].strip().endswith("json")


def model_name(name: str) -> str:
    """Approximates openapi-generator's Rust model name: `deal_status` -> `DealStatus`."""
    return "".join(word[:1].upper() + word[1:] for word in re.split(r"[^A-Za-z0-9]+", name) if word)


def clean_text(text: str) -> str:
    """Text goes into `r##"…"##` literals, which `"#` would end early."""
    return text.replace('"#', '" #')


def is_scalar(schema: dict) -> bool:
    return schema.get("type") not in ("object", "array") and "properties" not in schema


def is_object(schema: dict) -> bool:
    return schema.get("type") == "object" or "properties" in schema


def check_parameter(spec: dict, name: str, parameter: dict) -> list:
    """The parameters `parameter` is sent as: none (an optional cookie), itself,
    or one per field of an object query parameter."""
    resolved = deref(spec, parameter)
    if resolved.get("in") == "cookie":
        if resolved.get("required"):
            sys.exit(f"{name}: required cookie parameter `{resolved['name']}` is not supported")
        print(f"note: {name}: dropped optional cookie parameter `{resolved['name']}`")
        return []
    schema = deref(spec, resolved.get("schema") or {})
    if resolved.get("in") == "query" and schema.get("type") == "array" and resolved.get("style", "form") != "form":
        # The templates send an array repeated or comma-separated, nothing else.
        sys.exit(f"{name}: array query parameter `{resolved['name']}` with style `{resolved['style']}` "
                 "is not supported")
    if resolved.get("in") == "query" and (
        resolved.get("style") == "deepObject" or schema.get("type") == "object"
    ):
        return expand_object_parameter(spec, name, resolved, schema)
    alternatives = [deref(spec, s) for s in schema.get("oneOf") or schema.get("anyOf") or []]
    if alternatives:
        if not all(is_scalar(s) for s in alternatives):
            sys.exit(f"{name}: parameter `{resolved['name']}` has non-scalar alternatives (oneOf/anyOf)")
        # Sent as text whichever alternative the value is.
        return [{**resolved, "schema": {"type": "string"}}]
    return [parameter]


def expand_object_parameter(spec: dict, name: str, parameter: dict, schema: dict) -> list:
    """One query parameter per field of an object parameter, named as its style
    sends it: `page` with the fields `size` and `after` is `page[size]` and
    `page[after]` with `style: deepObject`, and `size` and `after` with the
    default `style: form` (exploded). Other styles are refused.

    Of a oneOf/anyOf, the alternative that declares fields is expanded, and a
    scalar alternative stays an input under the parameter's own name: Zendesk's
    `page` is a page number or a cursor object, and both are kept."""
    param = parameter["name"]
    style = parameter.get("style", "form")
    if style not in ("deepObject", "form") or not parameter.get("explode", True):
        sys.exit(f"{name}: object query parameter `{param}` with style `{style}`"
                 f"{'' if parameter.get('explode', True) else ' (explode: false)'} is not supported")

    def wire_name(field):
        return f"{param}[{field}]" if style == "deepObject" else field

    alternatives = [deref(spec, s) for s in schema.get("oneOf") or schema.get("anyOf") or [schema]]
    objects = [s for s in alternatives if s.get("properties")]
    if len(objects) != 1:
        sys.exit(f"{name}: object query parameter `{param}` ({style}) without declared fields is not supported")
    scalars = [s for s in alternatives if s is not objects[0]]
    if not all(is_scalar(s) for s in scalars):
        sys.exit(f"{name}: query parameter `{param}` has non-scalar alternatives besides its object")
    fields = objects[0]["properties"]
    # A required parameter makes the object's required fields required inputs,
    # unless a scalar alternative alone also satisfies it: then every input is
    # optional, and choosing one form is left to the vendor.
    required = [] if scalars else objects[0].get("required") or []
    expanded = []
    if scalars:
        scalar = copy.deepcopy(scalars[0]) if len(scalars) == 1 else {"type": "string"}
        description = scalar.pop("description", None) or parameter.get("description")
        expanded.append({
            "name": param,
            "in": "query",
            **({"description": description} if description else {}),
            "schema": scalar,
        })
    for field, field_schema in fields.items():
        field_schema = copy.deepcopy(deref(spec, field_schema))
        if not is_scalar(field_schema):
            sys.exit(f"{name}: field `{field}` of the object query parameter `{param}` is not a scalar")
        description = field_schema.pop("description", None) or parameter.get("description")
        expanded.append({
            "name": wire_name(field),
            "in": "query",
            **({"description": description} if description else {}),
            **({"required": True} if parameter.get("required") and field in required else {}),
            "schema": field_schema,
        })
    if scalars:
        print(f"note: {name}: `{param}` is either `{param}` or "
              f"{', '.join(f'`{wire_name(f)}`' for f in fields)}; send one form")
    return expanded


def envelope_of(spec: dict, name: str, media: dict, envelopes: list) -> str:
    """The `body_envelopes` name that wraps this request body, if any."""
    properties = deref(spec, media.get("schema") or {}).get("properties") or {}
    found = [key for key in properties if key in envelopes]
    if len(found) > 1:
        sys.exit(f"{name}: request body has several envelopes {found}")
    return found[0] if found else None


def unwrap_envelope(spec: dict, name: str, body: dict, media: dict, envelopes: list) -> str:
    """A body such as `{"ticket": {...}}` takes the ticket's fields as step
    inputs: the body schema becomes the envelope's schema, and the template
    wraps the fields back into `ticket`. Returns the envelope it unwrapped."""
    key = envelope_of(spec, name, media, envelopes)
    if key is None:
        return None
    wrapper = deref(spec, media.get("schema") or {})
    properties = wrapper["properties"]
    envelope = deref(spec, properties[key])
    schema = properties[key]
    alternatives = [deref(spec, s) for s in envelope.get("anyOf") or envelope.get("oneOf") or []]
    if not envelope.get("properties") and (not alternatives or not all(s.get("properties") for s in alternatives)):
        print(f"note: {name}: `{key}` declares no fields, so it stays one `{key}` input")
        return key
    dropped = [other for other in properties if other != key]
    required = [field for field in dropped if field in (wrapper.get("required") or [])]
    if required:
        sys.exit(f"{name}: unwrapping `{key}` would drop the required body fields {required}")
    if not envelope.get("properties"):
        # One of several shapes (create or merge, say): offer every field of
        # every shape, all optional, and let the vendor validate the mix.
        fields = {}
        for alternative in alternatives:
            for field, field_schema in alternative["properties"].items():
                fields.setdefault(field, field_schema)
        print(f"note: {name}: `{key}` is one of {len(alternatives)} shapes; "
              "the fields of all of them are optional inputs")
        schema = {"type": "object", "properties": fields}
    if dropped:
        print(f"note: {name}: dropped the body fields {dropped} beside the `{key}` envelope")
    media["schema"] = schema
    # On the request body, where the template's body section can see it.
    body["x-runtara-body-envelope"] = key
    return key


def check_operation(spec: dict, path: str, method: str, op: dict, item: dict, envelopes: list = ()) -> set:
    """Refuse the shapes the templates cannot express yet, and normalize the rest.
    Returns the `envelopes` the operation's body uses."""
    name = op["operationId"]

    for owner in (item, op):
        if "parameters" in owner:
            owner["parameters"] = [
                kept for parameter in owner["parameters"]
                for kept in check_parameter(spec, name, parameter)
            ]

    used = set()
    body = deref(spec, op.get("requestBody"))
    if isinstance(body, dict) and body.get("content"):
        json_types = {k: v for k, v in body["content"].items() if is_json(k)}
        if not json_types:
            sys.exit(
                f"{name}: request body {sorted(body['content'])} is not supported yet; "
                "only JSON bodies are (form and multipart bodies need template support)"
            )
        body["content"] = json_types
        if any(envelope_of(spec, name, media, envelopes) for media in json_types.values()):
            # The body may be a `#/components/requestBodies` entry other operations
            # share: unwrap this operation's own copy, or the next one would unwrap
            # the already unwrapped fields again.
            body = copy.deepcopy(body)
            op["requestBody"] = body
        for media in body["content"].values():
            used.add(unwrap_envelope(spec, name, body, media, envelopes))

    # An operation with several tags would be rendered once per tag.
    if len(op.get("tags") or []) > 1:
        op["tags"] = op["tags"][:1]

    # Facts the logic-less templates branch on, and text they print as is.
    if method != "get":
        op["x-runtara-side-effects"] = True
    op["x-runtara-summary"] = clean_text(op.get("summary") or humanize(name))
    return used - {None}


def drop_connection_parameters(spec: dict, op: dict, item: dict, names: list, locations: tuple) -> set:
    """Parameters the connection supplies are not step inputs: headers it sends
    (`connection_headers`, UPS's `transId`), and header or query parameters
    carrying its credential (`connection_parameters`, Slack's `token`).
    Returns the names dropped."""
    dropped = set()
    for owner in (item, op):
        if "parameters" not in owner:
            continue
        kept = []
        for parameter in owner["parameters"]:
            resolved = deref(spec, parameter)
            if resolved.get("in") in locations and resolved.get("name") in names:
                dropped.add(resolved["name"])
            else:
                kept.append(parameter)
        owner["parameters"] = kept
    return dropped


def is_pinned(spec: dict, parameter: dict, names: set) -> bool:
    resolved = deref(spec, parameter)
    return resolved.get("in") == "path" and resolved.get("name") in names


def pin_path_parameters(spec: dict, path: str, op: dict, item: dict, names: list) -> tuple:
    """A path parameter listed in `pinned_path_parameters` is fixed to the
    spec's own default: UPS's `/shipments/{version}/ship` with `version`
    defaulting to `v2409` becomes `/shipments/v2409/ship`, the API version the
    generated types describe, and is no step input. Returns the path and the
    names pinned."""
    values = {}
    # Operation parameters override path-level ones of the same name.
    for owner in (item, op):
        for parameter in owner.get("parameters") or []:
            if is_pinned(spec, parameter, names):
                resolved = deref(spec, parameter)
                default = deref(spec, resolved.get("schema") or {}).get("default")
                if default is None:
                    sys.exit(f"{op['operationId']}: pinned path parameter `{resolved['name']}` has no default")
                values[resolved["name"]] = str(default)
    for name, value in values.items():
        path = path.replace("{" + name + "}", urllib.parse.quote(value, safe=""))
    if "parameters" in op:
        op["parameters"] = [p for p in op["parameters"] if not is_pinned(spec, p, set(values))]
    return path, set(values)


def inline_refs(sliced: dict, spec: dict) -> None:
    """Replace two kinds of `$ref` with the schema they point to:

    - a `$ref` to an enum schema, so the field renders as its value type (the
      generator would otherwise reference an enum type);
    - a `$ref` into a component's internals (`…/Deal/properties/status`), so
      the field is labelled by its own name and resolves inside the slice.

    Keys next to a `$ref` (a description, say) are kept. Components nothing
    references any more are then dropped."""
    schemas = sliced["components"].get("schemas", {})

    def target(ref):
        if not isinstance(ref, str) or not ref.startswith("#/components/"):
            return None
        if ref.count("/") > 3:
            return resolve(spec, ref)
        schema = schemas.get(ref.rsplit("/", 1)[1]) if ref.startswith("#/components/schemas/") else None
        if isinstance(schema, dict) and "enum" in schema and schema.get("type") != "object":
            return schema
        return None

    def replace(node, properties=False, expanding=()):
        if isinstance(node, list):
            return [replace(value, expanding=expanding) for value in node]
        if not isinstance(node, dict):
            return node
        if properties:
            return {name: replace(schema, expanding=expanding) for name, schema in node.items()}
        ref = node.get("$ref")
        schema = target(ref)
        if schema is not None and ref not in expanding:
            siblings = {key: value for key, value in node.items() if key != "$ref"}
            return replace({**copy.deepcopy(schema), **siblings}, expanding=expanding + (ref,))
        return {
            key: replace(value, properties=True, expanding=expanding)
            if key in NAME_MAPS and isinstance(value, dict)
            else value if key in DATA_KEYS
            else replace(value, expanding=expanding)
            for key, value in node.items()
        }

    sliced["paths"] = replace(sliced["paths"])
    sliced["components"] = replace(sliced["components"])
    prune_components(sliced)


def prune_components(sliced: dict) -> None:
    reachable, pending = set(), set()
    refs_in(sliced["paths"], pending)
    while pending:
        ref = pending.pop()
        if ref not in reachable:
            reachable.add(ref)
            refs_in(resolve(sliced, ref), pending)
    for kind in list(sliced["components"]):
        if kind == "securitySchemes":
            continue
        entries = sliced["components"][kind]
        for name in list(entries):
            if f"#/components/{kind}/{name}" not in reachable:
                del entries[name]
        if not entries:
            del sliced["components"][kind]


def inline_example_refs(sliced: dict, spec: dict) -> None:
    """`refs_in` skips examples (they are data), so a `$ref` to
    `#/components/examples/…` is replaced by the example it names."""
    def visit(node):
        examples = node.get("examples")
        if isinstance(examples, dict):
            for key, example in examples.items():
                if isinstance(example, dict) and isinstance(example.get("$ref"), str):
                    examples[key] = copy.deepcopy(deref(spec, example))

    walk_schema(sliced, visit)


# The keywords that apply to one JSON type only.
TYPE_KEYWORDS = {
    "array": ("items", "minItems", "maxItems", "uniqueItems"),
    "object": ("properties", "required", "additionalProperties", "minProperties", "maxProperties"),
    "string": ("pattern", "minLength", "maxLength"),
}


def normalize_schemas(sliced: dict) -> None:
    def visit(node):
        # JSON Schema's list forms, which OpenAPI 3.0 does not have: Slack's
        # `items: [A, B]` means one of A or B, and `type: [string, "null"]` a
        # nullable string.
        if isinstance(node.get("items"), list):
            alternatives = node.pop("items")
            if node.get("type") == "array":
                node["items"] = {"anyOf": alternatives}
            else:
                node["anyOf"] = alternatives
        if isinstance(node.get("type"), list):
            listed = node.pop("type")
            types = [t for t in listed if t != "null"]
            if len(types) < len(listed):
                node["nullable"] = True
            if len(types) == 1:
                node["type"] = types[0]
            elif types:
                # Each alternative takes the keywords of its own type along
                # (`items` goes with `array`, `properties` with `object`).
                node["anyOf"] = [
                    {"type": t, **{key: node[key] for key in TYPE_KEYWORDS.get(t, ()) if key in node}}
                    for t in types
                ]
                for key in {key for t in types for key in TYPE_KEYWORDS.get(t, ())}:
                    node.pop(key, None)
        # `type: "null"` alone (a field Slack always sends as null), or as one
        # alternative of a union, is OpenAPI 3.0's `nullable`.
        if node.get("type") == "null":
            del node["type"]
            node["nullable"] = True
        for key in ("anyOf", "oneOf"):
            if isinstance(node.get(key), list):
                kept = [s for s in node[key] if not (isinstance(s, dict) and s.get("type") == "null")]
                if len(kept) < len(node[key]):
                    node[key] = kept
                    node["nullable"] = True
        # Plain `integer` would become i32; vendor ids and amounts outgrow it.
        if node.get("type") == "integer" and "format" not in node:
            node["format"] = "int64"
        # Beside declared fields, `additionalProperties: true` makes the generator
        # lose the model type; every output struct keeps undeclared fields anyway.
        if node.get("properties") and node.get("additionalProperties") in (True, {}):
            del node["additionalProperties"]
        # A schema's `required` lists field names; one written on the field itself
        # (UPS's `required: SecurePINType`) fails the generator's spec validation.
        # Parameters, request bodies and headers have a boolean `required` of their
        # own, and in a map of names (`components.schemas`) `required` is an entry.
        if isinstance(node.get("required"), (str, bool)) and not node.keys() & {"in", "content", "schema"}:
            del node["required"]
        # An inline object is named after a one-word `title`: the generator spells
        # the struct `Pod` but types the field with the raw `POD` (UPS's proof of
        # delivery), so the title is written as the struct name. A title with
        # spaces is a label, not a name (see `describe_field`).
        title = node.get("title")
        if isinstance(title, str) and is_object(node) and rust_model_name(title) != model_name(title):
            if " " in title.strip():
                # A phrase names the inline object too (Slack's "Conversation IM
                # Channel object"): its words are respelled the generator's way,
                # and the phrase as written stays the field's label.
                node["x-runtara-title"] = title
                node["title"] = " ".join(
                    word if rust_model_name(word) == model_name(word) else rust_model_name(word)
                    for word in title.split(" ")
                )
            else:
                node["title"] = rust_model_name(title)
        for key in ("description", "summary", "title"):
            if isinstance(node.get(key), str):
                node[key] = clean_text(node[key])

    walk_schema(sliced, visit)


def rust_model_name(name: str) -> str:
    """openapi-generator's Rust model name: `EmailCCObject` -> `EmailCcObject`,
    `IPv4Address` -> `Ipv4Address`. The patterns are its `StringUtils.underscore`."""
    spaced = re.sub(r"([A-Z]+)([A-Z][a-z][a-z]+)", r"\1_\2", name)
    spaced = re.sub(r"([a-z\d])([A-Z])", r"\1_\2", spaced)
    return "".join(word[:1].upper() + word[1:].lower() for word in re.split(r"[^A-Za-z0-9]+", spaced) if word)


def rename_schemas(sliced: dict, model_names: dict) -> None:
    """Rename schemas whose Rust struct name differs from the name the templates
    spell their references with (`EmailCCObject` is generated as `EmailCcObject`)
    to the struct name, so both agree, including the inline types named after
    them. A `model_names` entry takes precedence."""
    schemas = sliced["components"].get("schemas", {})
    renames = {
        name: rust_model_name(name) for name in schemas
        if name not in model_names and rust_model_name(name) != model_name(name)
    }
    for old, new in renames.items():
        if new in schemas:
            sys.exit(f"schema `{old}` is generated as `{new}`, which the spec also defines; "
                     f"rename it with `model_names: {{{old}: <OtherName>}}` in connector.yaml")
    rename_refs(sliced, renames)


def rename_refs(sliced: dict, renames: dict) -> None:
    """Rename schemas, and every `$ref` to them."""
    if not renames:
        return

    def visit(node):
        if isinstance(node, list):
            for value in node:
                visit(value)
        elif isinstance(node, dict):
            ref = node.get("$ref")
            if isinstance(ref, str) and ref.startswith("#/components/schemas/"):
                parts = ref.split("/")
                parts[3] = renames.get(parts[3], parts[3])
                node["$ref"] = "/".join(parts)
            for value in node.values():
                visit(value)

    visit(sliced)
    schemas = sliced["components"].get("schemas", {})
    sliced["components"]["schemas"] = {renames.get(name, name): schema for name, schema in schemas.items()}


def check_names(sliced: dict, operation_ids: list, model_names: dict) -> None:
    reserved = RESERVED_TYPE_NAMES | {model_name(op) + "Input" for op in operation_ids}
    for name in sliced["components"].get("schemas", {}):
        generated = model_names.get(name, model_name(name))
        if generated in reserved:
            sys.exit(
                f"schema `{name}` would generate the type `{generated}`, which the generated "
                f"code already uses; rename it with `model_names: {{{name}: <OtherName>}}` "
                "in connector.yaml"
            )


# The `connector.yaml` lists whose names must each match part of an operation,
# and what `fetch` says about a name that matches nothing.
NAME_LISTS = {
    "body_envelopes": "wrap no request body",
    "connection_headers": "are no header parameter",
    "connection_parameters": "are no header or query parameter",
    "pinned_path_parameters": "are no path parameter",
}


def check_used(options: dict, used: dict) -> None:
    """A listed name that matched nothing is a typo or a stale entry."""
    for key, what in NAME_LISTS.items():
        unused = set(options.get(key) or ()) - used.get(key, set())
        if unused:
            sys.exit(f"{key} {sorted(unused)} {what} of the listed operations")


def slice_spec(spec: dict, operation_ids: list, model_names: dict = None, body_envelopes: list = (),
               connection_headers: list = (), connection_parameters: list = (),
               pinned_path_parameters: list = ()) -> dict:
    """Keep the listed operations and every component they reference.

    openapi-generator's FILTER normalizer drops unlisted operations but still
    renders every model in the spec, so the slice is what keeps the generated
    file to the selected surface.
    """
    options = {"body_envelopes": body_envelopes, "connection_headers": connection_headers,
               "connection_parameters": connection_parameters,
               "pinned_path_parameters": pinned_path_parameters}
    sliced, used = slice_one(spec, operation_ids, operation_ids[0], model_names or {}, **options)
    check_used(options, used)
    return sliced


def slice_specs(specs: list, config: dict) -> dict:
    """The slice of every `spec_sources` entry, paired here with its loaded spec,
    merged into one when there are several."""
    options = {key: config.get(key) or [] for key in NAME_LISTS}
    model_names = config.get("model_names") or {}
    first = specs[0][0]["operations"][0]
    slices, used = [], {}
    for source, spec in specs:
        sliced, found = slice_one(spec, source["operations"], first, model_names, **options)
        for key, names in found.items():
            used.setdefault(key, set()).update(names)
        slices.append((source.get("name"), sliced))
    # A name only needs to match in one of the specs.
    check_used(options, used)
    if len(slices) == 1:
        sliced = slices[0][1]
    else:
        sliced = merge_slices(slices)
        check_names(sliced, [op for source, _ in specs for op in source["operations"]], model_names)
    make_optional(sliced, config.get("optional_fields") or {})
    retype_inputs(sliced, config.get("input_types") or {})
    return sliced


# What a retyped input keeps of the schema the vendor gave it.
RETYPE_KEPT = ("description", "title", "deprecated", "nullable", "readOnly", "writeOnly")


def retype_inputs(sliced: dict, types: dict) -> None:
    """`input_types`: input name -> the type (`string`) or schema the vendor's
    spec should have given it, applied to the parameters and request-body
    fields of every listed operation. Slack's spec types message timestamps
    (`ts`) as numbers, which would send `1712345678.000100` as
    `1712345678.0001`, another message's id."""
    if not types:
        return

    def retyped(schema: dict, name: str) -> dict:
        new = types[name]
        new = copy.deepcopy(new) if isinstance(new, dict) else {"type": new}
        return {**new, **{key: schema[key] for key in RETYPE_KEPT if key in schema}}

    used = set()
    for item in sliced["paths"].values():
        owners = [item] + [op for method, op in item.items() if method in HTTP_METHODS]
        for owner in owners:
            for parameter in owner.get("parameters") or []:
                parameter = deref(sliced, parameter)
                if parameter.get("name") in types and parameter.get("in") in ("path", "query", "header"):
                    parameter["schema"] = retyped(deref(sliced, parameter.get("schema") or {}), parameter["name"])
                    used.add(parameter["name"])
        for method, op in item.items():
            body = deref(sliced, op.get("requestBody")) if method in HTTP_METHODS else None
            for media in (body or {}).get("content", {}).values():
                properties = deref(sliced, media.get("schema") or {}).get("properties") or {}
                for name in properties.keys() & types.keys():
                    properties[name] = retyped(deref(sliced, properties[name]), name)
                    used.add(name)
    unused = set(types) - used
    if unused:
        sys.exit(f"input_types {sorted(unused)} are no input of the listed operations")


def make_optional(sliced: dict, fields: dict) -> None:
    """`optional_fields`: schema name -> fields the vendor's spec requires but its
    API does not (UPS's label recovery needs one of several keys; its spec
    requires all of them)."""
    schemas = sliced["components"].get("schemas", {})
    for schema, names in fields.items():
        required = schemas.get(schema, {}).get("required") or []
        stale = [name for name in names if name not in required]
        if stale:
            sys.exit(f"optional_fields: `{schema}` in spec.slice.json does not require {stale}")
        schemas[schema]["required"] = [name for name in required if name not in names]
        if not schemas[schema]["required"]:
            del schemas[schema]["required"]


def slice_one(spec: dict, operation_ids: list, first: str, model_names: dict, body_envelopes=(),
              connection_headers=(), connection_parameters=(), pinned_path_parameters=()) -> tuple:
    """The slice of one spec, and the listed names (`NAME_LISTS`) it used.
    `first` is the operation that carries the agent's module attributes."""
    if is_swagger2(spec):
        spec = convert_swagger2(spec)
    if "openapi" not in spec:
        sys.exit("only OpenAPI 3.x and Swagger 2.0 specs are supported")

    wanted = set(operation_ids)
    paths = {}
    for path, item in spec.get("paths", {}).items():
        kept = {m: op for m, op in item.items() if m in HTTP_METHODS and op.get("operationId") in wanted}
        if kept:
            if "parameters" in item:
                kept["parameters"] = item["parameters"]
            paths[path] = kept

    found, used = set(), {key: set() for key in NAME_LISTS}
    sliced_paths = {}
    for path, item in paths.items():
        targets = []
        for method, op in item.items():
            if method not in HTTP_METHODS:
                continue
            found.add(op["operationId"])
            used["connection_headers"] |= drop_connection_parameters(
                spec, op, item, connection_headers, ("header",))
            used["connection_parameters"] |= drop_connection_parameters(
                spec, op, item, connection_parameters, ("header", "query"))
            used["body_envelopes"] |= check_operation(spec, path, method, op, item, body_envelopes)
            target, pinned = pin_path_parameters(spec, path, op, item, pinned_path_parameters)
            used["pinned_path_parameters"] |= pinned
            if op["operationId"] == first:
                op["x-runtara-first"] = True
            entry = sliced_paths.setdefault(target, {})
            if method in entry:
                sys.exit(f"{op['operationId']}: pinned, it is `{method.upper()} {target}` like another operation")
            entry[method] = op
            targets.append((target, pinned))
        if "parameters" in item:
            for target, pinned in targets:
                sliced_paths[target]["parameters"] = [
                    p for p in item["parameters"] if not is_pinned(spec, p, pinned)
                ]
    paths = sliced_paths
    missing = wanted - found
    if missing:
        sys.exit(f"operations not found in the spec: {sorted(missing)}")

    pending, seen = set(), set()
    refs_in(paths, pending)
    while pending:
        ref = pending.pop()
        if ref not in seen:
            seen.add(ref)
            refs_in(resolve(spec, ref), pending)

    components = {}
    for ref in sorted(seen):
        _, _, kind, name = ref.split("/", 3)
        components.setdefault(kind, {})[name] = resolve(spec, ref)
    if "securitySchemes" in spec.get("components", {}):
        components["securitySchemes"] = spec["components"]["securitySchemes"]

    sliced = {key: spec[key] for key in ("openapi", "info", "servers", "security") if key in spec}
    sliced["paths"] = paths
    sliced["components"] = components
    inline_example_refs(sliced, spec)
    inline_refs(sliced, spec)
    normalize_schemas(sliced)
    rename_schemas(sliced, model_names)
    check_names(sliced, operation_ids, model_names)
    return sliced, used


def canonical(value) -> str:
    return json.dumps(value, sort_keys=True)


def merge_slices(slices: list) -> dict:
    """One slice from the slices of several specs of the same API, each paired
    with its `specs:` name. Paths are unioned. Two specs that define a schema
    differently under one generated name (UPS's Tracking and Shipping both
    define `ErrorResponse`) each get it renamed `<spec name><Schema>`
    (`TrackingErrorResponse`); a schema they define identically is kept once."""
    # Names are compared as the prefixes they become: `Tracking` and `tracking`
    # would rename a schema to the same `TrackingErrorResponse`, forever.
    prefixes = [rust_model_name(name or "") for name, _ in slices]
    if not all(prefixes) or len(set(prefixes)) != len(prefixes):
        sys.exit("each entry of `specs` needs a `name` of its own, "
                 "not one that differs from another only in case or punctuation")
    if len({canonical(sliced.get("servers")) for _, sliced in slices}) > 1:
        sys.exit("the specs list different `servers`; the generated requests share one base path")

    # Until no generated name clashes: renaming a schema changes the schemas
    # that refer to it, which may then differ between the specs too.
    while True:
        defined = {}
        for name, sliced in slices:
            for schema, value in sliced["components"].get("schemas", {}).items():
                defined.setdefault(model_name(schema), {})[name] = (schema, canonical(value))
        clashes = {generated: by_spec for generated, by_spec in defined.items() if len(set(by_spec.values())) > 1}
        if not clashes:
            break
        for name, sliced in slices:
            renames = {
                by_spec[name][0]: rust_model_name(f"{name}_{generated}")
                for generated, by_spec in clashes.items() if name in by_spec
            }
            for old, new in renames.items():
                if new in defined:
                    sys.exit(f"schema `{old}` of `{name}` would be renamed `{new}`, which a spec already defines")
            rename_refs(sliced, renames)

    merged = {key: slices[0][1][key] for key in ("openapi", "info", "servers", "security") if key in slices[0][1]}
    merged["paths"], merged["components"] = {}, {}
    for name, sliced in slices:
        for path, item in sliced["paths"].items():
            target = merged["paths"].setdefault(path, {})
            for key, value in item.items():
                if key in target and (key in HTTP_METHODS or canonical(target[key]) != canonical(value)):
                    sys.exit(f"`{path}` has `{key}` in more than one spec")
                target[key] = value
        for kind, entries in sliced["components"].items():
            for entry, value in entries.items():
                kept = merged["components"].setdefault(kind, {}).setdefault(entry, value)
                # The templates do not render security schemes; the first spec's stay.
                if kind != "securitySchemes" and canonical(kept) != canonical(value):
                    sys.exit(f"`#/components/{kind}/{entry}` differs between the specs")
    return merged


# Words generated labels spell in their usual form rather than capitalized.
LABEL_WORDS = {
    "id": "ID", "ids": "IDs", "url": "URL", "urls": "URLs", "uri": "URI",
    "api": "API", "html": "HTML", "cc": "CC", "bcc": "BCC", "sku": "SKU",
    "skus": "SKUs", "vat": "VAT", "utc": "UTC", "ip": "IP", "json": "JSON",
    "xml": "XML", "csv": "CSV", "pdf": "PDF", "sms": "SMS", "iban": "IBAN",
    "bic": "BIC", "ean": "EAN", "gtin": "GTIN", "upc": "UPC", "uuid": "UUID",
    "iso": "ISO", "http": "HTTP", "https": "HTTPS", "oauth": "OAuth",
}


def humanize(name: str) -> str:
    """`person_id`, `personId` and `person-id` all become `Person ID`, and
    `page[size]` becomes `Page Size`."""
    spaced = re.sub(r"([a-z0-9])([A-Z])", r"\1 \2", name)
    spaced = re.sub(r"([A-Z]+)([A-Z][a-z])", r"\1 \2", spaced)
    words = [word for word in re.split(r"[\s_\-.\[\]]+", spaced) if word]
    return " ".join(LABEL_WORDS.get(word.lower(), word[:1].upper() + word[1:]) for word in words)


def scalar_text(value):
    """A scalar as the plain text `#[field(example = …)]` shows, else None."""
    if isinstance(value, str):
        return clean_text(value)
    if isinstance(value, (bool, int, float)):
        return json.dumps(value)
    return None


def scalar_json(value):
    """A scalar as the JSON text `#[field(default = …)]` parses, else None."""
    if isinstance(value, (str, bool, int, float)):
        return clean_text(json.dumps(value))
    return None


def describe_field(target: dict, name: str, schema: dict, labels: dict, required: bool) -> None:
    """Mark a parameter or property with the label, example and default the
    templates render. A `connector.yaml` label wins; then the vendor's `title`,
    but only when it reads as a phrase (one-word titles name models); then the
    humanized field name. Required fields get no default: the metadata would
    then call them optional while the step still has to send them."""
    title = schema.get("x-runtara-title", schema.get("title"))
    if name in labels:
        label = labels[name]
    elif isinstance(title, str) and " " in title.strip():
        label = title.strip()
    else:
        label = humanize(name)
    target["x-runtara-label"] = clean_text(label)
    example = scalar_text(target.get("example", schema.get("example")))
    if example is not None:
        target["x-runtara-example"] = example
    default = scalar_json(schema.get("default"))
    if default is not None and not required:
        target["x-runtara-default"] = default


def describe_fields(node, labels: dict) -> None:
    """Annotate every parameter and schema property under `node`."""
    if isinstance(node, list):
        for value in node:
            describe_fields(value, labels)
        return
    if not isinstance(node, dict):
        return
    is_parameter = (
        isinstance(node.get("name"), str)
        and node.get("in") in ("path", "query", "header", "cookie")
        and ("schema" in node or "content" in node)
    )
    if is_parameter:
        describe_field(node, node["name"], node.get("schema") or {}, labels, bool(node.get("required")))
    properties = node.get("properties")
    if isinstance(properties, dict):
        required = node.get("required") if isinstance(node.get("required"), list) else []
        for name, prop in list(properties.items()):
            if not isinstance(prop, dict):
                continue
            if "$ref" in prop:
                # openapi-generator drops keys beside a `$ref` (OpenAPI 3.0 ignores
                # them), label included; `allOf` of the one `$ref` is the same type
                # and keeps them.
                prop = properties[name] = {"allOf": [{"$ref": prop["$ref"]}],
                                           **{key: value for key, value in prop.items() if key != "$ref"}}
            describe_field(prop, name, prop, labels, name in required)
            describe_fields(prop, labels)
    for key, value in node.items():
        # `properties` maps names to schemas and was walked above; the rest is data.
        if key == "responses" and isinstance(value, dict):
            describe_fields(list(value.values()), labels)
        elif key not in ("properties",) + DATA_KEYS:
            describe_fields(value, labels)


def response_examples(spec: dict) -> dict:
    """operationId -> the JSON example of its first 2xx response, if any."""
    examples = {}
    for item in spec["paths"].values():
        for method, op in item.items():
            if method not in HTTP_METHODS:
                continue
            for code, response in sorted(op.get("responses", {}).items()):
                media = response.get("content", {}).get("application/json", {})
                example = media.get("example")
                if example is None and media.get("examples"):
                    example = next(iter(media["examples"].values())).get("value")
                if code.startswith("2") and example is not None:
                    examples[op["operationId"]] = example
                    break
    return examples


def write_json(path: Path, value) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "w") as f:
        json.dump(value, f, indent=2, sort_keys=True, ensure_ascii=False)
        f.write("\n")


def download(source: dict) -> bytes:
    """The spec at `source["url"]`, refused unless it matches the pinned sha256."""
    # Some vendor CDNs (Zendesk's) refuse urllib's default `Python-urllib` agent.
    request = urllib.request.Request(source["url"], headers={"User-Agent": "runtara-connector-gen"})
    with urllib.request.urlopen(request) as response:
        raw = response.read()
    digest = hashlib.sha256(raw).hexdigest()
    if digest != source["sha256"]:
        sys.exit(
            f"{source['url']} changed: sha256 is {digest}, connector.yaml pins {source['sha256']}.\n"
            "Review the vendor's change, update the pin, then fetch again."
        )
    return raw


def fetch(crate: Path) -> None:
    config = load_config(crate)
    sliced = slice_specs([(source, load_spec(download(source))) for source in spec_sources(config)], config)
    describe_fields(sliced, config.get("labels") or {})
    write_json(crate / "spec.slice.json", sliced)
    fixtures = crate / "tests" / "fixtures"
    for stale in fixtures.glob("*.json") if fixtures.exists() else []:
        stale.unlink()
    for operation_id, example in response_examples(sliced).items():
        write_json(fixtures / f"{operation_id}.json", example)
    print(f"{crate / 'spec.slice.json'}: {len(operation_ids(config))} operations")


# ---------------------------------------------------------------------------
# generate / check
# ---------------------------------------------------------------------------


def generator_cli() -> str:
    candidates = [
        os.environ.get("OPENAPI_GENERATOR_CLI"),
        str(TOOL_DIR / ".venv" / "bin" / "openapi-generator-cli"),
        shutil.which("openapi-generator-cli"),
    ]
    for candidate in candidates:
        if candidate and Path(candidate).exists():
            return candidate
    sys.exit("openapi-generator-cli not found; see scripts/connector_gen/README.md")


def integration_ids(agent: dict) -> list:
    """`integration_id` names one connection type, or a list of them."""
    ids = agent["integration_id"]
    return [ids] if isinstance(ids, str) else list(ids)


def parameter_name_mappings(sliced: dict) -> dict:
    """`page[size]` would become the Rust field
    `page_left_square_bracket_size_right_square_bracket`; name it `page_size`.
    The step input keeps the wire name."""
    names = set()

    def visit(node):
        if isinstance(node.get("name"), str) and node.get("in") in ("path", "query", "header"):
            names.add(node["name"])

    walk_schema(sliced, visit)
    return {
        name: re.sub(r"[^A-Za-z0-9]+", "_", name).strip("_").lower()
        for name in sorted(names) if re.search(r"[\[\]]", name)
    }


def render(crate: Path) -> str:
    """Render `generated.rs` for `crate` and return its rustfmt-ed text."""
    config = load_config(crate)
    agent = config["agent"]
    sliced = json.loads((crate / "spec.slice.json").read_text())
    with tempfile.TemporaryDirectory() as tmp:
        out = Path(tmp) / "out"
        out.mkdir()
        # Only the single generated file is wanted from the run.
        (out / ".openapi-generator-ignore").write_text("*\n**/*\n!src/generated.rs\n")
        config_path = Path(tmp) / "config.json"
        config_path.write_text(json.dumps({
            "generatorName": "rust",
            "templateDir": str(TEMPLATE_DIR),
            "files": {
                "generated.rs.mustache": {
                    "templateType": "SupportingFiles",
                    "destinationFilename": "src/generated.rs",
                },
            },
            "additionalProperties": {
                "agentId": agent["id"],
                "agentName": clean_text(agent["name"]),
                "agentDescription": clean_text(agent["description"]),
                "integrationIds": integration_ids(agent),
                "integrationIdList": ",".join(integration_ids(agent)),
                "errorPrefix": agent["error_prefix"],
            },
            "typeMappings": TYPE_MAPPINGS,
            "modelNameMappings": config.get("model_names") or {},
            "parameterNameMappings": parameter_name_mappings(sliced),
        }))
        result = subprocess.run(
            [generator_cli(), "generate", "-c", str(config_path),
             "-i", str(crate / "spec.slice.json"), "-o", str(out)],
            capture_output=True, text=True,
        )
        generated = out / "src" / "generated.rs"
        if result.returncode != 0 or not generated.exists():
            sys.exit(f"openapi-generator failed:\n{result.stdout}\n{result.stderr}")
        # Run from the repository so rust-toolchain.toml picks the rustfmt
        # that `cargo fmt --check` uses.
        subprocess.run(["rustfmt", "--edition", "2024", str(generated)], cwd=REPO_ROOT, check=True)
        return generated.read_text()


def generate(crate: Path) -> None:
    target = crate / "src" / "generated.rs"
    target.write_text(render(crate))
    print(f"wrote {target}")


def drift(crate: Path) -> list:
    """What no longer matches `spec.slice.json`, short of re-downloading the spec."""
    config = load_config(crate)
    sliced = json.loads((crate / "spec.slice.json").read_text())
    problems = []

    listed = set(operation_ids(config))
    sliced_ops = {
        op["operationId"]
        for item in sliced["paths"].values()
        for method, op in item.items()
        if method in HTTP_METHODS
    }
    if listed != sliced_ops:
        problems.append(
            f"connector.yaml operations {sorted(listed - sliced_ops)} are not in spec.slice.json "
            f"and {sorted(sliced_ops - listed)} are not in connector.yaml"
        )

    applied = set()
    walk_schema(sliced, lambda node: applied.add(node.get("x-runtara-label")))
    for name, label in (config.get("labels") or {}).items():
        if clean_text(label) not in applied:
            problems.append(f"label override `{name}: {label}` is not in spec.slice.json")

    fixtures = crate / "tests" / "fixtures"
    actual = {p.stem: json.loads(p.read_text()) for p in fixtures.glob("*.json")} if fixtures.exists() else {}
    if actual != response_examples(sliced):
        problems.append("tests/fixtures do not match the response examples in spec.slice.json")
    return problems


def check(crates: list) -> None:
    failed = []
    for crate in crates:
        for problem in drift(crate):
            failed.append(f"{crate}: {problem} (run `fetch`)")
        target = crate / "src" / "generated.rs"
        expected = render(crate)
        actual = target.read_text() if target.exists() else ""
        if actual != expected:
            failed.append(f"{target} is stale or edited by hand (run `generate`)")
            diff = difflib.unified_diff(
                actual.splitlines(keepends=True), expected.splitlines(keepends=True),
                fromfile=f"{target} (committed)", tofile=f"{target} (regenerated)",
            )
            sys.stdout.writelines(list(diff)[:200])
    if failed:
        sys.exit("\n".join(failed))
    print(f"{len(crates)} connector(s) up to date")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("fetch", "generate"):
        commands.add_parser(name).add_argument("crate", type=Path)
    commands.add_parser("check").add_argument("crates", type=Path, nargs="+")
    args = parser.parse_args()

    if args.command == "fetch":
        fetch(args.crate.resolve())
    elif args.command == "generate":
        generate(args.crate.resolve())
    else:
        check([crate.resolve() for crate in args.crates])


if __name__ == "__main__":
    main()

"""A WordPress REST API for connector_gen: the routes a site serves, as an
OpenAPI 3.1 document the rest of the generator slices and renders.

WordPress publishes no OpenAPI. Every site describes its own REST API at
`/wp-json/<namespace>?context=help`: each route's regex path, the methods of
its endpoints with their arguments, and the JSON schema of what it answers.
A plugin's API (WooCommerce's `wc/v3`) is described the same way. The crate
keeps that document as a site served it (`spec: {path: …, format:
wordpress}`), and `fetch` turns it into OpenAPI here:

- `(?P<id>[\\d]+)` in a route becomes the path parameter `{id}`, an integer
  when its pattern is digits only, a string otherwise;
- a GET's or DELETE's arguments become query parameters, a POST's or PUT's a
  JSON body (WordPress takes JSON as well as forms), and an argument's
  `required: true` the body's `required`;
- WordPress's own types become JSON Schema's: `date-time` is a string of that
  format, `bool` is boolean, `mixed` and a union of several types are any
  value, and `[T, "null"]` stays as it is;
- `context`, `readonly` (as `readOnly`), callbacks and `$schema` are dropped
  or renamed, a `default` or `enum` value that doesn't fit its type is
  dropped, and so is every request-body `default`: some are the site's own
  settings (WooCommerce's order `currency` is the store's), and the site
  applies its defaults to a field a request leaves out anyway;
- WordPress lists POST, PUT and PATCH for an update: only PUT is kept, and
  only POST for a route ending in `/batch`;
- each route's schema becomes a component named by its `title`, and a GET of
  a route that doesn't end in a path parameter answers an array of it;
- operationIds come from the path: `/orders/{order_id}/refunds` gives
  `listOrderRefunds` and `createOrderRefund`, `/orders/{id}` gives `getOrder`,
  `updateOrder` and `deleteOrder`.

The server is the namespace's path, `/wp-json/<namespace>`: the host is the
connection's, and every generated path goes under it.
"""

import re
import sys

HTTP_METHODS = ("get", "post", "put", "delete")
# A named group of a route's regex, `(?P<id>[\d]+)`, nested groups allowed one deep.
PATH_GROUP = re.compile(r"\(\?P<(\w+)>((?:[^()]|\([^()]*\))+)\)")
DIGITS = re.compile(r"\[?\\d\]?\+")
TYPE_NAMES = {"date-time": "string", "bool": "boolean", "mixed": None}
SCALAR_KEYS = ("description", "enum", "format", "minimum", "maximum", "minItems", "maxItems",
               "minLength", "maxLength", "pattern")
FORMATS = ("date-time", "date", "email", "uri")


def openapi_spec(index: dict) -> dict:
    """The OpenAPI 3.1 document of one namespace's index."""
    namespace = index.get("namespace")
    routes = index.get("routes")
    if not isinstance(namespace, str) or not isinstance(routes, dict):
        sys.exit("a `wordpress` spec is a namespace's index, `/wp-json/<namespace>?context=help`: "
                 "it has `namespace` and `routes`")
    prefix = "/" + namespace.strip("/")
    if not any(isinstance(route, dict) and "schema" in route for route in routes.values()):
        sys.exit(f"the `{namespace}` index has no route schemas: save it with `?context=help`")
    paths, schemas = {}, {}
    for route, data in routes.items():
        if not route.startswith(prefix + "/") or not isinstance(data, dict):
            continue
        path, path_types = template(route[len(prefix):])
        if path is None:
            print(f"note: skipped route `{route}`: a group without a name")
            continue
        answer = component(schemas, path, data.get("schema"))
        item = {}
        for endpoint in data.get("endpoints") or []:
            for method in kept_methods(path, endpoint.get("methods") or []):
                if method not in item:
                    item[method] = operation(name_operation(method, path), method, path, path_types,
                                             as_dict(endpoint.get("args")), answer)
        if item:
            paths[path] = item
    tell_apart(paths)
    return {
        "openapi": "3.1.0",
        "info": {"title": f"WordPress REST API `{namespace}`", "version": namespace},
        "servers": [{"url": f"/wp-json{prefix}"}],
        "paths": paths,
        "components": {"schemas": schemas},
    }


def tell_apart(paths: dict) -> None:
    """Operations that would share a name: each whose path has parameters is
    named after them too (`/settings/batch` stays `batchSettings`,
    `/settings/{group_id}/batch` is `batchSettingsByGroupId`). Two that still
    share one are an error."""
    named = {}
    for path, item in paths.items():
        for method, op in item.items():
            named.setdefault(op["operationId"], []).append((path, method, op))
    for name, ops in named.items():
        if len(ops) < 2:
            continue
        for path, _, op in ops:
            parameters = re.findall(r"\{(\w+)\}", path)
            if parameters:
                op["operationId"] = name + "By" + "And".join(pascal(p) for p in parameters)
    final = {}
    for path, item in paths.items():
        for method, op in item.items():
            other = final.setdefault(op["operationId"], f"{method.upper()} {path}")
            if other != f"{method.upper()} {path}":
                sys.exit(f"`{method.upper()} {path}` and `{other}` would both be `{op['operationId']}`")


def as_dict(value) -> dict:
    """PHP encodes an empty array as `[]`, where JSON Schema means `{}`."""
    return value if isinstance(value, dict) else {}


def template(route: str) -> tuple:
    """`/orders/(?P<id>[\\d]+)` -> (`/orders/{id}`, {"id": "integer"}), or
    (None, {}) for a route with a group that has no name."""
    types = {}

    def parameter(match: re.Match) -> str:
        types[match.group(1)] = "integer" if DIGITS.fullmatch(match.group(2)) else "string"
        return "{" + match.group(1) + "}"

    path = PATH_GROUP.sub(parameter, route) or "/"
    return (None, {}) if "(" in path else (path, types)


def kept_methods(path: str, methods: list) -> list:
    """The methods an endpoint is generated for: an update's PUT (or a batch's
    POST) alone of the POST, PUT and PATCH WordPress lists for it."""
    methods = [method.lower() for method in methods]
    if "put" in methods:
        keep = "post" if path.endswith("/batch") else "put"
        methods = [method for method in methods if method not in ("post", "put", "patch") or method == keep]
    return [method for method in methods if method in HTTP_METHODS]


def component(schemas: dict, path: str, schema) -> dict | None:
    """A `$ref` to the route's schema, kept under its title (with a number
    after it when another route's schema of that title differs)."""
    if not isinstance(schema, dict):
        return None
    title = re.sub(r"[^A-Za-z0-9._-]+", "_", schema.get("title") or path.strip("/").replace("/", "_"))
    converted = normalize(schema)
    name, n = title, 2
    while name in schemas and schemas[name] != converted:
        name, n = f"{title}_{n}", n + 1
    schemas[name] = converted
    return {"$ref": f"#/components/schemas/{name}"}


def operation(operation_id: str, method: str, path: str, path_types: dict, args: dict, answer) -> dict:
    op = {"operationId": operation_id, "tags": [path.strip("/").split("/")[0] or "root"]}
    parameters = [
        {"name": name, "in": "path", "required": True,
         **({"description": as_dict(args.get(name)).get("description")}
            if as_dict(args.get(name)).get("description") else {}),
         "schema": {"type": kind}}
        for name, kind in path_types.items()
    ]
    rest = {name: as_dict(arg) for name, arg in args.items() if name not in path_types}
    if method in ("get", "delete"):
        for name, arg in rest.items():
            schema = normalize(arg)
            description = schema.pop("description", None)
            parameters.append({
                "name": name, "in": "query",
                **({"required": True} if arg.get("required") is True else {}),
                **({"description": description} if description else {}),
                "schema": schema,
            })
    elif rest:
        body = {"type": "object", "properties": {name: normalize(arg, body=True) for name, arg in rest.items()}}
        required = [name for name, arg in rest.items() if arg.get("required") is True]
        if required:
            body["required"] = required
        op["requestBody"] = {"required": bool(required), "content": {"application/json": {"schema": body}}}
    if parameters:
        op["parameters"] = parameters
    if method == "get" and answer and not path.endswith("}"):
        answer = {"type": "array", "items": answer}
    created = method == "post" and not path.endswith("}") and not path.endswith("/batch")
    op["responses"] = {"201" if created else "200": {
        "description": "Created" if created else "OK",
        **({"content": {"application/json": {"schema": answer}}} if answer else {}),
    }}
    return op


def json_schema_type(wordpress_type) -> tuple:
    """A WordPress type (or list of them) -> (JSON Schema type or None for any
    value, format or None)."""
    names = wordpress_type if isinstance(wordpress_type, list) else [wordpress_type]
    fmt = "date-time" if "date-time" in names else None
    types = []
    for name in names:
        name = TYPE_NAMES.get(name, name)
        if name is None:
            return None, None
        if name not in types:
            types.append(name)
    non_null = [name for name in types if name != "null"]
    if len(non_null) != 1:
        return None, None
    return (non_null[0] if len(types) == 1 else [non_null[0], "null"]), fmt


def fits(value, json_type) -> bool:
    if value is None:
        return isinstance(json_type, list)
    base = json_type[0] if isinstance(json_type, list) else json_type
    return {
        "string": isinstance(value, str),
        "integer": isinstance(value, int) and not isinstance(value, bool),
        "number": isinstance(value, (int, float)) and not isinstance(value, bool),
        "boolean": isinstance(value, bool),
        "array": isinstance(value, list),
        "object": isinstance(value, dict),
    }.get(base, False)


def normalize(schema, body: bool = False) -> dict:
    """A WordPress argument or schema as JSON Schema. `body`: a request
    body's field, which keeps nested `required: true` as its parent's
    `required` and has no `default`."""
    if not isinstance(schema, dict):
        return {}
    out = {}
    json_type, fmt = json_schema_type(schema["type"]) if "type" in schema else (None, None)
    if json_type is not None:
        out["type"] = json_type
    for key in SCALAR_KEYS:
        if key in schema:
            out[key] = schema[key]
    if fmt:
        out["format"] = fmt
    if out.get("format") not in (None, *FORMATS):
        del out["format"]
    if "enum" in out:
        values = [value for value in out["enum"] if json_type is None or fits(value, json_type)]
        if values:
            out["enum"] = values
        else:
            del out["enum"]
    if not body and "default" in schema and json_type is not None and fits(schema["default"], json_type):
        out["default"] = schema["default"]
    if schema.get("readonly"):
        out["readOnly"] = True
    base = json_type[0] if isinstance(json_type, list) else json_type
    if base == "object" or "properties" in schema:
        properties = as_dict(schema.get("properties"))
        if properties:
            out["properties"] = {name: normalize(p, body) for name, p in properties.items()}
            required = [name for name, p in properties.items() if body and as_dict(p).get("required") is True]
            if required:
                out["required"] = required
        extra = schema.get("additionalProperties")
        if isinstance(extra, dict):
            out["additionalProperties"] = normalize(extra, body)
        elif extra is False and properties:
            out["additionalProperties"] = False
    if base == "array":
        items = schema.get("items")
        out["items"] = normalize(items[0] if isinstance(items, list) and items else items, body)
    for key in ("oneOf", "anyOf"):
        if isinstance(schema.get(key), list):
            out[key] = [normalize(alternative, body) for alternative in schema[key]]
    return out


def singular(word: str) -> str:
    for plural, one in (("ies", "y"), ("sses", "ss"), ("xes", "x"), ("uses", "us")):
        if word.endswith(plural):
            return word[: -len(plural)] + one
    if word.endswith("s") and not word.endswith(("ss", "us")):
        return word[:-1]
    return word


def pascal(word: str) -> str:
    return "".join(part[:1].upper() + part[1:] for part in re.split(r"[_\-]", word) if part)


def name_operation(method: str, path: str) -> str:
    """`GET /orders` -> `listOrders`, `POST /orders` -> `createOrder`,
    `GET|PUT|DELETE /orders/{id}` -> `getOrder`, `updateOrder`, `deleteOrder`,
    `POST /orders/batch` -> `batchOrders`. A segment before the last names
    whose it is: `/orders/{order_id}/notes` -> `listOrderNotes`."""
    segments = [segment for segment in path.strip("/").split("/") if segment]
    static = [segment for segment in segments if not segment.startswith("{")]
    on_one = bool(segments) and segments[-1].startswith("{")
    if len(static) >= 2 and static[-1] == "batch":
        return "batch" + "".join(pascal(singular(s)) for s in static[:-2]) + pascal(static[-2])
    owners = "".join(pascal(singular(segment)) for segment in static[:-1])
    last = static[-1] if static else "root"
    if method == "get" and not on_one:
        return "list" + owners + pascal(last)
    verb = {"get": "get", "post": "update" if on_one else "create", "put": "update", "delete": "delete"}[method]
    return verb + owners + pascal(singular(last))

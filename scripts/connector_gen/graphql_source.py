"""A GraphQL API for connector_gen: the vendor's schema and our operations, as
an OpenAPI 3.0 document the rest of the generator slices and renders.

A GraphQL schema has types and top-level fields, not operations, so each
capability is an operation we write: `operations/<name>.graphql` in the crate,
listed under `operations:` in `connector.yaml`. Each becomes one POST
operation:

- its variables are the step's inputs (a request body of one field each);
- its output is what the one top-level field it selects returns, built from
  the fields it selects;
- `x-runtara-graphql-*` extensions carry what the template needs to send it:
  the printed operation, the top-level field's response key and the endpoint.

Only `fetch` and `check` import this module, so an OpenAPI-only run does not
need graphql-core.
"""

import hashlib
import json
import re
import sys
import urllib.request
from pathlib import Path

from graphql import (
    FieldNode,
    FragmentDefinitionNode,
    FragmentSpreadNode,
    GraphQLEnumType,
    GraphQLInputObjectType,
    GraphQLList,
    GraphQLNonNull,
    GraphQLScalarType,
    InlineFragmentNode,
    ListValueNode,
    ObjectValueNode,
    OperationDefinitionNode,
    OperationType,
    VariableNode,
    build_client_schema,
    build_schema,
    get_introspection_query,
    get_named_type,
    is_composite_type,
    is_leaf_type,
    lexicographic_sort_schema,
    parse,
    print_ast,
    print_schema,
    type_from_ast,
    validate,
    value_from_ast_untyped,
)
from graphql.error import GraphQLSyntaxError
from graphql.pyutils import Undefined

# The built-in scalars. Every other scalar is the vendor's and needs a
# `graphql.scalars` entry: only the vendor's documentation says what it is.
BUILTIN_SCALARS = {
    "ID": {"type": "string"},
    "String": {"type": "string"},
    "Int": {"type": "integer"},
    "Float": {"type": "number", "format": "double"},
    "Boolean": {"type": "boolean"},
}

# Markdown links (`[product](https://…)`) read as their text in the step editor;
# GraphQL descriptions are Markdown.
MARKDOWN_LINK = re.compile(r"\[([^\]]+)\]\((?:[^()\s]|\([^()\s]*\))*\)")

OPERATIONS_DIR = "operations"


def pascal(name: str) -> str:
    """`pageInfo` -> `PageInfo`, `get_product` -> `GetProduct`."""
    return "".join(word[:1].upper() + word[1:] for word in re.split(r"[^A-Za-z0-9]+", name) if word)


# A line that starts a Markdown block of its own (a list item, a table row, a
# heading, a quote, a code fence), and so is not joined to the line before it.
MARKDOWN_BLOCK = re.compile(r"\s*([-*+]\s|\d+[.)]\s|\||#|>|```)")
LIST_ITEM = re.compile(r"\s*([-*+]\s|\d+[.)]\s)")


def indented_code(line: str) -> bool:
    return line.startswith(("    ", "\t"))


def continues(previous: str, line: str) -> bool:
    """Whether Markdown reads `line` as the rest of `previous`'s paragraph, or
    of its list item when `line` is indented."""
    if not previous.strip() or not line.strip() or MARKDOWN_BLOCK.match(line):
        return False
    if LIST_ITEM.match(previous):
        return line.startswith(" ")
    return not MARKDOWN_BLOCK.match(previous)


def markdown_text(text: str) -> str:
    """A GraphQL description (Markdown) as the step editor shows it: links as
    their text, and the lines of a paragraph, which Markdown joins, joined.
    Lists, tables, headings and code blocks keep their lines, and code (fenced,
    or indented after a blank line) is kept as written, links included."""
    lines, fenced, code = [], False, False
    for line in text.strip().split("\n"):
        fence = line.lstrip().startswith("```")
        previous = lines[-1] if lines else ""
        # An indented line starts code only after a blank line; inside a
        # paragraph it is the paragraph's next line.
        if not fenced and not fence and indented_code(line) and (code or not previous.strip()):
            code = True
        elif line.strip() and not indented_code(line):
            code = False
        if fenced or fence or code:
            lines.append(line)
        elif lines and continues(previous, line):
            lines[-1] = previous.rstrip() + " " + MARKDOWN_LINK.sub(r"\1", line.strip())
        else:
            lines.append(MARKDOWN_LINK.sub(r"\1", line))
        if fence:
            fenced = not fenced
    return "\n".join(lines)


def described(text) -> dict:
    if not text:
        return {}
    return {"description": markdown_text(text)}


# ---------------------------------------------------------------------------
# The schema
# ---------------------------------------------------------------------------


def schema_from_text(text: str, where: str):
    """An SDL document, or an introspection result (`{"data": {"__schema": …}}`
    or `{"__schema": …}`).

    The schema is taken as valid: a vendor's can break rules newer than it
    (Shopify's deprecates fields of types whose interface does not), which
    graphql-core would refuse it for. The operations are still validated
    against it."""
    if text.lstrip().startswith("{"):
        result = json.loads(text)
        if result.get("errors"):
            sys.exit(f"{where}: introspection failed: {json.dumps(result['errors'])[:500]}")
        return build_client_schema(result.get("data", result), assume_valid=True)
    try:
        return build_schema(text, assume_valid=True)
    except GraphQLSyntaxError as e:
        sys.exit(f"{where}: {e}")


def introspection_query() -> str:
    """The introspection query, asking for deprecated arguments and input fields
    too: without `input_value_deprecation` a server leaves them out, so the
    schema would differ from its SDL (and pin differently), and an operation
    could not pass a deprecated argument the API still takes."""
    return get_introspection_query(descriptions=True, input_value_deprecation=True)


def introspect(url: str) -> str:
    body = json.dumps({"query": introspection_query()}).encode()
    request = urllib.request.Request(url, data=body, headers={
        "Content-Type": "application/json",
        "Accept": "application/json",
        "User-Agent": "runtara-connector-gen",
    })
    with urllib.request.urlopen(request) as response:
        return response.read().decode()


def canonical_sdl(schema) -> str:
    """The schema as SDL, types and fields sorted: an introspection result's order
    is the server's, so the same schema can come back in another order."""
    return print_schema(lexicographic_sort_schema(schema))


def load_schema(source: dict, crate: Path):
    """The schema `source` names, refused unless its canonical SDL matches the
    pinned sha256. `introspect:` is an endpoint to send an introspection query
    to, `url:` an SDL file (or introspection result) to download, and `path:`
    one in the crate."""
    kinds = [kind for kind in ("introspect", "url", "path") if kind in source]
    if len(kinds) != 1:
        sys.exit("graphql.schema in connector.yaml names exactly one of `introspect`, `url` and `path`")
    kind = kinds[0]
    where = source[kind]
    if kind == "introspect":
        text = introspect(where)
    elif kind == "url":
        request = urllib.request.Request(where, headers={"User-Agent": "runtara-connector-gen"})
        with urllib.request.urlopen(request) as response:
            text = response.read().decode()
    else:
        text = (crate / where).read_text()
    sdl = canonical_sdl(schema_from_text(text, where))
    digest = hashlib.sha256(sdl.encode()).hexdigest()
    if digest != source.get("sha256"):
        sys.exit(
            f"{where} changed: sha256 of its sorted SDL is {digest}, connector.yaml pins "
            f"{source.get('sha256')}.\nReview the change, update the pin, then fetch again."
        )
    return schema_from_text(sdl, where)


# ---------------------------------------------------------------------------
# Operations
# ---------------------------------------------------------------------------


def document_path(crate: Path, name: str) -> Path:
    return crate / OPERATIONS_DIR / f"{name}.graphql"


def document_text(crate: Path, name: str) -> str:
    path = document_path(crate, name)
    if not path.exists():
        sys.exit(f"{name}: {path} is missing; each listed operation is a file of that name")
    return path.read_text()


def printed_document(crate: Path, name: str) -> str:
    """The operation as the generated code sends it: graphql-core's printing,
    without comments or layout of ours."""
    try:
        return print_ast(parse(document_text(crate, name)))
    except GraphQLSyntaxError as e:
        sys.exit(f"{document_path(crate, name)}: {e}")


class Operation:
    """One operation document, checked against the schema, as the OpenAPI
    operation and the schemas it uses."""

    def __init__(self, schema, name: str, text: str, scalars: dict, user_errors, components: dict,
                 inputs: set):
        self.schema = schema
        self.name = name
        self.scalars = scalars
        self.user_errors = user_errors
        # Shared by every operation: the schemas so far, and which of them are
        # the API's input types (named as the API names them).
        self.components = components
        self.inputs = inputs
        try:
            self.document = parse(text)
        except GraphQLSyntaxError as e:
            sys.exit(f"{name}: {e}")
        errors = validate(schema, self.document)
        if errors:
            sys.exit(f"{name}: not valid against the schema:\n" + "\n".join(f"  {e.message}" for e in errors))
        operations = [d for d in self.document.definitions if isinstance(d, OperationDefinitionNode)]
        if len(operations) != 1:
            sys.exit(f"{name}: the file holds {len(operations)} operations; it takes exactly one")
        self.node = operations[0]
        if self.node.name is None or self.node.name.value != name:
            sys.exit(f"{name}: the operation is named "
                     f"`{self.node.name.value if self.node.name else ''}`; name it `{name}`, as its file")
        if self.node.operation == OperationType.SUBSCRIPTION:
            sys.exit(f"{name}: subscriptions are not supported; a capability is one request")
        self.fragments = {
            d.name.value: d for d in self.document.definitions if isinstance(d, FragmentDefinitionNode)
        }
        self.root_type = (
            schema.query_type if self.node.operation == OperationType.QUERY else schema.mutation_type
        )
        fields = [s for s in self.node.selection_set.selections
                  if not (isinstance(s, FieldNode) and s.name.value == "__typename")]
        if len(fields) != 1 or not isinstance(fields[0], FieldNode):
            sys.exit(f"{name}: select exactly one top-level field; the capability returns what it returns")
        self.root = fields[0]
        self.root_key = (self.root.alias or self.root.name).value
        if self.root.name.value.startswith("__"):
            sys.exit(f"{name}: `{self.root.name.value}` asks about the schema itself; a capability "
                     "selects one of the API's own top-level fields")
        self.root_field = self.root_type.fields[self.root.name.value]
        self.printed = print_ast(self.document)
        if '"##' in self.printed:
            sys.exit(f'{name}: the operation contains `"##`, which would end the string the '
                     "generated code holds it in")
        self.missing_scalars = set()

    @property
    def is_query(self) -> bool:
        return self.node.operation == OperationType.QUERY

    # -- types ----------------------------------------------------------------

    def leaf_schema(self, named) -> dict:
        if isinstance(named, GraphQLEnumType):
            return {"type": "string", "enum": list(named.values), **described(named.description)}
        if named.name in BUILTIN_SCALARS:
            return dict(BUILTIN_SCALARS[named.name])
        mapped = self.scalars.get(named.name)
        if mapped is None:
            self.missing_scalars.add(named.name)
            return {"type": "string"}
        schema = dict(mapped) if isinstance(mapped, dict) else {"type": mapped}
        return {**described(named.description), **schema}

    def input_schema(self, gql_type) -> dict:
        if isinstance(gql_type, GraphQLNonNull):
            return self.input_schema(gql_type.of_type)
        if isinstance(gql_type, GraphQLList):
            return {"type": "array", "items": self.input_schema(gql_type.of_type)}
        if is_leaf_type(gql_type):
            return self.leaf_schema(gql_type)
        assert isinstance(gql_type, GraphQLInputObjectType)
        name = gql_type.name
        if name in self.components and name not in self.inputs:
            sys.exit(f"{self.name}: the input type `{name}` has the name of a model an operation "
                     "returns; rename that operation")
        if name not in self.components:
            self.inputs.add(name)
            self.components[name] = {}  # placeholder: an input type may refer to itself
            properties, required = {}, []
            for field_name, field in gql_type.fields.items():
                properties[field_name] = {**self.input_schema(field.type), **described(field.description)}
                if field.deprecation_reason is not None:
                    properties[field_name]["deprecated"] = True
                if isinstance(field.type, GraphQLNonNull) and field.default_value is Undefined:
                    required.append(field_name)
            self.components[name] = {
                "type": "object",
                "properties": properties,
                **({"required": required} if required else {}),
                **described(gql_type.description),
            }
        return {"$ref": f"#/components/schemas/{name}"}

    def output_schema(self, gql_type, sources: list, model: str, prefix: str = None) -> dict:
        """The schema of what a field returns. `sources` are the (field
        definition, node) pairs that select it: several when fragments on
        different types select the same key, each typed by its own type. An
        object is the model `model`; the objects inside it are named `prefix`
        (by default `model`) and their response keys.

        An item of a list may be `null` unless the schema says it is not
        (Shopify's `nodes(ids:)` answers an unknown id with `null`), so it is
        marked nullable and read as an `Option`."""
        if isinstance(gql_type, GraphQLNonNull):
            return self.output_schema(gql_type.of_type, sources, model, prefix)
        if isinstance(gql_type, GraphQLList):
            items = self.output_schema(gql_type.of_type, sources, model, prefix)
            if not isinstance(gql_type.of_type, GraphQLNonNull):
                items = {"allOf": [items], "nullable": True} if "$ref" in items else {**items, "nullable": True}
            return {"type": "array", "items": items}
        if is_leaf_type(gql_type):
            return self.leaf_schema(gql_type)
        self.define_model(model, sources, prefix or model)
        return {"$ref": f"#/components/schemas/{model}"}

    def define_model(self, name: str, sources: list, prefix: str) -> None:
        if name in self.components:
            what = "an input type's name" if name in self.inputs else "defined twice"
            sys.exit(f"{self.name}: the model `{name}` would be {what}; "
                     "alias a field or rename the operation")
        self.components[name] = {}
        selected = {}
        types = []
        for field, node in sources:
            named = get_named_type(field.type)
            types.append(named)
            self.collect(named, node.selection_set, selected)
        properties = {}
        for key, entries in selected.items():
            typed = [(field, node, parent) for field, node, parent in entries if field is not None]
            if not typed:
                properties[key] = {"type": "string", "description": "The object's GraphQL type name."}
                continue
            field, node, parent = typed[0]
            schema = self.output_schema(field.type, [(f, n) for f, n, _ in typed], prefix + pascal(key))
            properties[key] = {**schema, **described(field.description)}
            if field.deprecation_reason is not None:
                properties[key]["deprecated"] = True
                print(f"note: {self.name}: `{parent.name}.{node.name.value}` is deprecated: "
                      f"{' '.join(field.deprecation_reason.split())}")
        # Fragments on different types (an `owner` that is a `User` here and a
        # `Customer` there) share no one description.
        description = described(types[0].description) if len({t.name for t in types}) == 1 else {}
        self.components[name] = {"type": "object", "properties": properties, **description}

    def collect(self, parent, selection_set, out: dict) -> None:
        """Response key -> [(field definition, node, the type it is a field of)]
        of the fields selected on `parent`, through inline fragments and
        fragment spreads. The schema's validation has made sure fields of one
        key can be merged; their definitions may still be of different types,
        one per fragment."""
        if selection_set is None:
            return
        for selection in selection_set.selections:
            if isinstance(selection, FieldNode):
                key = (selection.alias or selection.name).value
                if selection.name.value == "__typename":
                    out.setdefault(key, []).append((None, selection, parent))
                else:
                    out.setdefault(key, []).append((parent.fields[selection.name.value], selection, parent))
            elif isinstance(selection, InlineFragmentNode):
                condition = selection.type_condition
                target = self.schema.get_type(condition.name.value) if condition else parent
                self.collect(target, selection.selection_set, out)
            elif isinstance(selection, FragmentSpreadNode):
                fragment = self.fragments[selection.name.value]
                target = self.schema.get_type(fragment.type_condition.name.value)
                self.collect(target, fragment.selection_set, out)

    # -- variables ------------------------------------------------------------

    def variable_descriptions(self) -> dict:
        """Variable -> the description of the first argument or input field it is
        passed to: a variable has no description of its own."""
        found = {}

        def note_deprecated(what: str, definition) -> None:
            if definition.deprecation_reason is not None:
                print(f"note: {self.name}: {what} is deprecated: "
                      f"{' '.join(definition.deprecation_reason.split())}")

        def walk_value(node, gql_type, description):
            if isinstance(node, VariableNode):
                found.setdefault(node.name.value, description)
            elif isinstance(node, ListValueNode):
                inner = gql_type.of_type if isinstance(gql_type, GraphQLNonNull) else gql_type
                inner = inner.of_type if isinstance(inner, GraphQLList) else inner
                for item in node.values:
                    walk_value(item, inner, description)
            elif isinstance(node, ObjectValueNode):
                named = get_named_type(gql_type)
                for field in node.fields:
                    definition = named.fields[field.name.value]
                    note_deprecated(f"input field `{named.name}.{field.name.value}`", definition)
                    walk_value(field.value, definition.type, definition.description)

        seen_fragments = set()

        def walk(parent, selection_set):
            if selection_set is None:
                return
            for selection in selection_set.selections:
                if isinstance(selection, FieldNode):
                    if selection.name.value == "__typename":
                        continue
                    field = parent.fields[selection.name.value]
                    for argument in selection.arguments or ():
                        definition = field.args[argument.name.value]
                        note_deprecated(f"argument `{selection.name.value}({argument.name.value}:)`", definition)
                        walk_value(argument.value, definition.type, definition.description)
                    if is_composite_type(get_named_type(field.type)):
                        walk(get_named_type(field.type), selection.selection_set)
                elif isinstance(selection, InlineFragmentNode):
                    condition = selection.type_condition
                    walk(self.schema.get_type(condition.name.value) if condition else parent,
                         selection.selection_set)
                elif isinstance(selection, FragmentSpreadNode) and selection.name.value not in seen_fragments:
                    seen_fragments.add(selection.name.value)
                    fragment = self.fragments[selection.name.value]
                    walk(self.schema.get_type(fragment.type_condition.name.value), fragment.selection_set)

        walk(self.root_type, self.node.selection_set)
        return found

    def request_body(self):
        # Walked even without variables: it also notes deprecated arguments.
        descriptions = self.variable_descriptions()
        variables = self.node.variable_definitions
        if not variables:
            return None
        properties, required = {}, []
        for variable in variables:
            name = variable.variable.name.value
            gql_type = type_from_ast(self.schema, variable.type)
            schema = {**self.input_schema(gql_type), **described(descriptions.get(name))}
            if variable.default_value is not None:
                schema["default"] = value_from_ast_untyped(variable.default_value)
            elif isinstance(gql_type, GraphQLNonNull):
                required.append(name)
            properties[name] = schema
        body = {"type": "object", "properties": properties}
        if required:
            body["required"] = required
        return {"required": True, "content": {"application/json": {"schema": body}}}

    # -- the operation --------------------------------------------------------

    def check_user_errors(self) -> None:
        """A mutation that reports what it refused in a field (`userErrors`) has
        to select it, or a refused write would read as done."""
        if self.is_query or not self.user_errors:
            return
        payload = get_named_type(self.root_field.type)
        if self.user_errors not in getattr(payload, "fields", {}):
            return
        # Selected directly or through a fragment, under its own name: the
        # kit reads the answer's field of that name.
        selected = {}
        self.collect(payload, self.root.selection_set, selected)
        if not any(node.name.value == self.user_errors for _, node, _ in selected.get(self.user_errors, [])):
            sys.exit(f"{self.name}: select `{self.user_errors}` (not aliased) on `{self.root_key}`; "
                     "it is how the vendor says it refused the write")

    def openapi_operation(self, path: str) -> dict:
        self.check_user_errors()
        # The result is `ListProductsResult`; the objects inside it are named
        # after the operation, not after `…Result`: `ListProductsPageInfo`.
        response = self.output_schema(self.root_field.type, [(self.root_field, self.root)],
                                      pascal(self.name) + "Result", prefix=pascal(self.name))
        operation = {
            "operationId": self.name,
            "tags": ["graphql"],
            "responses": {"200": {
                "description": "OK",
                "content": {"application/json": {"schema": response}},
            }},
            "x-runtara-graphql-document": self.printed,
            "x-runtara-graphql-field": self.root_key,
            "x-runtara-graphql-path": path,
            **described(self.root_field.description),
        }
        body = self.request_body()
        if body is not None:
            operation["requestBody"] = body
        if self.missing_scalars:
            sys.exit(f"{self.name}: the vendor's scalars {sorted(self.missing_scalars)} have no type; "
                     "give each one under `graphql.scalars` in connector.yaml (`string`, or a schema)")
        return operation


def openapi_spec(config: dict, crate: Path) -> tuple:
    """The OpenAPI document of the listed operations, and the ones that only
    read (the queries)."""
    graphql = config.get("graphql") or {}
    path = graphql.get("path")
    if not isinstance(path, str) or not path.startswith("/"):
        sys.exit("graphql.path in connector.yaml is the endpoint's path, such as `/graphql`")
    schema = load_schema(graphql.get("schema") or {}, crate)
    scalars = graphql.get("scalars") or {}
    unknown = sorted(name for name in scalars if not isinstance(schema.get_type(name), GraphQLScalarType)
                     or name in BUILTIN_SCALARS)
    if unknown:
        sys.exit(f"graphql.scalars {unknown} are no custom scalar of the schema")
    components, inputs, paths, queries = {}, set(), {}, []
    for name in config["operations"]:
        operation = Operation(schema, name, document_text(crate, name), scalars,
                              graphql.get("user_errors"), components, inputs)
        # Every operation is a POST to the one endpoint; the path here only keeps
        # them apart in the document, and the template sends `x-runtara-graphql-path`.
        paths[f"/{name}"] = {"post": operation.openapi_operation(path)}
        if operation.is_query:
            queries.append(name)
    spec = {
        "openapi": "3.0.3",
        "info": {"title": config["agent"]["name"], "version": "graphql"},
        "paths": paths,
        "components": {"schemas": components},
    }
    return spec, queries


def document_drift(config: dict, crate: Path, sliced: dict) -> list:
    """What `connector.yaml` and the operation files say that `spec.slice.json`
    no longer holds: an operation, or the endpoint (an API version bump),
    edited without running `fetch`."""
    held = {
        op["operationId"]: op
        for item in sliced["paths"].values()
        for op in item.values()
        if isinstance(op, dict) and "operationId" in op
    }
    problems = [
        f"{document_path(crate, name).relative_to(crate)} does not match spec.slice.json"
        for name in config["operations"]
        if name in held and held[name].get("x-runtara-graphql-document") != printed_document(crate, name)
    ]
    path = (config.get("graphql") or {}).get("path")
    if any(op.get("x-runtara-graphql-path") != path for op in held.values()):
        problems.append(f"graphql.path `{path}` is not the endpoint in spec.slice.json")
    return problems

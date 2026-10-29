#!/usr/bin/env python3
"""Generate an agent crate's `src/generated.rs` from the vendor's OpenAPI spec.

Every generated agent crate carries a `connector.yaml` (see README.md). Commands,
where CRATE is an agent crate directory such as
`crates/agents/runtara-agent-pipedrive`:

    fetch CRATE       download the spec, verify its sha256, and write
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


def load_config(crate: Path) -> dict:
    with open(crate / "connector.yaml") as f:
        return yaml.safe_load(f)


# ---------------------------------------------------------------------------
# fetch
# ---------------------------------------------------------------------------


class SpecLoader(yaml.SafeLoader):
    """YAML 1.1 reads `yes`/`no`/`on`/`off` as booleans and unquoted dates as
    dates; OpenAPI (YAML 1.2 / JSON) means strings by both."""


SpecLoader.yaml_implicit_resolvers = {
    first: [
        (tag, regexp) for tag, regexp in resolvers
        if tag not in ("tag:yaml.org,2002:bool", "tag:yaml.org,2002:timestamp")
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


def load_spec(raw) -> dict:
    return stringify_keys(yaml.load(raw, Loader=SpecLoader))


def walk_schema(node, visit):
    """Call `visit` on every mapping under `node`, skipping data such as examples.
    A `properties` map is walked as name -> schema, whatever the names are."""
    if isinstance(node, list):
        for value in node:
            walk_schema(value, visit)
    elif isinstance(node, dict):
        visit(node)
        for key, value in node.items():
            if key == "properties" and isinstance(value, dict):
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


def check_operation(spec: dict, path: str, method: str, op: dict, item: dict) -> None:
    """Refuse the shapes the templates cannot express yet, and normalize the rest."""
    name = op["operationId"]

    parameters = []
    for parameter in item.get("parameters", []) + op.get("parameters", []):
        resolved = deref(spec, parameter)
        if resolved.get("in") == "cookie":
            if resolved.get("required"):
                sys.exit(f"{name}: required cookie parameter `{resolved['name']}` is not supported")
            print(f"note: {name}: dropped optional cookie parameter `{resolved['name']}`")
            continue
        schema = deref(spec, resolved.get("schema") or {})
        if resolved.get("in") == "query" and (
            resolved.get("style") == "deepObject" or schema.get("type") == "object"
        ):
            sys.exit(f"{name}: object query parameter `{resolved['name']}` (deepObject) is not supported yet")
        parameters.append(parameter)
    if "parameters" in op:
        op["parameters"] = [p for p in op["parameters"] if p in parameters]
    if "parameters" in item:
        item["parameters"] = [p for p in item["parameters"] if p in parameters]

    body = deref(spec, op.get("requestBody"))
    if isinstance(body, dict) and body.get("content"):
        json_types = {k: v for k, v in body["content"].items() if is_json(k)}
        if not json_types:
            sys.exit(
                f"{name}: request body {sorted(body['content'])} is not supported yet; "
                "only JSON bodies are (form and multipart bodies need template support)"
            )
        body["content"] = json_types

    # An operation with several tags would be rendered once per tag.
    if len(op.get("tags") or []) > 1:
        op["tags"] = op["tags"][:1]

    # Facts the logic-less templates branch on, and text they print as is.
    if method != "get":
        op["x-runtara-side-effects"] = True
    op["x-runtara-summary"] = clean_text(op.get("summary") or humanize(name))


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
            if key == "properties" and isinstance(value, dict)
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


def normalize_schemas(sliced: dict) -> None:
    def visit(node):
        # Plain `integer` would become i32; vendor ids and amounts outgrow it.
        if node.get("type") == "integer" and "format" not in node:
            node["format"] = "int64"
        for key in ("description", "summary", "title"):
            if isinstance(node.get(key), str):
                node[key] = clean_text(node[key])

    walk_schema(sliced, visit)


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


def slice_spec(spec: dict, operation_ids: list, model_names: dict = None) -> dict:
    """Keep the listed operations and every component they reference.

    openapi-generator's FILTER normalizer drops unlisted operations but still
    renders every model in the spec, so the slice is what keeps the generated
    file to the selected surface.
    """
    if "openapi" not in spec:
        sys.exit("only OpenAPI 3.x specs are supported (Swagger 2.0 needs converting first)")

    wanted = set(operation_ids)
    paths = {}
    for path, item in spec.get("paths", {}).items():
        kept = {m: op for m, op in item.items() if m in HTTP_METHODS and op.get("operationId") in wanted}
        if kept:
            if "parameters" in item:
                kept["parameters"] = item["parameters"]
            paths[path] = kept

    found = set()
    for path, item in paths.items():
        for method, op in item.items():
            if method not in HTTP_METHODS:
                continue
            found.add(op["operationId"])
            check_operation(spec, path, method, op, item)
            if op["operationId"] == operation_ids[0]:
                op["x-runtara-first"] = True
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
    inline_refs(sliced, spec)
    normalize_schemas(sliced)
    check_names(sliced, operation_ids, model_names or {})
    return sliced


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
    """`person_id`, `personId` and `person-id` all become `Person ID`."""
    spaced = re.sub(r"([a-z0-9])([A-Z])", r"\1 \2", name)
    spaced = re.sub(r"([A-Z]+)([A-Z][a-z])", r"\1 \2", spaced)
    words = [word for word in re.split(r"[\s_\-.]+", spaced) if word]
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
    title = schema.get("title")
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
        for name, prop in properties.items():
            if not isinstance(prop, dict):
                continue
            # OpenAPI 3.0 ignores keys next to a $ref, so such a field keeps its raw name.
            if "$ref" not in prop:
                describe_field(prop, name, prop, labels, name in required)
            describe_fields(prop, labels)
    for key, value in node.items():
        # `properties` maps names to schemas and was walked above; the rest is data.
        if key not in ("properties",) + DATA_KEYS:
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


def fetch(crate: Path) -> None:
    config = load_config(crate)
    source = config["spec"]
    with urllib.request.urlopen(source["url"]) as response:
        raw = response.read()
    digest = hashlib.sha256(raw).hexdigest()
    if digest != source["sha256"]:
        sys.exit(
            f"{source['url']} changed: sha256 is {digest}, connector.yaml pins {source['sha256']}.\n"
            "Review the vendor's change, update the pin, then fetch again."
        )

    sliced = slice_spec(load_spec(raw), config["operations"], config.get("model_names") or {})
    describe_fields(sliced, config.get("labels") or {})
    write_json(crate / "spec.slice.json", sliced)
    fixtures = crate / "tests" / "fixtures"
    for stale in fixtures.glob("*.json") if fixtures.exists() else []:
        stale.unlink()
    for operation_id, example in response_examples(sliced).items():
        write_json(fixtures / f"{operation_id}.json", example)
    print(f"{crate / 'spec.slice.json'}: {len(config['operations'])} operations")


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


def render(crate: Path) -> str:
    """Render `generated.rs` for `crate` and return its rustfmt-ed text."""
    config = load_config(crate)
    agent = config["agent"]
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
                "integrationId": agent["integration_id"],
                "errorPrefix": agent["error_prefix"],
            },
            "typeMappings": TYPE_MAPPINGS,
            "modelNameMappings": config.get("model_names") or {},
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

    listed = set(config["operations"])
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

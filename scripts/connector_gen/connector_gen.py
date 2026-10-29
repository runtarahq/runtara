#!/usr/bin/env python3
"""Generate an agent crate's `src/generated.rs` from the vendor's OpenAPI spec.

Every generated agent crate carries a `connector.yaml` (see README.md). Commands,
where CRATE is an agent crate directory such as
`crates/agents/runtara-agent-pipedrive`:

    fetch CRATE       download the spec, verify its sha256, and write
                      `spec.slice.json` plus `tests/fixtures/<operationId>.json`
    generate CRATE    render `src/generated.rs` from `spec.slice.json`
    check CRATE...    fail when `src/generated.rs` differs from a fresh render
"""

import argparse
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


def load_config(crate: Path) -> dict:
    with open(crate / "connector.yaml") as f:
        return yaml.safe_load(f)


# ---------------------------------------------------------------------------
# fetch
# ---------------------------------------------------------------------------


def refs_in(node, found):
    if isinstance(node, dict):
        ref = node.get("$ref")
        if isinstance(ref, str) and ref.startswith("#/components/"):
            found.add(ref)
        for value in node.values():
            refs_in(value, found)
    elif isinstance(node, list):
        for value in node:
            refs_in(value, found)


def resolve(spec, ref):
    node = spec
    for part in ref[2:].split("/"):
        node = node[part.replace("~1", "/").replace("~0", "~")]
    return node


def slice_spec(spec: dict, operation_ids: list) -> dict:
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
    for item in paths.values():
        for method, op in item.items():
            if method not in HTTP_METHODS:
                continue
            found.add(op["operationId"])
            # Facts the logic-less templates branch on.
            if method != "get":
                op["x-runtara-side-effects"] = True
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
        return value
    if isinstance(value, (bool, int, float)):
        return json.dumps(value)
    return None


def scalar_json(value):
    """A scalar as the JSON text `#[field(default = …)]` parses, else None."""
    if isinstance(value, (str, bool, int, float)):
        return json.dumps(value)
    return None


def describe_field(target: dict, name: str, schema: dict, labels: dict) -> None:
    """Mark a parameter or property with the label, example and default the
    templates render. A `connector.yaml` label wins; then the vendor's `title`,
    but only when it reads as a phrase (one-word titles name models); then the
    humanized field name."""
    title = schema.get("title")
    if name in labels:
        label = labels[name]
    elif isinstance(title, str) and " " in title.strip():
        label = title.strip()
    else:
        label = humanize(name)
    target["x-runtara-label"] = label
    example = scalar_text(target.get("example", schema.get("example")))
    if example is not None:
        target["x-runtara-example"] = example
    default = scalar_json(schema.get("default"))
    if default is not None:
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
        describe_field(node, node["name"], node.get("schema") or {}, labels)
    properties = node.get("properties")
    if isinstance(properties, dict):
        for name, prop in properties.items():
            # OpenAPI 3.0 ignores keys next to a $ref, so such a field keeps its raw name.
            if isinstance(prop, dict) and "$ref" not in prop:
                describe_field(prop, name, prop, labels)
    for key, value in node.items():
        # Example payloads and defaults are data, not schema.
        if key not in ("example", "examples", "default", "enum"):
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

    sliced = slice_spec(yaml.safe_load(raw), config["operations"])
    describe_fields(sliced, config.get("labels") or {})
    write_json(crate / "spec.slice.json", sliced)
    fixtures = crate / "tests" / "fixtures"
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
    agent = load_config(crate)["agent"]
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
                "agentName": agent["name"],
                "agentDescription": agent["description"],
                "integrationId": agent["integration_id"],
                "errorPrefix": agent["error_prefix"],
            },
            "typeMappings": TYPE_MAPPINGS,
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


def check(crates: list) -> None:
    stale = []
    for crate in crates:
        target = crate / "src" / "generated.rs"
        expected = render(crate)
        actual = target.read_text() if target.exists() else ""
        if actual != expected:
            stale.append(str(target))
            diff = difflib.unified_diff(
                actual.splitlines(keepends=True), expected.splitlines(keepends=True),
                fromfile=f"{target} (committed)", tofile=f"{target} (regenerated)",
            )
            sys.stdout.writelines(list(diff)[:200])
    if stale:
        sys.exit(f"stale generated files (run `generate`): {', '.join(stale)}")
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

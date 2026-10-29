#!/usr/bin/env python3
"""Tests for connector_gen.py.

The render tests need openapi-generator-cli (see README.md) and the
repository's rustfmt; they are skipped when the generator is not installed.

    python scripts/connector_gen/test_connector_gen.py
"""

import json
import sys
import tempfile
import unittest
from pathlib import Path

import yaml

sys.path.insert(0, str(Path(__file__).resolve().parent))
import connector_gen as cg  # noqa: E402

TESTDATA = Path(__file__).resolve().parent / "testdata"
OPERATIONS = ["listItems", "updateItem", "deleteItem", "countTags", "countItems"]


def sample_spec() -> dict:
    return cg.load_spec((TESTDATA / "sample-spec.yaml").read_bytes())


def generator_available() -> bool:
    try:
        cg.generator_cli()
        return True
    except SystemExit:
        return False


def write_crate(crate: Path, spec: dict, operations: list, **config) -> dict:
    (crate / "connector.yaml").write_text(yaml.safe_dump({
        "agent": {
            "id": "sample",
            "name": "Sample & Co",
            "description": 'Sample "agent".',
            "integration_id": "sample_token",
            "error_prefix": "SAMPLE",
        },
        "spec": {"url": "unused", "sha256": "unused"},
        "operations": operations,
        **config,
    }))
    sliced = cg.slice_spec(spec, operations, config.get("model_names"))
    cg.describe_fields(sliced, config.get("labels") or {})
    cg.write_json(crate / "spec.slice.json", sliced)
    return sliced


def operation(sliced: dict, operation_id: str) -> dict:
    return next(
        op for item in sliced["paths"].values() for method, op in item.items()
        if method in cg.HTTP_METHODS and op["operationId"] == operation_id
    )


class LoadSpecTest(unittest.TestCase):
    def test_yaml_1_1_quirks_are_read_as_openapi_means_them(self):
        spec = cg.load_spec(
            b"responses:\n  200: {description: ok}\n  default: {description: err}\n"
            b"example: 2024-01-31\nenum: [yes, no, on, off]\nflag: true\n"
        )
        self.assertEqual(sorted(spec["responses"]), ["200", "default"])
        self.assertEqual(spec["example"], "2024-01-31")
        self.assertEqual(spec["enum"], ["yes", "no", "on", "off"])
        self.assertIs(spec["flag"], True)


class HumanizeTest(unittest.TestCase):
    def test_splits_words_and_keeps_acronyms(self):
        self.assertEqual(cg.humanize("person_id"), "Person ID")
        self.assertEqual(cg.humanize("personId"), "Person ID")
        self.assertEqual(cg.humanize("x-api-token"), "X API Token")
        self.assertEqual(cg.humanize("HTMLBody"), "HTML Body")
        self.assertEqual(cg.humanize("label_ids"), "Label IDs")


class SliceTest(unittest.TestCase):
    def test_keeps_listed_operations_and_their_components(self):
        sliced = cg.slice_spec(sample_spec(), ["countItems"])
        self.assertEqual(list(sliced["paths"]), ["/count"])
        self.assertNotIn("schemas", sliced["components"])

        sliced = cg.slice_spec(sample_spec(), ["listItems"])
        # SortOrder is inlined and Legacy only reached through an inlined $ref.
        self.assertEqual(list(sliced["components"]["schemas"]), ["Item"])

    def test_marks_side_effects_first_operation_and_summary(self):
        sliced = cg.slice_spec(sample_spec(), ["updateItem", "listItems", "deleteItem"])
        update = operation(sliced, "updateItem")
        self.assertTrue(update["x-runtara-side-effects"])
        self.assertTrue(update["x-runtara-first"])
        self.assertNotIn("x-runtara-side-effects", operation(sliced, "listItems"))
        self.assertEqual(operation(sliced, "deleteItem")["x-runtara-summary"], "Delete Item")

    def test_one_tag_per_operation(self):
        sliced = cg.slice_spec(sample_spec(), ["listItems"])
        self.assertEqual(operation(sliced, "listItems")["tags"], ["items"])

    def test_optional_cookie_parameter_is_dropped(self):
        sliced = cg.slice_spec(sample_spec(), ["listItems"])
        names = [p["name"] for p in operation(sliced, "listItems")["parameters"]]
        self.assertNotIn("session", names)

    def test_enum_and_internal_refs_are_inlined(self):
        sliced = cg.slice_spec(sample_spec(), ["listItems"])
        order = next(p for p in operation(sliced, "listItems")["parameters"] if p["name"] == "order")
        self.assertEqual(order["schema"], {"type": "string", "enum": ["asc", "desc"]})
        legacy = sliced["components"]["schemas"]["Item"]["properties"]["legacy_state"]
        self.assertEqual(legacy, {"type": "string", "enum": ["a", "b"]})

    def test_plain_integers_widen_to_int64(self):
        sliced = cg.slice_spec(sample_spec(), ["countItems"])
        schema = operation(sliced, "countItems")["responses"]["200"]["content"]["application/json"]["schema"]
        self.assertEqual(schema, {"type": "integer", "format": "int64"})

    def test_unknown_operation_fails(self):
        with self.assertRaises(SystemExit):
            cg.slice_spec(sample_spec(), ["noSuchOperation"])


class RefusalTest(unittest.TestCase):
    """Shapes the templates cannot express yet fail at fetch, not at runtime."""

    @staticmethod
    def spec_with(op: dict, schemas: dict = None) -> dict:
        op = {"operationId": "op", "responses": {"204": {"description": "ok"}}, **op}
        return {"openapi": "3.0.1", "info": {"title": "t", "version": "1"},
                "paths": {"/x": {"post": op}}, "components": {"schemas": schemas or {}}}

    def assert_refused(self, spec: dict, fragment: str, operations=("op",), **kwargs):
        with self.assertRaises(SystemExit) as raised:
            cg.slice_spec(spec, list(operations), **kwargs)
        self.assertIn(fragment, str(raised.exception))

    def test_form_and_multipart_bodies(self):
        for media in ("application/x-www-form-urlencoded", "multipart/form-data"):
            spec = self.spec_with({"requestBody": {"content": {media: {"schema": {"type": "object"}}}}})
            self.assert_refused(spec, "only JSON bodies")

    def test_json_alternative_is_kept_when_present(self):
        spec = self.spec_with({"requestBody": {"content": {
            "application/x-www-form-urlencoded": {"schema": {"type": "object"}},
            "application/json": {"schema": {"type": "object"}},
        }}})
        sliced = cg.slice_spec(spec, ["op"])
        self.assertEqual(list(operation(sliced, "op")["requestBody"]["content"]), ["application/json"])

    def test_object_query_parameter(self):
        spec = self.spec_with({"parameters": [{"in": "query", "name": "filter", "style": "deepObject",
                                               "schema": {"type": "object"}}]})
        self.assert_refused(spec, "deepObject")

    def test_required_cookie_parameter(self):
        spec = self.spec_with({"parameters": [{"in": "cookie", "name": "sid", "required": True,
                                               "schema": {"type": "string"}}]})
        self.assert_refused(spec, "cookie")

    def test_schema_names_that_collide_with_generated_code(self):
        response = {"200": {"description": "ok", "content": {"application/json": {
            "schema": {"$ref": "#/components/schemas/Value"}}}}}
        spec = self.spec_with({"responses": response}, {"Value": {"type": "object"}})
        self.assert_refused(spec, "model_names")
        # ...unless connector.yaml renames it.
        cg.slice_spec(spec, ["op"], {"Value": "Amount"})

        response["200"]["content"]["application/json"]["schema"]["$ref"] = "#/components/schemas/OpInput"
        spec = self.spec_with({"responses": response}, {"OpInput": {"type": "object"}})
        self.assert_refused(spec, "OpInput")


class DescribeFieldsTest(unittest.TestCase):
    def test_labels_overrides_and_json_defaults(self):
        spec = {"paths": {"/x": {"get": {"parameters": [
            {"in": "query", "name": "sort_by", "schema": {"type": "string", "default": "id"}},
            {"in": "query", "name": "org_id", "schema": {"type": "integer"}},
            {"in": "query", "name": "limit", "required": True, "schema": {"type": "integer", "default": 50}},
        ]}}}}
        cg.describe_fields(spec, {"org_id": "Organization"})
        sort_by, org_id, limit = spec["paths"]["/x"]["get"]["parameters"]
        self.assertEqual(sort_by["x-runtara-label"], "Sort By")
        # `#[field(default = …)]` is parsed as JSON, so a string default is quoted.
        self.assertEqual(sort_by["x-runtara-default"], '"id"')
        self.assertEqual(org_id["x-runtara-label"], "Organization")
        # A required field gets no default: the metadata would call it optional.
        self.assertNotIn("x-runtara-default", limit)

    def test_property_named_properties_is_a_field_not_a_map(self):
        sliced = cg.slice_spec(sample_spec(), ["listItems"])
        cg.describe_fields(sliced, {})
        prop = sliced["components"]["schemas"]["Item"]["properties"]["properties"]
        self.assertEqual(prop["x-runtara-label"], "Properties")
        self.assertEqual(prop["example"], {"color": "red"})
        self.assertEqual(prop["properties"]["color"]["x-runtara-label"], "Color")


@unittest.skipUnless(generator_available(), "openapi-generator-cli is not installed")
class RenderTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        with tempfile.TemporaryDirectory() as tmp:
            write_crate(Path(tmp), sample_spec(), OPERATIONS)
            cls.text = cg.render(Path(tmp))

    def assert_returns(self, function: str, return_type: str):
        signature = next(
            line for line in self.text.splitlines()
            if line.startswith(f"pub async fn {function}(")
        )
        self.assertIn(f"-> Result<{return_type}, AgentError>", signature)

    def test_response_containers_are_kept(self):
        self.assert_returns("list_items", "Vec<Item>")
        self.assert_returns("count_tags", "HashMap<String, i64>")
        self.assert_returns("count_items", "i64")
        self.assert_returns("update_item", "Item")
        self.assert_returns("delete_item", "Value")

    def test_nullable_optional_body_field_keeps_explicit_null(self):
        self.assertIn("pub owner_id: Option<Option<i64>>,", self.text)
        self.assertIn('deserialize_with = "kit::nullable"', self.text)
        self.assertIn("pub name: String,", self.text)
        self.assertIn("pub note: Option<String>,", self.text)

    def test_enums_keep_their_wire_type(self):
        self.assertNotIn("enum ", self.text)
        self.assertIn("pub priority: Option<i64>,", self.text)
        self.assertIn("pub state: Option<String>,", self.text)
        self.assertIn("pub order: Option<String>,", self.text)
        self.assertIn("One of: open, closed.", self.text)

    def test_any_of_is_raw_json(self):
        self.assertIn("pub type ItemValue = Value;", self.text)

    def test_query_arrays_follow_explode(self):
        self.assertIn('.query_each_opt("ids", &self.ids)', self.text)
        self.assertIn('.query_opt("tags", &self.tags)', self.text)

    def test_required_parameter_with_default_stays_required(self):
        self.assertIn("pub limit: i64,", self.text)
        self.assertNotIn('default = r##"50"##', self.text)

    def test_multi_tag_operation_is_rendered_once(self):
        self.assertEqual(self.text.count("pub async fn list_items("), 1)

    def test_text_is_printed_as_written(self):
        self.assertIn('r##"Shown as "Name" & stored as is"##', self.text)
        self.assertIn('module_display_name = r##"Sample & Co"##', self.text)
        self.assertIn('display_name = r##"Delete Item"##', self.text)
        self.assertNotIn("&amp;", self.text)
        self.assertNotIn('\\"', self.text)


@unittest.skipUnless(generator_available(), "openapi-generator-cli is not installed")
class DriftTest(unittest.TestCase):
    def test_operations_labels_and_fixtures_must_match_the_slice(self):
        with tempfile.TemporaryDirectory() as tmp:
            crate = Path(tmp)
            write_crate(crate, sample_spec(), ["countItems"])
            self.assertEqual(cg.drift(crate), [])

            config = yaml.safe_load((crate / "connector.yaml").read_text())
            config["operations"].append("listItems")
            config["labels"] = {"limit": "Page size"}
            (crate / "connector.yaml").write_text(yaml.safe_dump(config))
            (crate / "tests" / "fixtures").mkdir(parents=True)
            (crate / "tests" / "fixtures" / "countItems.json").write_text(json.dumps(3))
            problems = cg.drift(crate)
            self.assertEqual(len(problems), 3, problems)


if __name__ == "__main__":
    unittest.main()

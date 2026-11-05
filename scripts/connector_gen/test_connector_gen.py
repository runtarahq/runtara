#!/usr/bin/env python3
"""Tests for connector_gen.py.

The render tests need openapi-generator-cli (see README.md) and the
repository's rustfmt; they are skipped when the generator is not installed.

    python scripts/connector_gen/test_connector_gen.py
"""

import copy
import json
import re
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


def write_crate(crate: Path, spec: dict, operations: list, integration_id="sample_token", **config) -> dict:
    (crate / "connector.yaml").write_text(yaml.safe_dump({
        "agent": {
            "id": "sample",
            "name": "Sample & Co",
            "description": 'Sample "agent".',
            "integration_id": integration_id,
            "error_prefix": "SAMPLE",
        },
        "spec": {"url": "unused", "sha256": "unused"},
        "operations": operations,
        **config,
    }))
    sliced = cg.slice_spec(spec, operations, config.get("model_names"), config.get("body_envelopes") or [])
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

    def test_bare_equals_sign_is_a_string(self):
        self.assertEqual(cg.load_spec(b"example:\n  - change: =\n"), {"example": [{"change": "="}]})

    def test_tabs_after_a_value_are_whitespace(self):
        # UPS's Tracking spec: `default: <tab><tab>` on a CRLF line.
        spec = cg.load_spec(b"a:\r\n  default: \t\t  \r\n  b: x\t\r\n  c: \"keep\tinside\"\r\n")
        self.assertEqual(spec, {"a": {"default": None, "b": "x", "c": "keep\tinside"}})


class HumanizeTest(unittest.TestCase):
    def test_splits_words_and_keeps_acronyms(self):
        self.assertEqual(cg.humanize("person_id"), "Person ID")
        self.assertEqual(cg.humanize("personId"), "Person ID")
        self.assertEqual(cg.humanize("x-api-token"), "X API Token")
        self.assertEqual(cg.humanize("HTMLBody"), "HTML Body")
        self.assertEqual(cg.humanize("label_ids"), "Label IDs")
        self.assertEqual(cg.humanize("page[size]"), "Page Size")


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


def spec_with(op: dict, schemas: dict = None, method: str = "post", **components) -> dict:
    op = {"operationId": "op", "responses": {"204": {"description": "ok"}}, **op}
    return {"openapi": "3.0.1", "info": {"title": "t", "version": "1"},
            "paths": {"/x": {method: op}}, "components": {"schemas": schemas or {}, **components}}


def json_body(schema: dict) -> dict:
    return {"requestBody": {"content": {"application/json": {"schema": schema}}}}


def ok_json(schema: dict, **media) -> dict:
    return {"responses": {"200": {"description": "ok", "content": {"application/json": {
        "schema": schema, **media}}}}}


class NormalizeTest(unittest.TestCase):
    """Spec shapes the templates could not render as written are rewritten by `fetch`."""

    def test_example_refs_are_inlined(self):
        spec = spec_with(
            ok_json({"type": "object"}, examples={"one": {"$ref": "#/components/examples/One"}}),
            examples={"One": {"value": {"id": 1}}},
        )
        sliced = cg.slice_spec(spec, ["op"])
        self.assertEqual(cg.response_examples(sliced), {"op": {"id": 1}})
        self.assertNotIn("examples", sliced["components"])

    def test_deep_object_query_parameter_is_one_parameter_per_field(self):
        page = {"in": "query", "name": "page", "style": "deepObject", "description": "Pagination", "schema": {
            "oneOf": [
                {"type": "integer"},
                {"type": "object", "properties": {
                    "size": {"type": "integer", "description": "Page size"},
                    "after": {"type": "string"},
                }},
            ]}}
        sliced = cg.slice_spec(spec_with({"parameters": [page]}, method="get"), ["op"])
        params = operation(sliced, "op")["parameters"]
        # The page-number alternative stays an input under the parameter's own name.
        self.assertEqual([p["name"] for p in params], ["page", "page[size]", "page[after]"])
        self.assertEqual(params[0]["schema"], {"type": "integer", "format": "int64"})
        self.assertEqual(params[0]["description"], "Pagination")
        self.assertEqual(params[1]["description"], "Page size")
        self.assertEqual(params[2]["description"], "Pagination")
        self.assertEqual(params[1]["schema"], {"type": "integer", "format": "int64"})

    def test_required_scalar_or_object_parameter_requires_neither_form(self):
        page = {"in": "query", "name": "page", "required": True, "style": "deepObject", "schema": {"oneOf": [
            {"type": "integer"},
            {"type": "object", "required": ["size"], "properties": {"size": {"type": "integer"}}},
        ]}}
        flt = {"in": "query", "name": "filter", "required": True, "style": "deepObject", "schema": {
            "type": "object", "required": ["status"], "properties": {"status": {"type": "string"}}}}
        sliced = cg.slice_spec(spec_with({"parameters": [page, flt]}, method="get"), ["op"])
        required = {p["name"]: p.get("required", False) for p in operation(sliced, "op")["parameters"]}
        # `{"page": 3}` alone is valid, so `page[size]` cannot be required...
        self.assertEqual(required["page"], False)
        self.assertEqual(required["page[size]"], False)
        # ...while an object-only parameter keeps its required fields.
        self.assertEqual(required["filter[status]"], True)

    def test_form_object_query_parameter_sends_its_fields_unprefixed(self):
        # No `style` means `form` with `explode: true`: status=…&owner=…, not filter[status]=….
        flt = {"in": "query", "name": "filter", "schema": {"type": "object", "properties": {
            "status": {"type": "string"}, "owner": {"type": "string"}}}}
        sliced = cg.slice_spec(spec_with({"parameters": [flt]}, method="get"), ["op"])
        self.assertEqual([p["name"] for p in operation(sliced, "op")["parameters"]], ["status", "owner"])

    def test_shared_request_body_is_unwrapped_once_per_operation(self):
        user = {"type": "object", "properties": {
            "name": {"type": "string"},
            "organization": {"type": "object", "properties": {"name": {"type": "string"}}},
        }}
        body = {"$ref": "#/components/requestBodies/UserBody"}
        spec = {"openapi": "3.0.1", "info": {"title": "t", "version": "1"},
                "paths": {"/users": {"post": {"operationId": "createUser", "requestBody": body,
                                              "responses": {"204": {"description": "ok"}}}},
                          "/users/{id}": {"put": {"operationId": "updateUser", "requestBody": dict(body),
                                                  "parameters": [{"in": "path", "name": "id", "required": True,
                                                                  "schema": {"type": "string"}}],
                                                  "responses": {"204": {"description": "ok"}}}},
                          "/organizations": {"post": {"operationId": "createOrganization",
                                                      "responses": {"204": {"description": "ok"}},
                                                      **json_body({"type": "object", "properties": {
                                                          "organization": user["properties"]["organization"]}})}}},
                "components": {"requestBodies": {"UserBody": {"content": {"application/json": {
                    "schema": {"type": "object", "properties": {"user": user}}}}}}}}
        sliced = cg.slice_spec(spec, ["createUser", "updateUser", "createOrganization"],
                               body_envelopes=["user", "organization"])
        for op_id in ("createUser", "updateUser"):
            body = operation(sliced, op_id)["requestBody"]
            self.assertEqual(body["x-runtara-body-envelope"], "user", op_id)
            fields = body["content"]["application/json"]["schema"]["properties"]
            self.assertEqual(list(fields), ["name", "organization"], op_id)

    def test_envelope_beside_a_required_field_fails(self):
        body = json_body({"type": "object", "required": ["ticket", "via_id"], "properties": {
            "ticket": {"type": "object", "properties": {"subject": {"type": "string"}}},
            "via_id": {"type": "integer"},
        }})
        with self.assertRaises(SystemExit) as raised:
            cg.slice_spec(spec_with(body), ["op"], body_envelopes=["ticket"])
        self.assertIn("via_id", str(raised.exception))

    def test_scalar_alternatives_of_a_parameter_are_sent_as_text(self):
        brand = {"in": "query", "name": "brand", "schema": {"oneOf": [{"type": "string"}, {"type": "integer"}]}}
        sliced = cg.slice_spec(spec_with({"parameters": [brand]}, method="get"), ["op"])
        self.assertEqual(operation(sliced, "op")["parameters"][0]["schema"], {"type": "string"})

    def test_open_object_with_fields_keeps_its_model(self):
        schema = {"type": "object", "additionalProperties": True, "properties": {"id": {"type": "string"}}}
        sliced = cg.slice_spec(spec_with(ok_json(schema)), ["op"])
        kept = operation(sliced, "op")["responses"]["200"]["content"]["application/json"]["schema"]
        self.assertNotIn("additionalProperties", kept)

    def test_schema_names_match_the_generated_struct(self):
        schemas = {
            "EmailCCObject": {"type": "object", "properties": {"user": {"$ref": "#/components/schemas/UserObject"}}},
            "UserObject": {"type": "object", "properties": {"id": {"type": "string"}}},
        }
        spec = spec_with(ok_json({"$ref": "#/components/schemas/EmailCCObject"}), schemas)
        sliced = cg.slice_spec(spec, ["op"])
        self.assertEqual(sorted(sliced["components"]["schemas"]), ["EmailCcObject", "UserObject"])
        response = operation(sliced, "op")["responses"]["200"]["content"]["application/json"]
        self.assertEqual(response["schema"], {"$ref": "#/components/schemas/EmailCcObject"})
        # A `model_names` entry wins.
        sliced = cg.slice_spec(spec_with(ok_json({"$ref": "#/components/schemas/EmailCCObject"}), schemas),
                               ["op"], {"EmailCCObject": "EmailCopy"})
        self.assertIn("EmailCCObject", sliced["components"]["schemas"])

    def test_body_envelope_becomes_the_body(self):
        ticket = {"type": "object", "properties": {"subject": {"type": "string"}}}
        body = json_body({"type": "object", "required": ["ticket"], "properties": {
            "ticket": {"$ref": "#/components/schemas/Ticket"},
            "system_metadata": {"type": "object"},
        }})
        sliced = cg.slice_spec(spec_with(body, {"Ticket": ticket}), ["op"], body_envelopes=["ticket"])
        body = operation(sliced, "op")["requestBody"]
        self.assertEqual(body["x-runtara-body-envelope"], "ticket")
        self.assertEqual(body["content"]["application/json"]["schema"], {"$ref": "#/components/schemas/Ticket"})

    def test_envelope_of_several_shapes_offers_all_their_fields(self):
        body = json_body({"type": "object", "properties": {"user": {"anyOf": [
            {"type": "object", "properties": {"name": {"type": "string"}, "email": {"type": "string"}}},
            {"type": "object", "properties": {"email": {"type": "string"}, "password": {"type": "string"}}},
        ]}}})
        sliced = cg.slice_spec(spec_with(body), ["op"], body_envelopes=["user"])
        body = operation(sliced, "op")["requestBody"]
        self.assertEqual(body["x-runtara-body-envelope"], "user")
        schema = body["content"]["application/json"]["schema"]
        self.assertEqual(list(schema["properties"]), ["name", "email", "password"])
        self.assertNotIn("required", schema)

    def test_envelope_without_fields_stays_one_input(self):
        body = json_body({"type": "object", "properties": {"user": {"oneOf": [{"type": "object"}]}}})
        sliced = cg.slice_spec(spec_with(body), ["op"], body_envelopes=["user"])
        self.assertNotIn("x-runtara-body-envelope", operation(sliced, "op")["requestBody"])

    def test_all_of_body_merges_its_parts(self):
        # Shippo's `ShipmentCreateRequest`: a shared base and the fields of creation.
        base = {"type": "object", "required": ["currency"], "properties": {
            "currency": {"type": "string"}, "note": {"type": "string", "description": "Base"}}}
        named = {"allOf": [{"$ref": "#/components/schemas/Base"}]}
        body = json_body({"allOf": [
            {"$ref": "#/components/schemas/Named"},
            {"required": ["to"], "properties": {"to": {"type": "string"}, "note": {"type": "string"}}},
        ]})
        sliced = cg.slice_spec(spec_with(body, {"Base": base, "Named": named}), ["op"])
        schema = operation(sliced, "op")["requestBody"]["content"]["application/json"]["schema"]
        self.assertEqual(schema["type"], "object")
        self.assertEqual(list(schema["properties"]), ["currency", "note", "to"])
        self.assertEqual(schema["required"], ["currency", "to"])
        # The later part refines the base.
        self.assertNotIn("description", schema["properties"]["note"])
        self.assertNotIn("allOf", schema)

    def test_one_of_body_offers_the_fields_of_every_shape(self):
        # Shippo's transaction: from a rate, or instant from a shipment.
        base = {"type": "object", "properties": {"label": {"type": "string"}}}
        instant = {"allOf": [{"$ref": "#/components/schemas/Base"}], "required": ["shipment"], "properties": {
            "shipment": {"type": "string"}}}
        rated = {"type": "object", "required": ["rate"], "properties": {
            "rate": {"type": "string"}, "label": {"type": "string"}}}
        body = json_body({"oneOf": [{"$ref": "#/components/schemas/Rated"}, {"$ref": "#/components/schemas/Instant"}]})
        sliced = cg.slice_spec(spec_with(body, {"Base": base, "Instant": instant, "Rated": rated}), ["op"])
        schema = operation(sliced, "op")["requestBody"]["content"]["application/json"]["schema"]
        self.assertEqual(list(schema["properties"]), ["rate", "label", "shipment"])
        self.assertNotIn("required", schema)
        self.assertNotIn("oneOf", schema)

    def test_composed_body_with_a_part_that_is_no_object_stays_as_it_is(self):
        for schema in (
            {"oneOf": [{"type": "object", "properties": {"a": {"type": "string"}}}, {"type": "array", "items": {}}]},
            {"oneOf": [{"type": "object", "properties": {"a": {"type": "string"}}}, {"type": "object"}]},
            {"allOf": [{"type": "object", "properties": {"a": {"type": "string"}}}, {"oneOf": [{"type": "object"}]}]},
        ):
            sliced = cg.slice_spec(spec_with(json_body(copy.deepcopy(schema))), ["op"])
            kept = operation(sliced, "op")["requestBody"]["content"]["application/json"]["schema"]
            self.assertEqual(sorted(kept), sorted(schema))

    def test_plain_object_body_keeps_its_reference(self):
        item = {"type": "object", "properties": {"a": {"type": "string"}}}
        sliced = cg.slice_spec(spec_with(json_body({"$ref": "#/components/schemas/Item"}), {"Item": item}), ["op"])
        schema = operation(sliced, "op")["requestBody"]["content"]["application/json"]["schema"]
        self.assertEqual(schema, {"$ref": "#/components/schemas/Item"})

    def test_shared_composed_body_is_flattened_per_operation(self):
        shared = {"content": {"application/json": {"schema": {"allOf": [
            {"type": "object", "properties": {"a": {"type": "string"}}},
            {"type": "object", "properties": {"b": {"type": "string"}}},
        ]}}}}
        spec = spec_with({"requestBody": {"$ref": "#/components/requestBodies/Shared"}},
                         requestBodies={"Shared": shared})
        spec["paths"]["/y"] = {"post": {"operationId": "other", "responses": {"204": {"description": "ok"}},
                                        "requestBody": {"$ref": "#/components/requestBodies/Shared"}}}
        sliced = cg.slice_spec(spec, ["op", "other"])
        for name in ("op", "other"):
            schema = operation(sliced, name)["requestBody"]["content"]["application/json"]["schema"]
            self.assertEqual(list(schema["properties"]), ["a", "b"])

    def test_composed_body_can_hold_an_envelope(self):
        ticket = {"type": "object", "properties": {"subject": {"type": "string"}}}
        body = json_body({"allOf": [
            {"type": "object", "properties": {"ticket": {"$ref": "#/components/schemas/Ticket"}}},
            {"type": "object", "properties": {"note": {"type": "string"}}},
        ]})
        sliced = cg.slice_spec(spec_with(body, {"Ticket": ticket}), ["op"], body_envelopes=["ticket"])
        body = operation(sliced, "op")["requestBody"]
        self.assertEqual(body["x-runtara-body-envelope"], "ticket")
        self.assertEqual(body["content"]["application/json"]["schema"], {"$ref": "#/components/schemas/Ticket"})

    def test_envelope_shapes_may_be_composed(self):
        base = {"type": "object", "properties": {"name": {"type": "string"}}}
        body = json_body({"type": "object", "properties": {"user": {"anyOf": [
            {"allOf": [{"$ref": "#/components/schemas/Base"}]},
            {"type": "object", "properties": {"email": {"type": "string"}}},
        ]}}})
        sliced = cg.slice_spec(spec_with(body, {"Base": base}), ["op"], body_envelopes=["user"])
        schema = operation(sliced, "op")["requestBody"]["content"]["application/json"]["schema"]
        self.assertEqual(list(schema["properties"]), ["name", "email"])

    def test_unmatched_envelope_fails(self):
        with self.assertRaises(SystemExit) as raised:
            cg.slice_spec(spec_with(json_body({"type": "object"})), ["op"], body_envelopes=["tikcet"])
        self.assertIn("tikcet", str(raised.exception))

    def test_required_written_on_a_field_is_dropped(self):
        # UPS's Shipping spec: `required: SecurePINType` on the field itself.
        pin = {"type": "object", "required": ["type"], "properties": {"type": {"type": "string", "required": "type"}}}
        header = {"in": "header", "name": "X-Id", "required": True, "schema": {"type": "string"}}
        body = json_body({"$ref": "#/components/schemas/Pin"})
        body["requestBody"]["required"] = True
        sliced = cg.slice_spec(spec_with({"parameters": [header], **body}, {"Pin": pin}), ["op"])
        kept = sliced["components"]["schemas"]["Pin"]
        self.assertEqual(kept["required"], ["type"])
        self.assertNotIn("required", kept["properties"]["type"])
        op = operation(sliced, "op")
        self.assertIs(op["parameters"][0]["required"], True)
        self.assertIs(op["requestBody"]["required"], True)

    def test_json_schema_list_forms_become_openapi_3(self):
        # Slack's spec: `items: [A, B]` for "A or B", `type: [T, "null"]`, `type: "null"`.
        reply = {"type": "object", "properties": {
            "either": {"items": [{"$ref": "#/components/schemas/A"}, {"type": "null"}]},
            "list": {"type": "array", "items": [{"type": "string"}, {"type": "integer"}]},
            "maybe": {"type": ["string", "null"]},
            "fields": {"type": ["object", "null", "array"], "items": {"type": "object"}},
            "never": {"type": "null"},
        }}
        a = {"type": "object", "properties": {"id": {"type": "string"}}}
        sliced = cg.slice_spec(spec_with(ok_json({"$ref": "#/components/schemas/Reply"}), {"Reply": reply, "A": a}),
                               ["op"])
        fields = sliced["components"]["schemas"]["Reply"]["properties"]
        self.assertEqual(fields["either"], {"anyOf": [{"$ref": "#/components/schemas/A"}], "nullable": True})
        self.assertIn("A", sliced["components"]["schemas"])
        self.assertEqual(fields["list"], {"type": "array", "items": {
            "anyOf": [{"type": "string"}, {"type": "integer", "format": "int64"}]}})
        self.assertEqual(fields["maybe"], {"type": "string", "nullable": True})
        # Each alternative keeps the keywords of its own type.
        self.assertEqual(fields["fields"], {"nullable": True, "anyOf": [
            {"type": "object"}, {"type": "array", "items": {"type": "object"}}]})
        self.assertEqual(fields["never"], {"nullable": True})

    def test_items_listed_beside_a_type_list_stay_the_arrays(self):
        a, b = {"$ref": "#/components/schemas/A"}, {"$ref": "#/components/schemas/B"}
        cases = (
            ({"type": ["array", "null"], "items": [a, b]},
             {"type": "array", "nullable": True, "items": {"anyOf": [a, b]}}),
            ({"type": ["array", "object"], "items": [a, b]},
             {"anyOf": [{"type": "array", "items": {"anyOf": [a, b]}}, {"type": "object"}]}),
            # Only ever null: no empty `anyOf`, which is no valid schema.
            ({"anyOf": [{"type": "null"}]}, {"nullable": True}),
            ({"items": [{"type": "null"}]}, {"nullable": True}),
        )
        for schema, expected in cases:
            spec = {"components": {"schemas": {"X": copy.deepcopy(schema)}}}
            cg.normalize_schemas(spec)
            self.assertEqual(spec["components"]["schemas"]["X"], expected, schema)

    def test_a_phrase_naming_an_inline_object_is_spelled_as_its_struct(self):
        # The generator names the struct `ConversationImChannel`; the phrase stays the label.
        holder = {"type": "object", "properties": {"channel": {
            "type": "object", "title": "Conversation IM Channel", "properties": {"id": {"type": "string"}}}}}
        sliced = cg.slice_spec(spec_with(ok_json({"$ref": "#/components/schemas/Holder"}), {"Holder": holder}),
                               ["op"])
        cg.describe_fields(sliced, {})
        channel = sliced["components"]["schemas"]["Holder"]["properties"]["channel"]
        self.assertEqual(channel["title"], "Conversation Im Channel")
        self.assertEqual(channel["x-runtara-label"], "Conversation IM Channel")

    def test_default_response_is_a_response_not_data(self):
        error = {"type": "object", "properties": {"ok": {"$ref": "#/components/schemas/False"}}}
        op = {"responses": {"200": {"description": "ok"}, "default": {"description": "error", "content": {
            "application/json": {"schema": {"$ref": "#/components/schemas/Error"}}}}}}
        sliced = cg.slice_spec(spec_with(op, {"Error": error, "False": {"type": "boolean", "enum": [False]}}),
                               ["op"])
        # The error schema is kept, and the enum it refers to inlined.
        self.assertEqual(sliced["components"]["schemas"]["Error"]["properties"]["ok"],
                         {"type": "boolean", "enum": [False]})

    def test_a_schema_named_required_is_kept(self):
        schema = {"type": "object", "properties": {"id": {"type": "string"}}}
        sliced = cg.slice_spec(spec_with(ok_json({"$ref": "#/components/schemas/required"}), {"required": schema}),
                               ["op"])
        self.assertEqual(sliced["components"]["schemas"]["required"]["properties"], schema["properties"])


class ConnectionPartsTest(unittest.TestCase):
    """Parameters no step input sets: headers the connection sends
    (`connection_headers`) and path parameters pinned to the spec's default
    (`pinned_path_parameters`)."""

    @staticmethod
    def spec() -> dict:
        version = {"in": "path", "name": "version", "required": True,
                   "schema": {"type": "string", "default": "v2409"}}
        trans_id = {"in": "header", "name": "transId", "required": True, "schema": {"type": "string"}}
        shipment = {"in": "path", "name": "id", "required": True, "schema": {"type": "string"}}
        return {"openapi": "3.0.1", "info": {"title": "t", "version": "1"}, "paths": {
            "/shipments/{version}/void/{id}": {"parameters": [version], "delete": {
                "operationId": "void", "parameters": [trans_id, shipment],
                "responses": {"204": {"description": "ok"}}}},
        }}

    def test_they_are_no_inputs(self):
        sliced = cg.slice_spec(self.spec(), ["void"], connection_headers=["transId"],
                               pinned_path_parameters=["version"])
        self.assertEqual(list(sliced["paths"]), ["/shipments/v2409/void/{id}"])
        item = sliced["paths"]["/shipments/v2409/void/{id}"]
        self.assertEqual(item["parameters"], [])
        self.assertEqual([p["name"] for p in item["delete"]["parameters"]], ["id"])

    def test_pinned_parameter_needs_a_default(self):
        spec = self.spec()
        del spec["paths"]["/shipments/{version}/void/{id}"]["parameters"][0]["schema"]["default"]
        with self.assertRaises(SystemExit) as raised:
            cg.slice_spec(spec, ["void"], pinned_path_parameters=["version"])
        self.assertIn("no default", str(raised.exception))

    def test_unmatched_names_fail(self):
        for kwargs, name in (({"connection_headers": ["transID"]}, "transID"),
                             ({"connection_headers": ["id"]}, "id"),
                             ({"connection_parameters": ["id"]}, "id"),
                             ({"pinned_path_parameters": ["transId"]}, "transId")):
            with self.assertRaises(SystemExit) as raised:
                cg.slice_spec(self.spec(), ["void"], **kwargs)
            self.assertIn(f"['{name}']", str(raised.exception))

    def test_credential_parameters_are_no_inputs_in_a_header_or_the_query(self):
        # Slack's `token`: a header on its POST methods, a query parameter on its GET ones.
        def token(where):
            return {"in": where, "name": "token", "required": True, "schema": {"type": "string"}}

        channel = {"in": "query", "name": "channel", "schema": {"type": "string"}}
        spec = {"openapi": "3.0.1", "info": {"title": "t", "version": "1"}, "paths": {
            "/chat.postMessage": {"post": {"operationId": "post", "parameters": [token("header")],
                                           "responses": {"200": {"description": "ok"}}}},
            "/conversations.history": {"get": {"operationId": "history", "parameters": [token("query"), channel],
                                               "responses": {"200": {"description": "ok"}}}},
        }}
        sliced = cg.slice_spec(spec, ["post", "history"], connection_parameters=["token"])
        self.assertEqual(operation(sliced, "post")["parameters"], [])
        self.assertEqual([p["name"] for p in operation(sliced, "history")["parameters"]], ["channel"])
        # `connection_headers` stays header-only.
        with self.assertRaises(SystemExit):
            cg.slice_spec(self.spec(), ["void"], connection_headers=["id"])


class InputTypesTest(unittest.TestCase):
    """`input_types` retypes the inputs a vendor's spec types wrongly."""

    @staticmethod
    def spec() -> dict:
        ts = {"in": "query", "name": "ts", "description": "Message timestamp.", "schema": {"type": "number"}}
        body = json_body({"type": "object", "properties": {
            "ts": {"type": "number", "description": "Message to delete."}, "channel": {"type": "string"}}})
        return {"openapi": "3.0.1", "info": {"title": "t", "version": "1"}, "paths": {
            "/replies": {"get": {"operationId": "replies", "parameters": [ts],
                                 "responses": {"200": {"description": "ok"}}}},
            "/delete": {"post": {"operationId": "delete", **body, "responses": {"200": {"description": "ok"}}}},
        }}

    def slice(self, input_types: dict) -> dict:
        return cg.slice_specs([({"operations": ["replies", "delete"]}, self.spec())], {"input_types": input_types})

    def test_parameters_and_body_fields_are_retyped(self):
        sliced = self.slice({"ts": "string"})
        self.assertEqual(operation(sliced, "replies")["parameters"][0]["schema"],
                         {"type": "string"})
        body = operation(sliced, "delete")["requestBody"]["content"]["application/json"]["schema"]
        self.assertEqual(body["properties"]["ts"], {"type": "string", "description": "Message to delete."})
        self.assertEqual(body["properties"]["channel"], {"type": "string"})

    def test_a_schema_replaces_the_type(self):
        sliced = self.slice({"ts": {"type": "array", "items": {"type": "string"}}})
        self.assertEqual(operation(sliced, "replies")["parameters"][0]["schema"],
                         {"type": "array", "items": {"type": "string"}})

    def test_a_schema_with_a_description_replaces_the_vendors(self):
        sliced = self.slice({"ts": {"type": "string", "description": "Message ID."}})
        body = operation(sliced, "delete")["requestBody"]["content"]["application/json"]["schema"]
        self.assertEqual(body["properties"]["ts"], {"type": "string", "description": "Message ID."})

    def test_a_name_no_input_has_fails(self):
        with self.assertRaises(SystemExit) as raised:
            self.slice({"ts": "string", "thread_ts": "string"})
        self.assertIn("['thread_ts']", str(raised.exception))


class Swagger2Test(unittest.TestCase):
    """A Swagger 2.0 spec is converted to OpenAPI 3 before it is sliced."""

    @staticmethod
    def spec() -> dict:
        token = {"in": "header", "name": "token", "type": "string", "required": True}
        return {
            "swagger": "2.0", "info": {"title": "Slack Web API", "version": "1.7.0"},
            "host": "slack.com", "basePath": "/api", "schemes": ["https"],
            "securityDefinitions": {"slackAuth": {"type": "oauth2", "flow": "accessCode",
                                                  "authorizationUrl": "https://slack.com/oauth/authorize",
                                                  "tokenUrl": "https://slack.com/api/oauth.access",
                                                  "scopes": {"chat:write": "chat:write"}}},
            "paths": {
                "/chat.postMessage": {"post": {
                    "operationId": "post",
                    "consumes": ["application/x-www-form-urlencoded", "application/json"],
                    "produces": ["application/json"],
                    "parameters": [
                        token,
                        {"in": "formData", "name": "channel", "type": "string", "required": True,
                         "description": "Channel to send to."},
                        {"in": "formData", "name": "unfurl_links", "type": "boolean"},
                    ],
                    "responses": {
                        "200": {"description": "ok", "schema": {"$ref": "#/definitions/Posted"},
                                "examples": {"application/json": {"ok": True, "ts": "1712345678.000100"}}},
                        "default": {"description": "error", "schema": {"$ref": "#/definitions/Error"}},
                    },
                }},
                "/conversations.history": {"get": {
                    "operationId": "history",
                    "parameters": [
                        {"in": "query", "name": "channel", "type": "string"},
                        {"in": "query", "name": "types", "type": "array", "items": {"type": "string"}},
                        {"in": "query", "name": "ids", "type": "array", "items": {"type": "integer"},
                         "collectionFormat": "multi"},
                    ],
                    "responses": {"200": {"description": "ok", "schema": {"type": "object"}}},
                }},
                "/files.remote.add": {"post": {
                    "operationId": "formOnly",
                    "consumes": ["application/x-www-form-urlencoded"],
                    "parameters": [{"in": "formData", "name": "title", "type": "string"}],
                    "responses": {"200": {"description": "ok"}},
                }},
                "/items": {"put": {
                    "operationId": "put",
                    "parameters": [{"in": "body", "name": "item", "required": True,
                                    "schema": {"$ref": "#/definitions/Posted"}}],
                    "responses": {"204": {"description": "ok"}},
                }},
            },
            "definitions": {
                "Posted": {"type": "object", "properties": {
                    "ok": {"type": "boolean"}, "ts": {"type": "string", "x-nullable": True}}},
                "Error": {"type": "object", "properties": {"error": {"type": "string"}}},
            },
        }

    def test_servers_components_and_security(self):
        converted = cg.convert_swagger2(self.spec())
        self.assertEqual(converted["openapi"], "3.0.3")
        self.assertEqual(converted["servers"], [{"url": "https://slack.com/api"}])
        self.assertEqual(sorted(converted["components"]["schemas"]), ["Error", "Posted"])
        self.assertEqual(converted["components"]["schemas"]["Posted"]["properties"]["ts"],
                         {"type": "string", "nullable": True})
        self.assertEqual(converted["components"]["securitySchemes"]["slackAuth"]["flows"]["authorizationCode"]
                         ["tokenUrl"], "https://slack.com/api/oauth.access")

    def test_form_parameters_become_a_body_in_every_media_type(self):
        post = cg.convert_swagger2(self.spec())["paths"]["/chat.postMessage"]["post"]
        self.assertEqual([p["name"] for p in post["parameters"]], ["token"])
        self.assertEqual(post["parameters"][0]["schema"], {"type": "string"})
        schema = {"type": "object", "required": ["channel"], "properties": {
            "channel": {"type": "string", "description": "Channel to send to."},
            "unfurl_links": {"type": "boolean"}}}
        self.assertEqual(post["requestBody"], {"required": True, "content": {
            "application/x-www-form-urlencoded": {"schema": schema}, "application/json": {"schema": schema}}})
        self.assertEqual(post["responses"]["200"]["content"]["application/json"], {
            "schema": {"$ref": "#/components/schemas/Posted"},
            "example": {"ok": True, "ts": "1712345678.000100"}})
        self.assertEqual(post["responses"]["default"]["content"]["application/json"]["schema"],
                         {"$ref": "#/components/schemas/Error"})

    def test_body_parameter_and_query_arrays(self):
        converted = cg.convert_swagger2(self.spec())
        put = converted["paths"]["/items"]["put"]
        self.assertEqual(put["requestBody"], {"required": True, "content": {
            "application/json": {"schema": {"$ref": "#/components/schemas/Posted"}}}})
        channel, types, ids = converted["paths"]["/conversations.history"]["get"]["parameters"]
        # Swagger 2.0 sends arrays comma-separated unless `collectionFormat: multi`.
        self.assertEqual((types["style"], types["explode"]), ("form", False))
        self.assertEqual((ids["style"], ids["explode"]), ("form", True))
        self.assertNotIn("style", channel)

    def test_slices_like_an_openapi_3_spec(self):
        sliced = cg.slice_spec(self.spec(), ["post", "history"], connection_parameters=["token"])
        self.assertEqual(sliced["servers"], [{"url": "https://slack.com/api"}])
        post = operation(sliced, "post")
        self.assertEqual(post["parameters"], [])
        # The JSON alternative of the form body is kept.
        self.assertEqual(list(post["requestBody"]["content"]), ["application/json"])
        self.assertEqual(sorted(sliced["components"]["schemas"]), ["Error", "Posted"])
        self.assertEqual(cg.response_examples(sliced), {"post": {"ok": True, "ts": "1712345678.000100"}})

    def test_form_only_operations_are_refused(self):
        with self.assertRaises(SystemExit) as raised:
            cg.slice_spec(self.spec(), ["formOnly"])
        self.assertIn("only JSON bodies", str(raised.exception))

    def test_collection_formats_without_an_openapi_3_form(self):
        spec = self.spec()
        spec["paths"]["/conversations.history"]["get"]["parameters"][1]["collectionFormat"] = "tsv"
        with self.assertRaises(SystemExit) as raised:
            cg.convert_swagger2(spec)
        self.assertIn("tsv", str(raised.exception))
        spec["paths"]["/conversations.history"]["get"]["parameters"][1]["collectionFormat"] = "pipes"
        with self.assertRaises(SystemExit) as raised:
            cg.slice_spec(spec, ["history"])
        self.assertIn("pipeDelimited", str(raised.exception))


def merge_spec(path: str, op_id: str, schemas: dict, root: str = "Reply", servers=("https://example.com/api",),
               parameters=()) -> dict:
    return {"openapi": "3.0.1", "info": {"title": op_id, "version": "1"},
            "servers": [{"url": url} for url in servers],
            "paths": {path: {"get": {"operationId": op_id, "parameters": list(parameters),
                                     **ok_json({"$ref": f"#/components/schemas/{root}"})}}},
            "components": {"schemas": schemas}}


def merge(*specs, **config) -> dict:
    """`slice_specs` over `(name, operationId, spec)` triples."""
    return cg.slice_specs([({"name": name, "operations": [op_id]}, spec) for name, op_id, spec in specs], config)


class MergeTest(unittest.TestCase):
    """A connector generated from several specs of one API (`specs:`)."""

    @staticmethod
    def schemas(error_description: str) -> dict:
        return {
            "Reply": {"type": "object", "properties": {"holder": {"$ref": "#/components/schemas/Holder"},
                                                       "shared": {"$ref": "#/components/schemas/Shared"}}},
            "Holder": {"type": "object", "properties": {"error": {"$ref": "#/components/schemas/Error"}}},
            "Shared": {"type": "object", "properties": {"id": {"type": "string"}}},
            "Error": {"type": "object", "description": error_description,
                      "properties": {"code": {"type": "string"}}},
        }

    def test_a_schema_the_specs_define_differently_gets_the_spec_name(self):
        merged = merge(("Track", "track", merge_spec("/track", "track", self.schemas("Tracking's"))),
                       ("Ship", "ship", merge_spec("/ship", "ship", self.schemas("Shipping's"))))
        schemas = merged["components"]["schemas"]
        # `Error` differs; `Holder` and `Reply` only through what they refer to;
        # `Shared` is the same in both and kept once.
        self.assertEqual(sorted(schemas), ["Shared", "ShipError", "ShipHolder", "ShipReply",
                                           "TrackError", "TrackHolder", "TrackReply"])
        self.assertEqual(schemas["TrackHolder"]["properties"]["error"], {"$ref": "#/components/schemas/TrackError"})
        self.assertEqual(schemas["ShipReply"]["properties"]["shared"], {"$ref": "#/components/schemas/Shared"})
        response = operation(merged, "ship")["responses"]["200"]["content"]["application/json"]["schema"]
        self.assertEqual(response, {"$ref": "#/components/schemas/ShipReply"})
        self.assertEqual(sorted(merged["paths"]), ["/ship", "/track"])

    def test_one_name_spelled_two_ways_is_renamed(self):
        reply = {"type": "object", "properties": {"id": {"type": "string"}}}
        merged = merge(("Track", "track", merge_spec("/track", "track", {"Reply": reply})),
                       ("Ship", "ship", merge_spec("/ship", "ship", {"reply": reply}, root="reply")))
        self.assertEqual(sorted(merged["components"]["schemas"]), ["ShipReply", "TrackReply"])

    def test_first_operation_is_the_first_of_the_first_spec(self):
        merged = merge(("Track", "track", merge_spec("/track", "track", self.schemas("e"))),
                       ("Ship", "ship", merge_spec("/ship", "ship", self.schemas("e"))))
        self.assertTrue(operation(merged, "track")["x-runtara-first"])
        self.assertNotIn("x-runtara-first", operation(merged, "ship"))

    def test_a_listed_name_needs_to_match_in_one_spec_only(self):
        def specs():
            trans_id = {"in": "header", "name": "transId", "schema": {"type": "string"}}
            return (("Track", "track", merge_spec("/track", "track", self.schemas("e"), parameters=[trans_id])),
                    ("Ship", "ship", merge_spec("/ship", "ship", self.schemas("e"))))

        merged = merge(*specs(), connection_headers=["transId"])
        self.assertEqual(operation(merged, "track")["parameters"], [])
        with self.assertRaises(SystemExit) as raised:
            merge(*specs(), connection_headers=["transId", "transactionSrc"])
        self.assertIn("['transactionSrc']", str(raised.exception))

    def test_optional_fields_drop_what_the_spec_wrongly_requires(self):
        reply = {"type": "object", "required": ["id", "code", "name"],
                 "properties": {"id": {"type": "string"}, "code": {"type": "string"}, "name": {"type": "string"}}}
        specs = (("Track", "track", merge_spec("/track", "track", {"Reply": reply})),)
        merged = merge(*specs, optional_fields={"Reply": ["code", "name"]})
        self.assertEqual(merged["components"]["schemas"]["Reply"]["required"], ["id"])
        with self.assertRaises(SystemExit) as raised:
            merge(("Track", "track", merge_spec("/track", "track", {"Reply": reply})), optional_fields={"Reply": ["note"]})
        self.assertIn("['note']", str(raised.exception))

    def test_refusals(self):
        track = ("Track", "track", merge_spec("/track", "track", self.schemas("e")))
        cases = (
            ((track, ("Ship", "ship", merge_spec("/ship", "ship", self.schemas("e"),
                                                 servers=("https://other.example.com",)))), "servers"),
            ((track, ("Ship", "ship", merge_spec("/track", "ship", self.schemas("e")))), "more than one spec"),
            ((track, ("", "ship", merge_spec("/ship", "ship", self.schemas("e")))), "`name`"),
            ((track, ("Track", "ship", merge_spec("/ship", "ship", self.schemas("e")))), "`name`"),
            # Both would prefix `TrackError`; renaming used to loop forever.
            ((track, ("track", "ship", merge_spec("/ship", "ship", self.schemas("other")))), "`name`"),
            ((track, ("__", "ship", merge_spec("/ship", "ship", self.schemas("other")))), "`name`"),
        )
        for specs, fragment in cases:
            with self.assertRaises(SystemExit) as raised:
                merge(*specs)
            self.assertIn(fragment, str(raised.exception))


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

    def test_deep_object_without_fields_or_with_object_fields(self):
        spec = self.spec_with({"parameters": [{"in": "query", "name": "filter", "style": "deepObject", "schema": {
            "type": "object", "properties": {"range": {"type": "object"}}}}]})
        self.assert_refused(spec, "`range`")

    def test_object_query_parameter_in_other_styles(self):
        fields = {"type": "object", "properties": {"status": {"type": "string"}}}
        for extra in ({"style": "form", "explode": False}, {"style": "spaceDelimited"}):
            spec = self.spec_with({"parameters": [{"in": "query", "name": "filter", "schema": fields, **extra}]})
            self.assert_refused(spec, "is not supported")

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

    def test_a_bare_ref_field_is_labelled_through_all_of(self):
        # openapi-generator drops a label written beside a `$ref`.
        schema = {"type": "object", "properties": {
            "bot_id": {"$ref": "#/components/schemas/defs_bot_id", "description": "The bot."},
            "ok": {"$ref": "#/components/schemas/defs_ok"},
        }}
        cg.describe_fields(schema, {"ok": "OK"})
        self.assertEqual(schema["properties"]["bot_id"], {
            "allOf": [{"$ref": "#/components/schemas/defs_bot_id"}],
            "description": "The bot.", "x-runtara-label": "Bot ID"})
        self.assertEqual(schema["properties"]["ok"]["x-runtara-label"], "OK")

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
class EnvelopeRenderTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        ticket = {"type": "object", "properties": {
            "subject": {"type": "string"},
            "id": {"type": "integer", "readOnly": True},
        }}
        page = {"in": "query", "name": "page", "style": "deepObject", "schema": {
            "type": "object", "properties": {"size": {"type": "integer"}}}}
        spec = spec_with({
            "parameters": [page],
            **json_body({"type": "object", "properties": {"ticket": {"$ref": "#/components/schemas/Ticket"}}}),
        }, {"Ticket": ticket})
        with tempfile.TemporaryDirectory() as tmp:
            write_crate(Path(tmp), spec, ["op"], integration_id=["sample_oauth", "sample_token"],
                        body_envelopes=["ticket"])
            cls.text = cg.render(Path(tmp))

    def test_envelope_fields_are_inputs_wrapped_back_into_the_body(self):
        self.assertIn("pub subject: Option<String>,", self.text)
        self.assertNotIn("pub ticket:", self.text)
        self.assertIn('.field_opt("subject", &self.subject)', self.text)
        self.assertIn('.envelope("ticket")', self.text)
        self.assertNotIn('.field_opt("id"', self.text)

    def test_bracketed_parameter_keeps_its_wire_name(self):
        self.assertIn('rename = "page[size]"', self.text)
        self.assertIn("pub page_size: Option<i64>,", self.text)
        self.assertIn('.query_opt("page[size]", &self.page_size)', self.text)

    def test_several_integrations(self):
        self.assertIn('module_integration_ids = "sample_oauth,sample_token"', self.text)
        self.assertIn('integration_ids: vec!["sample_oauth".to_string(), "sample_token".to_string()]', self.text)


@unittest.skipUnless(generator_available(), "openapi-generator-cli is not installed")
class ComposedBodyRenderTest(unittest.TestCase):
    @staticmethod
    def render(body: dict, schemas: dict) -> str:
        with tempfile.TemporaryDirectory() as tmp:
            write_crate(Path(tmp), spec_with(json_body(body), schemas), ["op"])
            return cg.render(Path(tmp))

    def test_all_of_fields_are_inputs_and_a_required_one_stays_required(self):
        base = {"type": "object", "properties": {"note": {"type": "string"}}}
        text = self.render({"allOf": [
            {"$ref": "#/components/schemas/Base"},
            {"required": ["to"], "properties": {"to": {"type": "string"}}},
        ]}, {"Base": base})
        self.assertIn("pub to: String,", text)
        self.assertIn("pub note: Option<String>,", text)
        self.assertIn('.field("to", &self.to)', text)
        self.assertIn('.field_opt("note", &self.note)', text)
        self.assertNotIn("pub body:", text)

    def test_one_of_fields_are_all_optional_inputs(self):
        rated = {"type": "object", "required": ["rate"], "properties": {"rate": {"type": "string"}}}
        instant = {"type": "object", "required": ["shipment"], "properties": {"shipment": {"type": "string"}}}
        text = self.render(
            {"oneOf": [{"$ref": "#/components/schemas/Rated"}, {"$ref": "#/components/schemas/Instant"}]},
            {"Rated": rated, "Instant": instant},
        )
        self.assertIn("pub rate: Option<String>,", text)
        self.assertIn("pub shipment: Option<String>,", text)
        self.assertNotIn("pub body:", text)

    def test_field_that_is_an_object_or_an_id_stays_raw_json(self):
        address = {"type": "object", "properties": {"city": {"type": "string"}}}
        text = self.render({"allOf": [
            {"type": "object", "required": ["address_to"], "properties": {"address_to": {"oneOf": [
                {"$ref": "#/components/schemas/Address"}, {"type": "string"},
            ]}}},
        ]}, {"Address": address})
        self.assertRegex(text, r"pub address_to: \w+,")
        self.assertRegex(text, r"pub type \w*AddressTo = Value;")


@unittest.skipUnless(generator_available(), "openapi-generator-cli is not installed")
class ScalarOrObjectRenderTest(unittest.TestCase):
    """A required parameter that is a scalar or an object: either form alone
    deserializes, so `{"page": 3}` is a valid step input."""

    def test_both_forms_are_optional_fields(self):
        page = {"in": "query", "name": "page", "required": True, "style": "deepObject", "schema": {"oneOf": [
            {"type": "integer"},
            {"type": "object", "required": ["size"], "properties": {"size": {"type": "integer"}}},
        ]}}
        with tempfile.TemporaryDirectory() as tmp:
            write_crate(Path(tmp), spec_with({"parameters": [page]}, method="get"), ["op"])
            text = cg.render(Path(tmp))
        self.assertIn("pub page: Option<i64>,", text)
        self.assertIn("pub page_size: Option<i64>,", text)
        self.assertIn('.query_opt("page", &self.page)', text)
        self.assertIn('.query_opt("page[size]", &self.page_size)', text)


@unittest.skipUnless(generator_available(), "openapi-generator-cli is not installed")
class ModelNameRenderTest(unittest.TestCase):
    """Rendered with the real generator: every model a field refers to exists
    under that name, however the schema name mixes capitals and underscores."""

    NAMES = ["EmailCCObject", "IPv4Address", "XYzABCd", "HTMLBody", "OAuthToken", "SLAPolicyV2",
             "getHTTPResponseCode", "V2Item", "deal_status", "Mixed_Case_Name", "APIKeyAbc", "ABCdef"]

    def test_references_match_struct_names(self):
        schemas = {name: {"type": "object", "properties": {"id": {"type": "string"}}} for name in self.NAMES}
        schemas["Holder"] = {"type": "object", "properties": {
            f"f{i}": {"$ref": f"#/components/schemas/{name}"} for i, name in enumerate(self.NAMES)}}
        spec = spec_with(ok_json({"$ref": "#/components/schemas/Holder"}), schemas, method="get")
        with tempfile.TemporaryDirectory() as tmp:
            write_crate(Path(tmp), spec, ["op"])
            text = cg.render(Path(tmp))
        structs = set(re.findall(r"pub struct (\w+)", text))
        referenced = re.findall(r"pub f\d+: Option<(\w+)>", text)
        self.assertEqual(len(referenced), len(self.NAMES))
        self.assertEqual([name for name in referenced if name not in structs], [])

    def test_a_ref_field_keeps_its_type_and_gets_its_label(self):
        targets = {
            "Bot": {"type": "object", "properties": {"id": {"type": "string"}}},
            "Ids": {"type": "array", "items": {"type": "string"}},
            "Counts": {"type": "object", "additionalProperties": {"type": "integer"}},
            "Either": {"anyOf": [{"type": "string"}, {"$ref": "#/components/schemas/Bot"}]},
            "channel_id": {"type": "string", "pattern": "^C[A-Z0-9]+$"},
        }
        holder = {"type": "object", "properties": {
            name.lower(): {"$ref": f"#/components/schemas/{name}"} for name in targets}}
        op = {**ok_json({"$ref": "#/components/schemas/Holder"}), **json_body(copy.deepcopy(holder))}
        spec = spec_with(op, {**targets, "Holder": holder})
        with tempfile.TemporaryDirectory() as tmp:
            write_crate(Path(tmp), spec, ["op"])
            text = cg.render(Path(tmp))
        for struct in ("Holder", "OpInput"):
            body = text[text.index(f"pub struct {struct} "):]
            body = body[:body.index("\n}\n")]
            fields = dict(re.findall(r'display_name = r##"([^"]*)"##.*?pub \w+: ([^\n]+),', body, re.S))
            self.assertEqual(fields, {
                "Bot": "Option<Bot>", "IDs": "Option<Vec<String>>", "Counts": "Option<HashMap<String, i64>>",
                "Either": "Option<Either>", "Channel ID": "Option<String>"}, struct)
        # No struct of its own for a wrapped field.
        self.assertEqual(sorted(set(re.findall(r"pub struct (\w+)", text))),
                         ["Bot", "Holder", "OpInput", "OpRequest"])

    def test_inline_object_named_by_its_title(self):
        # UPS's proof of delivery: an inline object titled `POD`; Slack's
        # "Conversation IM Channel object", whose nested objects are named after it.
        nested = {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}}}}
        holder = {"type": "object", "properties": {
            f"f{i}": {"type": "object", "title": name, "properties": {"id": {"type": "string"}, "shares": nested}}
            for i, name in enumerate(["POD", "HTMLBody", "Proof of Delivery", "Conversation IM Channel object"])}}
        spec = spec_with(ok_json({"$ref": "#/components/schemas/Holder"}), {"Holder": holder}, method="get")
        with tempfile.TemporaryDirectory() as tmp:
            sliced = write_crate(Path(tmp), spec, ["op"])
            text = cg.render(Path(tmp))
        fields = sliced["components"]["schemas"]["Holder"]["properties"]
        # A title with spaces stays the field's label, as the vendor wrote it.
        self.assertEqual(fields["f2"]["x-runtara-label"], "Proof of Delivery")
        self.assertEqual(fields["f3"]["x-runtara-label"], "Conversation IM Channel object")
        structs = set(re.findall(r"pub struct (\w+)", text))
        referenced = re.findall(r"pub (?:f\d+|shares): Option<(?:Vec<)?(\w+)>", text)
        self.assertEqual(len(referenced), 8)
        self.assertEqual([name for name in referenced if name not in structs], [])


@unittest.skipUnless(generator_available(), "openapi-generator-cli is not installed")
class MergeRenderTest(unittest.TestCase):
    """Rendered with the real generator: the renamed schemas of a merged slice
    are the structs its fields and capabilities refer to."""

    def test_renamed_schemas_are_the_generated_structs(self):
        specs = [("Track", "track", merge_spec("/track", "track", MergeTest.schemas("Tracking's"))),
                 ("Ship", "ship", merge_spec("/ship", "ship", MergeTest.schemas("Shipping's")))]
        with tempfile.TemporaryDirectory() as tmp:
            crate = Path(tmp)
            (crate / "connector.yaml").write_text(yaml.safe_dump({
                "agent": {"id": "sample", "name": "Sample", "description": "Sample.",
                          "integration_id": "sample_token", "error_prefix": "SAMPLE"},
                "specs": [{"name": name, "url": "unused", "sha256": "unused", "operations": [op_id]}
                          for name, op_id, _ in specs],
            }))
            cg.write_json(crate / "spec.slice.json", merge(*specs))
            self.assertEqual(cg.drift(crate), [])
            text = cg.render(crate)
        structs = set(re.findall(r"pub struct (\w+)", text))
        self.assertLessEqual({"TrackReply", "ShipReply", "TrackError", "ShipError", "Shared"}, structs)
        self.assertIn("pub error: Option<TrackError>,", text)
        self.assertIn("pub async fn ship(input: ShipInput) -> Result<ShipReply, AgentError>", text)


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

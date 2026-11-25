"""Tests of how `sealed_connection.py` enters a connection's secrets.

Needs no server: `python3 e2e/test_sealed_connection.py`.
"""

import importlib.util
import json
from pathlib import Path
import unittest
import unittest.mock

spec = importlib.util.spec_from_file_location(
    "sealed_connection", Path(__file__).with_name("sealed_connection.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

ENTERED = {"$entered": True}


class StandInApi:
    """Answers `GET /connections/types/<id>` with the given connection type."""

    def __init__(self, connection_type):
        self.connection_type = connection_type

    def send(self, method, path, body=None):
        return 200, json.dumps({"connectionType": self.connection_type})


class RoutedApi:
    """A server for `save`: a Jira connection type that trims its token, one
    held connection, and offers. Records each request it is sent."""

    def __init__(self):
        self.sent = []

    def send(self, method, path, body=None):
        self.sent.append((method, path, json.loads(json.dumps(body))))
        if path == "/connections/types/jira_api_token":
            return 200, json.dumps({"connectionType": {
                "sealedFields": ["api_token"], "trimmedFields": ["api_token"]}})
        if method == "GET" and path == "/connections/held":
            return 200, json.dumps({"connection": {"integrationId": "jira_api_token"}})
        if path.endswith("/offer"):
            return 200, json.dumps({
                "connectionId": "offered", "offer": {}, "fields": {"api_token": "api_token"}})
        return 200, "{}"


class SaveTests(unittest.TestCase):
    """`save` trims a listed secret before sealing, on create and update."""

    def sealed_by(self, request, update=None):
        api, sealed = RoutedApi(), {}
        with unittest.mock.patch.object(
                module, "seal", lambda offered, secrets: sealed.update(secrets) or {}):
            module.save(api, request, update)
        return sealed, api.sent[-1][2]

    def test_a_create_seals_a_listed_secret_trimmed(self):
        sealed, saved = self.sealed_by({
            "title": "Jira", "integrationId": "jira_api_token",
            "connectionParameters": {"site": "acme", "email": "me@example.com ",
                                     "api_token": " tok\n"}})
        self.assertEqual(sealed, {"api_token": "tok"})
        self.assertEqual(saved["connectionParameters"]["api_token"], ENTERED)
        self.assertEqual(saved["connectionParameters"]["email"], "me@example.com ")

    def test_an_update_seals_a_listed_secret_trimmed(self):
        sealed, saved = self.sealed_by({
            "version": "v1",
            "connectionParameterPatch": {"write": {"api_token": "tok \r\n"},
                                         "set": {"email": "me@example.com "}}}, update="held")
        self.assertEqual(sealed, {"api_token": "tok"})
        self.assertEqual(saved["connectionParameterPatch"]["write"]["api_token"], ENTERED)
        self.assertEqual(saved["connectionParameterPatch"]["set"]["email"], "me@example.com ")


class EnterTests(unittest.TestCase):
    def test_a_trimmed_secret_is_entered_without_the_whitespace_around_it(self):
        for typed in ["cs_key\n", "cs_key\r\n", "cs_key ", " cs_key", "\tcs_key \n"]:
            parameters = {"consumer_key": "ck_key", "consumer_secret": typed}
            secrets, _ = module.enter(
                "woocommerce_api_key", {"consumer_secret"}, parameters, {"consumer_secret"})
            self.assertEqual(secrets, {"consumer_secret": "cs_key"}, repr(typed))
            self.assertEqual(parameters["consumer_secret"], ENTERED)

    def test_an_unannotated_secret_is_entered_as_typed_by_default(self):
        for integration_id, field in [("http_basic", "password"), ("future_type", "credential")]:
            with self.subTest(integration_id=integration_id):
                typed = " correct horse \n"
                parameters = {field: typed}
                secrets, _ = module.enter(integration_id, {field}, parameters)
                self.assertEqual(secrets, {field: typed})
                self.assertEqual(parameters[field], ENTERED)

    def test_a_trimmed_secret_of_whitespace_alone_is_left_blank(self):
        parameters = {"token": " \n"}
        secrets, _ = module.enter("http_bearer", {"token"}, parameters, {"token"})
        self.assertEqual(secrets, {})
        self.assertEqual(parameters["token"], "")

    def test_a_readable_field_is_left_to_the_server(self):
        parameters = {"email": "me@example.com ", "api_token": "tok\n"}
        secrets, _ = module.enter("jira_api_token", {"api_token"}, parameters, {"api_token"})
        self.assertEqual(secrets, {"api_token": "tok"})
        self.assertEqual(parameters["email"], "me@example.com ")

    def test_an_extra_header_of_an_mcp_connection_is_entered_as_typed(self):
        parameters = {"url": "https://mcp.example.com", "extra_headers": {"X-Key": " k "}}
        secrets, _ = module.enter("mcp", set(), parameters, set())
        self.assertEqual(secrets, {"extra_headers.X-Key": " k "})

    def test_the_fields_to_trim_are_the_ones_the_server_names(self):
        api = StandInApi({"sealedFields": ["api_token"], "trimmedFields": ["api_token"]})
        self.assertEqual(module.secret_fields(api, "jira_api_token"), ({"api_token"}, {"api_token"}))

    def test_a_server_that_names_none_has_none_trimmed(self):
        api = StandInApi({"sealedFields": ["api_token"]})
        self.assertEqual(module.secret_fields(api, "jira_api_token"), ({"api_token"}, set()))


if __name__ == "__main__":
    unittest.main()

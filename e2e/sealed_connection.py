#!/usr/bin/env python3
"""Saves a connection the way an API client must: with its secrets sealed.

The server refuses a secret sent readable. A client asks for an offer with
the request it is about to save, seals the secrets under it with
`seal-sealer`, and saves the same request with the envelope:

    sealed_connection.py --api http://127.0.0.1:7001/api/runtime \
        '{"title": "Orders", "integrationId": "http_bearer",
          "connectionParameters": {"base_url": "https://api.example.com",
                                   "token": "..."}}'

The request is written as it was before secrets were sealed: readable. This
marks each secret field of the connection type as entered (`{"$entered":
true}`), and nothing readable leaves this process. A secret of the type's
`trimmedFields` (a key or token a provider issued) is trimmed first, as a
client trims it: a pasted one often ends in a newline. A Postgres URL is split
as a client splits it: the password is the secret, and the rest of the URL goes
beside it as `database_target`.

To update, name the connection: `--update <id>` takes an update request
(`{"version": ..., "connectionParameterPatch": {"write": {...}}}`).

Prints the body of the answer to the save, or of the refusal that stopped it,
and with `--status` the HTTP status on a last line of its own.

`seal-sealer` is `$SEAL_SEALER_BIN`, or on PATH, or installed under
`target/e2e-tools` from the seal revision this checkout builds against.
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import urllib.error
import urllib.parse
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ENTERED = {"$entered": True}
PG_URL_FIELD = "database_url"
PG_TARGET_FIELD = "database_target"
EXTRA_HEADERS_FIELD = "extra_headers"


def seal_sealer():
    """The path of `seal-sealer`, installing it if this machine has none."""
    named = os.environ.get("SEAL_SEALER_BIN")
    if named:
        return named
    found = shutil.which("seal-sealer")
    if found:
        return found
    tools = os.path.join(ROOT, "target", "e2e-tools")
    installed = os.path.join(tools, "bin", "seal-sealer")
    if os.path.exists(installed):
        return installed
    manifest = open(os.path.join(ROOT, "crates", "runtara-connections", "Cargo.toml")).read()
    rev = re.search(r'runtarahq/seal", rev = "([0-9a-f]{40})"', manifest).group(1)
    subprocess.run(
        ["cargo", "install", "--locked", "--git", "https://github.com/runtarahq/seal",
         "--rev", rev, "seal-sealer", "--root", tools],
        check=True, stdout=sys.stderr, stderr=sys.stderr,
    )
    return installed


class Api:
    def __init__(self, base, headers, timeout):
        self.base = base.rstrip("/")
        self.headers = {"Content-Type": "application/json"}
        for header in headers:
            name, _, value = header.partition(":")
            self.headers[name.strip()] = value.strip()
        self.timeout = timeout

    def send(self, method, path, body=None):
        """One request: its status and its body, as text."""
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(
            self.base + path, data=data, method=method, headers=self.headers)
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:
                return response.status, response.read().decode()
        except urllib.error.HTTPError as error:
            return error.code, error.read().decode()


def secret_fields(api, integration_id):
    """The names of the connection type's fields that are sent sealed, and of
    those that are trimmed before they are sealed. A server that does not say
    which to trim has none trimmed."""
    status, body = api.send(
        "GET", "/connections/types/" + urllib.parse.quote(integration_id, safe=""))
    if status != 200:
        return set(), set()
    connection_type = json.loads(body)["connectionType"]
    return set(connection_type["sealedFields"]), set(connection_type.get("trimmedFields") or [])


def split_postgres_url(url):
    """A Postgres URL's password, and the URL without it."""
    parsed = urllib.parse.urlsplit(url)
    if parsed.password is None:
        return None
    user = parsed.netloc.rsplit("@", 1)[0].split(":", 1)[0]
    host = parsed.netloc.rsplit("@", 1)[1]
    target = urllib.parse.urlunsplit(parsed._replace(netloc=f"{user}@{host}"))
    return urllib.parse.unquote(parsed.password), target


def enter(integration_id, secret, parameters, trimmed=frozenset()):
    """Marks each readable secret of `parameters` as entered, in place.

    A secret named in `trimmed` is entered without the whitespace around it,
    and one that is nothing but whitespace is left blank.

    Returns the secrets by the name the request has for them, and what is
    added beside them: a Postgres target.
    """
    secrets, beside = {}, {}
    for name in list(parameters):
        value = parameters[name]
        if name in secret and name in trimmed and isinstance(value, str):
            value = parameters[name] = value.strip()
        if name not in secret or not isinstance(value, str) or not value:
            continue
        if integration_id == "postgres" and name == PG_URL_FIELD:
            split = split_postgres_url(value)
            if split is None:
                continue
            value, beside[PG_TARGET_FIELD] = split
        parameters[name] = dict(ENTERED)
        secrets[name] = value
    headers = parameters.get(EXTRA_HEADERS_FIELD)
    if integration_id == "mcp" and isinstance(headers, dict):
        for header, value in list(headers.items()):
            if isinstance(value, str) and value:
                headers[header] = dict(ENTERED)
                secrets[f"{EXTRA_HEADERS_FIELD}.{header}"] = value
    return secrets, beside


def seal(offered, secrets):
    """The intake envelope of `secrets`, sealed under the offer.

    The secrets go to `seal-sealer` on its standard input, as the one JSON
    object an intake envelope holds, and are never written to a file.
    """
    # Each under the name the offer gives it in the object that is sealed.
    members = {offered["fields"].get(name, name): value for name, value in secrets.items()}
    with tempfile.TemporaryDirectory() as work:
        offer = os.path.join(work, "offer.json")
        with open(offer, "w") as out:
            json.dump(offered["offer"], out)
        sealed = subprocess.run(
            [seal_sealer(), "--offer", offer, "--secret", "-"],
            input=json.dumps(members), check=True, capture_output=True, text=True)
        return json.loads(sealed.stdout)


def save(api, request, update=None):
    """Saves `request`, readable as written, with its secrets sealed."""
    if update is None:
        method, path = "POST", "/connections"
        integration_id = request.get("integrationId") or ""
        parameters = request.get("connectionParameters")
        if not isinstance(parameters, dict):
            return api.send(method, path, request)
        secret, trimmed = secret_fields(api, integration_id)
        secrets, beside = enter(integration_id, secret, parameters, trimmed)
        parameters.update(beside)
    else:
        method, path = "PUT", "/connections/" + urllib.parse.quote(update, safe="")
        status, held = api.send("GET", path)
        if status != 200:
            return status, held
        integration_id = json.loads(held)["connection"].get("integrationId") or ""
        patch = request.get("connectionParameterPatch") or {}
        secret, trimmed = secret_fields(api, integration_id)
        secrets, beside = enter(integration_id, secret, patch.get("write") or {}, trimmed)
        more, _ = enter(integration_id, secret, patch.get("set") or {}, trimmed)
        secrets.update(more)
        if beside:
            patch.setdefault("set", {}).update(beside)
    if not secrets:
        return api.send(method, path, request)
    status, offered = api.send("POST", path + "/offer", request)
    if status != 200:
        return status, offered
    offered = json.loads(offered)
    request["sealedSecrets"] = seal(offered, secrets)
    if update is None:
        request["connectionId"] = offered["connectionId"]
    return api.send(method, path, request)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--api", required=True, help="the API's base, e.g. http://host/api/runtime")
    parser.add_argument("--header", action="append", default=[], help="NAME: VALUE, repeatable")
    parser.add_argument("--update", metavar="ID", help="update this connection instead of creating one")
    parser.add_argument("--status", action="store_true", help="print the HTTP status on a last line")
    parser.add_argument("--timeout", type=float, default=120)
    parser.add_argument("request", help="the request's JSON, or - for standard input")
    args = parser.parse_args()
    text = sys.stdin.read() if args.request == "-" else args.request
    status, body = save(Api(args.api, args.header, args.timeout), json.loads(text), args.update)
    print(body)
    if args.status:
        print(status)


if __name__ == "__main__":
    main()

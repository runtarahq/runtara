# Taking in a connection's secrets sealed

What the server does, as built on `feat/sealed-intake` against seal 0.3.1. seal's own
account is in its `docs/integrating.md`, "Taking in a secret without holding it".

## What changed for a caller

A connection's secret is never sent readable. Create and update refuse one with 400:

```
`token` must be sealed where it is entered: ask for an offer, seal the secrets under
it, and save the envelope
```

Instead, a caller:

1. **Writes the request it will save**, with `{"$entered": true}` in place of each
   secret. For an update that is in the patch's `write` (or in `set`, for an MCP
   connection's `extra_headers` map).
2. **Asks for an offer** with that request: `POST /api/runtime/connections/offer` for a
   new connection, `POST /api/runtime/connections/{id}/offer` for an update. The body is
   the create or update request itself, without `sealedSecrets`.
3. **Seals the secrets** under the offer, as one JSON object of name to value: the
   console with seal's WASM sealer, anything else with `seal-sealer`.
4. **Saves the same request**, with the envelope as `sealedSecrets`, and for a new
   connection the id the offer gave as `connectionId`.

Which fields those are is the connection type's `sealedFields`
(`GET /connections/types/{id}`). It is not every field the form marks secret: a
certificate or a client's id is kept off the screen and still sent as it is. Besides
those, each value of an MCP connection's `extra_headers` is sent sealed.

A caller trims the whitespace around each secret in the type's `trimmedFields` before
sealing it: keys and tokens a provider issued, which a paste often leaves a newline or a
space on. The server cannot trim what it cannot read, and seal attaches a secret exactly
as it was sealed, so an untrimmed key fails every call it is sent with. The other sealed
fields (a password a person chose, a private key) are sealed as typed. A readable field
that is half of a credential, such as a client id or an account's email, is trimmed by
the server when a request supplies it. A field's type says which it is: a secret trims
unless it is marked `keep_whitespace`, a readable field only when it is marked `trim`.

The offer's answer:

```json
{
  "success": true,
  "connectionId": "0f78f802-7835-43ab-8b03-b32b1ae23981",
  "offer": { "public_key": "…", "kid": "…", "keyset": "…", "item": "…", "field": "intake", "policy": { } },
  "fields": { "token": "token" },
  "notAfter": 1795504709
}
```

- `offer` is seal's, passed to the sealer as it is. It holds nothing secret.
- `fields` gives, for each secret the request marks as entered, the name to give it in
  the object that is sealed. They are the same, except for an MCP connection's extra
  header: `extra_headers.<name>` is sealed under a name made for it.
- `notAfter` is when seal stops taking the offer: five minutes on
  (`OFFER_LIFETIME_SECONDS`).
- `offer` is null when the request marks no secret as entered; it is then saved with no
  `sealedSecrets`.

A save whose secrets were sealed under an offer that no longer stands answers 409 with
`"error": "OFFER_STALE"`: the offer is past its time, or the request is not the one it
was made for. The caller asks for a new offer and seals again. Any other 409 is what it
was before (a title taken, a connection changed since it was opened).

From a terminal, `e2e/sealed_connection.py` does all four steps for a request written
with its secrets readable.

## What holds

- **The engine reads no secret a caller entered.** seal opens the envelope and makes
  the stored values (`SealClient::admit`); the engine gets back the markers it stores.
- **The envelope is good for one request.** The offer names the policy of each secret,
  derived from the request's other fields. The save derives them again; if a base URL
  or anything else a policy is made from differs, seal refuses (`OFFER_STALE`).
  Tested: `secrets_sealed_for_one_destination_are_not_stored_for_another`,
  `an_envelope_is_good_for_the_request_it_was_offered_for`.
- **And for one connection of one tenant.** It is bound to the connection's id and
  sealed to the tenant's key.
- **Nothing is kept between the offer and the save.** No row, no table, no key in
  Valkey.
- **A secret entered at the engine is still sealed there**: `oauth-apps put`,
  `object-model set`. Those are `seal_parameters`; a caller's are `admit_parameters`.

## Where it is

- `crates/runtara-connections/src/sealed/mod.rs`: `offer`, `admit_parameters`,
  `store_parameters` (what both saves share), the `$entered` marker.
- `crates/runtara-connections/src/service/connections.rs`: `prepare_create` and
  `prepare_update` check a request and settle everything but its secrets. An offer and
  the save it is for both start there, so they derive the same from the same request.
  What one checks or changes, the other must.
- `handler/connections.rs`, `router.rs`: the two offer routes. They are connection
  routes, which the engine serves already, so nothing was added to `wire.rs`.
- `runtara-server/src/middleware/authorization.rs`: an offer needs the permission of
  the save it is for.
- `sealed::testing`: `enter`, `seal_under`, `save_as_a_caller`, for tests that have
  secrets readable.

## What changed that a user can see

- **A wrong mTLS key is no longer refused at save.** The server checked a private key
  against its certificate as the connection was saved. It cannot read the key now, so
  the certificates are checked at save and a key that does not match is refused by
  seal when it is first used.
- **A Postgres URL is sent in two parts.** The password is the secret
  (`database_url`), and the URL without it goes beside it as `database_target`, as it
  was typed. The server holds it to the rules of a whole URL (a stated `sslmode`,
  nothing else in the query), and stores it as before.
- **One save enters at most 16 secrets**, which is what an intake envelope holds. An
  MCP connection with more extra headers than that is saved in parts.
- **A new connection's id may come from the caller**, since the offer gives it before
  anything is stored. It must be a UUID no connection has.

## What is not done

- **Most e2e scripts are converted and not run.** Each script that created a
  connection with a secret now goes through `e2e/sealed_connection.py`. Two were run
  and pass: `test_optional_platform_db.sh` and `test_credential_binding.sh`, which
  create Postgres connections and run workflows on them. The rest were only checked
  for syntax.
- **A seal that is a process of its own** was not tried: everything here ran with
  seal in the server's process. The configurations in this repository should need no
  change: they limit only the intake role's caller (to `verify`, which is right: it
  must not admit), and the engine's caller may do everything. The scripts and CI
  install seal at the revision `Cargo.toml` names, so they follow the pin.
- **The console.** Until it seals, it cannot save a connection with a secret against
  this server. Its brief is `runtara-frontend/docs/sealed-secrets.md`.
- **`runtara-smoke-tests`**, and anything else outside this repository that creates
  connections over the API.
- **Release**: this breaks the API, so it is the next major, and ships with the
  console release that seals. The console needs the seal WASM module of the seal the
  server runs (0.3.1).

## What this does not protect against

- **A substituted offer.** The offer reaches the caller through the API role. One that
  is compromised when the offer is made hands over a key or a destination of its own.
  A compromise after the secret was sealed gains nothing.
- **Replay.** Within five minutes the same save can be sent again. For a create it
  fails on the id; for an update it fails on the version.
- **seal in process.** The engine holds the opening key.

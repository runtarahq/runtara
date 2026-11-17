# seal: where connection secrets are kept and used

A connection's secrets are never stored readable. Each is sealed by
[seal](https://github.com/runtarahq/seal) to a key of its tenant, together
with the one place it may be sent. To use it, the server hands seal the
sealed value and a request with no credential; seal opens the value, checks
the request against the destination sealed into it, attaches the credential
and sends the request itself.

Where seal runs is the deployment's choice, and it decides what someone who
breaks into the server gets:

| `RUNTARA_SEAL_MODE` | Where the keys are | A broken server gets |
|---|---|---|
| `in_process` (default) | In the server's database, wrapped by a key in the server's environment | Every credential |
| `unix` | With a seal process on the same host, under another user | The use of credentials, toward their sealed destinations, each use in seal's audit log. Not the credentials, unless the attacker also has root on the host |
| `mtls` | With a seal process on another host | The same, even with root on the server's host |

One binary and no other process is the right default for trying Runtara out
and for a single-tenant install that accepts it. A deployment that serves
several tenants, or holds credentials worth more than the server, runs seal
on its own.

## seal in the server

Nothing to set. Each tenant's key is made when its first secret is sealed
and kept in the server database (`seal_wrapped_key`), wrapped by
`RUNTARA_SEAL_WRAPPING_KEY` (32 bytes, base64) or, if that is unset, a key
derived from `RUNTARA_CONNECTIONS_ENCRYPTION_KEY`. A copy of the database
opens nothing without that key. The server logs at start that credentials
are readable by its process.

## seal as a process of its own

seal is a separate program with its own configuration, documented in its
repository (`docs/operations.md` there). What Runtara needs of it:

- **Its keys.** A directory on seal's host, or a bucket several seal
  instances share. The key that wraps them is a file on seal's host or stays
  in a KMS.
- **The server as a caller** that may use every tenant's keyset, make one
  and read its public keys (`admin`), and destroy one when a tenant leaves
  (`destroy_keysets`):

  ```toml
  [[callers]]
  name = "runtara-server"
  unix_uid = 10001            # over a socket: the server's uid
  # cert_subject = "CN=runtara-server"   # over mutual TLS: its certificate
  keysets = ["*"]
  admin = true
  destroy_keysets = true
  ```

- **What it may reach.** The server decides what a secret is sealed *for*
  (`RUNTARA_PROXY_ALLOW_HTTP_HOSTS`, `RUNTARA_DATABASE_CONNECTION_GUARD` and
  the like). seal decides, by its own `[network]` settings, whether it goes
  there: a private address needs `honour_private_rules`, a database with no
  TLS `honour_plaintext_postgres_rules`, and a provider with a private CA
  `extra_ca_files`. In the server's process these followed the server's
  settings; a separate seal is told itself.

On the server:

| Setting | |
|---|---|
| `RUNTARA_SEAL_MODE` | `unix` or `mtls` |
| `RUNTARA_SEAL_SOCKET` | `unix`: seal's socket. Several, separated by commas, are instances of one seal |
| `RUNTARA_SEAL_ADDRESS` | `mtls`: seal's `host:port`. Several, separated by commas, as above |
| `RUNTARA_SEAL_TLS_DIR` | `mtls`: a directory with `ca.pem` (the CA of seal's certificate) and the server's own `tls.pem` and `tls.key` |
| `RUNTARA_SEAL_SERVER_NAME` | `mtls`: the name seal's certificate is for; `seal` if unset |

A server told of a seal of its own:

- **holds no wrapping key.** `RUNTARA_SEAL_WRAPPING_KEY` set beside it is
  refused at start: the key is seal's to hold.
  `RUNTARA_CONNECTIONS_ENCRYPTION_KEY` is still needed, for what it has
  always encrypted (a connection's parameters as a whole), and no longer
  has anything to do with seal's keys;
- **does not start without seal.** It waits ten seconds for seal to answer,
  then stops, saying whether seal could not be reached or does not know it;
- **does not start on a seal that lacks its keys** (below).

seal writes one audit record per use of a secret: who asked, for which
keyset and item, where it went and how it ended. Never a value. That log is
the record of what a broken server did with the credentials it could use.

### Moving from seal in the server

A deployment that ran seal in its server has its tenants' keys in its
database. A seal that was not given them would make new ones, and every
secret sealed so far would stop opening, so the server refuses to start on
one. Hand the keys over first:

1. **Export**, with the server's usual environment:

   ```sh
   runtara-server seal-keys export --out ./seal-keys --wrapping-key-out ./seal-kek
   ```

   `./seal-keys` holds every tenant's key, still wrapped, laid out as seal's
   key directory is. `./seal-kek` is the key that wraps them, as seal reads
   a wrapping key file: 32 bytes, readable by its owner only. It opens every
   key; treat it as such. The command can be run again and replaces
   nothing.
2. **Give both to seal**: the directory as `keys.source` (or copied into the
   bucket seal reads), the file as `keys.wrap`. To keep the wrapping key in
   a KMS instead, start seal with the file and move to the KMS with seal's
   own `rewrap`.
3. **Start the server** with `RUNTARA_SEAL_MODE` set and
   `RUNTARA_SEAL_WRAPPING_KEY` unset. It asks seal for each key the database
   has, and starts when seal lists them all.
4. **Delete the database's copy** once the server runs:

   ```sh
   runtara-server seal-keys forget --confirm
   ```

   It deletes the keys only when seal answers for every one of them, and
   none otherwise. Until then a copy of the database, with the old wrapping
   key, still opens them.
5. **Destroy the old wrapping key** wherever else it was kept. If it was
   derived from the encryption key, nothing more is needed: seal's copy is
   the only one that was ever written down.

### A tenant that leaves

`runtara-server destroy-seal-keys --tenant-id <id> --confirm <id>` has seal
delete the tenant's keys, in a bucket every version of them. Nothing sealed
for the tenant opens again.

### In the evaluation stack

`docker-compose.seal.yml` adds seal to the stack, on a socket the two
containers share:

```sh
docker compose -f docker-compose.yml -f docker-compose.seal.yml up
```

A stack that already ran without it hands its keys over first; the commands
are at the top of that file.

## What this does and does not protect

With seal on its own, a broken server can still *use* a credential, toward
the destination it was sealed for, for as long as the breach lasts, and
seal's audit log shows each use. It cannot copy one out: when the breach is
closed, nothing was taken that still works. It also still has its databases.
seal's own threat model, in its repository, says the rest.

seal holds a credential to where it goes, not to what it does there. A
request's method, a database on the server a password is for, an object in
the account a key is for: those are the credential's own permissions at the
provider, and seal does not stand in for them. Give a connection a
credential that may do only what the connection is for.

Running seal on the server's host under another user protects against a
broken server process, not against root on that host. Running it on another
host protects against that too, and then the host seal is on is the one to
guard: whoever has its wrapping key and its keys has every credential.

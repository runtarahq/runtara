# Upgrade after SFTP removal

SFTP is no longer a supported workflow agent or connection type. The server no
longer exposes `/api/internal/agents/{module}/{capability_id}`, and
`RUNTARA_AGENT_SERVICE_URL` is no longer configured or injected into guests.
There is no generic native-capability dispatcher or fallback.

Before upgrading:

1. Find workflows containing SFTP steps and replace those steps before compiling.
   Stored workflow graphs and history are retained. SFTP graphs cannot compile
   against the new component catalog.
2. Resolve running, suspended, and scheduled instances that contain SFTP steps.
   Previously compiled components still contain the old wrapper and cannot run
   those steps after the endpoint is removed. Recompiling cannot restore SFTP.
3. Remove `sftp` from explicit entitlement allowlists. Unknown agent IDs fail
   entitlement validation at startup. Remove the obsolete service-URL override.
4. Deploy the new release's complete component directory rather than overlaying
   an older directory. `scripts/build-agent-components.sh` stages declared
   components into `target/agent-components` (or `$CARGO_TARGET_DIR/agent-components`).
   Release packaging also selects only current workspace components, so stale
   Cargo outputs are never shipped. A staging directory containing undeclared
   files is rejected; use a fresh directory rather than merging releases.

Existing SFTP connection rows and credentials are not deleted automatically.
They can be listed and deleted, but the type is absent from creation forms and
new connection creation or credential patches are rejected as an unknown type.
There is no database migration or automatic workflow rewrite.

S3, Azure presigning, HTTP credential injection, Object Model, and the connection
service retain their existing APIs. The `runtara-agents` native feature still
selects native HTTP for the server's S3 client. SSH/libssh2 dependencies are gone;
OpenSSL remains necessary for database TLS.

-- Drop `container_heartbeats`, a table nothing has written since the OCI runner
-- was deleted.
--
-- Its only writer, `ContainerRegistry::send_heartbeat`, had no caller outside
-- the registry's own tests and went in 7f87a7b7, so the table has been
-- permanently empty in production. It was still read, by a LEFT JOIN in
-- `db::get_instance_full`, which surfaced `heartbeat_at` through the
-- environment's `heartbeatAtMs`, the management SDK's `heartbeat_at`, and the
-- server's `metadata.heartbeatAt` -- an always-null field on three API
-- surfaces. The frontend declared a type for it and never rendered it. All of
-- that goes with the table.
--
-- Liveness detection does not depend on this: `heartbeat_monitor` decides an
-- instance is stale from `instance_events`, which the guest writes through
-- runtara-core.

DROP TABLE IF EXISTS container_heartbeats;

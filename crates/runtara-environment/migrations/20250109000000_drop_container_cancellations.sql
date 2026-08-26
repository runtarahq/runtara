-- Drop `container_cancellations`, a table that was written and deleted but
-- never read.
--
-- `request_cancellation` inserted a row, and `ContainerRegistry::cleanup` --
-- called a few lines later in the same `handle_stop_instance` body -- deleted
-- it again, so a row's whole lifetime was a few milliseconds inside one
-- function. Its only reader, `check_cancellation`, had no caller outside the
-- registry's own tests and went in 7f87a7b7.
--
-- Cancellation itself does not run through this table: `handle_stop_instance`
-- cancels via `Runner::stop`, and the drain path signals each guest with
-- `insert_signal(.., "shutdown", ..)`, which the guest reads from
-- runtara-core. The token was OCI-era residue, from when a separate container
-- process polled the database to learn it had been asked to stop.

DROP TABLE IF EXISTS container_cancellations;

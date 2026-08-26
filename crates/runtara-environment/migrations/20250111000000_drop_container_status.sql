-- Drop `container_status`, the last of the OCI-era report-back tables.
--
-- The guest used to write its own status here for the environment to read.
-- Both halves of that -- `report_status` and `get_status` -- had no caller
-- outside the registry's own tests and went in 7f87a7b7, leaving a table that
-- was only ever DELETEd from: no INSERT and no SELECT remained anywhere. A
-- guest now reports terminal state through runtara-core, and the environment
-- reads it from `instances`.

DROP TABLE IF EXISTS container_status;

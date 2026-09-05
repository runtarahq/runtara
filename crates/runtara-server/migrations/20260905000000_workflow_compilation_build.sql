-- Record which compiler build produced a compilation row.
--
-- A cached FAILURE is a claim about a compiler, not about a workflow: the same
-- definition that one build rejects, the next build may accept. The existing
-- provenance columns cannot express that, because `template_major` and
-- `lowering_mode` deliberately stay stable across releases so successful
-- artifacts survive a deploy. A build that only widens what the compiler
-- accepts therefore left every recorded failure looking current, and the
-- terminal-failure short-circuit kept replaying the stored error without ever
-- invoking the new compiler.
-- Keep this nullable: rows written before this column existed carry an unknown
-- build, must not be trusted as terminal, and are retried once.

ALTER TABLE workflow_compilations
    ADD COLUMN IF NOT EXISTS compiler_build TEXT;

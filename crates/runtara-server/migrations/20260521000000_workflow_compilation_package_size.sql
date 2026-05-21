-- Store the size of the generated workflow-logic crate (lib.rs + Cargo.toml +
-- world.wit + workflow.wac). Distinct from `wasm_size`, which is the size of
-- the composed `workflow.wasm` after `cargo component build` + `wac compose`.
-- Frontend surfaces both in the version list so users can see the impact of
-- their workflow growing.
--
-- Nullable: existing rows pre-date the field. Newly recorded compiles
-- populate it; older rows stay NULL until they are recompiled.
ALTER TABLE workflow_compilations
    ADD COLUMN IF NOT EXISTS package_size INTEGER;

-- Small key-value table for singleton process-state bookkeeping — not domain data.
--
-- First use case: `RUNTARA_PRICING_TIER` is parsed once into a `OnceLock<Config>` at boot and
-- never re-read for the life of the process, so a plan change mid-process is invisible to
-- runtara. The only point a plan change CAN be observed is at boot, by comparing the
-- just-locked-in plan against the last one persisted here (key = 'plan') — a divergence is
-- the `plan.changed` signal. Not multi-tenant: runtara is single-tenant per host, so there is
-- exactly one row per key.
CREATE TABLE metadata (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

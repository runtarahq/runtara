-- Store the registered spec verbatim. JSONB normalizes numbers (1e16 becomes an
-- integer) and rejects NUL escapes, so round-trips were not exact.
ALTER TABLE instance_input_requests ALTER COLUMN spec TYPE TEXT USING spec::text;

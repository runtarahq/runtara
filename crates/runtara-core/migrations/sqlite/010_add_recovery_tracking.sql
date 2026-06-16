-- Migration: track automatic recovery of instances killed by an Environment
-- restart. See the PostgreSQL 012 migration for the column semantics.
--
-- The 'environment_restart' termination_reason needs no DDL on SQLite: like
-- migration 009, termination_reason values are validated at the application
-- layer (SQLite CHECK constraints can't be altered in place), so new rows with
-- the new value just need the app to write valid values.
ALTER TABLE instances ADD COLUMN recovery_attempts INTEGER NOT NULL DEFAULT 0;
ALTER TABLE instances ADD COLUMN recovery_marker TEXT;

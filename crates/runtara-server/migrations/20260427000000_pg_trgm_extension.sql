-- Enable pg_trgm for trigram-based text similarity (Tier 1 SMO object model).
-- Idempotent: safe to apply on databases that already have the extension.
-- Privileges: pg_trgm is in the trusted-extension list on AWS RDS, GCP Cloud
-- SQL, and Azure, so a non-superuser with CREATE on the database can install
-- it. Self-hosted deployments need the equivalent grant.
CREATE EXTENSION IF NOT EXISTS "pg_trgm";

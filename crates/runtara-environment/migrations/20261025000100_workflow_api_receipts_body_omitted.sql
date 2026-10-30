-- A receipt keeps a response body only up to the workflow API's receipt cap.
-- Over it, the body is dropped and the receipt marks it, so a retry of that
-- request is refused instead of running again.
ALTER TABLE workflow_api_receipts
    ADD COLUMN IF NOT EXISTS body_omitted BOOLEAN NOT NULL DEFAULT false;

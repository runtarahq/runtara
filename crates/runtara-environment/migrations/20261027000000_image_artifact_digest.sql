-- An image names its workflow artifact by digest; the bytes live in the
-- artifact store. Every image recorded its binary's SHA-256 in metadata when
-- it was stored, which is that digest. `binary_path` stays for the rows the
-- boot import has not moved into the store yet, and is NULL afterwards.
ALTER TABLE images ADD COLUMN IF NOT EXISTS binary_digest TEXT;
ALTER TABLE images ALTER COLUMN binary_path DROP NOT NULL;
UPDATE images
SET binary_digest = metadata->'workflow'->>'binaryChecksum'
WHERE binary_digest IS NULL
  AND metadata->'workflow'->>'binaryChecksum' ~ '^[0-9a-f]{64}$';
CREATE INDEX IF NOT EXISTS idx_images_binary_digest ON images(binary_digest);

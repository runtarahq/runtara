-- Migration: Ensure image_id is TEXT, not UUID
-- This handles databases where runtara-core's images table was created with UUID
-- or where migration 003 was not applied.

-- Only alter if the column is not already TEXT
DO $$
BEGIN
    -- Check if images.image_id is UUID type and convert to TEXT
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'images'
          AND column_name = 'image_id'
          AND data_type = 'uuid'
    ) THEN
        -- Drop foreign key constraints referencing images.image_id
        ALTER TABLE instances DROP CONSTRAINT IF EXISTS instances_image_id_fkey;
        ALTER TABLE instance_images DROP CONSTRAINT IF EXISTS instance_images_image_id_fkey;

        -- Drop the default if it exists (gen_random_uuid from core's migration)
        ALTER TABLE images ALTER COLUMN image_id DROP DEFAULT;

        -- Convert to TEXT
        ALTER TABLE images ALTER COLUMN image_id TYPE TEXT USING image_id::TEXT;
        ALTER TABLE instances ALTER COLUMN image_id TYPE TEXT USING image_id::TEXT;

        -- Re-add foreign key constraints
        ALTER TABLE instances ADD CONSTRAINT instances_image_id_fkey
            FOREIGN KEY (image_id) REFERENCES images(image_id);
        ALTER TABLE instance_images ADD CONSTRAINT instance_images_image_id_fkey
            FOREIGN KEY (image_id) REFERENCES images(image_id) ON DELETE CASCADE;
    END IF;
END $$;

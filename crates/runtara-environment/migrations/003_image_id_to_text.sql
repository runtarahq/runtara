-- Migration: Change image_id from UUID to TEXT
-- This allows using any unique non-null string as image_id

-- Step 1: Drop the foreign key constraint on instances
ALTER TABLE instances DROP CONSTRAINT IF EXISTS instances_image_id_fkey;

-- Step 2: Change the column types
ALTER TABLE images ALTER COLUMN image_id TYPE TEXT USING image_id::TEXT;
ALTER TABLE instances ALTER COLUMN image_id TYPE TEXT USING image_id::TEXT;

-- Step 3: Re-add the foreign key constraint
ALTER TABLE instances ADD CONSTRAINT instances_image_id_fkey
    FOREIGN KEY (image_id) REFERENCES images(image_id);

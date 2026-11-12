-- OAuth apps the platform registered with a provider, for tenants to connect
-- with instead of registering their own. They were read from a file that
-- held each client secret readable; they are kept here, with the secret
-- sealed by seal as it is entered (`runtara-server oauth-apps put`).
--
-- `client_secret` is a sealed value's marker: an envelope sealed to the
-- platform's own seal key, to be sent only to the app's token endpoint. No
-- row holds a secret anything can read.
--
-- The table has no tenant: an app is the platform's, and which tenants may
-- use one is the host's to say.
CREATE TABLE IF NOT EXISTS platform_oauth_app (
    id TEXT PRIMARY KEY,
    integration_id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    client_id TEXT NOT NULL,
    client_secret JSONB NOT NULL,
    scopes TEXT NOT NULL,
    required_scopes TEXT NOT NULL DEFAULT '',
    parameters JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    CONSTRAINT platform_oauth_app_secret_is_sealed CHECK (client_secret ? '$sealed')
);

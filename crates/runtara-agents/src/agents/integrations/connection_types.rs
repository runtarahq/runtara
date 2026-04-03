//! SMO-specific connection type definitions
//!
//! This module defines connection types for SMO-specific integrations:
//! - Shopify (shopify_access_token)
//! - OpenAI (openai_api_key)
//! - AWS Bedrock (aws_credentials)
//!
//! These connection types are registered via the inventory crate and will be
//! returned by the `/api/runtime/connections/types` endpoint.

use crate::extractors::{HttpConnectionConfig, HttpConnectionExtractor};
use runtara_agent_macro::ConnectionParams;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

// ============================================================================
// Shopify Connection Type
// ============================================================================

/// Parameters for Shopify Admin API authentication
#[derive(Debug, Deserialize, ConnectionParams)]
#[connection(
    integration_id = "shopify_access_token",
    display_name = "Shopify",
    description = "Connect to Shopify Admin API using an access token",
    category = "ecommerce"
)]
pub struct ShopifyAccessTokenParams {
    /// Shopify Admin API access token
    #[field(
        display_name = "Access Token",
        description = "Shopify Admin API access token (starts with shpat_)",
        placeholder = "shpat_xxxxxxxxxxxxx",
        secret
    )]
    pub access_token: String,

    /// Shopify store domain
    #[field(
        display_name = "Shop Domain",
        description = "Your Shopify store domain (e.g., mystore.myshopify.com)",
        placeholder = "mystore.myshopify.com"
    )]
    pub shop_domain: String,

    /// API version (optional, defaults to latest stable)
    #[serde(default = "default_shopify_api_version")]
    #[field(
        display_name = "API Version",
        description = "Shopify Admin API version (e.g., 2025-01)",
        default = "2025-01"
    )]
    pub api_version: String,
}

fn default_shopify_api_version() -> String {
    "2025-01".to_string()
}

/// HTTP extractor for Shopify connections
pub struct ShopifyExtractor;

impl HttpConnectionExtractor for ShopifyExtractor {
    fn integration_id(&self) -> &'static str {
        "shopify_access_token"
    }

    fn extract(&self, params: &Value) -> Result<HttpConnectionConfig, String> {
        let p: ShopifyAccessTokenParams = serde_json::from_value(params.clone())
            .map_err(|e| format!("Invalid shopify_access_token connection parameters: {}", e))?;

        let mut headers = HashMap::new();
        headers.insert("X-Shopify-Access-Token".to_string(), p.access_token.clone());
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        // Build Shopify GraphQL Admin API URL
        let shop = p.shop_domain.trim_end_matches('/');
        let url_prefix = format!("https://{}/admin/api/{}/graphql.json", shop, p.api_version);

        Ok(HttpConnectionConfig {
            headers,
            query_parameters: HashMap::new(),
            url_prefix,
            rate_limit_config: None,
        })
    }
}

#[cfg(not(target_family = "wasm"))]
inventory::submit! {
    &ShopifyExtractor as &'static dyn HttpConnectionExtractor
}

// ============================================================================
// Shopify Client Credentials Connection Type
// ============================================================================

/// Parameters for Shopify Admin API authentication using OAuth2 client credentials.
///
/// Instead of a static access token, the runtime exchanges client_id + client_secret
/// for a short-lived token (24h) at execution time via:
/// POST https://{shop}/admin/oauth/access_token
#[derive(Debug, Deserialize, ConnectionParams)]
#[connection(
    integration_id = "shopify_client_credentials",
    display_name = "Shopify (Client Credentials)",
    description = "Connect to Shopify Admin API using OAuth2 client credentials (client ID and secret)",
    category = "ecommerce"
)]
pub struct ShopifyClientCredentialsParams {
    /// Shopify app Client ID
    #[field(
        display_name = "Client ID",
        description = "Shopify app Client ID from the app dashboard",
        placeholder = "your-client-id",
        secret
    )]
    pub client_id: String,

    /// Shopify app Client Secret
    #[field(
        display_name = "Client Secret",
        description = "Shopify app Client Secret from the app dashboard",
        placeholder = "your-client-secret",
        secret
    )]
    pub client_secret: String,

    /// Shopify store domain
    #[field(
        display_name = "Shop Domain",
        description = "Your Shopify store domain (e.g., mystore.myshopify.com)",
        placeholder = "mystore.myshopify.com"
    )]
    pub shop_domain: String,

    /// API version (optional, defaults to latest stable)
    #[serde(default = "default_shopify_api_version")]
    #[field(
        display_name = "API Version",
        description = "Shopify Admin API version (e.g., 2025-01)",
        default = "2025-01"
    )]
    pub api_version: String,

    // -- Access scopes (requested when exchanging credentials for a token) --
    #[serde(default)]
    #[field(display_name = "Read Products", description = "Read product data")]
    pub scope_read_products: Option<bool>,

    #[serde(default)]
    #[field(
        display_name = "Write Products",
        description = "Create and update products"
    )]
    pub scope_write_products: Option<bool>,

    #[serde(default)]
    #[field(display_name = "Read Orders", description = "Read order data")]
    pub scope_read_orders: Option<bool>,

    #[serde(default)]
    #[field(
        display_name = "Write Orders",
        description = "Create and update orders"
    )]
    pub scope_write_orders: Option<bool>,

    #[serde(default)]
    #[field(display_name = "Read Inventory", description = "Read inventory levels")]
    pub scope_read_inventory: Option<bool>,

    #[serde(default)]
    #[field(
        display_name = "Write Inventory",
        description = "Update inventory levels"
    )]
    pub scope_write_inventory: Option<bool>,

    #[serde(default)]
    #[field(display_name = "Read Locations", description = "Read location data")]
    pub scope_read_locations: Option<bool>,

    #[serde(default)]
    #[field(display_name = "Read Customers", description = "Read customer data")]
    pub scope_read_customers: Option<bool>,

    #[serde(default)]
    #[field(
        display_name = "Write Customers",
        description = "Create and update customers"
    )]
    pub scope_write_customers: Option<bool>,

    #[serde(default)]
    #[field(
        display_name = "Read Fulfillments",
        description = "Read fulfillment data"
    )]
    pub scope_read_fulfillments: Option<bool>,

    #[serde(default)]
    #[field(
        display_name = "Write Fulfillments",
        description = "Create and update fulfillments"
    )]
    pub scope_write_fulfillments: Option<bool>,
}

/// HTTP extractor for Shopify client credentials connections.
///
/// Note: This extractor provides the URL and content-type header but NOT the
/// access token header. The actual token exchange happens in
/// `shopify::resolve_access_token()` at execution time, since the
/// `HttpConnectionExtractor` trait is synchronous and cannot perform async HTTP calls.
pub struct ShopifyClientCredentialsExtractor;

impl HttpConnectionExtractor for ShopifyClientCredentialsExtractor {
    fn integration_id(&self) -> &'static str {
        "shopify_client_credentials"
    }

    fn extract(&self, params: &Value) -> Result<HttpConnectionConfig, String> {
        let p: ShopifyClientCredentialsParams =
            serde_json::from_value(params.clone()).map_err(|e| {
                format!(
                    "Invalid shopify_client_credentials connection parameters: {}",
                    e
                )
            })?;

        let mut headers = HashMap::new();
        // Access token header is added at execution time by resolve_access_token()
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        let shop = p.shop_domain.trim_end_matches('/');
        let url_prefix = format!("https://{}/admin/api/{}/graphql.json", shop, p.api_version);

        Ok(HttpConnectionConfig {
            headers,
            query_parameters: HashMap::new(),
            url_prefix,
            rate_limit_config: None,
        })
    }
}

#[cfg(not(target_family = "wasm"))]
inventory::submit! {
    &ShopifyClientCredentialsExtractor as &'static dyn HttpConnectionExtractor
}

// ============================================================================
// OpenAI Connection Type
// ============================================================================

/// Parameters for OpenAI API authentication
#[derive(Debug, Deserialize, ConnectionParams)]
#[connection(
    integration_id = "openai_api_key",
    display_name = "OpenAI",
    description = "Connect to OpenAI API for LLM, embeddings, and image generation",
    category = "llm"
)]
pub struct OpenAiApiKeyParams {
    /// OpenAI API key
    #[field(
        display_name = "API Key",
        description = "OpenAI API key (starts with sk-)",
        placeholder = "sk-xxxxxxxxxxxxx",
        secret
    )]
    pub api_key: String,

    /// Optional base URL for API requests (for proxies or compatible APIs)
    #[serde(default = "default_openai_base_url")]
    #[field(
        display_name = "Base URL",
        description = "API base URL (use default for OpenAI, or custom URL for compatible APIs)",
        default = "https://api.openai.com/v1"
    )]
    pub base_url: String,

    /// Optional organization ID
    #[serde(default)]
    #[field(
        display_name = "Organization ID",
        description = "Optional OpenAI organization ID"
    )]
    pub organization_id: Option<String>,
}

fn default_openai_base_url() -> String {
    "https://api.openai.com/v1".to_string()
}

/// HTTP extractor for OpenAI connections
pub struct OpenAiExtractor;

impl HttpConnectionExtractor for OpenAiExtractor {
    fn integration_id(&self) -> &'static str {
        "openai_api_key"
    }

    fn extract(&self, params: &Value) -> Result<HttpConnectionConfig, String> {
        let p: OpenAiApiKeyParams = serde_json::from_value(params.clone())
            .map_err(|e| format!("Invalid openai_api_key connection parameters: {}", e))?;

        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), format!("Bearer {}", p.api_key));
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        if let Some(org_id) = p.organization_id
            && !org_id.is_empty()
        {
            headers.insert("OpenAI-Organization".to_string(), org_id);
        }

        Ok(HttpConnectionConfig {
            headers,
            query_parameters: HashMap::new(),
            url_prefix: p.base_url,
            rate_limit_config: None,
        })
    }
}

#[cfg(not(target_family = "wasm"))]
inventory::submit! {
    &OpenAiExtractor as &'static dyn HttpConnectionExtractor
}

// ============================================================================
// AWS Bedrock Connection Type
// ============================================================================

/// Parameters for AWS Bedrock authentication
#[derive(Debug, Deserialize, ConnectionParams)]
#[connection(
    integration_id = "aws_credentials",
    display_name = "AWS Credentials",
    description = "AWS credentials for Bedrock and other AWS services",
    category = "llm"
)]
pub struct AwsCredentialsParams {
    /// AWS Access Key ID
    #[field(
        display_name = "Access Key ID",
        description = "AWS Access Key ID",
        placeholder = "AKIAIOSFODNN7EXAMPLE"
    )]
    pub aws_access_key_id: String,

    /// AWS Secret Access Key
    #[field(
        display_name = "Secret Access Key",
        description = "AWS Secret Access Key",
        secret
    )]
    pub aws_secret_access_key: String,

    /// AWS Region
    #[field(
        display_name = "Region",
        description = "AWS region for Bedrock (e.g., us-east-1)",
        placeholder = "us-east-1",
        default = "us-east-1"
    )]
    #[serde(default = "default_aws_region")]
    pub aws_region: String,

    /// Optional session token for temporary credentials
    #[serde(default)]
    #[field(
        display_name = "Session Token",
        description = "Optional AWS session token for temporary credentials",
        secret
    )]
    pub aws_session_token: Option<String>,
}

fn default_aws_region() -> String {
    "us-east-1".to_string()
}

// Note: AWS Bedrock doesn't use a simple HTTP extractor because it requires
// AWS SigV4 signing. The bedrock agent handles authentication directly.
// We only register the connection type for the schema, not an HTTP extractor.

// ============================================================================
// PostgreSQL Database Connection Type (for Object Store)
// ============================================================================

/// Parameters for a Telegram Bot connection (used by channel triggers)
#[derive(Debug, Deserialize, ConnectionParams)]
#[connection(
    integration_id = "telegram_bot",
    display_name = "Telegram Bot",
    description = "Connect a Telegram Bot for conversational channel triggers",
    category = "messaging",
    auth_type = "api_key"
)]
pub struct TelegramBotParams {
    /// Telegram Bot API token (from @BotFather)
    #[field(
        display_name = "Bot Token",
        description = "Telegram Bot API token obtained from @BotFather",
        placeholder = "123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11",
        secret
    )]
    pub bot_token: String,
}

// ============================================================================
// Slack Bot Connection Type
// ============================================================================

/// Parameters for a Slack Bot connection (used by channel triggers)
#[derive(Debug, Deserialize, ConnectionParams)]
#[connection(
    integration_id = "slack_bot",
    display_name = "Slack Bot",
    description = "Connect a Slack Bot for conversational channel triggers",
    category = "messaging",
    auth_type = "api_key"
)]
pub struct SlackBotParams {
    /// Slack Bot User OAuth Token (starts with xoxb-)
    #[field(
        display_name = "Bot Token",
        description = "Slack Bot User OAuth Token from the app's OAuth & Permissions page",
        placeholder = "xoxb-xxxxxxxxxxxx-xxxxxxxxxxxx-xxxxxxxxxxxxxxxxxxxxxxxx",
        secret
    )]
    pub bot_token: String,

    /// Slack Signing Secret (for verifying webhook requests)
    #[field(
        display_name = "Signing Secret",
        description = "Signing Secret from the app's Basic Information page, used to verify webhook requests",
        placeholder = "a1b2c3d4e5f6...",
        secret
    )]
    pub signing_secret: String,
}

// ============================================================================
// Microsoft Teams Bot Connection Type
// ============================================================================

/// Parameters for a Microsoft Teams Bot connection (used by channel triggers)
#[derive(Debug, Deserialize, ConnectionParams)]
#[connection(
    integration_id = "teams_bot",
    display_name = "Microsoft Teams Bot",
    description = "Connect a Microsoft Teams Bot for conversational channel triggers",
    category = "messaging",
    auth_type = "oauth2_client_credentials"
)]
pub struct TeamsBotParams {
    /// Microsoft App ID (from Azure Bot registration)
    #[field(
        display_name = "App ID",
        description = "Microsoft App ID from the Azure Bot resource",
        placeholder = "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
    )]
    pub app_id: String,

    /// Microsoft App Password (Client Secret)
    #[field(
        display_name = "App Password",
        description = "Client secret from the Azure Bot resource's certificates & secrets",
        placeholder = "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
        secret
    )]
    pub app_password: String,

    /// Azure AD Tenant ID (optional — leave empty for multi-tenant bots)
    #[serde(default)]
    #[field(
        display_name = "Tenant ID",
        description = "Azure AD tenant ID (leave empty for multi-tenant bots)"
    )]
    pub azure_tenant_id: Option<String>,
}

// ============================================================================
// Mailgun Connection Type
// ============================================================================

/// Parameters for Mailgun email service connection
#[derive(Debug, Deserialize, ConnectionParams)]
#[connection(
    integration_id = "mailgun",
    display_name = "Mailgun",
    description = "Connect to Mailgun for sending and receiving emails",
    category = "email",
    auth_type = "api_key"
)]
pub struct MailgunParams {
    /// Mailgun API key
    #[field(
        display_name = "API Key",
        description = "Mailgun API key (starts with key-)",
        placeholder = "key-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
        secret
    )]
    pub api_key: String,

    /// Mailgun domain
    #[field(
        display_name = "Domain",
        description = "Mailgun sending domain (e.g. mg.example.com)",
        placeholder = "mg.example.com"
    )]
    pub domain: String,

    /// Mailgun region (US or EU)
    #[serde(default = "default_mailgun_region")]
    #[field(
        display_name = "Region",
        description = "Mailgun region: us (default) or eu",
        default = "us"
    )]
    pub region: String,

    /// Webhook signing key (for verifying inbound webhook requests)
    #[serde(default)]
    #[field(
        display_name = "Webhook Signing Key",
        description = "Webhook signing key from Mailgun dashboard (for verifying inbound webhooks)",
        secret
    )]
    pub webhook_signing_key: Option<String>,
}

fn default_mailgun_region() -> String {
    "us".to_string()
}

/// HTTP extractor for Mailgun connections
pub struct MailgunExtractor;

impl HttpConnectionExtractor for MailgunExtractor {
    fn integration_id(&self) -> &'static str {
        "mailgun"
    }

    fn extract(&self, params: &Value) -> Result<HttpConnectionConfig, String> {
        let p: MailgunParams = serde_json::from_value(params.clone())
            .map_err(|e| format!("Invalid mailgun connection parameters: {}", e))?;

        let base_url = match p.region.as_str() {
            "eu" => format!("https://api.eu.mailgun.net/v3/{}", p.domain),
            _ => format!("https://api.mailgun.net/v3/{}", p.domain),
        };

        // Mailgun uses HTTP Basic auth: api:{api_key}
        let auth = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            format!("api:{}", p.api_key),
        );

        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), format!("Basic {}", auth));

        Ok(HttpConnectionConfig {
            headers,
            query_parameters: HashMap::new(),
            url_prefix: base_url,
            rate_limit_config: None,
        })
    }
}

#[cfg(not(target_family = "wasm"))]
inventory::submit! {
    &MailgunExtractor as &'static dyn HttpConnectionExtractor
}

// ============================================================================
// HubSpot Connection Type (OAuth2 Authorization Code)
// ============================================================================

/// Parameters for HubSpot OAuth2 authorization code flow.
///
/// The user provides client_id, client_secret, and scopes when creating
/// the connection. The OAuth callback handler then merges access_token,
/// refresh_token, and token_expires_at into connection_parameters after
/// the user completes authorization in the browser.
#[derive(Debug, Deserialize, ConnectionParams)]
#[connection(
    integration_id = "hubspot_private_app",
    display_name = "HubSpot",
    description = "Connect to HubSpot CRM using OAuth2 authorization",
    category = "crm",
    auth_type = "oauth2_authorization_code",
    oauth_auth_url = "https://app.hubspot.com/oauth/authorize",
    oauth_token_url = "https://api.hubapi.com/oauth/v1/token",
    oauth_default_scopes = "oauth crm.objects.contacts.read crm.objects.contacts.write crm.objects.companies.read crm.objects.companies.write crm.objects.deals.read crm.objects.deals.write crm.objects.quotes.read crm.objects.quotes.write crm.objects.line_items.read crm.objects.line_items.write crm.objects.owners.read"
)]
pub struct HubSpotPrivateAppParams {
    /// HubSpot app Client ID
    #[field(
        display_name = "Client ID",
        description = "Client ID from your HubSpot app settings",
        placeholder = "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
    )]
    pub client_id: String,

    /// HubSpot app Client Secret
    #[field(
        display_name = "Client Secret",
        description = "Client Secret from your HubSpot app settings",
        placeholder = "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx",
        secret
    )]
    pub client_secret: String,

    /// OAuth2 scopes (space-separated)
    #[serde(default = "default_hubspot_scopes")]
    #[field(
        display_name = "Scopes",
        description = "Space-separated OAuth2 scopes (e.g. 'crm.objects.contacts.read crm.objects.deals.read')",
        default = "oauth crm.objects.contacts.read crm.objects.contacts.write crm.objects.companies.read crm.objects.companies.write crm.objects.deals.read crm.objects.deals.write crm.objects.quotes.read crm.objects.quotes.write crm.objects.line_items.read crm.objects.line_items.write crm.objects.owners.read"
    )]
    pub scopes: String,
}

fn default_hubspot_scopes() -> String {
    "oauth crm.objects.contacts.read crm.objects.contacts.write crm.objects.companies.read crm.objects.companies.write crm.objects.deals.read crm.objects.deals.write crm.objects.quotes.read crm.objects.quotes.write crm.objects.line_items.read crm.objects.line_items.write crm.objects.owners.read".to_string()
}

/// HTTP extractor for HubSpot connections.
///
/// Note: the `Authorization` header is NOT set here because the access token
/// must be resolved at request time via `resolve_access_token()` in the agent.
/// This extractor only sets the base URL and Content-Type.
pub struct HubSpotExtractor;

impl HttpConnectionExtractor for HubSpotExtractor {
    fn integration_id(&self) -> &'static str {
        "hubspot_private_app"
    }

    fn extract(&self, params: &Value) -> Result<HttpConnectionConfig, String> {
        // Validate at least client_id exists (access_token/refresh_token may come later via OAuth callback)
        if params.get("client_id").and_then(|v| v.as_str()).is_none() {
            return Err("Invalid hubspot_private_app connection: missing client_id".to_string());
        }

        let mut headers = HashMap::new();
        // Authorization header is added at execution time by resolve_access_token()
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        Ok(HttpConnectionConfig {
            headers,
            query_parameters: HashMap::new(),
            url_prefix: "https://api.hubapi.com".to_string(),
            rate_limit_config: None,
        })
    }
}

#[cfg(not(target_family = "wasm"))]
inventory::submit! {
    &HubSpotExtractor as &'static dyn HttpConnectionExtractor
}

// ============================================================================
// HubSpot Connection Type (Static Access Token — Developer Projects)
// ============================================================================

/// Parameters for HubSpot developer project apps with static auth.
///
/// HubSpot developer projects with `"auth": { "type": "static" }` provide
/// a static access token managed by the platform. No OAuth2 flow needed.
#[derive(Debug, Deserialize, ConnectionParams)]
#[connection(
    integration_id = "hubspot_access_token",
    display_name = "HubSpot (Access Token)",
    description = "Connect to HubSpot CRM using a static access token from a developer project app",
    category = "crm",
    auth_type = "api_key"
)]
pub struct HubSpotAccessTokenParams {
    /// HubSpot access token
    #[field(
        display_name = "Access Token",
        description = "Static access token from your HubSpot developer project app",
        placeholder = "pat-eu1-xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx",
        secret
    )]
    pub access_token: String,
}

/// HTTP extractor for HubSpot access token connections
pub struct HubSpotAccessTokenExtractor;

impl HttpConnectionExtractor for HubSpotAccessTokenExtractor {
    fn integration_id(&self) -> &'static str {
        "hubspot_access_token"
    }

    fn extract(&self, params: &Value) -> Result<HttpConnectionConfig, String> {
        let p: HubSpotAccessTokenParams = serde_json::from_value(params.clone())
            .map_err(|e| format!("Invalid hubspot_access_token connection parameters: {}", e))?;

        let mut headers = HashMap::new();
        headers.insert(
            "Authorization".to_string(),
            format!("Bearer {}", p.access_token),
        );
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        Ok(HttpConnectionConfig {
            headers,
            query_parameters: HashMap::new(),
            url_prefix: "https://api.hubapi.com".to_string(),
            rate_limit_config: None,
        })
    }
}

#[cfg(not(target_family = "wasm"))]
inventory::submit! {
    &HubSpotAccessTokenExtractor as &'static dyn HttpConnectionExtractor
}

// ============================================================================
// PostgreSQL Database Connection Type (for Object Store)
// ============================================================================

/// Parameters for PostgreSQL database connection (used by Object Store agent)
#[derive(Debug, Deserialize, ConnectionParams)]
#[connection(
    integration_id = "postgres",
    display_name = "PostgreSQL Database",
    description = "Connect to a PostgreSQL database for Object Store operations",
    category = "database"
)]
pub struct PostgresDatabaseParams {
    /// PostgreSQL connection string
    #[field(
        display_name = "Database URL",
        description = "PostgreSQL connection string (postgresql://user:pass@host:port/dbname)",
        placeholder = "postgresql://user:password@localhost:5432/dbname",
        secret
    )]
    pub database_url: String,
}

// ============================================================================
// S3-Compatible Storage Connection Type
// ============================================================================

/// Parameters for S3-compatible storage connection (RustFS, MinIO, AWS S3, etc.)
#[derive(Debug, Deserialize, ConnectionParams)]
#[connection(
    integration_id = "s3_compatible",
    display_name = "S3-Compatible Storage",
    description = "Connect to S3-compatible object storage (RustFS, MinIO, AWS S3)",
    category = "storage"
)]
pub struct S3CompatibleParams {
    /// S3 endpoint URL
    #[field(
        display_name = "Endpoint",
        description = "S3-compatible endpoint URL (e.g., http://localhost:9000 for RustFS/MinIO)",
        placeholder = "http://localhost:9000"
    )]
    pub endpoint: String,

    /// Access key ID
    #[field(
        display_name = "Access Key ID",
        description = "S3 access key ID for authentication",
        placeholder = "minioadmin"
    )]
    pub access_key_id: String,

    /// Secret access key
    #[field(
        display_name = "Secret Access Key",
        description = "S3 secret access key for authentication",
        placeholder = "minioadmin",
        secret
    )]
    pub secret_access_key: String,

    /// AWS region (required by S3 protocol, use 'us-east-1' for most S3-compatible stores)
    #[serde(default = "default_s3_region")]
    #[field(
        display_name = "Region",
        description = "S3 region (use 'us-east-1' for most S3-compatible stores)",
        default = "us-east-1"
    )]
    pub region: String,

    /// Whether to use path-style addressing (required for most S3-compatible stores)
    #[serde(default = "default_path_style")]
    #[field(
        display_name = "Path Style",
        description = "Use path-style addressing (required for RustFS, MinIO; disable for AWS S3)",
        default = "true"
    )]
    pub path_style: Option<bool>,
}

fn default_s3_region() -> String {
    "us-east-1".to_string()
}

fn default_path_style() -> Option<bool> {
    Some(true)
}

// ============================================================================
// Stripe Connection Type
// ============================================================================

/// Parameters for Stripe API authentication
#[derive(Debug, Deserialize, ConnectionParams)]
#[connection(
    integration_id = "stripe_api_key",
    display_name = "Stripe",
    description = "Connect to Stripe API for payments, invoices, and subscriptions",
    category = "payment",
    auth_type = "api_key"
)]
pub struct StripeApiKeyParams {
    /// Stripe secret API key
    #[field(
        display_name = "Secret Key",
        description = "Stripe secret API key (starts with sk_live_ or sk_test_)",
        placeholder = "sk_test_xxxxxxxxxxxxxxxxxxxx",
        secret
    )]
    pub secret_key: String,
}

/// HTTP extractor for Stripe connections
pub struct StripeExtractor;

impl HttpConnectionExtractor for StripeExtractor {
    fn integration_id(&self) -> &'static str {
        "stripe_api_key"
    }

    fn extract(&self, params: &Value) -> Result<HttpConnectionConfig, String> {
        let p: StripeApiKeyParams = serde_json::from_value(params.clone())
            .map_err(|e| format!("Invalid stripe_api_key connection parameters: {}", e))?;

        let mut headers = HashMap::new();
        headers.insert(
            "Authorization".to_string(),
            format!("Bearer {}", p.secret_key),
        );

        Ok(HttpConnectionConfig {
            headers,
            query_parameters: HashMap::new(),
            url_prefix: "https://api.stripe.com/v1".to_string(),
            rate_limit_config: None,
        })
    }
}

#[cfg(not(target_family = "wasm"))]
inventory::submit! {
    &StripeExtractor as &'static dyn HttpConnectionExtractor
}

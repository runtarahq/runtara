//! SMO-specific connection type definitions
//!
//! This module defines connection types for SMO-specific integrations:
//! - Shopify (shopify_access_token)
//! - OpenAI (openai_api_key)
//! - AWS Bedrock (aws_credentials)
//!
//! These connection types are included in the static agent registry and returned
//! by the `/api/runtime/connections/types` endpoint.

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

    /// Optional custom endpoint. Leave blank for the default AWS regional
    /// endpoint of whatever service the calling agent targets (e.g. SQS →
    /// `https://sqs.{region}.amazonaws.com`). Set for LocalStack, VPC
    /// endpoints, GovCloud, or other custom hosts.
    #[serde(default)]
    #[field(
        display_name = "Endpoint",
        description = "Custom endpoint URL; leave blank for AWS defaults (LocalStack, VPC endpoints, GovCloud)",
        placeholder = "https://localhost:4566"
    )]
    pub endpoint: Option<String>,
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
// Microsoft Entra OAuth2 Client Credentials Connection Type
// ============================================================================

/// Parameters for Microsoft Entra OAuth2 client credentials authentication.
///
/// This is intentionally resource-agnostic: callers provide the resource scope
/// and API base URL, so the same connection type can be used for Microsoft Graph,
/// Dynamics 365 Business Central, or another API protected by Microsoft Entra ID.
#[derive(Debug, Deserialize, ConnectionParams)]
#[connection(
    integration_id = "microsoft_entra_client_credentials",
    display_name = "Microsoft Entra OAuth2 (Client Credentials)",
    description = "Connect to Microsoft Entra-protected APIs using OAuth2 client credentials",
    category = "api",
    service_id = "microsoft_entra",
    auth_type = "oauth2_client_credentials"
)]
pub struct MicrosoftEntraClientCredentialsParams {
    /// Microsoft Entra tenant ID or tenant domain.
    #[field(
        display_name = "Tenant ID",
        description = "Microsoft Entra tenant ID or tenant domain",
        placeholder = "00000000-0000-0000-0000-000000000000"
    )]
    pub tenant_id: String,

    /// Microsoft Entra application client ID.
    #[field(
        display_name = "Client ID",
        description = "Application (client) ID from the Microsoft Entra app registration",
        placeholder = "00000000-0000-0000-0000-000000000000"
    )]
    pub client_id: String,

    /// Microsoft Entra application client secret.
    #[field(
        display_name = "Client Secret",
        description = "Client secret from the Microsoft Entra app registration",
        secret
    )]
    pub client_secret: String,

    /// OAuth2 client credentials scope for the target resource.
    #[field(
        display_name = "Scope",
        description = "OAuth2 scope for the target resource, for example https://graph.microsoft.com/.default or https://api.businesscentral.dynamics.com/.default",
        placeholder = "https://graph.microsoft.com/.default"
    )]
    pub scope: String,

    /// Base URL for proxied API requests.
    #[field(
        display_name = "Base URL",
        description = "Base URL for API requests, for example https://graph.microsoft.com/v1.0 or https://api.businesscentral.dynamics.com/v2.0/production/api/v2.0 (must be https)",
        placeholder = "https://graph.microsoft.com/v1.0",
        is_url,
        is_required
    )]
    pub base_url: String,

    /// Microsoft identity authority host.
    #[serde(default = "default_microsoft_entra_authority_host")]
    #[field(
        display_name = "Authority Host",
        description = "Microsoft identity authority host (must be https)",
        default = "https://login.microsoftonline.com",
        is_url
    )]
    pub authority_host: String,
}

fn default_microsoft_entra_authority_host() -> String {
    "https://login.microsoftonline.com".to_string()
}

/// HTTP extractor for Microsoft Entra client credentials connections.
///
/// The bearer token is not resolved here. Workflows forward the connection ID
/// and the host-side proxy exchanges the token and injects Authorization.
pub struct MicrosoftEntraClientCredentialsExtractor;

impl HttpConnectionExtractor for MicrosoftEntraClientCredentialsExtractor {
    fn integration_id(&self) -> &'static str {
        "microsoft_entra_client_credentials"
    }

    fn extract(&self, params: &Value) -> Result<HttpConnectionConfig, String> {
        let p: MicrosoftEntraClientCredentialsParams = serde_json::from_value(params.clone())
            .map_err(|e| {
                format!(
                    "Invalid microsoft_entra_client_credentials connection parameters: {}",
                    e
                )
            })?;

        let base_url = p.base_url.trim();
        if base_url.is_empty() {
            return Err(
                "Invalid microsoft_entra_client_credentials connection: missing base_url"
                    .to_string(),
            );
        }

        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        Ok(HttpConnectionConfig {
            headers,
            query_parameters: HashMap::new(),
            url_prefix: base_url.trim_end_matches('/').to_string(),
            rate_limit_config: None,
        })
    }
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
    oauth_default_scopes = "oauth crm.objects.contacts.read crm.objects.contacts.write crm.objects.companies.read crm.objects.companies.write crm.objects.deals.read crm.objects.deals.write crm.objects.quotes.read crm.objects.quotes.write crm.objects.line_items.read crm.objects.line_items.write crm.objects.owners.read",
    oauth_base_url = "https://api.hubapi.com"
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

/// QuickBooks Online (Intuit) — OAuth 2.0 authorization-code connection.
///
/// Bring-your-own Intuit app: `client_id`/`client_secret` are entered per connection.
/// The descriptor drives every provider quirk: HTTP Basic on the token endpoint,
/// rotating refresh tokens, sandbox/prod host selection by `environment`, the
/// `/v3/company/{realm_id}` path template, and capturing `realmId` off the callback.
#[derive(Debug, Deserialize, ConnectionParams)]
#[connection(
    integration_id = "quickbooks_online",
    display_name = "QuickBooks Online",
    description = "Connect to Intuit QuickBooks Online (Accounting API v3) using OAuth2 authorization",
    category = "erp",
    auth_type = "oauth2_authorization_code",
    oauth_auth_url = "https://appcenter.intuit.com/connect/oauth2",
    oauth_token_url = "https://oauth.platform.intuit.com/oauth2/v1/tokens/bearer",
    oauth_default_scopes = "com.intuit.quickbooks.accounting",
    oauth_token_auth = "basic",
    oauth_refresh_rotates = true,
    oauth_base_url = "https://quickbooks.api.intuit.com",
    oauth_sandbox_base_url = "https://sandbox-quickbooks.api.intuit.com",
    oauth_base_url_path_template = "/v3/company/{realm_id}",
    oauth_extra_callback_params = "realm_id:realmId:true",
    oauth_reauth_on_error_codes = "invalid_grant",
    oauth_revocation_endpoint = "https://developer.api.intuit.com/v2/oauth2/tokens/revoke",
    oauth_pkce_required = true
)]
pub struct QuickBooksOnlineParams {
    /// Intuit app Client ID
    #[field(
        display_name = "Client ID",
        description = "Client ID from your Intuit app's keys"
    )]
    pub client_id: String,

    /// Intuit app Client Secret
    #[field(
        display_name = "Client Secret",
        description = "Client Secret from your Intuit app's keys",
        secret
    )]
    pub client_secret: String,

    /// Target Intuit environment: "sandbox" or "production"
    #[serde(default = "default_quickbooks_environment")]
    #[field(
        display_name = "Environment",
        description = "Target Intuit environment: 'sandbox' or 'production'",
        default = "sandbox"
    )]
    pub environment: String,

    /// Realm ID (Company ID) — populated automatically by the OAuth callback
    #[serde(default)]
    #[field(
        display_name = "Realm ID (Company ID)",
        description = "Populated automatically after OAuth consent; may be left blank"
    )]
    pub realm_id: Option<String>,

    /// OAuth2 scopes (space-separated)
    #[serde(default = "default_quickbooks_scopes")]
    #[field(
        display_name = "Scopes",
        description = "Space-separated OAuth2 scopes",
        default = "com.intuit.quickbooks.accounting"
    )]
    pub scopes: String,
}

fn default_quickbooks_environment() -> String {
    "sandbox".to_string()
}

fn default_quickbooks_scopes() -> String {
    "com.intuit.quickbooks.accounting".to_string()
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
// Azure Blob Storage Connection Type
// ============================================================================

/// Parameters for Azure Blob Storage connection (Shared Key authentication).
///
/// Credentials are kept server-side; the runtime proxy signs every request
/// with Azure Shared Key before forwarding upstream.
#[derive(Debug, Deserialize, ConnectionParams)]
#[connection(
    integration_id = "azure_blob_storage",
    display_name = "Azure Blob Storage",
    description = "Connect to Azure Blob Storage using a storage account name and access key",
    category = "storage"
)]
pub struct AzureBlobStorageParams {
    /// Storage account name (e.g. `mystorageacct`)
    #[field(
        display_name = "Account Name",
        description = "Azure storage account name",
        placeholder = "mystorageacct"
    )]
    pub account_name: String,

    /// Base64-encoded account key (primary or secondary)
    #[field(
        display_name = "Account Key",
        description = "Storage account access key (primary or secondary)",
        placeholder = "base64-encoded key",
        secret
    )]
    pub account_key: String,

    /// DNS suffix for the blob endpoint. Defaults to `core.windows.net`.
    /// Use `core.usgovcloudapi.net` for Azure Government, `core.chinacloudapi.cn`
    /// for Azure China, or `core.cloudapi.de` for Azure Germany.
    #[serde(default)]
    #[field(
        display_name = "Endpoint Suffix",
        description = "DNS suffix for the blob endpoint (defaults to core.windows.net)",
        placeholder = "core.windows.net"
    )]
    pub endpoint_suffix: Option<String>,

    /// Full base URL override for Azurite or Azure Stack deployments
    /// (e.g. `http://127.0.0.1:10000`). When set, takes precedence over `endpoint_suffix`.
    #[serde(default)]
    #[field(
        display_name = "Endpoint Override",
        description = "Full endpoint URL for Azurite or Azure Stack (e.g. http://127.0.0.1:10000)",
        placeholder = "http://127.0.0.1:10000"
    )]
    pub endpoint_override: Option<String>,
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

// ============================================================================
// MCP (Model Context Protocol) Connection Type
// ============================================================================

/// Parameters for connecting to an external MCP (Model Context Protocol) server.
///
/// The connection is used by the `runtara-agent-mcp` WASM agent (see
/// `crates/agents/runtara-agent-mcp`). Each AI Agent step that wants to call
/// an MCP server gets one labeled edge `mcp.<toolset>` to an Agent step
/// pointing at the `mcp` agent, parameterized with this connection.
///
/// Auth is configured via `auth_mode`:
///   - `none`   — no Authorization header is injected.
///   - `bearer` — `Authorization: Bearer <bearer_token>` is injected by the proxy.
///   - `api_key` — `<api_key_header>: <api_key_value>` is injected by the proxy.
///
/// `tool_hints` is a free-form map of `tool_name → extra description text` that
/// the search ranker treats as additional documentation per tool. Use it to
/// nudge the LLM toward (or away from) specific tools without renaming them.
///
/// `tool_scope`, when non-empty, restricts the agent to that allowlist of
/// tool names. The agent-side enforces this on both search and invoke.
///
/// OAuth2 is intentionally NOT supported in v1 — extend `auth_mode` later.
#[derive(Debug, Deserialize, ConnectionParams)]
#[connection(
    integration_id = "mcp",
    display_name = "MCP Server",
    description = "Connect to an external MCP (Model Context Protocol) server for dynamic tool discovery and invocation from AI Agent steps.",
    category = "api",
    auth_type = "api_key"
)]
pub struct McpConnectionParams {
    /// Streamable-HTTP endpoint URL of the MCP server.
    #[field(
        display_name = "Server URL",
        description = "Streamable-HTTP endpoint URL of the MCP server (e.g. https://mcp.example.com/jsonrpc).",
        placeholder = "https://mcp.example.com/jsonrpc"
    )]
    pub url: String,

    /// Authentication mode: `none`, `bearer`, or `api_key`.
    #[serde(default = "default_mcp_auth_mode")]
    #[field(
        display_name = "Auth Mode",
        description = "Authentication mode: none, bearer, or api_key.",
        default = "none",
        enum_values = "none,bearer,api_key"
    )]
    pub auth_mode: String,

    /// Bearer token (only used when auth_mode = "bearer").
    #[serde(default)]
    #[field(
        display_name = "Bearer Token",
        description = "Bearer token (when auth_mode = \"bearer\").",
        secret
    )]
    pub bearer_token: Option<String>,

    /// API key header name (only used when auth_mode = "api_key"). Defaults to X-API-Key.
    #[serde(default)]
    #[field(
        display_name = "API Key Header",
        description = "Header name for the API key (when auth_mode = \"api_key\"). Defaults to X-API-Key.",
        placeholder = "X-API-Key"
    )]
    pub api_key_header: Option<String>,

    /// API key value (only used when auth_mode = "api_key").
    #[serde(default)]
    #[field(
        display_name = "API Key",
        description = "API key value (when auth_mode = \"api_key\").",
        secret
    )]
    pub api_key_value: Option<String>,

    /// Extra static headers to forward on every request.
    #[serde(default)]
    #[field(
        display_name = "Extra Headers",
        description = "Extra static headers to forward on every request (header_name → value)."
    )]
    pub extra_headers: HashMap<String, String>,

    /// Optional per-tool hint strings to bias the tool_search ranker.
    #[serde(default)]
    #[field(
        display_name = "Tool Hints",
        description = "Optional per-tool hint strings (tool_name → extra description) used by the search ranker."
    )]
    pub tool_hints: HashMap<String, String>,

    /// Optional allowlist of tool names — empty allows everything.
    #[serde(default)]
    #[field(
        display_name = "Tool Scope",
        description = "Optional allowlist of tool names. Empty = allow all tools advertised by the MCP server."
    )]
    pub tool_scope: Vec<String>,
}

fn default_mcp_auth_mode() -> String {
    "none".to_string()
}

/// HTTP extractor for MCP connections. The agent embeds the auth choice in
/// the parameters JSON; the proxy reads `auth_mode` + matching fields at
/// request time and injects the right `Authorization` / api-key header.
pub struct McpExtractor;

impl HttpConnectionExtractor for McpExtractor {
    fn integration_id(&self) -> &'static str {
        "mcp"
    }

    fn extract(&self, params: &Value) -> Result<HttpConnectionConfig, String> {
        let p: McpConnectionParams = serde_json::from_value(params.clone())
            .map_err(|e| format!("Invalid mcp connection parameters: {}", e))?;

        let url = p.url.trim();
        if url.is_empty() {
            return Err("MCP connection: `url` is required".to_string());
        }
        // Allow http(s) for local dev / loopback; the proxy is server-side so
        // there's no risk of leaking the request out of the cluster.
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(format!(
                "MCP connection: `url` must start with http:// or https://, got `{}`",
                url
            ));
        }

        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        for (k, v) in &p.extra_headers {
            headers.insert(k.clone(), v.clone());
        }

        Ok(HttpConnectionConfig {
            headers,
            query_parameters: HashMap::new(),
            url_prefix: url.to_string(),
            rate_limit_config: None,
        })
    }
}

// ============================================================================
// Generic HTTP OAuth2 (Client Credentials) Connection Type
// ============================================================================

/// Generic OAuth2 client-credentials (machine-to-machine) connection.
///
/// Bring-your-own endpoints: the token is minted server-side from the
/// user-supplied `token_url` (cached + single-flighted) and injected as a
/// Bearer header; the proxy pins all credentialed egress to `base_url`. The
/// hardened egress client (no redirects, DNS-guarded) makes the mint call.
#[derive(Debug, Deserialize, ConnectionParams)]
#[connection(
    integration_id = "http_oauth2_client_credentials",
    display_name = "HTTP OAuth2 (Client Credentials)",
    description = "Authenticate HTTP requests with an OAuth2 client-credentials (M2M) token minted from your own token endpoint",
    category = "http",
    auth_type = "oauth2_client_credentials"
)]
pub struct HttpOAuth2ClientCredentialsParams {
    /// OAuth2 token endpoint the mint request is POSTed to
    #[field(
        display_name = "Token URL",
        description = "OAuth2 token endpoint (must be https)",
        placeholder = "https://auth.example.com/oauth/token",
        is_url,
        is_required
    )]
    pub token_url: String,

    /// OAuth2 client id
    #[field(display_name = "Client ID", description = "OAuth2 client id")]
    pub client_id: String,

    /// OAuth2 client secret
    #[field(
        display_name = "Client Secret",
        description = "OAuth2 client secret",
        secret
    )]
    pub client_secret: String,

    /// Space-separated OAuth2 scopes (optional)
    #[serde(default)]
    #[field(
        display_name = "Scope",
        description = "Space-separated OAuth2 scopes (optional)"
    )]
    pub scope: Option<String>,

    /// API base URL — the proxy pins every credentialed request to this host
    #[serde(default)]
    #[field(
        display_name = "Base URL",
        description = "API base URL — all requests using this connection are pinned to it (must be https)",
        placeholder = "https://api.example.com",
        is_url,
        is_required
    )]
    pub base_url: Option<String>,

    /// How client credentials reach the token endpoint
    #[serde(default = "default_generic_token_auth")]
    #[field(
        display_name = "Token Endpoint Auth",
        description = "How client credentials are sent to the token endpoint: 'form_body' (default) or 'basic' (HTTP Basic header)",
        default = "form_body"
    )]
    pub token_auth: String,

    /// Optional `audience` body parameter (Auth0-style M2M)
    #[serde(default)]
    #[field(
        display_name = "Audience",
        description = "Optional 'audience' parameter sent with the token request (Auth0-style)"
    )]
    pub audience: Option<String>,

    /// Optional `resource` body parameter
    #[serde(default)]
    #[field(
        display_name = "Resource",
        description = "Optional 'resource' parameter sent with the token request"
    )]
    pub resource: Option<String>,
}

fn default_generic_token_auth() -> String {
    "form_body".to_string()
}

/// HTTP extractor for generic OAuth2 client-credentials connections.
///
/// The Bearer token is minted + injected at request time by the connection
/// subsystem (`describe_connection_auth`); this extractor only pins the base
/// URL and Content-Type.
pub struct HttpOAuth2ClientCredentialsExtractor;

impl HttpConnectionExtractor for HttpOAuth2ClientCredentialsExtractor {
    fn integration_id(&self) -> &'static str {
        "http_oauth2_client_credentials"
    }

    fn extract(&self, params: &Value) -> Result<HttpConnectionConfig, String> {
        let p: HttpOAuth2ClientCredentialsParams =
            serde_json::from_value(params.clone()).map_err(|e| {
                format!(
                    "Invalid http_oauth2_client_credentials connection parameters: {}",
                    e
                )
            })?;

        let base_url = p.base_url.as_deref().map(str::trim).unwrap_or("");
        if base_url.is_empty() {
            return Err(
                "Invalid http_oauth2_client_credentials connection: missing base_url".to_string(),
            );
        }

        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        Ok(HttpConnectionConfig {
            headers,
            query_parameters: HashMap::new(),
            url_prefix: base_url.trim_end_matches('/').to_string(),
            rate_limit_config: None,
        })
    }
}

// ============================================================================
// Generic HTTP OAuth2 (Authorization Code) Connection Type
// ============================================================================

/// Generic OAuth2 authorization-code (interactive) connection.
///
/// Bring-your-own endpoints: the user supplies auth/token URLs and app
/// credentials; runtara runs the standard popup flow (PKCE on by default),
/// captures + refreshes the tokens, and pins credentialed egress to
/// `base_url`. `oauth_params_driven` is what lets this ONE type read its OAuth
/// config from connection parameters — curated providers never do.
#[derive(Debug, Deserialize, ConnectionParams)]
#[connection(
    integration_id = "http_oauth2_authorization_code",
    display_name = "HTTP OAuth2 (Authorization Code)",
    description = "Interactive OAuth2 authorization-code flow against your own endpoints; runtara captures and refreshes the token",
    category = "http",
    auth_type = "oauth2_authorization_code",
    oauth_auth_url = "",
    oauth_token_url = "",
    oauth_reauth_on_error_codes = "invalid_grant",
    oauth_params_driven = true
)]
pub struct HttpOAuth2AuthorizationCodeParams {
    /// OAuth2 authorization endpoint the user's browser is sent to
    #[field(
        display_name = "Authorization URL",
        description = "OAuth2 authorization endpoint (must be https)",
        placeholder = "https://auth.example.com/oauth/authorize",
        is_url,
        is_required
    )]
    pub auth_url: String,

    /// OAuth2 token endpoint (code exchange + refresh)
    #[field(
        display_name = "Token URL",
        description = "OAuth2 token endpoint for the code exchange and refreshes (must be https)",
        placeholder = "https://auth.example.com/oauth/token",
        is_url,
        is_required
    )]
    pub token_url: String,

    /// OAuth2 client id
    #[field(display_name = "Client ID", description = "OAuth2 client id")]
    pub client_id: String,

    /// OAuth2 client secret
    #[field(
        display_name = "Client Secret",
        description = "OAuth2 client secret",
        secret
    )]
    pub client_secret: String,

    /// Space-separated OAuth2 scopes to request
    #[serde(default)]
    #[field(
        display_name = "Scopes",
        description = "Space-separated OAuth2 scopes to request at authorization"
    )]
    pub scopes: Option<String>,

    /// API base URL — the proxy pins every credentialed request to this host
    #[serde(default)]
    #[field(
        display_name = "Base URL",
        description = "API base URL — all requests using this connection are pinned to it (must be https)",
        placeholder = "https://api.example.com",
        is_url,
        is_required
    )]
    pub base_url: Option<String>,

    /// How client credentials reach the token endpoint
    #[serde(default = "default_generic_token_auth")]
    #[field(
        display_name = "Token Endpoint Auth",
        description = "How client credentials are sent to the token endpoint: 'form_body' (default) or 'basic' (HTTP Basic header)",
        default = "form_body"
    )]
    pub token_auth: String,

    /// PKCE (RFC 7636) — on by default; disable only for providers that reject code_challenge
    #[serde(default)]
    #[field(
        display_name = "PKCE",
        description = "Use PKCE (S256 code challenge) on the authorization flow — recommended, on by default",
        default = "true"
    )]
    pub pkce: Option<bool>,

    /// Whether the provider rotates the refresh token on every refresh
    #[serde(default)]
    #[field(
        display_name = "Refresh Token Rotates",
        description = "Whether the provider rotates the refresh token on every refresh (default on — safest)",
        default = "true"
    )]
    pub refresh_rotates: Option<bool>,

    /// Optional token revocation endpoint, called on disconnect
    #[serde(default)]
    #[field(
        display_name = "Revocation URL",
        description = "Optional token revocation endpoint called when the connection is deleted (must be https)",
        is_url
    )]
    pub revocation_url: Option<String>,
}

/// HTTP extractor for generic OAuth2 authorization-code connections.
///
/// The Bearer token is resolved (and refreshed) at request time by the
/// connection subsystem; this extractor only pins the base URL and Content-Type.
pub struct HttpOAuth2AuthorizationCodeExtractor;

impl HttpConnectionExtractor for HttpOAuth2AuthorizationCodeExtractor {
    fn integration_id(&self) -> &'static str {
        "http_oauth2_authorization_code"
    }

    fn extract(&self, params: &Value) -> Result<HttpConnectionConfig, String> {
        let p: HttpOAuth2AuthorizationCodeParams =
            serde_json::from_value(params.clone()).map_err(|e| {
                format!(
                    "Invalid http_oauth2_authorization_code connection parameters: {}",
                    e
                )
            })?;

        let base_url = p.base_url.as_deref().map(str::trim).unwrap_or("");
        if base_url.is_empty() {
            return Err(
                "Invalid http_oauth2_authorization_code connection: missing base_url".to_string(),
            );
        }

        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        Ok(HttpConnectionConfig {
            headers,
            query_parameters: HashMap::new(),
            url_prefix: base_url.trim_end_matches('/').to_string(),
            rate_limit_config: None,
        })
    }
}

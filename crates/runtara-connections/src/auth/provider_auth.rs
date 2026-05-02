use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use reqwest::Client;
use runtara_dsl::agent_meta::find_connection_type;
use serde_json::Value;
use std::collections::HashMap;

use super::aws_signing::AwsSigningParams;
use super::token_cache::{
    self, DEFAULT_CLIENT_CREDENTIALS_TTL_SECONDS, DeferredAuth, TokenRequestBody,
};

pub struct ResolvedConnectionAuth {
    pub base_url: Option<String>,
    pub aws_signing: Option<AwsSigningParams>,
}

pub(crate) struct ConnectionAuthDescriptor {
    pub base_url: Option<String>,
    pub aws_signing: Option<AwsSigningParams>,
    pub deferred_auth: Option<DeferredAuth>,
}

const SHOPIFY_SCOPE_FIELDS: &[(&str, &str)] = &[
    ("scope_read_products", "read_products"),
    ("scope_write_products", "write_products"),
    ("scope_read_orders", "read_orders"),
    ("scope_write_orders", "write_orders"),
    ("scope_read_inventory", "read_inventory"),
    ("scope_write_inventory", "write_inventory"),
    ("scope_read_locations", "read_locations"),
    ("scope_read_customers", "read_customers"),
    ("scope_write_customers", "write_customers"),
    ("scope_read_fulfillments", "read_fulfillments"),
    ("scope_write_fulfillments", "write_fulfillments"),
];

pub async fn resolve_connection_auth(
    client: &Client,
    connection_id: &str,
    integration_id: &str,
    params: &Value,
    headers: &mut HashMap<String, String>,
) -> Result<ResolvedConnectionAuth, String> {
    let descriptor = describe_connection_auth(connection_id, integration_id, params, headers);

    if let Some(deferred_auth) = descriptor.deferred_auth {
        let resolved = token_cache::resolve_deferred_auth(client, deferred_auth).await?;
        headers.insert(resolved.0, resolved.1);
    }

    Ok(ResolvedConnectionAuth {
        base_url: descriptor.base_url,
        aws_signing: descriptor.aws_signing,
    })
}

fn describe_connection_auth(
    connection_id: &str,
    integration_id: &str,
    params: &Value,
    headers: &mut HashMap<String, String>,
) -> ConnectionAuthDescriptor {
    match integration_id {
        "openai_api_key" => {
            if let Some(key) = params["api_key"].as_str() {
                headers.insert("Authorization".into(), format!("Bearer {}", key));
            }
            ConnectionAuthDescriptor {
                base_url: Some("https://api.openai.com".into()),
                aws_signing: None,
                deferred_auth: None,
            }
        }
        "shopify_access_token" => {
            if let Some(token) = params["access_token"].as_str() {
                headers.insert("X-Shopify-Access-Token".into(), token.to_string());
            }
            ConnectionAuthDescriptor {
                base_url: params["shop_domain"]
                    .as_str()
                    .map(|domain| format!("https://{}", domain)),
                aws_signing: None,
                deferred_auth: None,
            }
        }
        "shopify_client_credentials" => ConnectionAuthDescriptor {
            base_url: params["shop_domain"]
                .as_str()
                .map(|domain| format!("https://{}", domain)),
            aws_signing: None,
            deferred_auth: describe_shopify_client_credentials_auth(connection_id, params),
        },
        "microsoft_entra_client_credentials" => ConnectionAuthDescriptor {
            base_url: first_string_param(params, &["base_url"]),
            aws_signing: None,
            deferred_auth: describe_microsoft_entra_client_credentials_auth(connection_id, params),
        },
        "hubspot_access_token" => {
            if let Some(token) = params["access_token"].as_str() {
                headers.insert("Authorization".into(), format!("Bearer {}", token));
            }
            ConnectionAuthDescriptor {
                base_url: Some("https://api.hubapi.com".into()),
                aws_signing: None,
                deferred_auth: None,
            }
        }
        "hubspot_private_app" => {
            if let Some(auth) =
                describe_oauth_refresh_auth(connection_id, integration_id, params, "Authorization")
            {
                ConnectionAuthDescriptor {
                    base_url: Some("https://api.hubapi.com".into()),
                    aws_signing: None,
                    deferred_auth: Some(auth),
                }
            } else {
                if let Some(token) = params["access_token"].as_str() {
                    headers.insert("Authorization".into(), format!("Bearer {}", token));
                }
                ConnectionAuthDescriptor {
                    base_url: Some("https://api.hubapi.com".into()),
                    aws_signing: None,
                    deferred_auth: None,
                }
            }
        }
        "stripe_api_key" => {
            if let Some(key) = params["secret_key"].as_str() {
                headers.insert("Authorization".into(), format!("Bearer {}", key));
            }
            ConnectionAuthDescriptor {
                base_url: Some("https://api.stripe.com".into()),
                aws_signing: None,
                deferred_auth: None,
            }
        }
        "slack_bot" => {
            if let Some(token) = params["bot_token"].as_str() {
                headers.insert("Authorization".into(), format!("Bearer {}", token));
            }
            ConnectionAuthDescriptor {
                base_url: Some("https://slack.com".into()),
                aws_signing: None,
                deferred_auth: None,
            }
        }
        "mailgun" => {
            if let Some(key) = params["api_key"].as_str() {
                let encoded = BASE64.encode(format!("api:{}", key));
                headers.insert("Authorization".into(), format!("Basic {}", encoded));
            }
            let region = params["region"].as_str().unwrap_or("us");
            let base = match region {
                "eu" => "https://api.eu.mailgun.net".to_string(),
                _ => "https://api.mailgun.net".to_string(),
            };
            ConnectionAuthDescriptor {
                base_url: Some(base),
                aws_signing: None,
                deferred_auth: None,
            }
        }
        // ── HTTP Bearer token ────────────────────────────────────
        "http_bearer" => {
            if let Some(token) = params["token"].as_str() {
                headers.insert("Authorization".into(), format!("Bearer {}", token));
            }
            ConnectionAuthDescriptor {
                base_url: params["base_url"].as_str().map(|u| u.to_string()),
                aws_signing: None,
                deferred_auth: None,
            }
        }
        // ── HTTP API Key ────────────────────────────────────────
        "http_api_key" => {
            if let Some(key) = params["api_key"].as_str() {
                let header_name = params["header_name"]
                    .as_str()
                    .unwrap_or("X-API-Key")
                    .to_string();
                headers.insert(header_name, key.to_string());
            }
            ConnectionAuthDescriptor {
                base_url: params["base_url"].as_str().map(|u| u.to_string()),
                aws_signing: None,
                deferred_auth: None,
            }
        }
        "aws_credentials" | "s3_compatible" => {
            let access_key_id = first_string_param(params, &["access_key_id", "aws_access_key_id"])
                .unwrap_or_default();
            let secret_access_key =
                first_string_param(params, &["secret_access_key", "aws_secret_access_key"])
                    .unwrap_or_default();
            let region = first_string_param(params, &["region", "aws_region"])
                .unwrap_or_else(|| "us-east-1".to_string());
            let session_token = first_string_param(params, &["session_token", "aws_session_token"]);

            let (base_url, service) = if integration_id == "s3_compatible" {
                let endpoint = params["endpoint"]
                    .as_str()
                    .map(normalize_endpoint)
                    .unwrap_or_else(|| format!("https://s3.{}.amazonaws.com", region));
                (Some(endpoint), "s3".to_string())
            } else {
                let svc = params["service"].as_str().unwrap_or("bedrock").to_string();
                let base = params["endpoint"]
                    .as_str()
                    .map(normalize_endpoint)
                    .or_else(|| {
                        (svc == "bedrock")
                            .then(|| format!("https://bedrock-runtime.{}.amazonaws.com", region))
                    });
                (base, svc)
            };

            ConnectionAuthDescriptor {
                base_url,
                aws_signing: Some(AwsSigningParams {
                    access_key_id,
                    secret_access_key,
                    region,
                    service,
                    session_token,
                }),
                deferred_auth: None,
            }
        }
        _ => {
            if let Some(key) = params["api_key"].as_str() {
                headers
                    .entry("Authorization".into())
                    .or_insert_with(|| format!("Bearer {}", key));
            } else if let Some(token) = params["access_token"].as_str() {
                headers
                    .entry("Authorization".into())
                    .or_insert_with(|| format!("Bearer {}", token));
            }
            ConnectionAuthDescriptor {
                base_url: params["base_url"].as_str().map(|u| u.to_string()),
                aws_signing: None,
                deferred_auth: None,
            }
        }
    }
}

fn describe_shopify_client_credentials_auth(
    connection_id: &str,
    params: &Value,
) -> Option<DeferredAuth> {
    let shop_domain = params["shop_domain"].as_str()?.trim_end_matches('/');
    let client_id = params["client_id"].as_str()?;
    let client_secret = params["client_secret"].as_str()?;

    let scopes = collect_shopify_scopes(params);
    let mut body = serde_json::Map::new();
    body.insert(
        "client_id".to_string(),
        Value::String(client_id.to_string()),
    );
    body.insert(
        "client_secret".to_string(),
        Value::String(client_secret.to_string()),
    );
    body.insert(
        "grant_type".to_string(),
        Value::String("client_credentials".to_string()),
    );
    if !scopes.is_empty() {
        body.insert("scope".to_string(), Value::String(scopes.clone()));
    }

    Some(DeferredAuth::OAuth2ClientCredentials {
        cache_key: token_cache::build_token_cache_key(&[
            "shopify_client_credentials",
            connection_id,
            shop_domain,
            &scopes,
        ]),
        token_url: format!("https://{shop_domain}/admin/oauth/access_token"),
        header_name: "X-Shopify-Access-Token".to_string(),
        header_value_prefix: None,
        request_body: TokenRequestBody::Json(Value::Object(body)),
        default_ttl_seconds: DEFAULT_CLIENT_CREDENTIALS_TTL_SECONDS,
    })
}

fn describe_microsoft_entra_client_credentials_auth(
    connection_id: &str,
    params: &Value,
) -> Option<DeferredAuth> {
    let _base_url = first_string_param(params, &["base_url"])?;
    let tenant_id = first_string_param(params, &["tenant_id"])?
        .trim_matches('/')
        .to_string();
    let client_id = first_string_param(params, &["client_id"])?;
    let client_secret = first_string_param(params, &["client_secret"])?;
    let scope = first_string_param(params, &["scope"])?;
    let authority_host = first_string_param(params, &["authority_host"])
        .unwrap_or_else(|| "https://login.microsoftonline.com".to_string())
        .trim_end_matches('/')
        .to_string();

    Some(DeferredAuth::OAuth2ClientCredentials {
        cache_key: token_cache::build_token_cache_key(&[
            "microsoft_entra_client_credentials",
            connection_id,
            &authority_host,
            &tenant_id,
            &client_id,
            &scope,
        ]),
        token_url: format!("{authority_host}/{tenant_id}/oauth2/v2.0/token"),
        header_name: "Authorization".to_string(),
        header_value_prefix: Some("Bearer ".to_string()),
        request_body: TokenRequestBody::FormUrlEncoded(vec![
            ("grant_type".to_string(), "client_credentials".to_string()),
            ("client_id".to_string(), client_id),
            ("client_secret".to_string(), client_secret),
            ("scope".to_string(), scope),
        ]),
        default_ttl_seconds: DEFAULT_CLIENT_CREDENTIALS_TTL_SECONDS,
    })
}

fn describe_oauth_refresh_auth(
    connection_id: &str,
    integration_id: &str,
    params: &Value,
    header_name: &str,
) -> Option<DeferredAuth> {
    let refresh_token = params["refresh_token"].as_str()?.to_string();
    let client_id = params["client_id"].as_str()?.to_string();
    let client_secret = params["client_secret"].as_str()?.to_string();
    let token_url = find_connection_type(integration_id)?
        .oauth_config
        .map(|config| config.token_url.to_string())?;

    Some(DeferredAuth::OAuth2RefreshToken {
        cache_key: token_cache::build_token_cache_key(&[
            "oauth_refresh",
            connection_id,
            integration_id,
        ]),
        token_url,
        header_name: header_name.to_string(),
        client_id,
        client_secret,
        refresh_token,
        fallback_access_token: params["access_token"].as_str().map(|s| s.to_string()),
        fallback_expires_at: token_cache::parse_expiry(params["token_expires_at"].as_str()),
    })
}

fn collect_shopify_scopes(params: &Value) -> String {
    SHOPIFY_SCOPE_FIELDS
        .iter()
        .filter(|(field, _)| params[*field].as_bool().unwrap_or(false))
        .map(|(_, scope)| *scope)
        .collect::<Vec<_>>()
        .join(",")
}

fn first_string_param(params: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        params[*key]
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn normalize_endpoint(endpoint: &str) -> String {
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        endpoint.to_string()
    } else {
        format!("https://{}", endpoint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn collect_shopify_scopes_keeps_enabled_scopes_only() {
        let params = json!({
            "scope_read_products": true,
            "scope_write_products": false,
            "scope_read_inventory": true
        });

        assert_eq!(
            collect_shopify_scopes(&params),
            "read_products,read_inventory"
        );
    }

    #[test]
    fn microsoft_entra_client_credentials_builds_form_token_request() {
        let params = json!({
            "tenant_id": "contoso.onmicrosoft.com",
            "client_id": "client-id",
            "client_secret": "client-secret",
            "scope": "https://api.businesscentral.dynamics.com/.default",
            "base_url": "https://api.businesscentral.dynamics.com/v2.0/production/api/v2.0"
        });
        let mut headers = HashMap::new();

        let descriptor = describe_connection_auth(
            "conn-bc",
            "microsoft_entra_client_credentials",
            &params,
            &mut headers,
        );

        assert_eq!(
            descriptor.base_url.as_deref(),
            Some("https://api.businesscentral.dynamics.com/v2.0/production/api/v2.0")
        );
        assert!(headers.is_empty());

        let auth = descriptor.deferred_auth.expect("deferred auth");
        match auth {
            DeferredAuth::OAuth2ClientCredentials {
                token_url,
                header_name,
                header_value_prefix,
                request_body,
                ..
            } => {
                assert_eq!(
                    token_url,
                    "https://login.microsoftonline.com/contoso.onmicrosoft.com/oauth2/v2.0/token"
                );
                assert_eq!(header_name, "Authorization");
                assert_eq!(header_value_prefix.as_deref(), Some("Bearer "));

                match request_body {
                    TokenRequestBody::FormUrlEncoded(fields) => {
                        assert!(fields.contains(&(
                            "grant_type".to_string(),
                            "client_credentials".to_string()
                        )));
                        assert!(
                            fields.contains(&("client_id".to_string(), "client-id".to_string()))
                        );
                        assert!(
                            fields.contains(&(
                                "client_secret".to_string(),
                                "client-secret".to_string()
                            ))
                        );
                        assert!(fields.contains(&(
                            "scope".to_string(),
                            "https://api.businesscentral.dynamics.com/.default".to_string()
                        )));
                    }
                    TokenRequestBody::Json(_) => panic!("expected form-encoded token body"),
                }
            }
            DeferredAuth::OAuth2RefreshToken { .. } => {
                panic!("expected client credentials auth")
            }
        }
    }

    #[test]
    fn aws_credentials_defaults_to_bedrock_runtime_and_accepts_aws_field_names() {
        let params = json!({
            "aws_access_key_id": "AKIA_TEST",
            "aws_secret_access_key": "secret",
            "aws_region": "eu-central-1",
            "aws_session_token": "token"
        });
        let mut headers = HashMap::new();

        let descriptor = describe_connection_auth("conn", "aws_credentials", &params, &mut headers);

        assert_eq!(
            descriptor.base_url.as_deref(),
            Some("https://bedrock-runtime.eu-central-1.amazonaws.com")
        );
        let aws = descriptor.aws_signing.expect("aws signing params");
        assert_eq!(aws.access_key_id, "AKIA_TEST");
        assert_eq!(aws.secret_access_key, "secret");
        assert_eq!(aws.region, "eu-central-1");
        assert_eq!(aws.service, "bedrock");
        assert_eq!(aws.session_token.as_deref(), Some("token"));
    }

    #[test]
    fn aws_credentials_ignores_blank_optional_session_token() {
        let params = json!({
            "aws_access_key_id": "AKIA_TEST",
            "aws_secret_access_key": "secret",
            "aws_region": "ap-southeast-2",
            "aws_session_token": ""
        });
        let mut headers = HashMap::new();

        let descriptor = describe_connection_auth("conn", "aws_credentials", &params, &mut headers);
        let aws = descriptor.aws_signing.expect("aws signing params");

        assert_eq!(aws.access_key_id, "AKIA_TEST");
        assert_eq!(aws.region, "ap-southeast-2");
        assert!(aws.session_token.is_none());
    }
}

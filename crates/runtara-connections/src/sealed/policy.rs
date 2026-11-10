//! The seal policy of each secret of a connection: where it may go and how it
//! is attached.
//!
//! A policy is sealed with its secret, so it is made from the connection's
//! parameters when the secret is entered and cannot be changed afterwards
//! without the secret. Only connection types listed here are sealed; every
//! other type stays with the credential cipher.

use std::collections::BTreeMap;

use runtara_agents::extractors::connection_types::{GEMINI_API_BASE_URL, SHIPPO_API_BASE_URL};
use runtara_agents::registry::find_connection_type;
use seal_format::Policy;
use seal_format::policy::{Destination, HmacHash, Host, Inject, Part, Rule, Scheme, Verify};
use serde_json::Value;

use crate::auth::provider_auth::{jira_account_url, openai_proxy_base_url, resolve_named_endpoint};

/// Destinations a deployment lets credentials go to that seal refuses by
/// default: the development and test escape hatches of the outbound proxy.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Allowances {
    /// Hosts that may be private addresses (`RUNTARA_PROXY_ALLOWED_HOSTS`),
    /// each `host` or `host:port`, lower case.
    pub private_hosts: Vec<String>,
    /// Hosts that may be reached over plain HTTP
    /// (`RUNTARA_PROXY_ALLOW_HTTP_HOSTS`), lower case.
    pub plain_http_hosts: Vec<String>,
    /// The API to use for a connection type whose API is otherwise fixed, by
    /// type: a stand-in for the provider's. Empty outside tests.
    pub api_bases: BTreeMap<String, String>,
}

impl Allowances {
    fn allows_private(&self, host: &str, port: Option<u16>) -> bool {
        self.private_hosts.iter().any(|entry| {
            entry == host || port.is_some_and(|port| *entry == format!("{host}:{port}"))
        })
    }

    fn allows_plain_http(&self, host: &str) -> bool {
        self.plain_http_hosts.iter().any(|entry| entry == host)
    }
}

/// Telegram's Bot API.
pub const TELEGRAM_API_BASE: &str = "https://api.telegram.org";

/// What a sealed secret is for.
// A handful are built per connection and dropped at once: their size does not
// matter, and boxing the injection would only obscure the table below.
#[allow(clippy::large_enum_variant)]
enum Usage {
    /// Sent with the connection's requests.
    Send {
        /// The API the secret is for.
        base_url: String,
        inject: Inject,
        /// The secret may go anywhere on the API's host, not only below the
        /// base URL's path: for a type whose requests the proxy does not pin
        /// to a path either.
        any_path: bool,
        /// Whether requests present it. A secret the connection holds but its
        /// current settings do not use is sealed and left out.
        presented: bool,
    },
    /// Never sent: checks that a message came from the provider.
    Verify(HmacHash),
}

/// One secret parameter of a connection.
struct Secret {
    /// The parameter holding the secret.
    field: &'static str,
    usage: Usage,
}

fn sent(field: &'static str, base_url: &str, inject: Inject) -> Secret {
    Secret {
        field,
        usage: Usage::Send {
            base_url: base_url.to_owned(),
            inject,
            any_path: false,
            presented: true,
        },
    }
}

fn verifies(field: &'static str) -> Secret {
    Secret {
        field,
        usage: Usage::Verify(HmacHash::Sha256),
    }
}

fn header(name: &str, format: &str) -> Inject {
    Inject::Header {
        name: name.to_owned(),
        format: format.to_owned(),
    }
}

fn bearer() -> Inject {
    header("Authorization", "Bearer {}")
}

fn basic(username: &str) -> Inject {
    Inject::Basic {
        username: Part::Literal(username.to_owned()),
    }
}

fn filled<'a>(params: &'a Value, name: &str) -> Option<&'a str> {
    params[name]
        .as_str()
        .filter(|value| !value.trim().is_empty())
}

/// The header an injection sets.
fn header_of(inject: &Inject) -> Option<&str> {
    match inject {
        Inject::Header { name, .. } => Some(name),
        Inject::Basic { .. } => Some("Authorization"),
        _ => None,
    }
}

/// An MCP server's secrets. The one its `auth_mode` names is sent; the other,
/// if the connection still holds it, is sealed and left out. The server's URL
/// is one endpoint, and the proxy does not hold requests to its path, so the
/// secret may go anywhere on its host.
fn mcp_secrets(params: &Value) -> Option<Vec<Secret>> {
    let url = filled(params, "url")?;
    let mode = params["auth_mode"].as_str().unwrap_or("none");
    let secrets = [
        ("bearer_token", bearer(), mode == "bearer"),
        (
            "api_key_value",
            header(
                filled(params, "api_key_header").unwrap_or("X-API-Key"),
                "{}",
            ),
            mode == "api_key",
        ),
    ];
    // A static header of the connection's own replaces the credential's
    // today. seal would set the credential instead, so such a connection is
    // not sealed.
    let overridden = |inject: &Inject| {
        params["extra_headers"].as_object().is_some_and(|extras| {
            extras
                .keys()
                .any(|name| header_of(inject).is_some_and(|set| name.eq_ignore_ascii_case(set)))
        })
    };
    if secrets
        .iter()
        .any(|(_, inject, presented)| *presented && overridden(inject))
    {
        return None;
    }
    Some(
        secrets
            .into_iter()
            .map(|(field, inject, presented)| Secret {
                field,
                usage: Usage::Send {
                    base_url: url.to_owned(),
                    inject,
                    any_path: true,
                    presented,
                },
            })
            .collect(),
    )
}

/// The connection types whose secrets are sealed, and what each secret is
/// for. This mirrors what `describe_connection_auth` does for the same types.
fn secrets(integration_id: &str, params: &Value) -> Option<Vec<Secret>> {
    let one = |field, base_url: &str, inject| Some(vec![sent(field, base_url, inject)]);
    match integration_id {
        "openai_api_key" => one("api_key", &openai_proxy_base_url(params), bearer()),
        "gemini_api_key" => one(
            "api_key",
            GEMINI_API_BASE_URL,
            header("x-goog-api-key", "{}"),
        ),
        "shopify_access_token" => one(
            "access_token",
            &format!("https://{}", filled(params, "shop_domain")?),
            header("X-Shopify-Access-Token", "{}"),
        ),
        "hubspot_access_token" => one("access_token", "https://api.hubapi.com", bearer()),
        "stripe_api_key" => one("secret_key", "https://api.stripe.com", bearer()),
        "pipedrive_api_token" => one(
            "api_token",
            "https://api.pipedrive.com",
            header("x-api-token", "{}"),
        ),
        "shippo_api_token" => one(
            "api_token",
            SHIPPO_API_BASE_URL,
            header("Authorization", "ShippoToken {}"),
        ),
        "jira_api_token" => one(
            "api_token",
            &jira_account_url(params)?,
            basic(filled(params, "email")?),
        ),
        // Telegram takes the token in the URL's path: `/bot<token>/<method>`.
        // A request names the method only, and seal puts the token before it.
        "telegram_bot" => one(
            "bot_token",
            TELEGRAM_API_BASE,
            Inject::Path {
                prefix: "/bot{}".to_owned(),
            },
        ),
        "slack_bot" => Some(vec![
            sent("bot_token", "https://slack.com", bearer()),
            verifies("signing_secret"),
        ]),
        "mailgun" => {
            let base_url = match params["region"].as_str().unwrap_or("us") {
                "eu" => "https://api.eu.mailgun.net",
                _ => "https://api.mailgun.net",
            };
            Some(vec![
                sent("api_key", base_url, basic("api")),
                verifies("webhook_signing_key"),
            ])
        }
        "mcp" => mcp_secrets(params),
        "http_bearer" => one("token", filled(params, "base_url")?, bearer()),
        "http_basic" => {
            let username = params["username"].as_str()?;
            // As `http_basic_auth` refuses it: the header could not be read back.
            if username.contains(':') {
                return None;
            }
            one("password", filled(params, "base_url")?, basic(username))
        }
        "http_api_key" => one(
            "api_key",
            filled(params, "base_url")?,
            header(filled(params, "header_name").unwrap_or("X-API-Key"), "{}"),
        ),
        _ => None,
    }
}

/// The parameters a connection's requests present to seal: its sealed secrets
/// that are sent, and that its current settings use. A secret that only
/// verifies is never among them, since it may go nowhere.
pub fn request_fields(integration_id: &str, params: &Value) -> Vec<String> {
    secrets(integration_id, params)
        .into_iter()
        .flatten()
        .filter(|secret| {
            matches!(
                secret.usage,
                Usage::Send {
                    presented: true,
                    ..
                }
            )
        })
        .map(|secret| secret.field.to_owned())
        .collect()
}

/// The request headers seal sets for a connection's secrets. A caller may not
/// set them too, so the proxy drops any the agent sent before handing the
/// request to seal, as it always overrode them.
pub fn injected_headers(integration_id: &str, params: &Value) -> Vec<String> {
    secrets(integration_id, params)
        .into_iter()
        .flatten()
        .filter_map(|secret| match &secret.usage {
            Usage::Send {
                inject,
                presented: true,
                ..
            } => header_of(inject).map(str::to_owned),
            _ => None,
        })
        .collect()
}

/// Whether a connection type's secrets can be sealed at all, whatever its
/// parameters.
pub fn is_sealable_type(integration_id: &str) -> bool {
    SEALABLE_TYPES.contains(&integration_id)
}

/// The connection types [`policies`] knows.
pub const SEALABLE_TYPES: &[&str] = &[
    "openai_api_key",
    "gemini_api_key",
    "shopify_access_token",
    "hubspot_access_token",
    "stripe_api_key",
    "pipedrive_api_token",
    "shippo_api_token",
    "jira_api_token",
    "telegram_bot",
    "slack_bot",
    "mailgun",
    "mcp",
    "http_bearer",
    "http_basic",
    "http_api_key",
];

/// A rule that sends a secret to `base_url` and below (or anywhere on its
/// host, with `any_path`), or `None` if seal cannot express the destination or
/// this deployment does not allow it.
fn rule_for(
    base_url: &str,
    inject: &Inject,
    any_path: bool,
    allowances: &Allowances,
) -> Option<Rule> {
    let url = url::Url::parse(base_url).ok()?;
    if !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    // A query or fragment is part of no destination. With `any_path` the URL
    // names a host only, so they are beside the point.
    if !any_path && (url.query().is_some() || url.fragment().is_some()) {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    let scheme = match url.scheme() {
        "https" => Scheme::Https,
        "http" if allowances.allows_plain_http(&host) => Scheme::Http,
        _ => return None,
    };
    let path = url.path().trim_end_matches('/');
    let mut rule = Rule::new(
        Destination::Http {
            scheme,
            host: Host::new(host.as_str()).ok()?,
            port: url.port(),
            path_prefix: (!any_path && !path.is_empty()).then(|| path.to_owned()),
        },
        inject.clone(),
    );
    rule.allow_plain_http = scheme == Scheme::Http;
    rule.allow_private_destination = allowances.allows_private(&host, url.port_or_known_default());
    Some(rule)
}

/// The policy of each secret parameter of a connection, by parameter name.
///
/// `None` means the connection is not sealed: its type is not one seal handles
/// yet, its parameters name no destination, or the destination is one seal
/// cannot express. Such a connection stays with the credential cipher, and
/// behaves as it always has.
pub fn policies(
    integration_id: &str,
    params: &Value,
    allowances: &Allowances,
) -> Option<BTreeMap<String, Policy>> {
    let mut policies = BTreeMap::new();
    for secret in secrets(integration_id, params)? {
        let policy = match &secret.usage {
            Usage::Verify(hash) => Policy {
                rules: Vec::new(),
                definitions: BTreeMap::new(),
                shared: false,
                verify: Some(Verify::Hmac { hash: *hash }),
            },
            Usage::Send {
                base_url,
                inject,
                any_path,
                ..
            } => {
                let base_url = allowances.api_bases.get(integration_id).unwrap_or(base_url);
                let mut rules = vec![rule_for(base_url, inject, *any_path, allowances)?];
                // The type's other endpoints, which an agent selects by name.
                // One whose URL does not render from these parameters cannot
                // be selected either.
                if let Some(meta) = find_connection_type(integration_id) {
                    for endpoint in meta.named_endpoints {
                        let Ok(resolved) = resolve_named_endpoint(meta, endpoint.name, params)
                        else {
                            continue;
                        };
                        let rule = rule_for(&resolved.base_url, inject, *any_path, allowances)?;
                        if !rules.contains(&rule) {
                            rules.push(rule);
                        }
                    }
                }
                Policy {
                    rules,
                    definitions: BTreeMap::new(),
                    shared: false,
                    verify: None,
                }
            }
        };
        policy.validate().ok()?;
        policies.insert(secret.field.to_owned(), policy);
    }
    Some(policies)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn only(integration_id: &str, params: Value) -> (String, Value) {
        let policies = policies(integration_id, &params, &Allowances::default())
            .unwrap_or_else(|| panic!("{integration_id} is sealed"));
        assert_eq!(policies.len(), 1, "{integration_id}");
        let (field, policy) = policies.into_iter().next().unwrap();
        let json = serde_json::from_slice(&policy.canonical_json().unwrap()).unwrap();
        (field, json)
    }

    #[test]
    fn each_static_type_sends_its_secret_where_the_proxy_did() {
        let cases = [
            (
                "shopify_access_token",
                json!({"access_token": "s", "shop_domain": "acme.myshopify.com"}),
                "access_token",
                json!({"scheme": "https", "host": "acme.myshopify.com"}),
                json!({"header": {"name": "X-Shopify-Access-Token", "format": "{}"}}),
            ),
            (
                "hubspot_access_token",
                json!({"access_token": "s"}),
                "access_token",
                json!({"scheme": "https", "host": "api.hubapi.com"}),
                json!({"header": {"name": "Authorization", "format": "Bearer {}"}}),
            ),
            (
                "stripe_api_key",
                json!({"secret_key": "s"}),
                "secret_key",
                json!({"scheme": "https", "host": "api.stripe.com"}),
                json!({"header": {"name": "Authorization", "format": "Bearer {}"}}),
            ),
            (
                "pipedrive_api_token",
                json!({"api_token": "s"}),
                "api_token",
                json!({"scheme": "https", "host": "api.pipedrive.com"}),
                json!({"header": {"name": "x-api-token", "format": "{}"}}),
            ),
            (
                "shippo_api_token",
                json!({"api_token": "s"}),
                "api_token",
                json!({"scheme": "https", "host": "api.goshippo.com"}),
                json!({"header": {"name": "Authorization", "format": "ShippoToken {}"}}),
            ),
            (
                "http_bearer",
                json!({"token": "s", "base_url": "https://api.example.com/v2/"}),
                "token",
                json!({"scheme": "https", "host": "api.example.com", "path_prefix": "/v2"}),
                json!({"header": {"name": "Authorization", "format": "Bearer {}"}}),
            ),
            (
                "http_basic",
                json!({"password": "s", "username": "ada", "base_url": "https://api.example.com:8443"}),
                "password",
                json!({"scheme": "https", "host": "api.example.com", "port": 8443}),
                json!({"basic": {"username": {"literal": "ada"}}}),
            ),
            (
                "http_api_key",
                json!({"api_key": "s", "base_url": "https://api.example.com"}),
                "api_key",
                json!({"scheme": "https", "host": "api.example.com"}),
                json!({"header": {"name": "X-API-Key", "format": "{}"}}),
            ),
            (
                "http_api_key",
                json!({"api_key": "s", "header_name": "X-Token", "base_url": "https://api.example.com"}),
                "api_key",
                json!({"scheme": "https", "host": "api.example.com"}),
                json!({"header": {"name": "X-Token", "format": "{}"}}),
            ),
        ];
        for (integration_id, params, field, destination, inject) in cases {
            let (sealed_field, policy) = only(integration_id, params);
            assert_eq!(sealed_field, field, "{integration_id}");
            assert_eq!(
                policy,
                json!({"rules": [{"destination": destination, "inject": inject}]}),
                "{integration_id}"
            );
        }
    }

    #[test]
    fn jira_sends_the_token_as_the_password_of_the_accounts_email() {
        let (field, policy) = only(
            "jira_api_token",
            json!({"api_token": "s", "email": "ada@example.com", "site": "acme"}),
        );
        assert_eq!(field, "api_token");
        assert_eq!(
            policy["rules"][0]["inject"],
            json!({"basic": {"username": {"literal": "ada@example.com"}}})
        );
        assert_eq!(policy["rules"][0]["destination"]["scheme"], "https");
    }

    #[test]
    fn every_listed_type_is_one_the_registry_knows() {
        for integration_id in SEALABLE_TYPES {
            assert!(
                find_connection_type(integration_id).is_some(),
                "{integration_id} is not a connection type"
            );
            assert!(is_sealable_type(integration_id));
        }
        assert!(!is_sealable_type("quickbooks_online"));
    }

    /// The secret's parameter is one the type marks secret, so that nothing
    /// readable is sealed and no secret is left out.
    #[test]
    fn the_sealed_parameter_is_the_types_only_secret() {
        let params = json!({
            "api_key": "s", "access_token": "s", "secret_key": "s", "api_token": "s",
            "token": "s", "password": "s", "username": "u", "email": "e@example.com",
            "site": "acme", "shop_domain": "acme.myshopify.com",
            "base_url": "https://api.example.com", "url": "https://mcp.example.com/mcp",
        });
        for integration_id in SEALABLE_TYPES {
            let meta = find_connection_type(integration_id).unwrap();
            let mut secrets: Vec<&str> = meta
                .fields
                .iter()
                .filter(|field| field.is_secret)
                .map(|field| field.name)
                .collect();
            secrets.sort_unstable();
            let sealed: Vec<String> = policies(integration_id, &params, &Allowances::default())
                .unwrap_or_else(|| panic!("{integration_id} is sealed"))
                .into_keys()
                .collect();
            assert_eq!(sealed, secrets, "{integration_id}");
        }
    }

    fn canonical(policy: &Policy) -> Value {
        serde_json::from_slice(&policy.canonical_json().unwrap()).unwrap()
    }

    #[test]
    fn the_ai_providers_send_their_key_to_the_providers_api() {
        let (field, policy) = only("openai_api_key", json!({"api_key": "s"}));
        assert_eq!(field, "api_key");
        assert_eq!(
            policy,
            json!({"rules": [{
                "destination": {"scheme": "https", "host": "api.openai.com"},
                "inject": {"header": {"name": "Authorization", "format": "Bearer {}"}},
            }]})
        );
        // A gateway's base URL names the host; `/v1` is the request's.
        let (_, policy) = only(
            "openai_api_key",
            json!({"api_key": "s", "base_url": "https://llm.example.com/v1/"}),
        );
        assert_eq!(
            policy["rules"][0]["destination"],
            json!({"scheme": "https", "host": "llm.example.com"})
        );

        let (field, policy) = only("gemini_api_key", json!({"api_key": "s"}));
        assert_eq!(field, "api_key");
        assert_eq!(
            policy["rules"][0],
            json!({
                "destination": {
                    "scheme": "https",
                    "host": "generativelanguage.googleapis.com",
                    "path_prefix": "/v1beta",
                },
                "inject": {"header": {"name": "x-goog-api-key", "format": "{}"}},
            })
        );
    }

    #[test]
    fn a_channels_token_is_sent_and_its_signing_secret_only_verifies() {
        let slack = policies("slack_bot", &json!({}), &Allowances::default()).unwrap();
        let bearer = json!({"header": {"name": "Authorization", "format": "Bearer {}"}});
        // The API, and the host files shared in a channel are fetched from.
        assert_eq!(
            canonical(&slack["bot_token"]),
            json!({"rules": [
                {
                    "destination": {"scheme": "https", "host": "slack.com"},
                    "inject": bearer,
                },
                {
                    "destination": {
                        "scheme": "https",
                        "host": "files.slack.com",
                        "path_prefix": "/files-pri",
                    },
                    "inject": bearer,
                },
            ]})
        );
        assert_eq!(
            canonical(&slack["signing_secret"]),
            json!({"verify": {"hmac": {"hash": "sha256"}}})
        );
        assert_eq!(request_fields("slack_bot", &json!({})), ["bot_token"]);
        assert_eq!(injected_headers("slack_bot", &json!({})), ["Authorization"]);

        let params = json!({"domain": "mg.example.com", "region": "eu"});
        let mailgun = policies("mailgun", &params, &Allowances::default()).unwrap();
        let api_key = canonical(&mailgun["api_key"]);
        let rules = api_key["rules"].as_array().unwrap();
        assert_eq!(
            rules[0],
            json!({
                "destination": {"scheme": "https", "host": "api.eu.mailgun.net"},
                "inject": {"basic": {"username": {"literal": "api"}}},
            })
        );
        // Stored messages are fetched from the regional storage hosts, each
        // for this connection's domain only.
        assert!(rules.len() > 1 && rules.len() <= 16, "{}", rules.len());
        for rule in &rules[1..] {
            assert_eq!(
                rule["destination"]["path_prefix"],
                "/v3/domains/mg.example.com/messages"
            );
            assert_eq!(rule["inject"], rules[0]["inject"]);
        }
        assert_eq!(
            canonical(&mailgun["webhook_signing_key"]),
            json!({"verify": {"hmac": {"hash": "sha256"}}})
        );
        assert_eq!(request_fields("mailgun", &params), ["api_key"]);
        // The US region is the default.
        let us = policies(
            "mailgun",
            &json!({"domain": "d.example.com"}),
            &Allowances::default(),
        );
        assert_eq!(
            canonical(&us.unwrap()["api_key"])["rules"][0]["destination"]["host"],
            "api.mailgun.net"
        );
    }

    #[test]
    fn a_telegram_token_goes_in_the_path_of_the_bot_api() {
        let (field, policy) = only("telegram_bot", json!({"bot_token": "1:a"}));
        assert_eq!(field, "bot_token");
        assert_eq!(
            policy,
            json!({"rules": [{
                "destination": {"scheme": "https", "host": "api.telegram.org"},
                "inject": {"path": {"prefix": "/bot{}"}},
            }]})
        );
        assert_eq!(request_fields("telegram_bot", &json!({})), ["bot_token"]);
        // No header is seal's to set.
        assert!(injected_headers("telegram_bot", &json!({})).is_empty());
    }

    #[test]
    fn a_types_api_can_be_stood_in_for() {
        let allowances = Allowances {
            private_hosts: vec!["127.0.0.1:4000".to_owned()],
            plain_http_hosts: vec!["127.0.0.1".to_owned()],
            api_bases: BTreeMap::from([(
                "telegram_bot".to_owned(),
                "http://127.0.0.1:4000".to_owned(),
            )]),
        };
        let policy = policies("telegram_bot", &json!({}), &allowances).unwrap();
        assert_eq!(
            canonical(&policy["bot_token"])["rules"][0]["destination"],
            json!({"scheme": "http", "host": "127.0.0.1", "port": 4000})
        );
        // Only that type's.
        let slack = policies("slack_bot", &json!({}), &allowances).unwrap();
        assert_eq!(
            canonical(&slack["bot_token"])["rules"][0]["destination"]["host"],
            "slack.com"
        );
    }

    #[test]
    fn an_mcp_server_gets_the_secret_its_auth_mode_names() {
        let url = "https://mcp.example.com/servers/a/mcp?session=1";
        let host = json!({"scheme": "https", "host": "mcp.example.com"});

        let params = json!({"url": url, "auth_mode": "bearer"});
        let bearer = policies("mcp", &params, &Allowances::default()).unwrap();
        // Anywhere on the server's host: the proxy does not hold MCP requests
        // to the URL's path either.
        assert_eq!(
            canonical(&bearer["bearer_token"]),
            json!({"rules": [{
                "destination": host,
                "inject": {"header": {"name": "Authorization", "format": "Bearer {}"}},
            }]})
        );
        assert_eq!(request_fields("mcp", &params), ["bearer_token"]);
        assert_eq!(injected_headers("mcp", &params), ["Authorization"]);

        let params = json!({"url": url, "auth_mode": "api_key", "api_key_header": "X-Mcp-Key"});
        let api_key = policies("mcp", &params, &Allowances::default()).unwrap();
        assert_eq!(
            canonical(&api_key["api_key_value"])["rules"][0]["inject"],
            json!({"header": {"name": "X-Mcp-Key", "format": "{}"}})
        );
        assert_eq!(request_fields("mcp", &params), ["api_key_value"]);
        assert_eq!(injected_headers("mcp", &params), ["X-Mcp-Key"]);
        // The secret of the other mode has a policy too, so that it is never
        // stored unsealed; it is just not presented.
        assert!(api_key.contains_key("bearer_token"));

        // No auth: nothing is presented.
        let params = json!({"url": url});
        assert!(policies("mcp", &params, &Allowances::default()).is_some());
        assert!(request_fields("mcp", &params).is_empty());
        assert!(injected_headers("mcp", &params).is_empty());
    }

    #[test]
    fn an_mcp_connection_whose_own_header_replaces_the_credential_is_not_sealed() {
        let params = json!({
            "url": "https://mcp.example.com/mcp",
            "auth_mode": "bearer",
            "extra_headers": {"authorization": "Custom scheme"},
        });
        assert_eq!(policies("mcp", &params, &Allowances::default()), None);
        // A header that is not the credential's does not matter.
        let params = json!({
            "url": "https://mcp.example.com/mcp",
            "auth_mode": "bearer",
            "extra_headers": {"X-Team": "a"},
        });
        assert!(policies("mcp", &params, &Allowances::default()).is_some());
        // Nor does one for a mode that is not in use.
        let params = json!({
            "url": "https://mcp.example.com/mcp",
            "auth_mode": "api_key",
            "extra_headers": {"Authorization": "Custom scheme"},
        });
        assert!(policies("mcp", &params, &Allowances::default()).is_some());
    }

    #[test]
    fn a_type_seal_does_not_handle_yet_is_not_sealed() {
        for integration_id in ["quickbooks_online", "aws_credentials", "teams_bot", "nope"] {
            let params = json!({"api_key": "s", "base_url": "https://api.example.com"});
            assert_eq!(
                policies(integration_id, &params, &Allowances::default()),
                None,
                "{integration_id}"
            );
        }
    }

    #[test]
    fn a_destination_seal_cannot_express_is_not_sealed() {
        for base_url in [
            "",
            "not a url",
            "ftp://api.example.com",
            "http://api.example.com",
            "https://user:pw@api.example.com",
            "https://api.example.com/?tenant=1",
            "https://api.example.com/a/../b#frag",
        ] {
            let params = json!({"token": "s", "base_url": base_url});
            assert_eq!(
                policies("http_bearer", &params, &Allowances::default()),
                None,
                "{base_url}"
            );
        }
        // Parameters that name no destination.
        assert_eq!(
            policies(
                "shopify_access_token",
                &json!({"access_token": "s"}),
                &Allowances::default()
            ),
            None
        );
        assert_eq!(
            policies(
                "http_basic",
                &json!({"password": "s", "username": "a:b", "base_url": "https://api.example.com"}),
                &Allowances::default()
            ),
            None
        );
    }

    #[test]
    fn plain_http_and_private_hosts_need_the_deployments_allowance() {
        let params = json!({"token": "s", "base_url": "http://127.0.0.1:8080/api"});
        assert_eq!(
            policies("http_bearer", &params, &Allowances::default()),
            None
        );

        let allowances = Allowances {
            private_hosts: vec!["127.0.0.1:8080".to_owned()],
            plain_http_hosts: vec!["127.0.0.1".to_owned()],
            ..Allowances::default()
        };
        let policy = policies("http_bearer", &params, &allowances).unwrap()["token"].clone();
        let rule = &policy.rules[0];
        assert!(rule.allow_plain_http && rule.allow_private_destination);

        // Allowed for one port only.
        let other = json!({"token": "s", "base_url": "http://127.0.0.1:9090/api"});
        let policy = policies("http_bearer", &other, &allowances).unwrap()["token"].clone();
        assert!(!policy.rules[0].allow_private_destination);

        // A public HTTPS host gets neither.
        let public = json!({"token": "s", "base_url": "https://api.example.com"});
        let policy = policies("http_bearer", &public, &allowances).unwrap()["token"].clone();
        assert!(!policy.rules[0].allow_plain_http && !policy.rules[0].allow_private_destination);
    }

    #[test]
    fn the_header_seal_sets_is_named_for_the_proxy_to_drop() {
        let cases = [
            (
                "http_bearer",
                json!({"base_url": "https://a.example.com"}),
                "Authorization",
            ),
            (
                "http_basic",
                json!({"base_url": "https://a.example.com", "username": "u"}),
                "Authorization",
            ),
            (
                "http_api_key",
                json!({"base_url": "https://a.example.com", "header_name": "X-Token"}),
                "X-Token",
            ),
            (
                "shopify_access_token",
                json!({"shop_domain": "acme.myshopify.com"}),
                "X-Shopify-Access-Token",
            ),
        ];
        for (integration_id, params, header) in cases {
            assert_eq!(
                injected_headers(integration_id, &params),
                [header],
                "{integration_id}"
            );
        }
        assert!(injected_headers("quickbooks_online", &json!({})).is_empty());
    }

    #[test]
    fn the_same_parameters_always_make_the_same_policy() {
        let params = json!({"token": "s", "base_url": "https://api.example.com/v1"});
        let first = policies("http_bearer", &params, &Allowances::default()).unwrap();
        let second = policies("http_bearer", &params, &Allowances::default()).unwrap();
        assert_eq!(
            first["token"].canonical_json().unwrap(),
            second["token"].canonical_json().unwrap()
        );
        // The secret's own value is not part of it.
        let other = json!({"token": "other", "base_url": "https://api.example.com/v1"});
        assert_eq!(
            first,
            policies("http_bearer", &other, &Allowances::default()).unwrap()
        );
    }
}

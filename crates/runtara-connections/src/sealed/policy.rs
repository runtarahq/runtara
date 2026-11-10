//! The seal policy of each secret of a connection: where it may go and how it
//! is attached.
//!
//! A policy is sealed with its secret, so it is made from the connection's
//! parameters when the secret is entered and cannot be changed afterwards
//! without the secret. Only connection types listed here are sealed; every
//! other type stays with the credential cipher.

use std::collections::BTreeMap;

use runtara_agents::extractors::connection_types::SHIPPO_API_BASE_URL;
use runtara_agents::registry::find_connection_type;
use seal_format::Policy;
use seal_format::policy::{Destination, Host, Inject, Part, Rule, Scheme};
use serde_json::Value;

use crate::auth::provider_auth::{jira_account_url, resolve_named_endpoint};

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

/// How a connection type's one secret is sent.
struct StaticSecret {
    /// The parameter holding the secret.
    field: &'static str,
    /// The API the secret is for.
    base_url: String,
    inject: Inject,
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

fn filled<'a>(params: &'a Value, name: &str) -> Option<&'a str> {
    params[name]
        .as_str()
        .filter(|value| !value.trim().is_empty())
}

/// The connection types whose secret is sealed, and how each is sent. This
/// mirrors what `describe_connection_auth` does for the same types.
fn static_secret(integration_id: &str, params: &Value) -> Option<StaticSecret> {
    let fixed = |field, base_url: &str, inject| {
        Some(StaticSecret {
            field,
            base_url: base_url.to_owned(),
            inject,
        })
    };
    match integration_id {
        "shopify_access_token" => fixed(
            "access_token",
            &format!("https://{}", filled(params, "shop_domain")?),
            header("X-Shopify-Access-Token", "{}"),
        ),
        "hubspot_access_token" => fixed("access_token", "https://api.hubapi.com", bearer()),
        "stripe_api_key" => fixed("secret_key", "https://api.stripe.com", bearer()),
        "pipedrive_api_token" => fixed(
            "api_token",
            "https://api.pipedrive.com",
            header("x-api-token", "{}"),
        ),
        "shippo_api_token" => fixed(
            "api_token",
            SHIPPO_API_BASE_URL,
            header("Authorization", "ShippoToken {}"),
        ),
        "jira_api_token" => fixed(
            "api_token",
            &jira_account_url(params)?,
            Inject::Basic {
                username: Part::Literal(filled(params, "email")?.to_owned()),
            },
        ),
        "http_bearer" => fixed("token", filled(params, "base_url")?, bearer()),
        "http_basic" => {
            let username = params["username"].as_str()?;
            // As `http_basic_auth` refuses it: the header could not be read back.
            if username.contains(':') {
                return None;
            }
            fixed(
                "password",
                filled(params, "base_url")?,
                Inject::Basic {
                    username: Part::Literal(username.to_owned()),
                },
            )
        }
        "http_api_key" => fixed(
            "api_key",
            filled(params, "base_url")?,
            header(filled(params, "header_name").unwrap_or("X-API-Key"), "{}"),
        ),
        _ => None,
    }
}

/// The request header seal sets for a connection's secret. A caller may not
/// set it too, so the proxy drops any the agent sent before handing the
/// request to seal, as it always overrode one.
pub fn injected_header(integration_id: &str, params: &Value) -> Option<String> {
    match static_secret(integration_id, params)?.inject {
        Inject::Header { name, .. } => Some(name),
        Inject::Basic { .. } => Some("Authorization".to_owned()),
        _ => None,
    }
}

/// Whether a connection type's secrets can be sealed at all, whatever its
/// parameters.
///
/// A type is listed only when nothing but the outbound proxy uses its secret.
/// `openai_api_key` and `gemini_api_key` are not: listing a provider's models
/// reads the key directly. Nor are `slack_bot`, `mailgun` and `mcp`, whose
/// secrets the channels and the MCP client read.
pub fn is_sealable_type(integration_id: &str) -> bool {
    SEALABLE_TYPES.contains(&integration_id)
}

/// The connection types [`policies`] knows.
pub const SEALABLE_TYPES: &[&str] = &[
    "shopify_access_token",
    "hubspot_access_token",
    "stripe_api_key",
    "pipedrive_api_token",
    "shippo_api_token",
    "jira_api_token",
    "http_bearer",
    "http_basic",
    "http_api_key",
];

/// A rule that sends a secret to `base_url` and below, or `None` if seal cannot
/// express the destination or this deployment does not allow it.
fn rule_for(base_url: &str, inject: &Inject, allowances: &Allowances) -> Option<Rule> {
    let url = url::Url::parse(base_url).ok()?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
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
            path_prefix: (!path.is_empty()).then(|| path.to_owned()),
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
    let secret = static_secret(integration_id, params)?;
    let mut rules = vec![rule_for(&secret.base_url, &secret.inject, allowances)?];
    // The type's other endpoints, which an agent selects by name. One whose
    // URL does not render from these parameters cannot be selected either.
    if let Some(meta) = find_connection_type(integration_id) {
        for endpoint in meta.named_endpoints {
            let Ok(resolved) = resolve_named_endpoint(meta, endpoint.name, params) else {
                continue;
            };
            let rule = rule_for(&resolved.base_url, &secret.inject, allowances)?;
            if !rules.contains(&rule) {
                rules.push(rule);
            }
        }
    }
    let policy = Policy {
        rules,
        definitions: BTreeMap::new(),
        shared: false,
        verify: None,
    };
    policy.validate().ok()?;
    Some(BTreeMap::from([(secret.field.to_owned(), policy)]))
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
            "base_url": "https://api.example.com",
        });
        for integration_id in SEALABLE_TYPES {
            let meta = find_connection_type(integration_id).unwrap();
            let secrets: Vec<&str> = meta
                .fields
                .iter()
                .filter(|field| field.is_secret)
                .map(|field| field.name)
                .collect();
            let sealed: Vec<String> = policies(integration_id, &params, &Allowances::default())
                .unwrap_or_else(|| panic!("{integration_id} is sealed"))
                .into_keys()
                .collect();
            assert_eq!(sealed, secrets, "{integration_id}");
        }
    }

    #[test]
    fn a_type_seal_does_not_handle_yet_is_not_sealed() {
        for integration_id in [
            "quickbooks_online",
            "aws_credentials",
            "slack_bot",
            "openai_api_key",
            "nope",
        ] {
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
                injected_header(integration_id, &params).as_deref(),
                Some(header),
                "{integration_id}"
            );
        }
        assert_eq!(injected_header("quickbooks_online", &json!({})), None);
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

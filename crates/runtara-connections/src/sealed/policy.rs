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
use seal_format::policy::{
    Definition, Destination, HmacHash, Host, Inject, Method, Part, PgTls, ResponseFormat, Rule,
    Scheme, SealResponse, SealedField, Verify,
};
use seal_format::{FieldName, PolicyName};
use serde_json::{Value, json};

use crate::auth::provider_auth::{
    first_string_param, jira_account_url, normalize_endpoint, openai_proxy_base_url,
    resolve_azure_blob_base_url, resolve_named_endpoint,
};

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
    /// Certificate authorities seal trusts besides the public ones, each in
    /// PEM: for a database or provider whose certificate a private CA signed.
    pub extra_roots: Vec<String>,
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
    /// Sent with the connection's requests, to each of these destinations:
    /// for a credential that signs, or one that goes with a credential that
    /// does.
    Targets(Vec<Target>),
    /// An OAuth client's secret: sent to its token endpoint only, where the
    /// access token that comes back is sealed for the API.
    ClientSecret(TokenGrant),
    /// A token of an authorization-code connection: the access token, sent
    /// to the API, or the refresh token, sent to the token endpoint for the
    /// next pair, which comes back sealed.
    Token(TokenRole, AuthorizedTokens),
    /// The client secret of an authorization-code connection: sent to the
    /// token endpoint, with a code or beside a refresh token, where the
    /// tokens that come back are sealed; and to the revocation endpoint,
    /// where the provider wants the client to say who it is.
    AuthorizedClient(AuthorizedTokens, Inject),
    /// A database password: seal logs in with it, and relays the session.
    Postgres(PgTarget),
    /// Never sent: checks that a message came from the provider.
    Verify(HmacHash),
}

/// The parameter a Postgres connection's URL is entered in. Sealed, it holds
/// the URL's password, and [`PG_TARGET_FIELD`] the rest.
pub const PG_URL_FIELD: &str = "database_url";

/// The parameter holding a sealed Postgres connection's URL without its
/// password: where seal logs in, and as whom.
pub const PG_TARGET_FIELD: &str = "database_target";

/// The database a sealed Postgres connection is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PgTarget {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
}

fn percent_decoded(value: &str) -> Option<String> {
    urlencoding::decode(value)
        .ok()
        .map(|value| value.into_owned())
}

/// A Postgres URL seal can log in for, as the URL without its password, and
/// the password.
///
/// seal always verifies the server's certificate against its host name, so
/// only a URL that asks for that (`sslmode=verify-full`) is sealed: any other
/// may be for a server whose certificate would not verify, and sealing it
/// would break the connection. Nothing else may be in the query, since seal,
/// not the URL, decides how the session is opened.
fn split_postgres_url(url: &str) -> Option<(String, String)> {
    let parsed = url::Url::parse(url.trim()).ok()?;
    if !matches!(parsed.scheme(), "postgres" | "postgresql") {
        return None;
    }
    let host = parsed
        .host_str()
        .filter(|host| !host.is_empty())?
        .to_ascii_lowercase();
    let user = Some(parsed.username()).filter(|user| !user.is_empty())?;
    let password = percent_decoded(parsed.password()?).filter(|password| !password.is_empty())?;
    let database = parsed
        .path()
        .strip_prefix('/')
        .filter(|database| !database.is_empty() && !database.contains('/'))?;
    let mut query = parsed.query_pairs();
    let verifies = matches!(
        (query.next(), query.next()),
        (Some((key, value)), None) if key == "sslmode" && value == "verify-full"
    );
    if !verifies || parsed.fragment().is_some() {
        return None;
    }
    let port = parsed.port().unwrap_or(5432);
    Some((
        format!("postgres://{user}@{host}:{port}/{database}"),
        password,
    ))
}

/// The database of a sealed Postgres connection, from its stored target.
pub fn postgres_target(params: &Value) -> Option<PgTarget> {
    let parsed = url::Url::parse(filled(params, PG_TARGET_FIELD)?).ok()?;
    Some(PgTarget {
        host: parsed.host_str()?.to_ascii_lowercase(),
        port: parsed.port()?,
        database: percent_decoded(parsed.path().strip_prefix('/')?)?,
        user: percent_decoded(parsed.username())?,
    })
}

/// A connection's parameters as they are sealed: for a Postgres connection
/// whose URL was just entered, the password apart from the rest of the URL,
/// since seal takes the password alone and the rest is where it may go. A
/// URL seal cannot log in for is left as it is, whole.
pub fn prepared(integration_id: &str, params: &Value) -> Value {
    let mut params = params.clone();
    if integration_id != "postgres" {
        return params;
    }
    let Some(fields) = params.as_object_mut() else {
        return params;
    };
    match fields.get(PG_URL_FIELD) {
        // Still sealed: its target stays with it.
        Some(sealed) if super::is_sealed(sealed) => {}
        Some(Value::String(url)) => match split_postgres_url(url) {
            Some((target, password)) => {
                fields.insert(PG_URL_FIELD.to_owned(), Value::String(password));
                fields.insert(PG_TARGET_FIELD.to_owned(), Value::String(target));
            }
            None => {
                fields.remove(PG_TARGET_FIELD);
            }
        },
        _ => {
            fields.remove(PG_TARGET_FIELD);
        }
    }
    params
}

/// Which of an authorization's two tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TokenRole {
    Access,
    Refresh,
}

/// The field a sealed refresh token is presented as.
pub const REFRESH_TOKEN_FIELD: &str = "refresh_token";

/// The definition of what refresh tokens are for.
const REFRESH_DEFINITION: &str = "refresh";

/// Where the tokens of an authorization-code connection may go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizedTokens {
    /// The token endpoint, where a refresh token gets the next pair.
    pub token_url: String,
    /// The APIs the access token may go to, each with whether it may go
    /// anywhere on the host.
    api: Vec<(String, bool)>,
    /// Where either token may be revoked.
    pub revocation_url: Option<String>,
    /// Claims of the access token this process is told, where the provider
    /// puts what a connection needs in the token itself.
    claims: Vec<String>,
}

/// The field an authorization-code connection's client secret is held in.
pub const CLIENT_SECRET_FIELD: &str = "client_secret";

/// The claim of a Xero access token naming the consent it came from, by
/// which the organisation just authorized is told from the user's others.
pub const XERO_EVENT_CLAIM: &str = "authentication_event_id";

/// The field a sealed access token is presented as: the name seal seals it
/// under as it comes back from the token endpoint.
pub const ACCESS_TOKEN_FIELD: &str = "access_token";

/// The definition, within a client secret's policy, of what its access tokens
/// are for.
const API_DEFINITION: &str = "api";

/// How an OAuth client gets an access token, and where the token may go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenGrant {
    /// The token endpoint.
    pub token_url: String,
    /// How the client secret is sent there.
    client: Inject,
    /// The APIs an access token may go to, each with whether it may go
    /// anywhere on the host.
    api: Vec<(String, bool)>,
    /// How an access token is sent to them.
    token: Inject,
}

/// One destination of a secret, and how it is used there.
struct Target {
    base_url: String,
    inject: Inject,
    /// The methods the secret may be used with there; any, if `None`.
    methods: Option<Vec<Method>>,
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

/// The headers an injection sets.
fn headers_of(inject: &Inject) -> Vec<&str> {
    match inject {
        Inject::Header { name, .. } => vec![name],
        Inject::Basic { .. } => vec!["Authorization"],
        Inject::AwsSigv4 { .. } => vec![
            "Authorization",
            "X-Amz-Date",
            "X-Amz-Content-SHA256",
            "X-Amz-Security-Token",
        ],
        Inject::AzureSharedKey { .. } => vec!["Authorization", "x-ms-date", "Date"],
        _ => Vec::new(),
    }
}

/// An injection from its JSON form. seal's own types gain options between
/// versions; one this build's seal does not know is `None`, and the
/// connection that needs it is then not sealed.
fn inject_from(value: Value) -> Option<Inject> {
    serde_json::from_value(value).ok()
}

/// The longest a presigned URL or shared access signature may last: what the
/// storage agents have always allowed.
const MAX_PRESIGN_SECONDS: u32 = 7 * 24 * 3600;

/// The methods a URL can be presigned for. A rule that allows presigning must
/// name its methods, so every other method goes by a second rule.
fn presign_methods() -> Vec<Method> {
    vec![Method::Get, Method::Head, Method::Put, Method::Delete]
}

/// AWS services whose regional endpoint is
/// `https://{service}.{region}.amazonaws.com`, which a sealed AWS connection
/// may sign for. A rule names one host and one service, so a connection is
/// sealed for these; an agent that signs for another is refused.
pub const AWS_SERVICES: &[&str] = &[
    "bedrock",
    "sqs",
    "sns",
    "dynamodb",
    "lambda",
    "kinesis",
    "sts",
    "events",
    "states",
    "secretsmanager",
    "ssm",
];

/// Whether this build's seal lets a SigV4 request name its operation in
/// `X-Amz-Target`, which SQS, DynamoDB and other JSON services need.
pub fn seal_takes_amz_target() -> bool {
    seal_format::policy::amz_header_allowed("x-amz-target", &[])
}

/// Whether this build's seal lets a rule for a whole endpoint allow copies
/// from any bucket there, which a connection to S3-compatible storage needs.
pub fn seal_copies_from_any_bucket() -> bool {
    inject_from(json!({"aws_sigv4": {
        "region": "us-east-1",
        "service": "s3",
        "access_key_id": {"literal": "probe"},
        "payload": ["computed"],
        "allow_headers": ["x-amz-copy-source"],
        "allow_copy_from_any_bucket": true,
    }}))
    .is_some()
}

/// The name a parameter is stored under: `name`, or `alias` if the connection
/// has that instead.
fn stored_as(params: &Value, name: &'static str, alias: &'static str) -> &'static str {
    if params.get(name).is_none() && params.get(alias).is_some() {
        alias
    } else {
        name
    }
}

/// Whether a connection holds a parameter, readable or sealed.
fn holds(params: &Value, name: &str) -> bool {
    filled(params, name).is_some() || super::is_sealed(&params[name])
}

/// An AWS secret access key, which signs each request (SigV4), and the
/// session token of temporary credentials, which goes with it.
///
/// The proxy hashes the body and says so (`caller_hash`), so a body of any
/// size is signed without seal holding it.
fn aws_secrets(integration_id: &str, params: &Value) -> Option<Vec<Secret>> {
    let storage = integration_id == "s3_compatible";
    // The two types name the same parameters differently, and each reads the
    // other's names too.
    let (key_field, token_field) = if storage {
        (
            stored_as(params, "secret_access_key", "aws_secret_access_key"),
            stored_as(params, "session_token", "aws_session_token"),
        )
    } else {
        (
            stored_as(params, "aws_secret_access_key", "secret_access_key"),
            stored_as(params, "aws_session_token", "session_token"),
        )
    };
    let access_key_id = first_string_param(params, &["access_key_id", "aws_access_key_id"])?;
    let region = first_string_param(params, &["region", "aws_region"])
        .unwrap_or_else(|| "us-east-1".to_owned());
    let endpoint = filled(params, "endpoint").map(normalize_endpoint);

    // Each host signed for, and the service it is.
    let destinations: Vec<(String, String)> = if storage {
        vec![(
            endpoint.unwrap_or_else(|| format!("https://s3.{region}.amazonaws.com")),
            "s3".to_owned(),
        )]
    } else {
        // Without the operation's header, half of AWS cannot be called; such
        // a connection stays readable rather than half working.
        if !seal_takes_amz_target() {
            return None;
        }
        match endpoint {
            // An endpoint of the connection's own is one host, and the
            // service signed for there is the calling agent's to name. seal
            // signs for one, so the connection must say which.
            Some(endpoint) => vec![(endpoint, filled(params, "service")?.to_owned())],
            None => std::iter::once((
                format!("https://bedrock-runtime.{region}.amazonaws.com"),
                "bedrock".to_owned(),
            ))
            .chain(AWS_SERVICES.iter().map(|service| {
                (
                    crate::auth::provider_auth::aws_default_endpoint(service, &region),
                    (*service).to_owned(),
                )
            }))
            .collect(),
        }
    };

    let with_token = holds(params, token_field);
    let sigv4 = |service: &str, presign: bool| {
        let mut options = json!({
            "region": region,
            "service": service,
            "access_key_id": {"literal": access_key_id},
            "payload": ["caller_hash", "computed"],
        });
        if with_token {
            options["session_token"] = json!({"field": token_field});
        }
        if storage {
            // The endpoint is addressed by path, so its host is every
            // bucket's, and a copy may read any of them.
            options["allow_headers"] = json!(["x-amz-copy-source"]);
            options["allow_copy_from_any_bucket"] = json!(true);
        }
        if presign {
            options["allow_presign"] = json!({"max_expiry_s": MAX_PRESIGN_SECONDS});
        }
        inject_from(json!({"aws_sigv4": options}))
    };

    let mut key_targets = Vec::new();
    let mut token_targets = Vec::new();
    for (base_url, service) in &destinations {
        if storage {
            key_targets.push(Target {
                base_url: base_url.clone(),
                inject: sigv4(service, true)?,
                methods: Some(presign_methods()),
            });
        }
        key_targets.push(Target {
            base_url: base_url.clone(),
            inject: sigv4(service, false)?,
            methods: None,
        });
        token_targets.push(Target {
            base_url: base_url.clone(),
            inject: Inject::Companion {},
            methods: None,
        });
    }
    let mut secrets = vec![Secret {
        field: key_field,
        usage: Usage::Targets(key_targets),
    }];
    if with_token {
        secrets.push(Secret {
            field: token_field,
            usage: Usage::Targets(token_targets),
        });
    }
    Some(secrets)
}

/// An Azure storage account key, which signs each request (Shared Key).
fn azure_blob_secrets(params: &Value) -> Option<Vec<Secret>> {
    let account = filled(params, "account_name")?;
    let base_url = resolve_azure_blob_base_url(params, account)?;
    let shared_key = |presign: bool| {
        let mut options = json!({"account": {"literal": account}, "service": "blob"});
        if presign {
            options["allow_presign"] = json!({"max_expiry_s": MAX_PRESIGN_SECONDS});
        }
        inject_from(json!({"azure_shared_key": options}))
    };
    Some(vec![Secret {
        field: "account_key",
        usage: Usage::Targets(vec![
            Target {
                base_url: base_url.clone(),
                inject: shared_key(true)?,
                methods: Some(presign_methods()),
            },
            Target {
                base_url,
                inject: shared_key(false)?,
                methods: None,
            },
        ]),
    }])
}

/// The client-credentials types, and the parameter that holds each one's
/// client secret.
const CLIENT_CREDENTIALS_TYPES: &[(&str, &str)] = &[
    ("shopify_client_credentials", "client_secret"),
    ("microsoft_entra_client_credentials", "client_secret"),
    ("http_oauth2_client_credentials", "client_secret"),
    ("zendesk_client_credentials", "client_secret"),
    ("ups_client_credentials", "client_secret"),
    ("teams_bot", "app_password"),
];

/// The Bot Connector, where a Teams bot's replies go. A conversation names
/// its own service URL on this host; seal takes no wildcard, so the regional
/// hosts and the other Bot Framework channels are not reached with a sealed
/// secret.
const TEAMS_BOT_CONNECTOR: &str = "https://smba.trafficmanager.net";

/// What stands for a secret while a connection's token request is worked
/// out: the request's shape is taken, and the stand-in never sent.
const SECRET_STAND_IN: &str = "(sealed)";

/// The parameter holding a client-credentials connection's secret.
pub fn client_secret_field(integration_id: &str) -> Option<&'static str> {
    CLIENT_CREDENTIALS_TYPES
        .iter()
        .find(|(known, _)| *known == integration_id)
        .map(|(_, field)| *field)
}

/// A client-credentials connection's token request, as the type builds it,
/// with a stand-in for the secret: the grant's shape, without the secret.
pub(crate) fn client_credentials_request(
    connection_id: &str,
    integration_id: &str,
    params: &Value,
) -> Option<(crate::auth::token_cache::DeferredAuth, Option<String>)> {
    let field = client_secret_field(integration_id)?;
    if !holds(params, field) {
        return None;
    }
    let mut stand_in = params.clone();
    stand_in[field] = Value::String(SECRET_STAND_IN.to_owned());
    let mut headers = std::collections::HashMap::new();
    let described = crate::auth::provider_auth::describe_connection_auth(
        connection_id,
        integration_id,
        &stand_in,
        &mut headers,
    );
    Some((described.deferred_auth?, described.base_url))
}

/// How a client-credentials connection gets its access token.
fn token_grant(integration_id: &str, params: &Value) -> Option<TokenGrant> {
    use crate::auth::token_cache::{DeferredAuth, TokenRequestBody};
    let (request, base_url) = client_credentials_request("", integration_id, params)?;
    let DeferredAuth::OAuth2ClientCredentials {
        token_url,
        header_name,
        header_value_prefix,
        request_body,
        basic_auth,
        ..
    } = request
    else {
        return None;
    };
    let client = match (basic_auth, request_body) {
        (Some((client_id, _)), _) => basic(&client_id),
        (None, TokenRequestBody::Json(_)) => Inject::JsonField {
            pointer: "/client_secret".to_owned(),
        },
        (None, TokenRequestBody::FormUrlEncoded(_)) => Inject::FormField {
            name: "client_secret".to_owned(),
        },
    };
    let api = match integration_id {
        "teams_bot" => vec![(TEAMS_BOT_CONNECTOR.to_owned(), true)],
        _ => vec![(base_url?, false)],
    };
    Some(TokenGrant {
        token_url,
        client,
        api,
        token: header(
            &header_name,
            &format!("{}{{}}", header_value_prefix.unwrap_or_default()),
        ),
    })
}

/// How a connection whose client secret is sealed gets its access token, if
/// it is of a type that gets one.
pub fn sealed_token_grant(integration_id: &str, params: &Value) -> Option<TokenGrant> {
    let field = client_secret_field(integration_id)?;
    super::is_sealed(&params[field])
        .then(|| token_grant(integration_id, params))
        .flatten()
}

/// Where the tokens of an authorization-code connection may go, if the type
/// is one.
pub fn authorized_tokens(integration_id: &str, params: &Value) -> Option<AuthorizedTokens> {
    let meta = find_connection_type(integration_id)?;
    let config = meta.oauth_config?;
    let effective = crate::auth::provider_auth::resolve_effective_oauth_config(config, params);
    if effective.token_url.is_empty() {
        return None;
    }
    let base_url = crate::auth::provider_auth::resolve_oauth_base_url(config, params)?;
    // A base that takes part of its path from the authorization itself (a
    // QuickBooks company) is not known until a flow completes, and the
    // proxy holds requests to it. The token is sealed for the API's host.
    let mut api = vec![(base_url, !config.base_url_path_template.is_empty())];
    for endpoint in meta.named_endpoints {
        if let Ok(resolved) = resolve_named_endpoint(meta, endpoint.name, params) {
            api.push((resolved.base_url, false));
        }
    }
    Some(AuthorizedTokens {
        token_url: effective.token_url,
        api,
        revocation_url: Some(effective.revocation_endpoint).filter(|url| !url.is_empty()),
        claims: match integration_id {
            "xero_oauth" => vec![XERO_EVENT_CLAIM.to_owned()],
            _ => Vec::new(),
        },
    })
}

/// How an authorization-code connection's own client secret is sent to its
/// token endpoint, if the connection holds one: a connection on a platform
/// OAuth app does not.
fn authorized_client(integration_id: &str, params: &Value) -> Option<Inject> {
    use runtara_dsl::agent_meta::TokenEndpointAuth;
    if !holds(params, CLIENT_SECRET_FIELD)
        || params.get(crate::oauth_apps::OAUTH_APP_PARAM).is_some()
    {
        return None;
    }
    let config = find_connection_type(integration_id)?.oauth_config?;
    let effective = crate::auth::provider_auth::resolve_effective_oauth_config(config, params);
    Some(match effective.token_endpoint_auth {
        TokenEndpointAuth::HttpBasic => basic(filled(params, "client_id")?),
        TokenEndpointAuth::FormBody => Inject::FormField {
            name: CLIENT_SECRET_FIELD.to_owned(),
        },
    })
}

/// Whether an authorization-code connection's client secret is sealed, and
/// so seal's to send.
pub fn sealed_authorized_client(integration_id: &str, params: &Value) -> bool {
    super::is_sealed(&params[CLIENT_SECRET_FIELD])
        && authorized_client(integration_id, params).is_some()
        && authorized_tokens(integration_id, params).is_some()
}

/// Whether a sealed client secret is also sent when a token is revoked: where
/// the provider has the client authenticate with HTTP Basic.
pub fn client_secret_revokes(integration_id: &str, params: &Value) -> bool {
    sealed_authorized_client(integration_id, params)
        && matches!(
            authorized_client(integration_id, params),
            Some(Inject::Basic { .. })
        )
}

/// Where the tokens of a connection whose refresh token is sealed may go.
pub fn sealed_authorized_tokens(integration_id: &str, params: &Value) -> Option<AuthorizedTokens> {
    super::is_sealed(&params[REFRESH_TOKEN_FIELD])
        .then(|| authorized_tokens(integration_id, params))
        .flatten()
}

/// What an authorization's tokens are sealed for: the rules of the access
/// token, those of the refresh token, the definitions naming both, and how
/// a token endpoint's answer is sealed.
struct TokenRules {
    to_api: Vec<Rule>,
    to_token_endpoint: Vec<Rule>,
    definitions: BTreeMap<PolicyName, Definition>,
    seal_response: SealResponse,
}

fn token_rules(tokens: &AuthorizedTokens, allowances: &Allowances) -> Option<TokenRules> {
    // Runtara revokes with a JSON body naming the token.
    let revoke = |rules: &mut Vec<Rule>| {
        if let Some(url) = &tokens.revocation_url {
            let inject = Inject::JsonField {
                pointer: "/token".to_owned(),
            };
            rules.push(rule_for(url, &inject, false, allowances)?);
        }
        Some(())
    };

    let mut to_api = Vec::new();
    for (base_url, any_path) in &tokens.api {
        let rule = rule_for(base_url, &bearer(), *any_path, allowances)?;
        if !to_api.contains(&rule) {
            to_api.push(rule);
        }
    }
    revoke(&mut to_api)?;

    let seal_response = SealResponse {
        format: ResponseFormat::Json,
        fields: vec![
            SealedField {
                at: "/access_token".to_owned(),
                as_field: FieldName::new(ACCESS_TOKEN_FIELD).ok()?,
                policy: PolicyName::new(API_DEFINITION).ok()?,
                expose_claims: tokens.claims.clone(),
            },
            SealedField {
                at: "/refresh_token".to_owned(),
                as_field: FieldName::new(REFRESH_TOKEN_FIELD).ok()?,
                policy: PolicyName::new(REFRESH_DEFINITION).ok()?,
                expose_claims: Vec::new(),
            },
        ],
    };
    let mut refresh = rule_for(
        &tokens.token_url,
        &Inject::FormField {
            name: REFRESH_TOKEN_FIELD.to_owned(),
        },
        false,
        allowances,
    )?;
    refresh.seal_response = Some(seal_response.clone());
    let mut to_token_endpoint = vec![refresh];
    revoke(&mut to_token_endpoint)?;

    let definitions = BTreeMap::from([
        (
            PolicyName::new(API_DEFINITION).ok()?,
            Definition {
                rules: to_api.clone(),
            },
        ),
        (
            PolicyName::new(REFRESH_DEFINITION).ok()?,
            Definition {
                rules: to_token_endpoint.clone(),
            },
        ),
    ]);
    Some(TokenRules {
        to_api,
        to_token_endpoint,
        definitions,
        seal_response,
    })
}

/// The policy of one of an authorization's tokens. Each names both
/// definitions, so that the pair a refresh brings back has these same
/// policies, however many times it is refreshed.
fn token_policy(
    role: TokenRole,
    tokens: &AuthorizedTokens,
    allowances: &Allowances,
) -> Option<Policy> {
    let rules = token_rules(tokens, allowances)?;
    Some(Policy {
        rules: match role {
            TokenRole::Access => rules.to_api,
            TokenRole::Refresh => rules.to_token_endpoint,
        },
        definitions: rules.definitions,
        shared: false,
        verify: None,
    })
}

/// The policy of an authorization-code connection's client secret: to the
/// token endpoint, where what comes back is sealed as the tokens' own
/// policies say, and to the revocation endpoint if the client authenticates
/// there.
fn authorized_client_policy(
    tokens: &AuthorizedTokens,
    client: &Inject,
    allowances: &Allowances,
) -> Option<Policy> {
    let rules = token_rules(tokens, allowances)?;
    let mut to_token_endpoint = rule_for(&tokens.token_url, client, false, allowances)?;
    to_token_endpoint.seal_response = Some(rules.seal_response);
    let mut client_rules = vec![to_token_endpoint];
    if let (Some(url), Inject::Basic { .. }) = (&tokens.revocation_url, client) {
        client_rules.push(rule_for(url, client, false, allowances)?);
    }
    Some(Policy {
        rules: client_rules,
        definitions: rules.definitions,
        shared: false,
        verify: None,
    })
}

/// The policy of a client secret, and of the access tokens it gets.
fn client_secret_policy(grant: &TokenGrant, allowances: &Allowances) -> Option<Policy> {
    let mut to_token_endpoint = rule_for(&grant.token_url, &grant.client, false, allowances)?;
    to_token_endpoint.seal_response = Some(SealResponse {
        format: ResponseFormat::Json,
        fields: vec![SealedField {
            at: "/access_token".to_owned(),
            as_field: FieldName::new(ACCESS_TOKEN_FIELD).ok()?,
            policy: PolicyName::new(API_DEFINITION).ok()?,
            expose_claims: Vec::new(),
        }],
    });
    let mut to_api = Vec::new();
    for (base_url, any_path) in &grant.api {
        to_api.push(rule_for(base_url, &grant.token, *any_path, allowances)?);
    }
    Some(Policy {
        rules: vec![to_token_endpoint],
        definitions: BTreeMap::from([(
            PolicyName::new(API_DEFINITION).ok()?,
            Definition { rules: to_api },
        )]),
        shared: false,
        verify: None,
    })
}

/// Whether a connection's requests are signed by seal with SigV4. The body's
/// hash is then the caller's to send.
pub fn signs_with_sigv4(integration_id: &str, params: &Value) -> bool {
    secrets(integration_id, params)
        .into_iter()
        .flatten()
        .any(|secret| match &secret.usage {
            Usage::Targets(targets) => targets
                .iter()
                .any(|target| matches!(target.inject, Inject::AwsSigv4 { .. })),
            _ => false,
        })
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
            extras.keys().any(|name| {
                headers_of(inject)
                    .iter()
                    .any(|set| name.eq_ignore_ascii_case(set))
            })
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
    // A connection type an agent declared is its base type with a base URL
    // rendered from the declaration: its secrets are the base's, under the
    // same names, sealed for the rendered URL. Parameters that do not render
    // name no destination, and are not sealed.
    if let Some(translated) = runtara_agents::derived::to_base(integration_id, params) {
        let (base, base_params) = translated.ok()?;
        return secrets(base, &base_params);
    }
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
        // The private key of a client certificate. The certificate, and any
        // CA the server is checked with, are public: they are part of the
        // key's policy, so changing either needs the key again.
        "http_mtls" => Some(vec![Secret {
            field: "client_private_key_pem",
            usage: Usage::Targets(vec![Target {
                base_url: filled(params, "base_url")?.to_owned(),
                inject: Inject::ClientCert {
                    cert: Part::Literal(
                        filled(params, "client_certificate_pem")?.trim().to_owned(),
                    ),
                    chain: None,
                    ca: filled(params, "server_ca_pem").map(|ca| ca.trim().to_owned()),
                    server_name: None,
                },
                methods: None,
            }]),
        }]),
        "aws_credentials" | "s3_compatible" => aws_secrets(integration_id, params),
        "postgres" => Some(vec![Secret {
            field: PG_URL_FIELD,
            usage: Usage::Postgres(postgres_target(params)?),
        }]),
        _ if authorized_tokens(integration_id, params).is_some() => {
            let tokens = authorized_tokens(integration_id, params)?;
            let mut secrets = vec![
                Secret {
                    field: ACCESS_TOKEN_FIELD,
                    usage: Usage::Token(TokenRole::Access, tokens.clone()),
                },
                Secret {
                    field: REFRESH_TOKEN_FIELD,
                    usage: Usage::Token(TokenRole::Refresh, tokens.clone()),
                },
            ];
            if let Some(client) = authorized_client(integration_id, params) {
                secrets.push(Secret {
                    field: CLIENT_SECRET_FIELD,
                    usage: Usage::AuthorizedClient(tokens, client),
                });
            }
            Some(secrets)
        }
        _ if client_secret_field(integration_id).is_some() => Some(vec![Secret {
            field: client_secret_field(integration_id)?,
            usage: Usage::ClientSecret(token_grant(integration_id, params)?),
        }]),
        "azure_blob_storage" => azure_blob_secrets(params),
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
    // A client secret goes to its token endpoint only. What a request
    // presents is the access token got with it.
    if client_secret_field(integration_id).is_some() {
        return vec![ACCESS_TOKEN_FIELD.to_owned()];
    }
    secrets(integration_id, params)
        .into_iter()
        .flatten()
        .filter(|secret| {
            matches!(
                secret.usage,
                Usage::Send {
                    presented: true,
                    ..
                } | Usage::Targets(_)
                    | Usage::Token(TokenRole::Access, _)
                    | Usage::Postgres(_)
            )
        })
        .map(|secret| secret.field.to_owned())
        .collect()
}

/// The request headers seal sets for a connection's secrets. A caller may not
/// set them too, so the proxy drops any the agent sent before handing the
/// request to seal, as it always overrode them.
pub fn injected_headers(integration_id: &str, params: &Value) -> Vec<String> {
    let mut headers: Vec<String> = Vec::new();
    for secret in secrets(integration_id, params).into_iter().flatten() {
        let injects: Vec<&Inject> = match &secret.usage {
            Usage::Send {
                inject,
                presented: true,
                ..
            } => vec![inject],
            Usage::Targets(targets) => targets.iter().map(|target| &target.inject).collect(),
            Usage::ClientSecret(grant) => vec![&grant.token],
            Usage::Token(TokenRole::Access, _) => {
                headers.push("Authorization".to_owned());
                Vec::new()
            }
            _ => Vec::new(),
        };
        for header in injects.into_iter().flat_map(headers_of) {
            if !headers
                .iter()
                .any(|known| known.eq_ignore_ascii_case(header))
            {
                headers.push(header.to_owned());
            }
        }
    }
    headers
}

/// Whether a connection type's secrets can be sealed at all, whatever its
/// parameters.
pub fn is_sealable_type(integration_id: &str) -> bool {
    SEALABLE_TYPES.contains(&integration_id)
        // A type an agent declared, by its base.
        || runtara_agents::derived::find(integration_id)
            .is_some_and(|derived| SEALABLE_TYPES.contains(&derived.base.integration_id))
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
    "aws_credentials",
    "s3_compatible",
    "azure_blob_storage",
    "http_mtls",
    "shopify_client_credentials",
    "microsoft_entra_client_credentials",
    "http_oauth2_client_credentials",
    "zendesk_client_credentials",
    "ups_client_credentials",
    "teams_bot",
    "hubspot_private_app",
    "quickbooks_online",
    "xero_oauth",
    "http_oauth2_authorization_code",
    "postgres",
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
            Usage::Postgres(target) => {
                let mut rule = Rule::new(
                    Destination::Postgres {
                        host: Host::new(target.host.as_str()).ok()?,
                        port: target.port,
                        databases: vec![target.database.clone()],
                        user: Part::Literal(target.user.clone()),
                        tls: PgTls::VerifyFull,
                        allow_replication: false,
                    },
                    // As the connection logged in before it was sealed: with
                    // whatever the server asks for, over a verified session.
                    Inject::PgPassword {
                        allow_md5: true,
                        allow_cleartext_password: true,
                        require_channel_binding: false,
                    },
                );
                rule.allow_private_destination =
                    allowances.allows_private(&target.host, Some(target.port));
                Policy {
                    rules: vec![rule],
                    definitions: BTreeMap::new(),
                    shared: false,
                    verify: None,
                }
            }
            Usage::Token(role, tokens) => {
                let mut tokens = tokens.clone();
                if let Some(api_base) = allowances.api_bases.get(integration_id) {
                    tokens.api = vec![(api_base.clone(), true)];
                }
                token_policy(*role, &tokens, allowances)?
            }
            Usage::AuthorizedClient(tokens, client) => {
                let mut tokens = tokens.clone();
                if let Some(api_base) = allowances.api_bases.get(integration_id) {
                    tokens.api = vec![(api_base.clone(), true)];
                }
                authorized_client_policy(&tokens, client, allowances)?
            }
            Usage::ClientSecret(grant) => {
                let mut grant = grant.clone();
                if let Some(api_base) = allowances.api_bases.get(integration_id) {
                    grant.api = vec![(api_base.clone(), true)];
                }
                client_secret_policy(&grant, allowances)?
            }
            Usage::Targets(targets) => {
                let mut rules = Vec::new();
                for target in targets {
                    let mut rule = rule_for(&target.base_url, &target.inject, false, allowances)?;
                    rule.methods = target.methods.clone();
                    if !rules.contains(&rule) {
                        rules.push(rule);
                    }
                }
                Policy {
                    rules,
                    definitions: BTreeMap::new(),
                    shared: false,
                    verify: None,
                }
            }
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
        assert!(!is_sealable_type("sftp"));
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
            "access_key_id": "AKIA", "aws_access_key_id": "AKIA",
            "account_name": "account",
            "client_certificate_pem": "-----BEGIN CERTIFICATE-----\nAA==\n-----END CERTIFICATE-----",
            "client_id": "client", "client_secret": "s", "app_id": "app", "app_password": "s",
            "tenant_id": "tenant", "scope": "read", "subdomain": "acme",
            "token_url": "https://auth.example.com/token",
            "database_target": "postgres://app@db.example.com:5432/orders",
        });
        for integration_id in SEALABLE_TYPES {
            if !this_seal_can_seal(integration_id) {
                continue;
            }
            let mut params = params.clone();
            if *integration_id == "aws_credentials" {
                params["aws_session_token"] = json!("s");
            }
            let meta = find_connection_type(integration_id).unwrap();
            let mut secrets: Vec<&str> = meta
                .fields
                .iter()
                .filter(|field| field.is_secret)
                .map(|field| field.name)
                .collect();
            // A certificate, or a client's id, may be marked secret to keep
            // it out of the console, but it is public: seal is given it as
            // it is.
            secrets.retain(|name| {
                !["client_certificate_pem", "server_ca_pem", "client_id"].contains(name)
            });
            // An authorization's tokens are what is sealed of it; they are
            // the provider's, not fields of the type.
            if meta.oauth_config.is_some() {
                secrets = vec!["access_token", "client_secret", "refresh_token"];
            }
            secrets.sort_unstable();
            let sealed: Vec<String> = policies(integration_id, &params, &Allowances::default())
                .unwrap_or_else(|| panic!("{integration_id} is sealed"))
                .into_keys()
                .collect();
            assert_eq!(sealed, secrets, "{integration_id}");
        }
    }

    /// Whether this build's seal can do what a type's agents need. Where it
    /// cannot, the type's connections are not sealed.
    fn this_seal_can_seal(integration_id: &str) -> bool {
        match integration_id {
            "aws_credentials" => seal_takes_amz_target(),
            "s3_compatible" => seal_copies_from_any_bucket(),
            _ => true,
        }
    }

    #[test]
    fn an_aws_key_signs_for_bedrock_and_the_regional_services() {
        let params = json!({
            "aws_access_key_id": "AKIA", "aws_secret_access_key": "s", "aws_region": "eu-west-1",
        });
        let Some(policies) = policies("aws_credentials", &params, &Allowances::default()) else {
            assert!(!seal_takes_amz_target());
            return;
        };
        assert_eq!(policies.len(), 1);
        let policy = canonical(&policies["aws_secret_access_key"]);
        let rules = policy["rules"].as_array().unwrap();
        assert_eq!(rules.len(), AWS_SERVICES.len() + 1);
        assert_eq!(
            rules[0],
            json!({
                "destination": {
                    "scheme": "https",
                    "host": "bedrock-runtime.eu-west-1.amazonaws.com",
                },
                "inject": {"aws_sigv4": {
                    "region": "eu-west-1",
                    "service": "bedrock",
                    "access_key_id": {"literal": "AKIA"},
                    "payload": ["caller_hash", "computed"],
                }},
            })
        );
        let sqs = rules
            .iter()
            .find(|rule| rule["destination"]["host"] == "sqs.eu-west-1.amazonaws.com")
            .expect("a rule for SQS");
        assert_eq!(sqs["inject"]["aws_sigv4"]["service"], "sqs");
        assert_eq!(
            request_fields("aws_credentials", &params),
            ["aws_secret_access_key"]
        );
        assert_eq!(
            injected_headers("aws_credentials", &params),
            [
                "Authorization",
                "X-Amz-Date",
                "X-Amz-Content-SHA256",
                "X-Amz-Security-Token"
            ]
        );
        assert!(signs_with_sigv4("aws_credentials", &params));
        assert!(!signs_with_sigv4(
            "http_bearer",
            &json!({"base_url": "https://a.example.com"})
        ));
    }

    #[test]
    fn an_aws_session_token_goes_only_with_the_key_it_belongs_to() {
        if !seal_takes_amz_target() {
            return;
        }
        // The names the parameters were stored under are the ones sealed.
        let params = json!({
            "access_key_id": "AKIA", "secret_access_key": "s", "session_token": "t",
            "region": "us-east-1", "endpoint": "https://sqs.internal.example.com", "service": "sqs",
        });
        let policies = policies("aws_credentials", &params, &Allowances::default()).unwrap();
        let key = canonical(&policies["secret_access_key"]);
        assert_eq!(
            key["rules"],
            json!([{
                "destination": {"scheme": "https", "host": "sqs.internal.example.com"},
                "inject": {"aws_sigv4": {
                    "region": "us-east-1",
                    "service": "sqs",
                    "access_key_id": {"literal": "AKIA"},
                    "session_token": {"field": "session_token"},
                    "payload": ["caller_hash", "computed"],
                }},
            }])
        );
        assert_eq!(
            canonical(&policies["session_token"])["rules"],
            json!([{
                "destination": {"scheme": "https", "host": "sqs.internal.example.com"},
                "inject": {"companion": {}},
            }])
        );
        assert_eq!(
            request_fields("aws_credentials", &params),
            ["secret_access_key", "session_token"]
        );
        // Stored sealed, the token is still part of the credential.
        let mut stored = params.clone();
        stored["session_token"] = json!({"$sealed": {}});
        assert_eq!(
            request_fields("aws_credentials", &stored),
            ["secret_access_key", "session_token"]
        );
    }

    #[test]
    fn an_aws_endpoint_of_the_connections_own_must_say_its_service() {
        // One host is signed for as one service, and only the connection can
        // say which.
        let params = json!({
            "aws_access_key_id": "AKIA", "aws_secret_access_key": "s",
            "endpoint": "https://vpce.example.com",
        });
        assert_eq!(
            policies("aws_credentials", &params, &Allowances::default()),
            None
        );
        // No access key id, no signature.
        assert_eq!(
            policies(
                "aws_credentials",
                &json!({"aws_secret_access_key": "s"}),
                &Allowances::default()
            ),
            None
        );
    }

    #[test]
    fn a_storage_key_signs_for_its_endpoint_and_may_presign() {
        let params = json!({
            "access_key_id": "AKIA", "secret_access_key": "s", "region": "us-east-1",
            "endpoint": "storage.example.com",
        });
        let Some(policies) = policies("s3_compatible", &params, &Allowances::default()) else {
            assert!(!seal_copies_from_any_bucket());
            return;
        };
        let sigv4 = json!({
            "region": "us-east-1",
            "service": "s3",
            "access_key_id": {"literal": "AKIA"},
            "payload": ["caller_hash", "computed"],
            "allow_headers": ["x-amz-copy-source"],
            "allow_copy_from_any_bucket": true,
        });
        let mut presigning = sigv4.clone();
        presigning["allow_presign"] = json!({"max_expiry_s": 604_800});
        let destination = json!({"scheme": "https", "host": "storage.example.com"});
        // The rule that may presign comes first, so that it is the one a
        // presigned method matches; every other method goes by the second.
        assert_eq!(
            canonical(&policies["secret_access_key"])["rules"],
            json!([
                {
                    "destination": destination,
                    "methods": ["GET", "HEAD", "PUT", "DELETE"],
                    "inject": {"aws_sigv4": presigning},
                },
                {"destination": destination, "inject": {"aws_sigv4": sigv4}},
            ])
        );
        // AWS's own S3, when the connection names no endpoint.
        let aws = json!({"access_key_id": "AKIA", "secret_access_key": "s", "region": "eu-west-1"});
        let policies = super::policies("s3_compatible", &aws, &Allowances::default()).unwrap();
        assert_eq!(
            canonical(&policies["secret_access_key"])["rules"][0]["destination"]["host"],
            "s3.eu-west-1.amazonaws.com"
        );
    }

    #[test]
    fn a_client_certificates_key_is_sealed_with_the_certificate_it_belongs_to() {
        let certificate = "-----BEGIN CERTIFICATE-----\nAA==\n-----END CERTIFICATE-----";
        let ca = "-----BEGIN CERTIFICATE-----\nBB==\n-----END CERTIFICATE-----";
        let params = json!({
            "base_url": "https://partner.example.com/api",
            "client_certificate_pem": format!("{certificate}\n"),
            "client_private_key_pem": "key",
            "server_ca_pem": format!("\n{ca}\n"),
        });
        let (field, policy) = only("http_mtls", params.clone());
        assert_eq!(field, "client_private_key_pem");
        assert_eq!(
            policy,
            json!({"rules": [{
                "destination": {
                    "scheme": "https", "host": "partner.example.com", "path_prefix": "/api",
                },
                "inject": {"client_cert": {"cert": {"literal": certificate}, "ca": ca}},
            }]})
        );
        assert_eq!(
            request_fields("http_mtls", &params),
            ["client_private_key_pem"]
        );
        assert!(injected_headers("http_mtls", &params).is_empty());

        // No server CA: the usual roots.
        let mut public = params.clone();
        public["server_ca_pem"] = json!("");
        let (_, policy) = only("http_mtls", public);
        assert_eq!(policy["rules"][0]["inject"]["client_cert"].get("ca"), None);

        // A client certificate is never offered over plain HTTP, whatever the
        // deployment allows elsewhere.
        let mut plain = params.clone();
        plain["base_url"] = json!("http://127.0.0.1:8080");
        let allowances = Allowances {
            private_hosts: vec!["127.0.0.1:8080".to_owned()],
            plain_http_hosts: vec!["127.0.0.1".to_owned()],
            ..Allowances::default()
        };
        assert_eq!(policies("http_mtls", &plain, &allowances), None);
        // No certificate, nothing to present.
        let mut bare = params;
        bare["client_certificate_pem"] = json!("");
        assert_eq!(policies("http_mtls", &bare, &Allowances::default()), None);
    }

    /// The policy of a client-credentials connection's secret.
    fn client_secret(integration_id: &str, params: Value) -> (String, Value) {
        let policies = policies(integration_id, &params, &Allowances::default())
            .unwrap_or_else(|| panic!("{integration_id} is sealed"));
        assert_eq!(policies.len(), 1, "{integration_id}");
        let (field, policy) = policies.into_iter().next().unwrap();
        (field, canonical(&policy))
    }

    /// What a token endpoint's answer is sealed as.
    fn sealed_access_token() -> Value {
        json!({
            "format": "json",
            "fields": [{"at": "/access_token", "as": "access_token", "policy": "api"}],
        })
    }

    #[test]
    fn a_client_secret_goes_to_its_token_endpoint_and_its_tokens_to_the_api() {
        let (field, policy) = client_secret(
            "http_oauth2_client_credentials",
            json!({
                "client_id": "cid", "client_secret": "s",
                "token_url": "https://auth.example.com/oauth/token",
                "base_url": "https://api.example.com/v2",
            }),
        );
        assert_eq!(field, "client_secret");
        assert_eq!(
            policy,
            json!({
                "rules": [{
                    "destination": {
                        "scheme": "https", "host": "auth.example.com", "path_prefix": "/oauth/token",
                    },
                    "inject": {"form_field": {"name": "client_secret"}},
                    "seal_response": sealed_access_token(),
                }],
                "definitions": {"api": {"rules": [{
                    "destination": {
                        "scheme": "https", "host": "api.example.com", "path_prefix": "/v2",
                    },
                    "inject": {"header": {"name": "Authorization", "format": "Bearer {}"}},
                }]}},
            })
        );
        // A request presents the token, never the secret.
        let params = json!({"client_secret": {"$sealed": {}}});
        assert_eq!(
            request_fields("http_oauth2_client_credentials", &params),
            ["access_token"]
        );
    }

    #[test]
    fn each_client_credentials_type_sends_its_secret_as_it_always_did() {
        // In an `Authorization: Basic` header, with the client's id.
        let (_, ups) = client_secret(
            "ups_client_credentials",
            json!({"client_id": "cid", "client_secret": "s"}),
        );
        assert_eq!(
            ups["rules"][0]["inject"],
            json!({"basic": {"username": {"literal": "cid"}}})
        );
        assert_eq!(
            ups["rules"][0]["destination"]["path_prefix"],
            "/security/v1/oauth/token"
        );
        let (_, generic) = client_secret(
            "http_oauth2_client_credentials",
            json!({
                "client_id": "cid", "client_secret": "s", "token_auth": "basic",
                "token_url": "https://auth.example.com/token",
                "base_url": "https://api.example.com",
            }),
        );
        assert_eq!(
            generic["rules"][0]["inject"],
            json!({"basic": {"username": {"literal": "cid"}}})
        );

        // In a JSON body, and the token in the provider's own header.
        let (_, shopify) = client_secret(
            "shopify_client_credentials",
            json!({"shop_domain": "acme.myshopify.com", "client_id": "cid", "client_secret": "s"}),
        );
        assert_eq!(
            shopify["rules"][0],
            json!({
                "destination": {
                    "scheme": "https", "host": "acme.myshopify.com",
                    "path_prefix": "/admin/oauth/access_token",
                },
                "inject": {"json_field": {"pointer": "/client_secret"}},
                "seal_response": sealed_access_token(),
            })
        );
        assert_eq!(
            shopify["definitions"]["api"]["rules"],
            json!([{
                "destination": {"scheme": "https", "host": "acme.myshopify.com"},
                "inject": {"header": {"name": "X-Shopify-Access-Token", "format": "{}"}},
            }])
        );
        assert_eq!(
            injected_headers(
                "shopify_client_credentials",
                &json!({"shop_domain": "acme.myshopify.com", "client_id": "cid", "client_secret": "s"})
            ),
            ["X-Shopify-Access-Token"]
        );
        let (_, zendesk) = client_secret(
            "zendesk_client_credentials",
            json!({"subdomain": "acme", "client_id": "cid", "client_secret": "s"}),
        );
        assert_eq!(
            zendesk["rules"][0]["inject"],
            json!({"json_field": {"pointer": "/client_secret"}})
        );
        assert_eq!(
            zendesk["rules"][0]["destination"]["host"],
            "acme.zendesk.com"
        );

        // In a form body, to the tenant's own Entra authority.
        let (_, entra) = client_secret(
            "microsoft_entra_client_credentials",
            json!({
                "tenant_id": "tenant-1", "client_id": "cid", "client_secret": "s",
                "scope": "https://graph.microsoft.com/.default",
                "base_url": "https://graph.microsoft.com",
            }),
        );
        assert_eq!(
            entra["rules"][0]["destination"],
            json!({
                "scheme": "https", "host": "login.microsoftonline.com",
                "path_prefix": "/tenant-1/oauth2/v2.0/token",
            })
        );
        assert_eq!(
            entra["rules"][0]["inject"],
            json!({"form_field": {"name": "client_secret"}})
        );
    }

    #[test]
    fn an_authorizations_tokens_go_to_the_api_and_back_for_the_next_pair() {
        let params = json!({"client_id": "cid", "client_secret": "s", "realm_id": "123"});
        let policies = policies("quickbooks_online", &params, &Allowances::default()).unwrap();
        assert_eq!(
            policies.keys().collect::<Vec<_>>(),
            ["access_token", "client_secret", "refresh_token"]
        );
        let bearer = json!({"header": {"name": "Authorization", "format": "Bearer {}"}});
        let revoke = json!({
            "destination": {
                "scheme": "https", "host": "developer.api.intuit.com",
                "path_prefix": "/v2/oauth2/tokens/revoke",
            },
            "inject": {"json_field": {"pointer": "/token"}},
        });
        // The API's host (the company is the proxy's to hold a request to),
        // the type's other endpoint, and revocation.
        let to_api = json!([
            {
                "destination": {"scheme": "https", "host": "quickbooks.api.intuit.com"},
                "inject": bearer,
            },
            {
                "destination": {
                    "scheme": "https", "host": "qb.api.intuit.com", "path_prefix": "/graphql",
                },
                "inject": bearer,
            },
            revoke,
        ]);
        let to_token_endpoint = json!([
            {
                "destination": {
                    "scheme": "https", "host": "oauth.platform.intuit.com",
                    "path_prefix": "/oauth2/v1/tokens/bearer",
                },
                "inject": {"form_field": {"name": "refresh_token"}},
                "seal_response": {"format": "json", "fields": [
                    {"at": "/access_token", "as": "access_token", "policy": "api"},
                    {"at": "/refresh_token", "as": "refresh_token", "policy": "refresh"},
                ]},
            },
            revoke,
        ]);
        let definitions = json!({
            "api": {"rules": to_api},
            "refresh": {"rules": to_token_endpoint},
        });
        assert_eq!(
            canonical(&policies["access_token"]),
            json!({"rules": to_api, "definitions": definitions})
        );
        assert_eq!(
            canonical(&policies["refresh_token"]),
            json!({"rules": to_token_endpoint, "definitions": definitions})
        );
        // What seal gives a refreshed token is the policy its marker names.
        let refresh = &policies["refresh_token"];
        for (field, definition) in [("access_token", "api"), ("refresh_token", "refresh")] {
            assert_eq!(
                refresh
                    .for_response_field(&PolicyName::new(definition).unwrap())
                    .unwrap(),
                policies[field],
                "{field}"
            );
        }
        // The client's secret goes to the token endpoint, where what comes
        // back is sealed as the tokens' own policies say, and to revocation:
        // Intuit has the client say who it is with HTTP Basic at both.
        let basic = json!({"basic": {"username": {"literal": "cid"}}});
        assert_eq!(
            canonical(&policies["client_secret"]),
            json!({
                "rules": [
                    {
                        "destination": to_token_endpoint[0]["destination"],
                        "inject": basic,
                        "seal_response": to_token_endpoint[0]["seal_response"],
                    },
                    {"destination": revoke["destination"], "inject": basic},
                ],
                "definitions": definitions,
            })
        );
        assert!(client_secret_revokes(
            "quickbooks_online",
            &json!({"client_id": "cid", "client_secret": {"$sealed": {}}})
        ));
        // A connection on a platform OAuth app holds no client secret.
        let on_app = json!({"oauth_app": "intuit", "client_id": "cid", "client_secret": "s"});
        assert_eq!(
            super::policies("quickbooks_online", &on_app, &Allowances::default())
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            ["access_token", "refresh_token"]
        );
        // Xero's access token says which consent it came from, and this
        // process is told that much of it.
        let xero = super::policies(
            "xero_oauth",
            &json!({"client_id": "cid", "client_secret": "s"}),
            &Allowances::default(),
        )
        .unwrap();
        assert_eq!(
            canonical(&xero["refresh_token"])["rules"][0]["seal_response"]["fields"][0],
            json!({
                "at": "/access_token", "as": "access_token", "policy": "api",
                "expose_claims": ["authentication_event_id"],
            })
        );
        // A request presents the access token only.
        assert_eq!(
            request_fields("quickbooks_online", &params),
            ["access_token"]
        );
        assert_eq!(
            injected_headers("quickbooks_online", &params),
            ["Authorization"]
        );
        // The company does not change what the tokens are sealed for.
        let other = json!({"client_id": "cid", "client_secret": "s", "realm_id": "456"});
        assert_eq!(
            super::policies("quickbooks_online", &other, &Allowances::default()).unwrap(),
            policies
        );
    }

    #[test]
    fn a_database_password_is_sealed_apart_from_where_it_logs_in() {
        let entered = json!({
            "database_url": "postgresql://app%40acme:p%40ss%2Fword@DB.example.com/orders?sslmode=verify-full",
        });
        // Nothing is sealed of a URL as it is entered: it is split first.
        assert_eq!(policies("postgres", &entered, &Allowances::default()), None);
        let prepared = prepared("postgres", &entered);
        assert_eq!(
            prepared,
            json!({
                "database_url": "p@ss/word",
                "database_target": "postgres://app%40acme@db.example.com:5432/orders",
            })
        );
        assert_eq!(
            postgres_target(&prepared),
            Some(PgTarget {
                host: "db.example.com".into(),
                port: 5432,
                database: "orders".into(),
                user: "app@acme".into(),
            })
        );
        let (field, policy) = only("postgres", prepared.clone());
        assert_eq!(field, "database_url");
        assert_eq!(
            policy,
            json!({"rules": [{
                "destination": {
                    "scheme": "postgres", "host": "db.example.com", "port": 5432,
                    "databases": ["orders"], "user": {"literal": "app@acme"},
                },
                "inject": {"pg_password": {
                    "allow_md5": true, "allow_cleartext_password": true,
                }},
            }]})
        );
        assert_eq!(request_fields("postgres", &prepared), ["database_url"]);

        // Sealed, the password's marker and its target stay as they are.
        let stored = json!({
            "database_url": {"$sealed": {}},
            "database_target": "postgres://app@db.example.com:6432/orders",
        });
        assert_eq!(super::prepared("postgres", &stored), stored);
        // A URL entered anew replaces both.
        let mut again = stored.clone();
        again["database_url"] = json!("postgres://app:pw@other.example.com/db?sslmode=verify-full");
        assert_eq!(
            super::prepared("postgres", &again)["database_target"],
            "postgres://app@other.example.com:5432/db"
        );
        // Only a Postgres connection is split.
        assert_eq!(super::prepared("http_bearer", &entered), entered);
    }

    #[test]
    fn a_database_url_seal_could_not_log_in_for_is_left_whole() {
        for url in [
            // seal always verifies the server; these do not ask for that.
            "postgres://app:pw@db.example.com/orders",
            "postgres://app:pw@db.example.com/orders?sslmode=require",
            "postgres://app:pw@db.example.com/orders?sslmode=disable",
            // seal, not the URL, decides how the session is opened.
            "postgres://app:pw@db.example.com/orders?sslmode=verify-full&sslrootcert=/ca.pem",
            "postgres://app:pw@db.example.com/orders?sslmode=verify-full&host=/tmp",
            // Nothing to log in with, as, or to.
            "postgres://app@db.example.com/orders?sslmode=verify-full",
            "postgres://:pw@db.example.com/orders?sslmode=verify-full",
            "postgres://app:pw@db.example.com/?sslmode=verify-full",
            "postgres://app:pw@db.example.com/a/b?sslmode=verify-full",
            "mysql://app:pw@db.example.com/orders?sslmode=verify-full",
            "not a url",
        ] {
            // A target left from an earlier, sealed URL goes with it.
            let entered = json!({
                "database_url": url,
                "database_target": "postgres://old@db.example.com:5432/old",
            });
            assert_eq!(
                prepared("postgres", &entered),
                json!({"database_url": url}),
                "{url}"
            );
        }
    }

    #[test]
    fn a_teams_bots_token_goes_to_the_bot_connector() {
        let (field, policy) = client_secret(
            "teams_bot",
            json!({"app_id": "app-1", "app_password": "s", "azure_tenant_id": "tenant-1"}),
        );
        assert_eq!(field, "app_password");
        assert_eq!(
            policy["rules"][0]["destination"]["path_prefix"],
            "/tenant-1/oauth2/v2.0/token"
        );
        // A conversation's service URL is a path on this host.
        assert_eq!(
            policy["definitions"]["api"]["rules"],
            json!([{
                "destination": {"scheme": "https", "host": "smba.trafficmanager.net"},
                "inject": {"header": {"name": "Authorization", "format": "Bearer {}"}},
            }])
        );
    }

    #[test]
    fn a_client_secret_with_nowhere_to_go_is_not_sealed() {
        // No token endpoint, or no API.
        for params in [
            json!({"client_id": "cid", "client_secret": "s", "base_url": "https://api.example.com"}),
            json!({"client_id": "cid", "client_secret": "s", "token_url": "https://auth.example.com/token"}),
            json!({"client_id": "cid", "client_secret": "s",
                "token_url": "https://auth.example.com/token?tenant=1",
                "base_url": "https://api.example.com"}),
        ] {
            assert_eq!(
                policies(
                    "http_oauth2_client_credentials",
                    &params,
                    &Allowances::default()
                ),
                None,
                "{params}"
            );
        }
    }

    #[test]
    fn an_azure_account_key_signs_for_the_accounts_blob_endpoint() {
        let params = json!({"account_name": "acme", "account_key": "a2V5"});
        let policies = policies("azure_blob_storage", &params, &Allowances::default()).unwrap();
        let destination = json!({"scheme": "https", "host": "acme.blob.core.windows.net"});
        assert_eq!(
            canonical(&policies["account_key"])["rules"],
            json!([
                {
                    "destination": destination,
                    "methods": ["GET", "HEAD", "PUT", "DELETE"],
                    "inject": {"azure_shared_key": {
                        "account": {"literal": "acme"},
                        "service": "blob",
                        "allow_presign": {"max_expiry_s": 604_800},
                    }},
                },
                {
                    "destination": destination,
                    "inject": {"azure_shared_key": {
                        "account": {"literal": "acme"},
                        "service": "blob",
                    }},
                },
            ])
        );
        assert_eq!(
            request_fields("azure_blob_storage", &params),
            ["account_key"]
        );
        assert_eq!(
            injected_headers("azure_blob_storage", &params),
            ["Authorization", "x-ms-date", "Date"]
        );
        // An endpoint addressed by path holds the key to the account's path.
        let azurite = json!({
            "account_name": "devstore", "account_key": "a2V5",
            "endpoint_override": "https://azurite.example.com",
        });
        let policies = super::policies("azure_blob_storage", &azurite, &Allowances::default());
        assert_eq!(
            canonical(&policies.unwrap()["account_key"])["rules"][1]["destination"],
            json!({"scheme": "https", "host": "azurite.example.com", "path_prefix": "/devstore"})
        );
        // No account, no destination.
        assert_eq!(
            super::policies("azure_blob_storage", &json!({}), &Allowances::default()),
            None
        );
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
            ..Default::default()
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
        for integration_id in ["sftp", "nope"] {
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
        assert!(injected_headers("nope", &json!({})).is_empty());
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

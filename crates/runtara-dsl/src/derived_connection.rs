// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Connection types an agent declares on top of a generic base type.
//!
//! A component cannot add a connection type to the host: connection types
//! are compiled into runtara because the host, not the component, holds the
//! credentials. What a component *can* do is declare a **derived** type: a
//! branded preset of one of the generic types ([`DERIVABLE_BASES`]). The
//! user then connects "Example CRM", with Example CRM's fields, rather than a generic
//! "HTTP Basic" connection whose base URL they must work out themselves.
//!
//! ```json
//! "connectionTypes": [{
//!   "integrationId": "example_crm",
//!   "displayName": "Example CRM",
//!   "category": "crm",
//!   "basedOn": "http_basic",
//!   "baseUrl": "https://{account}.vendor.example/api",
//!   "params": [{ "name": "account", "displayName": "Account",
//!                "pattern": "^[a-z0-9]([a-z0-9-]*[a-z0-9])?$" }],
//!   "fields": [{ "name": "username", "displayName": "API user" }]
//! }]
//! ```
//!
//! - `params` are fields of the derived type only: they fill the
//!   `baseUrl` template and are never sent anywhere themselves.
//! - `fields` relabel fields of the base type. Base fields that are not
//!   relabelled keep their base labels; secrecy is always the base's.
//! - `fixed` gives base fields a value the user cannot change (an
//!   `http_api_key` base's `header_name`, say, or its `value_format`:
//!   `"Token {}"` where the service wants text around the key).
//! - The base type's `base_url` is never a user field: it is `baseUrl`
//!   with the params substituted.
//!
//! At request time the host turns the stored parameters into the base
//! type's ([`DerivedConnectionType::to_base_parameters`]) and authenticates
//! exactly as the base type does, pinned to the rendered base URL.
//!
//! The declaration lives in the agent's `meta.json` document under
//! `connectionTypes`, beside the [`AgentInfo`](crate::agent_meta::AgentInfo)
//! fields ([`agent_meta_document`], [`AgentMetaExtensions`]).
//!
//! Host safety: the host of `baseUrl` must end in at least two literal
//! labels (`vendor.example`), and every substituted value must match both its
//! declared `pattern` and [`PARAM_VALUE_CHARSET`], so a parameter can choose
//! *which* tenant of the vendor a connection talks to, never *which vendor*.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::url_template::{self, Segment};

/// The generic connection types a derived type may be based on: those that
/// authenticate with headers against one base URL.
pub const DERIVABLE_BASES: &[&str] = &["http_basic", "http_api_key", "http_bearer"];

/// The base field that a derived type always computes from `baseUrl`.
pub const BASE_URL_FIELD: &str = "base_url";

/// The `http_api_key` field that says what the key's header holds: the key
/// alone (`{}`, the default) or with text around it (`Token {}`).
pub const VALUE_FORMAT_FIELD: &str = "value_format";

/// What an absent or empty [`VALUE_FORMAT_FIELD`] means: the key alone.
pub const DEFAULT_VALUE_FORMAT: &str = "{}";

/// Longest [`VALUE_FORMAT_FIELD`] accepted.
pub const MAX_VALUE_FORMAT_LEN: usize = 256;

/// Check a header value format: printable ASCII with `{}`, where the secret
/// goes, exactly once and no other brace.
pub fn check_header_value_format(format: &str) -> Result<(), String> {
    if format.len() > MAX_VALUE_FORMAT_LEN {
        return Err(format!("is longer than {MAX_VALUE_FORMAT_LEN} characters"));
    }
    if !format.bytes().all(|b| (0x20..0x7f).contains(&b)) {
        return Err("may hold printable ASCII characters only".into());
    }
    let Some((before, after)) = format.split_once("{}") else {
        return Err("must contain `{}`, where the key goes".into());
    };
    if before.contains(['{', '}']) || after.contains(['{', '}']) {
        return Err("must contain `{}` once, and no other brace".into());
    }
    if before.starts_with(' ') || after.ends_with(' ') {
        return Err("must not start or end with a space".into());
    }
    Ok(())
}

/// The header value format of `http_api_key` parameters: the stored one,
/// checked, or the default when none is stored.
pub fn header_value_format(params: &Value) -> Result<&str, String> {
    match params.get(VALUE_FORMAT_FIELD) {
        None | Some(Value::Null) => Ok(DEFAULT_VALUE_FORMAT),
        Some(Value::String(format)) if format.trim().is_empty() => Ok(DEFAULT_VALUE_FORMAT),
        Some(Value::String(format)) => check_header_value_format(format).map(|()| format.as_str()),
        Some(_) => Err("must be text".into()),
    }
}

/// Longest `iconUrl` accepted.
pub const MAX_ICON_URL_LEN: usize = 2048;

/// Characters a substituted parameter value may hold, whatever its pattern.
pub const PARAM_VALUE_CHARSET: &str = "^[A-Za-z0-9_-]{1,63}$";

static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new("^[a-z][a-z0-9_]{1,62}$").unwrap());
static PARAM_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("^[a-z][a-z0-9_]{0,62}$").unwrap());
static VALUE_CHARSET: LazyLock<Regex> = LazyLock::new(|| Regex::new(PARAM_VALUE_CHARSET).unwrap());
static LITERAL_LABEL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("^[a-z0-9]([a-z0-9-]*[a-z0-9])?$").unwrap());

/// A connection type declared by an agent as a preset of a generic type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DerivedConnectionType {
    /// The connection type's id (`integration_id`), e.g. `example_crm`.
    pub integration_id: String,
    /// Shown in the connection picker, e.g. `Example CRM`.
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A [`ConnectionCategory`](crate::agent_meta::ConnectionCategory) id;
    /// the base's when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// One of [`DERIVABLE_BASES`].
    pub based_on: String,
    /// `https://` URL whose `{param}` placeholders are filled from `params`.
    pub base_url: String,
    /// Fields of this type that fill `base_url`'s placeholders.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<DerivedParam>,
    /// Relabelled fields of the base type.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<DerivedFieldLabel>,
    /// Base fields with a fixed value, hidden from the user.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fixed: BTreeMap<String, String>,
    /// The connection type's icon: an `https://` URL (a CDN), which clients
    /// load as an image. The server only passes it on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
}

/// A field of a derived type that fills a `baseUrl` placeholder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DerivedParam {
    pub name: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    /// Anchored regex (`^…$`) every value must match. Values must also match
    /// [`PARAM_VALUE_CHARSET`].
    pub pattern: String,
}

/// New labels for a field of the base type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DerivedFieldLabel {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}

impl DerivedConnectionType {
    /// Check everything that does not depend on the base type's fields: ids,
    /// the base, the URL template and its params, patterns and the icon.
    /// [`Self::check_against_base`] checks the rest.
    pub fn validate_shape(&self) -> Result<(), String> {
        let id = &self.integration_id;
        let fail = |message: String| Err(format!("connection type `{id}`: {message}"));
        if !ID.is_match(id) {
            return fail("integrationId must be lowercase snake_case, 2-63 characters".into());
        }
        if self.display_name.trim().is_empty() {
            return fail("displayName is empty".into());
        }
        if !DERIVABLE_BASES.contains(&self.based_on.as_str()) {
            return fail(format!(
                "basedOn `{}` is not one of {DERIVABLE_BASES:?}",
                self.based_on
            ));
        }
        if let Some(category) = &self.category
            && crate::agent_meta::ConnectionCategory::parse(category).is_none()
        {
            return fail(format!("unknown category `{category}`"));
        }

        let mut seen = std::collections::BTreeSet::new();
        for param in &self.params {
            if !PARAM_NAME.is_match(&param.name) {
                return fail(format!("param name `{}` must be snake_case", param.name));
            }
            if !seen.insert(param.name.as_str()) {
                return fail(format!("param `{}` is declared twice", param.name));
            }
            if param.display_name.trim().is_empty() {
                return fail(format!("param `{}` has no displayName", param.name));
            }
            if !(param.pattern.starts_with('^') && param.pattern.ends_with('$')) {
                return fail(format!(
                    "param `{}`: pattern must be anchored with ^…$",
                    param.name
                ));
            }
            Regex::new(&param.pattern)
                .map_err(|e| format!("connection type `{id}`: param `{}`: {e}", param.name))?;
        }
        self.check_template()?;
        // A format the base would refuse fails the declaration, not every
        // connection made with it.
        if self.based_on == "http_api_key"
            && let Some(format) = self.fixed.get(VALUE_FORMAT_FIELD)
            && let Err(problem) = check_header_value_format(format)
        {
            return fail(format!("fixed `{VALUE_FORMAT_FIELD}` {problem}"));
        }

        if let Some(url) = &self.icon_url {
            check_icon_url(url).map_err(|e| format!("connection type `{id}`: iconUrl {e}"))?;
        }
        Ok(())
    }

    /// The template's placeholders are exactly the params; it is https; its
    /// host ends in two or more literal labels.
    fn check_template(&self) -> Result<(), String> {
        let id = &self.integration_id;
        let fail = |message: String| Err(format!("connection type `{id}`: baseUrl {message}"));
        let Some(rest) = self.base_url.strip_prefix("https://") else {
            return fail("must start with https://".into());
        };
        let segments = url_template::segments(&self.base_url);
        let unbalanced = segments.iter().any(|segment| match segment {
            Segment::Literal(text) => text.contains('}'),
            Segment::Placeholder { name, closed } => !closed || name.contains('{'),
        });
        if unbalanced {
            return fail("has an unbalanced brace".into());
        }
        let used = url_template::placeholders(&self.base_url);
        for name in &used {
            if !self.params.iter().any(|p| p.name == *name) {
                return fail(format!("uses `{{{name}}}`, which is not a declared param"));
            }
        }
        for param in &self.params {
            if !used.contains(&param.name.as_str()) {
                return fail(format!("does not use param `{}`", param.name));
            }
        }

        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if authority.contains(['@', ':']) {
            return fail("may not carry user info or a port".into());
        }
        let labels: Vec<&str> = authority.split('.').collect();
        let literal_suffix = labels
            .iter()
            .rev()
            .take_while(|label| LITERAL_LABEL.is_match(label))
            .count();
        if literal_suffix < 2 {
            return fail(
                "host must end in at least two literal labels (e.g. `{account}.vendor.com`)".into(),
            );
        }
        for label in &labels[..labels.len() - literal_suffix] {
            // A label with each placeholder standing in for one character.
            let filled: String = url_template::segments(label)
                .into_iter()
                .map(|segment| match segment {
                    Segment::Literal(text) => text,
                    Segment::Placeholder { .. } => "x",
                })
                .collect();
            if !LITERAL_LABEL.is_match(&filled) {
                return fail(format!("host label `{label}` is not a valid DNS label"));
            }
        }
        if rest.contains(['?', '#']) {
            return fail("may not carry a query or fragment".into());
        }
        Ok(())
    }

    /// Check the declaration against the base type's field names: relabelled
    /// and fixed fields exist, the base has a `base_url`, and no param shadows
    /// a base field.
    pub fn check_against_base(&self, base_fields: &[&str]) -> Result<(), String> {
        let id = &self.integration_id;
        let fail = |message: String| Err(format!("connection type `{id}`: {message}"));
        if !base_fields.contains(&BASE_URL_FIELD) {
            return fail(format!(
                "base `{}` has no `{BASE_URL_FIELD}`",
                self.based_on
            ));
        }
        for label in &self.fields {
            if !base_fields.contains(&label.name.as_str()) || label.name == BASE_URL_FIELD {
                return fail(format!(
                    "relabels `{}`, which is not a user field of `{}`",
                    label.name, self.based_on
                ));
            }
        }
        for name in self.fixed.keys() {
            if !base_fields.contains(&name.as_str()) || name == BASE_URL_FIELD {
                return fail(format!(
                    "fixes `{name}`, which is not a user field of `{}`",
                    self.based_on
                ));
            }
            if self.fields.iter().any(|label| &label.name == name) {
                return fail(format!("both fixes and relabels `{name}`"));
            }
        }
        for param in &self.params {
            if base_fields.contains(&param.name.as_str()) {
                return fail(format!(
                    "param `{}` shadows a field of `{}`",
                    param.name, self.based_on
                ));
            }
        }
        Ok(())
    }

    /// Check one stored value of `param` (present, matching its pattern and
    /// [`PARAM_VALUE_CHARSET`]).
    pub fn check_param_value(param: &DerivedParam, value: Option<&str>) -> Result<(), String> {
        let value = value.map(str::trim).unwrap_or_default();
        if value.is_empty() {
            return Err(format!("{} is required", param.display_name));
        }
        let pattern = Regex::new(&param.pattern).map_err(|e| e.to_string())?;
        if !VALUE_CHARSET.is_match(value) || !pattern.is_match(value) {
            return Err(format!("{} has an invalid value", param.display_name));
        }
        Ok(())
    }

    /// `base_url` with every param substituted from `params`. Fails closed
    /// when any value is missing or invalid.
    pub fn render_base_url(&self, params: &Value) -> Result<String, String> {
        // Check every value before any is placed in the URL, then substitute
        // the checked (trimmed) values only.
        let mut checked = Map::new();
        for param in &self.params {
            let value = params.get(&param.name).and_then(Value::as_str);
            Self::check_param_value(param, value)?;
            checked.insert(
                param.name.clone(),
                Value::String(value.unwrap_or_default().trim().to_string()),
            );
        }
        url_template::substitute(&self.base_url, &Value::Object(checked))
            .map_err(|name| format!("baseUrl placeholder `{{{name}}}` has no value"))
    }

    /// The parameters of the base type for a connection stored with
    /// `params`: base fields as stored, fixed fields fixed, `base_url`
    /// rendered, and this type's own params dropped.
    pub fn to_base_parameters(&self, params: &Value) -> Result<Value, String> {
        let base_url = self.render_base_url(params)?;
        let mut base: Map<String, Value> = params
            .as_object()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|(key, _)| !self.params.iter().any(|p| &p.name == key))
            .collect();
        for (name, value) in &self.fixed {
            base.insert(name.clone(), Value::String(value.clone()));
        }
        base.insert(BASE_URL_FIELD.into(), Value::String(base_url));
        Ok(Value::Object(base))
    }
}

/// An icon URL is an absolute `https://` URL without credentials, a port,
/// a query or a fragment.
fn check_icon_url(url: &str) -> Result<(), String> {
    if url.len() > MAX_ICON_URL_LEN {
        return Err(format!("is longer than {MAX_ICON_URL_LEN} characters"));
    }
    let Some(rest) = url.strip_prefix("https://") else {
        return Err("must start with https://".into());
    };
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("contains whitespace".into());
    }
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty() || authority.contains(['@', ':']) {
        return Err("must name a host, without user info or a port".into());
    }
    if rest.contains(['?', '#']) {
        return Err("may not carry a query or fragment".into());
    }
    if !rest.contains('/') || rest.ends_with('/') {
        return Err("must name a file".into());
    }
    Ok(())
}

/// The sections of an agent's `meta.json` beyond its
/// [`AgentInfo`](crate::agent_meta::AgentInfo). Absent sections are empty,
/// so every existing `meta.json` parses.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMetaExtensions {
    #[serde(default)]
    pub connection_types: Vec<DerivedConnectionType>,
}

impl AgentMetaExtensions {
    /// Read the extension sections of a `meta.json` document.
    pub fn from_meta_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

/// The `meta.json` document of an agent that declares connection types:
/// `info`'s fields plus `connectionTypes`. Every declaration's shape is
/// validated, so a bad declaration fails the agent's build, not the server.
pub fn agent_meta_document(
    info: &crate::agent_meta::AgentInfo,
    connection_types: &[DerivedConnectionType],
) -> Result<Value, String> {
    for declared in connection_types {
        declared.validate_shape()?;
    }
    let mut document = serde_json::to_value(info).map_err(|e| e.to_string())?;
    if !connection_types.is_empty() {
        let object = document
            .as_object_mut()
            .ok_or("AgentInfo did not serialize to an object")?;
        object.insert(
            "connectionTypes".into(),
            serde_json::to_value(connection_types).map_err(|e| e.to_string())?,
        );
    }
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn example_crm() -> DerivedConnectionType {
        serde_json::from_value(json!({
            "integrationId": "example_crm",
            "displayName": "Example CRM",
            "category": "crm",
            "basedOn": "http_basic",
            "baseUrl": "https://{account}.vendor.example/api",
            "params": [{
                "name": "account", "displayName": "Account",
                "pattern": "^[a-z0-9]([a-z0-9-]*[a-z0-9])?$"
            }],
            "fields": [{"name": "username", "displayName": "API user"}],
            "iconUrl": "https://cdn.example.com/icons/example-crm.svg"
        }))
        .unwrap()
    }

    fn with(edit: impl FnOnce(&mut DerivedConnectionType)) -> DerivedConnectionType {
        let mut declared = example_crm();
        edit(&mut declared);
        declared
    }

    fn shape_error(declared: DerivedConnectionType) -> String {
        declared.validate_shape().unwrap_err()
    }

    #[test]
    fn a_well_formed_declaration_is_accepted() {
        example_crm().validate_shape().unwrap();
        example_crm()
            .check_against_base(&["username", "password", "base_url"])
            .unwrap();
    }

    #[test]
    fn the_base_must_be_derivable() {
        let error = shape_error(with(|d| {
            d.based_on = "http_oauth2_authorization_code".into()
        }));
        assert!(error.contains("basedOn"), "{error}");
    }

    #[test]
    fn the_host_must_end_in_literal_labels() {
        for url in [
            "https://{account}/api",
            "https://{account}.com/api",
            "https://api.{account}.com",
            "https://vendor.{account}",
        ] {
            let error = shape_error(with(|d| d.base_url = url.into()));
            assert!(error.contains("baseUrl"), "{url}: {error}");
        }
        with(|d| d.base_url = "https://api.vendor.example/{account}/v1".into())
            .validate_shape()
            .unwrap();
    }

    #[test]
    fn the_template_must_be_https_without_credentials_port_or_query() {
        for url in [
            "http://{account}.vendor.example",
            "https://user@{account}.vendor.example",
            "https://{account}.vendor.example:8443",
            "https://{account}.vendor.example/api?x=1",
        ] {
            assert!(
                with(|d| d.base_url = url.into()).validate_shape().is_err(),
                "{url}"
            );
        }
    }

    #[test]
    fn placeholders_and_params_must_match_exactly() {
        let error = shape_error(with(|d| {
            d.base_url = "https://{tenant}.vendor.example".into()
        }));
        assert!(error.contains("`{tenant}`"), "{error}");
        let error = shape_error(with(|d| d.base_url = "https://api.vendor.example".into()));
        assert!(error.contains("does not use param"), "{error}");
    }

    #[test]
    fn patterns_must_be_anchored_regexes() {
        let error = shape_error(with(|d| d.params[0].pattern = "[a-z]+".into()));
        assert!(error.contains("anchored"), "{error}");
        assert!(
            with(|d| d.params[0].pattern = "^[a-z+$".into())
                .validate_shape()
                .is_err()
        );
    }

    #[test]
    fn icon_urls_must_be_plain_https_files() {
        for url in [
            "http://cdn.example.com/example_crm.svg",
            "https://user@cdn.example.com/example_crm.svg",
            "https://cdn.example.com:8443/example_crm.svg",
            "https://cdn.example.com/example_crm.svg?v=1",
            "https://cdn.example.com/",
            "https://cdn.example.com",
            "javascript:alert(1)",
            "data:image/svg+xml,<svg/>",
            "https://cdn.example.com/a b.svg",
        ] {
            let error = shape_error(with(|d| d.icon_url = Some(url.into())));
            assert!(error.contains("iconUrl"), "{url}: {error}");
        }
        let long = format!(
            "https://cdn.example.com/{}.svg",
            "a".repeat(MAX_ICON_URL_LEN)
        );
        assert!(shape_error(with(|d| d.icon_url = Some(long))).contains("longer"));
        with(|d| d.icon_url = None).validate_shape().unwrap();
    }

    #[test]
    fn declarations_must_fit_the_base() {
        let base = ["username", "password", "base_url"];
        let error = with(|d| d.fields[0].name = "token".into())
            .check_against_base(&base)
            .unwrap_err();
        assert!(error.contains("relabels `token`"), "{error}");
        let error = with(|d| d.fields[0].name = "base_url".into())
            .check_against_base(&base)
            .unwrap_err();
        assert!(error.contains("relabels `base_url`"), "{error}");
        let error = with(|d| {
            d.fixed
                .insert("base_url".into(), "https://x.example.com".into());
        })
        .check_against_base(&base)
        .unwrap_err();
        assert!(error.contains("fixes `base_url`"), "{error}");
        let error = with(|d| {
            d.params[0].name = "username".into();
            d.base_url = "https://{username}.vendor.example".into();
        })
        .check_against_base(&base)
        .unwrap_err();
        assert!(error.contains("shadows"), "{error}");
    }

    #[test]
    fn a_header_value_format_holds_the_key_once() {
        for format in ["{}", "Token {}", "Device {}:project", "{}:suffix"] {
            assert_eq!(check_header_value_format(format), Ok(()), "{format}");
        }
        for format in [
            "",
            "Token",
            "{} {}",
            "Token {name}",
            "a}{}",
            " {}",
            "{} ",
            "Token\t{}",
            "Tok\u{e9}n {}",
        ] {
            assert!(check_header_value_format(format).is_err(), "{format:?}");
        }
        let long = format!("{}{{}}", "a".repeat(MAX_VALUE_FORMAT_LEN));
        assert!(check_header_value_format(&long).is_err());

        assert_eq!(header_value_format(&json!({})), Ok("{}"));
        assert_eq!(header_value_format(&json!({"value_format": " "})), Ok("{}"));
        assert_eq!(
            header_value_format(&json!({"value_format": "Token {}"})),
            Ok("Token {}")
        );
        assert!(header_value_format(&json!({"value_format": "Token"})).is_err());
        assert!(header_value_format(&json!({"value_format": 1})).is_err());
    }

    #[test]
    fn a_fixed_value_format_is_checked_with_the_declaration() {
        let api_key = |format: &str| {
            with(|d| {
                d.based_on = "http_api_key".into();
                d.fields.clear();
                d.fixed.insert("header_name".into(), "Authorization".into());
                d.fixed.insert(VALUE_FORMAT_FIELD.into(), format.into());
            })
        };
        assert_eq!(api_key("Device {}:project").validate_shape(), Ok(()));
        let error = shape_error(api_key("Device"));
        assert!(
            error.contains("fixed `value_format` must contain"),
            "{error}"
        );
    }

    #[test]
    fn stored_parameters_become_the_base_parameters() {
        let declared = with(|d| {
            d.fixed.insert("realm".into(), "api".into());
        });
        let base = declared
            .to_base_parameters(&json!({
                "account": "acme", "username": "api_user", "password": "secret"
            }))
            .unwrap();
        assert_eq!(
            base,
            json!({
                "username": "api_user", "password": "secret", "realm": "api",
                "base_url": "https://acme.vendor.example/api"
            })
        );
    }

    #[test]
    fn a_param_value_cannot_change_the_host() {
        for value in [
            "evil.com#",
            "acme.evil",
            "acme/../x",
            "a@b",
            "acme:443",
            "",
            "-acme",
            "%2e",
        ] {
            let error = example_crm()
                .render_base_url(&json!({ "account": value }))
                .unwrap_err();
            assert!(error.starts_with("Account"), "{value}: {error}");
        }
        assert!(example_crm().render_base_url(&json!({})).is_err());
    }

    #[test]
    fn a_stored_value_cannot_override_the_rendered_base_url() {
        let base = example_crm()
            .to_base_parameters(&json!({
                "account": "acme", "base_url": "https://evil.example.com"
            }))
            .unwrap();
        assert_eq!(base["base_url"], "https://acme.vendor.example/api");
    }

    #[test]
    fn meta_documents_carry_connection_types_beside_agent_info() {
        let info = crate::agent_meta::AgentInfo {
            id: "example_crm".into(),
            name: "Example CRM".into(),
            description: "d".into(),
            has_side_effects: true,
            supports_connections: true,
            integration_ids: vec!["example_crm".into()],
            capabilities: vec![],
        };
        let document = agent_meta_document(&info, &[example_crm()]).unwrap();
        let bytes = serde_json::to_vec(&document).unwrap();
        // Still a valid AgentInfo for every existing reader…
        let parsed: crate::agent_meta::AgentInfo = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed.id, "example_crm");
        // …and the extension reads back.
        let extensions = AgentMetaExtensions::from_meta_json(&bytes).unwrap();
        assert_eq!(extensions.connection_types, vec![example_crm()]);
        // An agent without the section parses to none.
        let plain = serde_json::to_vec(&info).unwrap();
        assert!(
            AgentMetaExtensions::from_meta_json(&plain)
                .unwrap()
                .connection_types
                .is_empty()
        );
        // A bad declaration fails the build.
        assert!(agent_meta_document(&info, &[with(|d| d.based_on = "x".into())]).is_err());
    }
}

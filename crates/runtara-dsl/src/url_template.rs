// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! `{param}` templates filled from a connection's parameters.
//!
//! One syntax and one implementation for every connection URL template: OAuth
//! base URL path templates (`/v3/company/{realm_id}`), named endpoints, and the
//! base URLs of connection types agents declare
//! ([`crate::derived_connection`]).
//!
//! A placeholder is `{name}`; its value is the string parameter `name`. An
//! unclosed `{` makes the rest of the template the placeholder's name, and a
//! `}` outside a placeholder is literal text. Callers that need well-formed
//! templates check [`segments`] for an unclosed placeholder or a stray `}`.
//!
//! Substitution does not validate values: a caller that places values where
//! they could change the meaning of the URL (a host label, say) checks them
//! first, as declared connection types do.

use serde_json::Value;

/// A piece of a template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Segment<'a> {
    /// Text copied as is.
    Literal(&'a str),
    /// A placeholder's name; `closed` is false for an unclosed `{` (the name
    /// then runs to the end of the template).
    Placeholder { name: &'a str, closed: bool },
}

/// The template split into literal text and placeholders.
pub fn segments(template: &str) -> Vec<Segment<'_>> {
    let mut segments = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        if open > 0 {
            segments.push(Segment::Literal(&rest[..open]));
        }
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                segments.push(Segment::Placeholder {
                    name: &after[..close],
                    closed: true,
                });
                rest = &after[close + 1..];
            }
            None => {
                segments.push(Segment::Placeholder {
                    name: after,
                    closed: false,
                });
                rest = "";
            }
        }
    }
    if !rest.is_empty() {
        segments.push(Segment::Literal(rest));
    }
    segments
}

/// The placeholder names of `template`, in order.
pub fn placeholders(template: &str) -> Vec<&str> {
    segments(template)
        .into_iter()
        .filter_map(|segment| match segment {
            Segment::Placeholder { name, .. } => Some(name),
            Segment::Literal(_) => None,
        })
        .collect()
}

/// `template` with every placeholder replaced by its parameter. Fails with the
/// placeholder's name when a parameter is missing, not a string, or empty: a
/// URL that silently loses a segment would address something else.
pub fn substitute(template: &str, params: &Value) -> Result<String, String> {
    let mut out = String::with_capacity(template.len());
    for segment in segments(template) {
        match segment {
            Segment::Literal(text) => out.push_str(text),
            Segment::Placeholder { name, .. } => {
                match params[name].as_str().filter(|value| !value.is_empty()) {
                    Some(value) => out.push_str(value),
                    None => return Err(name.to_string()),
                }
            }
        }
    }
    Ok(out)
}

/// `template` with every placeholder replaced by its parameter, or by nothing
/// when the parameter is missing or not a string. For path templates, where a
/// missing value yields an obviously broken path the provider rejects.
pub fn substitute_or_empty(template: &str, params: &Value) -> String {
    let mut out = String::with_capacity(template.len());
    for segment in segments(template) {
        match segment {
            Segment::Literal(text) => out.push_str(text),
            Segment::Placeholder { name, .. } => {
                if let Some(value) = params[name].as_str() {
                    out.push_str(value);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn templates_split_into_literals_and_placeholders() {
        assert_eq!(
            segments("https://{account}.vendor.example/v3/{id}"),
            vec![
                Segment::Literal("https://"),
                Segment::Placeholder {
                    name: "account",
                    closed: true
                },
                Segment::Literal(".vendor.example/v3/"),
                Segment::Placeholder {
                    name: "id",
                    closed: true
                },
            ]
        );
        assert_eq!(placeholders("/v3/company/{realm_id}"), vec!["realm_id"]);
        assert!(segments("").is_empty());
    }

    #[test]
    fn an_unclosed_placeholder_runs_to_the_end_and_a_stray_close_is_literal() {
        assert_eq!(
            segments("/a/{id/b"),
            vec![
                Segment::Literal("/a/"),
                Segment::Placeholder {
                    name: "id/b",
                    closed: false
                },
            ]
        );
        assert_eq!(segments("/a}b"), vec![Segment::Literal("/a}b")]);
    }

    #[test]
    fn strict_substitution_fills_every_placeholder_or_names_the_missing_one() {
        let params = json!({"realm_id": "123", "empty": "", "number": 5});
        assert_eq!(
            substitute("/v3/company/{realm_id}", &params).unwrap(),
            "/v3/company/123"
        );
        assert_eq!(substitute("/{missing}", &params).unwrap_err(), "missing");
        assert_eq!(substitute("/{empty}", &params).unwrap_err(), "empty");
        assert_eq!(substitute("/{number}", &params).unwrap_err(), "number");
        // An unclosed placeholder names everything up to the end.
        assert_eq!(substitute("/a/{realm_id", &params).unwrap(), "/a/123");
        assert_eq!(
            substitute("/a/{realm_id/b", &params).unwrap_err(),
            "realm_id/b"
        );
        assert_eq!(substitute("/plain}", &params).unwrap(), "/plain}");
    }

    #[test]
    fn lenient_substitution_leaves_missing_values_empty() {
        let params = json!({"realm_id": "123", "empty": ""});
        assert_eq!(
            substitute_or_empty("/v3/company/{realm_id}", &params),
            "/v3/company/123"
        );
        assert_eq!(
            substitute_or_empty("/v3/company/{missing}", &params),
            "/v3/company/"
        );
        assert_eq!(substitute_or_empty("/{empty}/x", &params), "//x");
        assert_eq!(substitute_or_empty("/a/{unclosed", &params), "/a/");
    }

    #[test]
    fn values_are_inserted_verbatim() {
        // Validation is the caller's job; substitution does not escape.
        let params = json!({"v": "a/b?c"});
        assert_eq!(substitute("/x/{v}", &params).unwrap(), "/x/a/b?c");
    }
}

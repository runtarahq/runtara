// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Template rendering for MappingValue::Template.
//!
//! Uses minijinja to render template strings with the full execution context.

use minijinja::{Environment, ErrorKind};

/// Render a minijinja template string with the given JSON context.
///
/// The context should be a JSON object containing the execution state
/// (data, variables, steps, workflow). Template expressions can reference
/// any path in this context using dot notation.
///
/// # Examples
///
/// ```
/// use runtara_workflow_stdlib::serde_json::json;
/// use runtara_workflow_stdlib::template::render_template;
///
/// let ctx = json!({"data": {"name": "World"}});
/// let result = render_template("Hello {{ data.name }}", &ctx).unwrap();
/// assert_eq!(result, "Hello World");
/// ```
pub fn render_template(template_str: &str, context: &serde_json::Value) -> Result<String, String> {
    let mut env = Environment::new();
    env.add_template("__inline", template_str)
        .map_err(|e| format!("Template parse error: {e}"))?;
    let tmpl = env
        .get_template("__inline")
        .map_err(|e| format!("Template retrieval error: {e}"))?;
    tmpl.render(context)
        .map_err(|e| format!("Template render error: {e}{}", unknown_helper_hint(&e)))
}

/// When a render fails because the template referenced a filter/function/test/
/// method the Runtara template sandbox does not provide, append a pointer to the
/// supported-helper list so authors don't discover the gap one deploy-execute
/// cycle at a time. See `docs/templating.md` (SYN-449).
fn unknown_helper_hint(error: &minijinja::Error) -> &'static str {
    match error.kind() {
        ErrorKind::UnknownFilter
        | ErrorKind::UnknownFunction
        | ErrorKind::UnknownTest
        | ErrorKind::UnknownMethod => {
            " — this helper is not available in Runtara templates; see docs/templating.md \
             for the supported filters and functions. Note `now()` and `joiner()` are not \
             provided (use the datetime agent for timestamps)."
        }
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_simple_interpolation() {
        let ctx = json!({"data": {"name": "World"}});
        let result = render_template("Hello {{ data.name }}", &ctx).unwrap();
        assert_eq!(result, "Hello World");
    }

    #[test]
    fn test_nested_path() {
        let ctx = json!({
            "steps": {
                "my_conn": {
                    "outputs": {
                        "parameters": {
                            "api_key": "sk-12345"
                        }
                    }
                }
            }
        });
        let result = render_template(
            "Bearer {{ steps.my_conn.outputs.parameters.api_key }}",
            &ctx,
        )
        .unwrap();
        assert_eq!(result, "Bearer sk-12345");
    }

    #[test]
    fn test_filter_upper() {
        let ctx = json!({"data": {"name": "hello"}});
        let result = render_template("{{ data.name | upper }}", &ctx).unwrap();
        assert_eq!(result, "HELLO");
    }

    #[test]
    fn test_filter_default() {
        let ctx = json!({"data": {}});
        let result = render_template("{{ data.missing | default('N/A') }}", &ctx).unwrap();
        assert_eq!(result, "N/A");
    }

    #[test]
    fn test_multiple_interpolations() {
        let ctx = json!({"data": {"first": "John", "last": "Doe"}});
        let result = render_template("{{ data.first }} {{ data.last }}", &ctx).unwrap();
        assert_eq!(result, "John Doe");
    }

    #[test]
    fn test_conditional() {
        let ctx = json!({"data": {"active": true}});
        let result =
            render_template("{% if data.active %}yes{% else %}no{% endif %}", &ctx).unwrap();
        assert_eq!(result, "yes");
    }

    #[test]
    fn test_syntax_error() {
        let ctx = json!({});
        let result = render_template("{{ unclosed", &ctx);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("parse error"));
    }

    #[test]
    fn test_missing_variable_renders_empty() {
        // minijinja renders undefined variables as empty string by default
        let ctx = json!({"data": {}});
        let result = render_template("Hello {{ data.nonexistent }}", &ctx).unwrap();
        assert_eq!(result, "Hello ");
    }

    /// SYN-449: the `tojson` filter is available (minijinja `json` feature).
    #[test]
    fn test_tojson_filter_available() {
        let ctx = json!({"data": {"obj": {"a": 1, "b": [2, 3]}}});
        let result = render_template("{{ data.obj | tojson }}", &ctx).unwrap();
        assert_eq!(result, r#"{"a":1,"b":[2,3]}"#);
    }

    /// SYN-449: an unknown helper (e.g. `now()`) fails with a hint pointing at the
    /// supported-helper docs instead of a bare minijinja message.
    #[test]
    fn test_unknown_function_error_mentions_docs() {
        let ctx = json!({});
        let err = render_template("{{ now() }}", &ctx).unwrap_err();
        assert!(err.contains("unknown function"), "{err}");
        assert!(err.contains("docs/templating.md"), "{err}");
    }
}

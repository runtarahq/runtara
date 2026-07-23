// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Template rendering for MappingValue::Template.
//!
//! Uses minijinja to render template strings with the full execution context.

use minijinja::{Environment, ErrorKind};

/// Name under which the single inline template is registered in the
/// environment. Shared by [`render_template`] and [`CompiledTemplate`] so both
/// paths key on the same name.
const INLINE_TEMPLATE_NAME: &str = "__inline";

/// Render a minijinja template string with the given JSON context.
///
/// The context should be a JSON object containing the execution state
/// (data, variables, steps, workflow). Template expressions can reference
/// any path in this context using dot notation.
///
/// This parses the template from scratch on every call. Callers that render the
/// same template repeatedly (e.g. a compiled mapping evaluated per element of a
/// Split/While/Filter body) should parse it once with [`CompiledTemplate::parse`]
/// and reuse the result instead.
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
    env.add_template(INLINE_TEMPLATE_NAME, template_str)
        .map_err(|e| format!("Template parse error: {e}"))?;
    let tmpl = env
        .get_template(INLINE_TEMPLATE_NAME)
        .map_err(|e| format!("Template retrieval error: {e}"))?;
    tmpl.render(context)
        .map_err(|e| format!("Template render error: {e}{}", unknown_helper_hint(&e)))
}

/// A minijinja template lexed and compiled once, ready to render many times
/// without re-parsing the source.
///
/// [`render_template`] rebuilds an [`Environment`] and re-parses the template on
/// every call. Where the same template is rendered repeatedly — the compiled
/// mapping fast path evaluates one per element/iteration of a Split/While/Filter
/// body — that parse cost is paid on every element. `CompiledTemplate` hoists the
/// parse out of the hot loop: [`parse`](CompiledTemplate::parse) compiles the
/// template once (minijinja compiles eagerly at `add_template_owned` time), and
/// each [`render`](CompiledTemplate::render) reuses the pre-compiled instructions.
///
/// Rendering is byte-identical to [`render_template`]: same engine, same error
/// text (including the unknown-helper hint).
#[derive(Debug, Clone)]
pub struct CompiledTemplate {
    /// Owns the single compiled template, registered under
    /// [`INLINE_TEMPLATE_NAME`]. `'static` because the source is owned (via
    /// `add_template_owned`), so no borrow ties the environment to the caller.
    env: Environment<'static>,
}

impl CompiledTemplate {
    /// Parse and compile `template_str` up front.
    ///
    /// On a syntax error this returns the same `"Template parse error: …"`
    /// message [`render_template`] would produce at render time, so callers that
    /// defer parse failures keep identical error text.
    pub fn parse(template_str: &str) -> Result<Self, String> {
        let mut env = Environment::new();
        env.add_template_owned(INLINE_TEMPLATE_NAME, template_str.to_owned())
            .map_err(|e| format!("Template parse error: {e}"))?;
        Ok(Self { env })
    }

    /// Render the pre-compiled template with `context`.
    ///
    /// Reuses the instructions compiled by [`parse`](CompiledTemplate::parse) —
    /// no re-lex, no re-parse. Errors match [`render_template`].
    pub fn render(&self, context: &serde_json::Value) -> Result<String, String> {
        let tmpl = self
            .env
            .get_template(INLINE_TEMPLATE_NAME)
            .map_err(|e| format!("Template retrieval error: {e}"))?;
        tmpl.render(context)
            .map_err(|e| format!("Template render error: {e}{}", unknown_helper_hint(&e)))
    }
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

    /// A `CompiledTemplate` parses once and renders many times, producing a fresh
    /// result for each distinct context.
    #[test]
    fn test_compiled_template_renders_repeatedly() {
        let tmpl = CompiledTemplate::parse("Hello {{ data.name }}").unwrap();
        assert_eq!(
            tmpl.render(&json!({"data": {"name": "World"}})).unwrap(),
            "Hello World"
        );
        assert_eq!(
            tmpl.render(&json!({"data": {"name": "Ada"}})).unwrap(),
            "Hello Ada"
        );
        // Re-rendering the original context still works (no per-render mutation).
        assert_eq!(
            tmpl.render(&json!({"data": {"name": "World"}})).unwrap(),
            "Hello World"
        );
    }

    /// The pre-parsed path is byte-identical to `render_template` — same engine,
    /// same filters, same output.
    #[test]
    fn test_compiled_template_matches_render_template() {
        let cases = [
            ("{{ data.name | upper }}", json!({"data": {"name": "hi"}})),
            (
                "{{ data.obj | tojson }}",
                json!({"data": {"obj": {"a": 1}}}),
            ),
            (
                "{% if data.active %}yes{% else %}no{% endif %}",
                json!({"data": {"active": false}}),
            ),
            ("Hello {{ data.missing }}", json!({"data": {}})),
        ];
        for (src, ctx) in cases {
            let compiled = CompiledTemplate::parse(src).unwrap();
            assert_eq!(
                compiled.render(&ctx).unwrap(),
                render_template(src, &ctx).unwrap(),
                "mismatch for template {src:?}"
            );
        }
    }

    /// A syntax error surfaces at `parse` time with the same `Template parse
    /// error` message `render_template` produces, so callers that defer the
    /// failure keep identical text.
    #[test]
    fn test_compiled_template_parse_error() {
        let err = CompiledTemplate::parse("{{ unclosed").unwrap_err();
        assert!(err.contains("Template parse error"), "{err}");
        assert_eq!(
            err,
            render_template("{{ unclosed", &json!({})).unwrap_err(),
            "parse-time error text must match render_template"
        );
    }

    /// An unknown helper parses fine but fails at render time, and the compiled
    /// path still appends the supported-helper hint.
    #[test]
    fn test_compiled_template_render_error_mentions_docs() {
        let tmpl = CompiledTemplate::parse("{{ now() }}").unwrap();
        let err = tmpl.render(&json!({})).unwrap_err();
        assert!(err.contains("unknown function"), "{err}");
        assert!(err.contains("docs/templating.md"), "{err}");
    }
}

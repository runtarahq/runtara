//! Display-template and viewer-template rendering, backed by `minijinja`.
//!
//! Replaces the three hand-rolled `{{ field | format:arg }}` parsers across
//! the old server (`services/reports.rs`), the MCP authoring tools
//! (`mcp/tools/reports.rs`), and the FE's `utils.ts`.
//!
//! The crate stays locale-agnostic: filters dispatch to whatever
//! [`Formatter`] is plugged in. The browser supplies a JS-backed
//! formatter (uses `Intl`); the server supplies [`SimpleAsciiFormatter`]
//! or any future locale-aware impl. See `format.rs` for the contract.

use std::sync::Arc;

use minijinja::{Environment, Value as MJValue, value::ViaDeserialize};
use serde_json::Value;
use thiserror::Error;

use crate::format::{FormatSpec, Formatter, RenderContext, SimpleAsciiFormatter};

#[derive(Debug, Error)]
pub enum TemplateError {
    #[error("template parse failed: {0}")]
    Parse(String),
    #[error("template render failed: {0}")]
    Render(String),
}

/// Render a template string against a row, using the supplied formatter
/// for any `{{ field | format }}` placeholders.
///
/// The `ctx` carries locale + currency + timezone for the formatter.
pub fn render_template(
    template: &str,
    row: &Value,
    ctx: &RenderContext,
    formatter: Arc<dyn Formatter>,
) -> Result<String, TemplateError> {
    render_template_with_extras(template, row, ctx, formatter, |_| {})
}

/// Render with the report filters plus a caller-supplied filter closure.
/// Useful when a caller needs to add a request-scoped filter (e.g. a
/// timezone-aware date renderer) without rebuilding the whole filter set.
pub fn render_template_with_extras<F>(
    template: &str,
    row: &Value,
    ctx: &RenderContext,
    formatter: Arc<dyn Formatter>,
    extra: F,
) -> Result<String, TemplateError>
where
    F: FnOnce(&mut Environment<'static>),
{
    let mut env = make_environment(ctx.clone(), formatter);
    extra(&mut env);
    let tmpl = env
        .template_from_str(template)
        .map_err(|e| TemplateError::Parse(e.to_string()))?;
    tmpl.render(row)
        .map_err(|e| TemplateError::Render(e.to_string()))
}

/// Build a minijinja environment with the report filter set registered
/// and the supplied formatter bound to each filter. The environment is
/// sandboxed — no `include`, no `extends`, no `import`.
pub fn make_environment(ctx: RenderContext, formatter: Arc<dyn Formatter>) -> Environment<'static> {
    let mut env = Environment::new();
    register_report_filters(&mut env, ctx, formatter);
    env
}

/// A no-op `Environment` for save-time validation that doesn't render
/// anything. Filters are registered but use the simple ASCII formatter
/// — sufficient for parse-time validation since the parser doesn't
/// invoke filters.
pub fn make_validation_environment() -> Environment<'static> {
    make_environment(RenderContext::default(), Arc::new(SimpleAsciiFormatter))
}

/// Register the report-DSL format filters on a minijinja environment.
/// Each filter delegates to `formatter.format(value, spec, ctx)` so
/// host-provided locale-aware formatters take effect.
pub fn register_report_filters(
    env: &mut Environment<'static>,
    ctx: RenderContext,
    formatter: Arc<dyn Formatter>,
) {
    register_filter(env, "currency", ctx.clone(), formatter.clone(), |arg| {
        FormatSpec::Currency { code: arg }
    });
    register_filter(
        env,
        "currency_compact",
        ctx.clone(),
        formatter.clone(),
        |arg| FormatSpec::CurrencyCompact { code: arg },
    );
    register_filter(env, "number", ctx.clone(), formatter.clone(), |_| {
        FormatSpec::Number
    });
    register_filter(
        env,
        "number_compact",
        ctx.clone(),
        formatter.clone(),
        |_| FormatSpec::NumberCompact,
    );
    register_filter(env, "decimal", ctx.clone(), formatter.clone(), |_| {
        FormatSpec::Decimal
    });
    register_filter(env, "percent", ctx.clone(), formatter.clone(), |_| {
        FormatSpec::Percent
    });
    register_filter(env, "bytes", ctx.clone(), formatter.clone(), |_| {
        FormatSpec::Bytes
    });
    register_filter(env, "date", ctx.clone(), formatter.clone(), |_| {
        FormatSpec::Date
    });
    register_filter(env, "datetime", ctx.clone(), formatter.clone(), |_| {
        FormatSpec::Datetime
    });
    register_filter(env, "pill", ctx.clone(), formatter.clone(), |_| {
        FormatSpec::Pill
    });
    register_filter(env, "bar_indicator", ctx.clone(), formatter.clone(), |_| {
        FormatSpec::BarIndicator
    });
    // `string` and `raw` route through the same `Formatter` as `format_value`
    // so `{{ x | string }}` / `{{ x | raw }}` and `format: 'string' | 'raw'`
    // agree. `string` overrides minijinja's built-in filter of the same name;
    // `raw` isn't a minijinja filter at all, so without this the pipe would
    // throw "unknown filter" at render despite validating green.
    register_filter(env, "string", ctx.clone(), formatter.clone(), |_| {
        FormatSpec::String
    });
    register_filter(env, "raw", ctx, formatter, |_| FormatSpec::Raw);
}

/// Helper: register a one-or-two-arg filter that delegates to the
/// configured formatter. The `spec_from_arg` closure picks the
/// `FormatSpec` variant given the optional filter argument (e.g.
/// `currency:eur` → `FormatSpec::Currency { code: Some("eur") }`).
fn register_filter(
    env: &mut Environment<'static>,
    name: &'static str,
    ctx: RenderContext,
    formatter: Arc<dyn Formatter>,
    spec_from_arg: fn(Option<String>) -> FormatSpec,
) {
    let filter_closure = move |value: ViaDeserialize<Value>, arg: Option<String>| -> String {
        let spec = spec_from_arg(arg);
        formatter.format(&value.0, &spec, &ctx)
    };
    env.add_filter(name, filter_closure);
}

/// One-shot value formatting outside a template — same dispatch path as
/// the template filters, so output is consistent between
/// `{{ x | currency }}` placeholders and raw `format: 'currency'` cell
/// renderers.
pub fn format_value(
    value: &Value,
    format: &str,
    ctx: &RenderContext,
    formatter: &dyn Formatter,
) -> String {
    let spec = FormatSpec::parse(format);
    formatter.format(value, &spec, ctx)
}

/// Convenience wrapper for save-time validation. Returns `Ok(())` on
/// successful parse; `Err(TemplateError::Parse)` otherwise. Does not
/// require a formatter — parse errors surface before any filter is
/// invoked.
pub fn validate_template(template: &str) -> Result<(), TemplateError> {
    let env = make_validation_environment();
    env.template_from_str(template)
        .map(|_| ())
        .map_err(|e| TemplateError::Parse(e.to_string()))
}

/// Safe-subset validator for display templates: accepts only
/// `{{field.path}}` and `{{field.path | format[:arg]}}` shapes. Used at
/// save time as a guardrail in addition to the regular minijinja parse,
/// so dangerous patterns (arithmetic, control-flow blocks, function
/// calls) cannot land in stored report definitions even though the
/// minijinja parser would accept them.
///
/// Returns a short static reason on failure; callers wrap it with their
/// own context string when surfacing to users.
pub fn validate_safe_display_template(template: &str) -> Result<(), &'static str> {
    let mut cursor = 0;
    while cursor < template.len() {
        let open = find_from(template, "{{", cursor);
        let close = find_from(template, "}}", cursor);
        if close.is_some_and(|close| open.is_none_or(|open| close < open)) {
            return Err("unexpected close delimiter");
        }
        let Some(open) = open else {
            return Ok(());
        };
        let Some(close) = find_from(template, "}}", open + 2) else {
            return Err("unclosed variable");
        };

        let token = template[open + 2..close].trim();
        validate_display_template_token(token)?;
        cursor = close + 2;
    }
    Ok(())
}

fn validate_display_template_token(token: &str) -> Result<(), &'static str> {
    if token.is_empty() {
        return Err("empty variable");
    }
    if token.contains("{{") || token.contains("}}") {
        return Err("nested variables are not supported");
    }

    let parts = token.split('|').collect::<Vec<_>>();
    match parts.as_slice() {
        [field] => validate_display_template_field(field.trim()),
        [field, format] => {
            validate_display_template_field(field.trim())?;
            validate_display_template_format(format.trim())
        }
        _ => Err("only one format pipe is supported"),
    }
}

fn validate_display_template_field(field: &str) -> Result<(), &'static str> {
    let field = field.strip_prefix("row.").unwrap_or(field);
    let mut parts = field.split('.');
    let Some(first) = parts.next().filter(|part| !part.is_empty()) else {
        return Err("field path is empty");
    };
    if !is_identifier_part(first) {
        return Err("field path is invalid");
    }
    for part in parts {
        if part.is_empty() {
            return Err("field path is invalid");
        }
        if part.chars().all(|ch| ch.is_ascii_digit()) {
            continue;
        }
        if !is_identifier_part(part) {
            return Err("field path is invalid");
        }
    }
    Ok(())
}

fn validate_display_template_format(format: &str) -> Result<(), &'static str> {
    if format.is_empty() {
        return Err("format is empty");
    }
    let mut parts = format.split(':');
    let Some(name) = parts.next() else {
        return Err("format is invalid");
    };
    if !is_identifier_part(name) {
        return Err("format is invalid");
    }
    // Reject identifier-shaped-but-unknown names (e.g. `foobar`) here rather
    // than letting them save and throw "unknown filter" at render time. The
    // known set is shared with the render-time filter registry via `FormatSpec`.
    if !FormatSpec::is_known_name(name) {
        return Err("unknown format");
    }
    if let Some(argument) = parts.next()
        && (argument.is_empty()
            || !argument
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'))
    {
        return Err("format is invalid");
    }
    if parts.next().is_some() {
        return Err("format is invalid");
    }
    Ok(())
}

fn is_identifier_part(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn find_from(value: &str, pattern: &str, cursor: usize) -> Option<usize> {
    value[cursor..].find(pattern).map(|index| cursor + index)
}

// Keep `MJValue` referenced so future filters can use it without a
// stale-import lint. Today only `ViaDeserialize<Value>` is used.
#[allow(dead_code)]
fn _mjvalue_anchor(_: &MJValue) {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn render(template: &str, row: &Value) -> String {
        render_template(
            template,
            row,
            &RenderContext {
                locale: "en-US".into(),
                currency: "USD".into(),
                timezone: "UTC".into(),
            },
            Arc::new(SimpleAsciiFormatter),
        )
        .unwrap()
    }

    #[test]
    fn render_plain_field() {
        assert_eq!(
            render("Hello {{ name }}", &json!({ "name": "world" })),
            "Hello world"
        );
    }

    #[test]
    fn render_currency_filter() {
        assert_eq!(
            render(
                "Total: {{ amount | currency }}",
                &json!({ "amount": 1234.5 })
            ),
            "Total: $1,234.50"
        );
    }

    #[test]
    fn render_currency_filter_with_arg() {
        assert_eq!(
            render(
                "Total: {{ amount | currency('eur') }}",
                &json!({ "amount": 50.0 })
            ),
            "Total: €50.00"
        );
    }

    #[test]
    fn render_number_filter() {
        assert_eq!(
            render("{{ n | number }}", &json!({ "n": 1234567 })),
            "1,234,567"
        );
        assert_eq!(render("{{ n | number }}", &json!({ "n": 1.5 })), "1.50");
    }

    #[test]
    fn render_percent_filter() {
        assert_eq!(
            render("{{ ratio | percent }}", &json!({ "ratio": 0.123 })),
            "12.3%"
        );
    }

    #[test]
    fn render_bytes_filter() {
        assert_eq!(render("{{ n | bytes }}", &json!({ "n": 1_500 })), "1.50 KB");
        assert_eq!(
            render("{{ n | bytes }}", &json!({ "n": 68_719_476_736_i64 })),
            "68.72 GB"
        );
    }

    #[test]
    fn render_date_filter() {
        assert_eq!(
            render("{{ d | date }}", &json!({ "d": "2026-01-15T08:30:00Z" })),
            "2026-01-15"
        );
    }

    #[test]
    fn render_datetime_filter() {
        assert_eq!(
            render(
                "{{ d | datetime }}",
                &json!({ "d": "2026-01-15T08:30:00.123Z" })
            ),
            "2026-01-15T08:30:00"
        );
    }

    #[test]
    fn render_undefined_field_is_empty() {
        assert_eq!(render("Hello {{ missing }}", &json!({})), "Hello ");
    }

    #[test]
    fn parse_error_is_reported_as_parse() {
        let err = render_template(
            "Unclosed {{ field",
            &json!({}),
            &RenderContext::default(),
            Arc::new(SimpleAsciiFormatter),
        )
        .unwrap_err();
        assert!(matches!(err, TemplateError::Parse(_)));
    }

    #[test]
    fn format_value_matches_template_output() {
        let fmt = SimpleAsciiFormatter;
        let ctx = RenderContext {
            locale: "en-US".into(),
            currency: "USD".into(),
            timezone: "UTC".into(),
        };
        assert_eq!(
            format_value(&json!(1234.5), "currency", &ctx, &fmt),
            "$1,234.50"
        );
        assert_eq!(
            render("{{ x | currency }}", &json!({ "x": 1234.5 })),
            "$1,234.50"
        );
    }

    #[test]
    fn unknown_format_name_is_rejected_at_save() {
        // Regression: before the known-name check these validated green and
        // then threw "unknown filter" at render time.
        assert!(validate_safe_display_template("{{ x | foobar }}").is_err());
        assert!(validate_safe_display_template("{{ amount | dollars }}").is_err());
        // A plain field with no format pipe is still fine.
        assert!(validate_safe_display_template("{{ x }}").is_ok());
    }

    #[test]
    fn every_known_format_name_validates_and_renders() {
        let row = json!({ "v": 1234.5 });
        for name in FormatSpec::KNOWN_NAMES {
            let template = format!("{{{{ v | {name} }}}}");
            // Save-time validation accepts every advertised name...
            validate_safe_display_template(&template)
                .unwrap_or_else(|e| panic!("validator rejected known name `{name}`: {e}"));
            // ...and each name resolves to a registered filter at render time
            // (before this change, `raw` threw "unknown filter" here).
            render_template(
                &template,
                &row,
                &RenderContext::default(),
                Arc::new(SimpleAsciiFormatter),
            )
            .unwrap_or_else(|e| panic!("render failed for known name `{name}`: {e}"));
        }
    }

    #[test]
    fn string_and_raw_filters_match_format_value() {
        // The pipe path and the raw `format_value` path must produce the same
        // output for `string`/`raw`. Before registering these filters, `string`
        // hit minijinja's built-in and `raw` threw, so the two paths diverged.
        let ctx = RenderContext {
            locale: "en-US".into(),
            currency: "USD".into(),
            timezone: "UTC".into(),
        };
        let fmt = SimpleAsciiFormatter;
        for value in [json!("hi"), json!(42), json!(true), json!(3.5)] {
            for format in ["string", "raw"] {
                let via_pipe = render(
                    &format!("{{{{ v | {format} }}}}"),
                    &json!({ "v": value.clone() }),
                );
                let via_format_value = format_value(&value, format, &ctx, &fmt);
                assert_eq!(
                    via_pipe, via_format_value,
                    "`{format}` diverged for value {value}"
                );
            }
        }
    }
}

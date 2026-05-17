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
    register_filter(env, "date", ctx.clone(), formatter.clone(), |_| {
        FormatSpec::Date
    });
    register_filter(env, "datetime", ctx.clone(), formatter.clone(), |_| {
        FormatSpec::Datetime
    });
    register_filter(env, "pill", ctx.clone(), formatter.clone(), |_| {
        FormatSpec::Pill
    });
    register_filter(env, "bar_indicator", ctx, formatter, |_| {
        FormatSpec::BarIndicator
    });
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
}

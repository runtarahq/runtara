//! Display-template and viewer-template rendering, backed by `minijinja`.
//!
//! Replaces the three hand-rolled `{{ field | format:arg }}` parsers in the
//! old server (`services/reports.rs:6244-6395`, `mcp/tools/reports.rs:3752-3839`,
//! and the FE's `utils.ts:419-513`). All three callers now share one grammar.

use minijinja::{Environment, Value as MJValue, value::ViaDeserialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TemplateError {
    #[error("template parse failed: {0}")]
    Parse(String),
    #[error("template render failed: {0}")]
    Render(String),
}

/// Build a minijinja environment with the report-DSL filter set registered.
/// The environment is sandboxed — no `include`, no `extends`, no `import`.
pub fn make_environment() -> Environment<'static> {
    let mut env = Environment::new();
    register_report_filters(&mut env);
    env
}

/// Register the report-DSL format filters (`currency`, `pill`, `datetime`,
/// `bar_indicator`, `percent`, `number`, `decimal`, `date`) on a minijinja
/// environment. Use this on caller-managed environments (e.g. when caching
/// a pre-built env across renders).
pub fn register_report_filters(env: &mut Environment<'static>) {
    env.add_filter("currency", filter_currency);
    env.add_filter("number", filter_number);
    env.add_filter("decimal", filter_decimal);
    env.add_filter("percent", filter_percent);
    env.add_filter("date", filter_date);
    env.add_filter("datetime", filter_datetime);
    env.add_filter("pill", filter_pill);
    env.add_filter("bar_indicator", filter_bar_indicator);
}

/// Render a template string against a row's fields. Convenience wrapper
/// around minijinja for the common case where the caller doesn't need to
/// hold the `Environment` between renders.
pub fn render_template(template: &str, row: &Value) -> Result<String, TemplateError> {
    render_template_with_filters(template, row, |_| {})
}

/// Render a template with the report filters plus a caller-supplied filter
/// closure. Useful when a caller needs to add a request-scoped filter
/// (e.g. timezone) that the standard filter set doesn't cover.
pub fn render_template_with_filters<F>(
    template: &str,
    row: &Value,
    extra: F,
) -> Result<String, TemplateError>
where
    F: FnOnce(&mut Environment<'static>),
{
    let mut env = make_environment();
    extra(&mut env);
    let tmpl = env
        .template_from_str(template)
        .map_err(|e| TemplateError::Parse(e.to_string()))?;
    tmpl.render(row)
        .map_err(|e| TemplateError::Render(e.to_string()))
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

fn filter_currency(value: ViaDeserialize<Option<f64>>) -> String {
    match value.0 {
        Some(n) => format_currency(n),
        None => String::new(),
    }
}

fn filter_number(value: ViaDeserialize<Option<f64>>) -> String {
    match value.0 {
        Some(n) if n.fract() == 0.0 => format_with_thousands(n as i64),
        Some(n) => format!("{:.2}", n),
        None => String::new(),
    }
}

fn filter_decimal(value: ViaDeserialize<Option<f64>>) -> String {
    value.0.map(|n| format!("{:.2}", n)).unwrap_or_default()
}

fn filter_percent(value: ViaDeserialize<Option<f64>>) -> String {
    match value.0 {
        Some(n) => format!("{:.1}%", n * 100.0),
        None => String::new(),
    }
}

fn filter_date(value: ViaDeserialize<Option<String>>) -> String {
    // Render YYYY-MM-DDTHH:MM:SS as YYYY-MM-DD without timezone math; locale
    // conversion is the renderer's job, not the template's.
    value
        .0
        .map(|s| s.split('T').next().unwrap_or(&s).to_string())
        .unwrap_or_default()
}

fn filter_datetime(value: ViaDeserialize<Option<String>>) -> String {
    // Mirror the FE: strip trailing Z + fractional seconds for compactness.
    value
        .0
        .map(|s| {
            let trimmed = s.strip_suffix('Z').unwrap_or(&s);
            trimmed
                .split_once('.')
                .map(|(head, _)| head.to_string())
                .unwrap_or_else(|| trimmed.to_string())
        })
        .unwrap_or_default()
}

fn filter_pill(value: MJValue) -> String {
    // Pill is a passthrough at template time — the colorization happens in
    // the renderer, not the string output.
    value.to_string()
}

fn filter_bar_indicator(value: MJValue) -> String {
    // Same as pill: rendering decoration happens in the FE.
    value.to_string()
}

// ---------------------------------------------------------------------------
// Format helpers
// ---------------------------------------------------------------------------

fn format_currency(n: f64) -> String {
    let sign = if n < 0.0 { "-" } else { "" };
    let abs = n.abs();
    let whole = abs.trunc() as i64;
    let cents = ((abs.fract() * 100.0).round()) as i64;
    format!("{}${}.{:02}", sign, format_with_thousands(whole), cents)
}

fn format_with_thousands(n: i64) -> String {
    let sign = if n < 0 { "-" } else { "" };
    let s = n.abs().to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*c as char);
    }
    format!("{sign}{out}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn render_plain_field() {
        let out = render_template("Hello {{ name }}", &json!({ "name": "world" })).unwrap();
        assert_eq!(out, "Hello world");
    }

    #[test]
    fn render_currency_filter() {
        let out = render_template(
            "Total: {{ amount | currency }}",
            &json!({ "amount": 1234.5 }),
        )
        .unwrap();
        assert_eq!(out, "Total: $1,234.50");
    }

    #[test]
    fn render_number_filter() {
        assert_eq!(
            render_template("{{ n | number }}", &json!({ "n": 1234567 })).unwrap(),
            "1,234,567"
        );
        assert_eq!(
            render_template("{{ n | number }}", &json!({ "n": 1.5 })).unwrap(),
            "1.50"
        );
    }

    #[test]
    fn render_percent_filter() {
        assert_eq!(
            render_template("{{ ratio | percent }}", &json!({ "ratio": 0.123 })).unwrap(),
            "12.3%"
        );
    }

    #[test]
    fn render_date_filter() {
        assert_eq!(
            render_template("{{ d | date }}", &json!({ "d": "2026-01-15T08:30:00Z" })).unwrap(),
            "2026-01-15"
        );
    }

    #[test]
    fn render_datetime_filter() {
        assert_eq!(
            render_template(
                "{{ d | datetime }}",
                &json!({ "d": "2026-01-15T08:30:00.123Z" })
            )
            .unwrap(),
            "2026-01-15T08:30:00"
        );
    }

    #[test]
    fn render_undefined_field_is_empty() {
        // Minijinja default: undefined → empty string. Matches the old
        // hand-rolled parser's behavior.
        let out = render_template("Hello {{ missing }}", &json!({})).unwrap();
        assert_eq!(out, "Hello ");
    }

    #[test]
    fn parse_error_is_reported_as_parse() {
        let err = render_template("Unclosed {{ field", &json!({})).unwrap_err();
        assert!(matches!(err, TemplateError::Parse(_)));
    }
}

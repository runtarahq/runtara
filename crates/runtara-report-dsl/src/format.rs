//! Format-string grammar, `FormatSpec` parser, and `Formatter` trait.
//!
//! This module owns the format-string grammar that templates and raw cell
//! renderers share. The crate stays locale-agnostic: it dispatches to
//! whatever `Formatter` is plugged in. The browser supplies a JS-backed
//! formatter (uses `Intl`); the server supplies `SimpleAsciiFormatter` by
//! default and can swap to an ICU-backed impl in the future without
//! touching call sites.
//!
//! See `docs/reports-refactoring-plan.md` § "Phase 2 sub-plan: utils.ts
//! swap" for the architectural decisions behind this module.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Per-render context shared with every `Formatter::format` call.
///
/// Carries the host-resolved locale + currency + timezone. The crate makes
/// no assumptions about these values beyond passing them through.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RenderContext {
    /// BCP-47 locale tag — e.g. `"en-US"`, `"de-DE"`. Empty string means
    /// "host default" and is passed through to the formatter.
    pub locale: String,
    /// ISO 4217 currency code — e.g. `"USD"`, `"EUR"`. Used when a
    /// `FormatSpec::Currency` doesn't carry an explicit code.
    pub currency: String,
    /// IANA timezone — e.g. `"Europe/Berlin"`. Used by date/datetime
    /// formatters.
    pub timezone: String,
}

/// The closed set of format specs the report DSL supports. Parsed from the
/// `format:` string at the boundary; every `Formatter` impl pattern-matches
/// on this enum.
///
/// The shape is the frozen contract. New variants are additive — never
/// remove or repurpose an existing one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FormatSpec {
    /// Locale-aware currency. `code` overrides `RenderContext::currency`
    /// when present (e.g. `currency:eur`).
    Currency { code: Option<String> },
    /// Compact-notation currency (`$1.2K`, `€3M`).
    CurrencyCompact { code: Option<String> },
    /// Integer with thousands grouping.
    Number,
    /// Compact integer (`1.2K`, `3M`).
    NumberCompact,
    /// Decimal with two fraction digits.
    Decimal,
    /// Percentage — input is the raw ratio (`0.123` → `12.3%`).
    Percent,
    /// Bytes with SI unit suffix (`1500` → `1.50 KB`,
    /// `2_500_000_000` → `2.50 GB`). Uses decimal thresholds (10^3 each
    /// step), not binary (1024). For locale-correct rendering the JS
    /// bridge dispatches to `Intl.NumberFormat({ style: 'unit', unit:
    /// 'megabyte' })`; the ASCII fallback uses plain `B/KB/MB/…`.
    Bytes,
    /// Calendar date (no time component).
    Date,
    /// Date + time, locale-formatted.
    Datetime,
    /// Pill rendering — formatter returns the raw stringified value; the
    /// renderer decorates with a pill badge.
    Pill,
    /// Bar-indicator rendering — formatter returns the raw stringified
    /// value; the renderer decorates with a bar.
    BarIndicator,
    /// Explicit string coercion.
    String,
    /// Unknown or absent format — passthrough.
    Raw,
}

impl FormatSpec {
    /// Every format name the DSL recognizes, in the same order as the
    /// `parse` match arms below. This is the single source of truth shared
    /// by save-time validation (`template::validate_display_template_format`)
    /// and the render-time filter set (`template::register_report_filters`):
    /// keeping the three in lockstep is what stops a template from validating
    /// green and then throwing an "unknown filter" at render.
    pub const KNOWN_NAMES: &'static [&'static str] = &[
        "currency",
        "currency_compact",
        "number",
        "number_compact",
        "decimal",
        "percent",
        "bytes",
        "date",
        "datetime",
        "pill",
        "bar_indicator",
        "string",
        "raw",
    ];

    /// Whether `name` is a recognized format name. Unlike `parse` (which is
    /// total and maps anything unknown to `Raw`), this distinguishes a real
    /// format from an arbitrary word, so validators can reject the latter.
    pub fn is_known_name(name: &str) -> bool {
        Self::KNOWN_NAMES.contains(&name)
    }

    /// Parse a format string into a `FormatSpec`.
    ///
    /// Grammar: `name` or `name:arg`. Unknown names map to `Raw`. Empty
    /// strings (or `None`) also map to `Raw`. This keeps the parser
    /// total — no errors — so renderers degrade gracefully on stored
    /// format strings the crate doesn't know about.
    pub fn parse(format: &str) -> Self {
        let trimmed = format.trim();
        if trimmed.is_empty() {
            return FormatSpec::Raw;
        }
        let (name, arg) = match trimmed.find(':') {
            Some(idx) => {
                let name = trimmed[..idx].trim();
                let arg = trimmed[idx + 1..].trim();
                let arg = if arg.is_empty() {
                    None
                } else {
                    Some(arg.to_string())
                };
                (name, arg)
            }
            None => (trimmed, None),
        };

        match name {
            "currency" => FormatSpec::Currency { code: arg },
            "currency_compact" => FormatSpec::CurrencyCompact { code: arg },
            "number" => FormatSpec::Number,
            "number_compact" => FormatSpec::NumberCompact,
            "decimal" => FormatSpec::Decimal,
            "percent" => FormatSpec::Percent,
            "bytes" => FormatSpec::Bytes,
            "date" => FormatSpec::Date,
            "datetime" => FormatSpec::Datetime,
            "pill" => FormatSpec::Pill,
            "bar_indicator" => FormatSpec::BarIndicator,
            "string" => FormatSpec::String,
            "raw" => FormatSpec::Raw,
            _ => FormatSpec::Raw,
        }
    }

    /// Re-emit the original format string for a parsed spec.
    /// Useful for round-tripping a spec back to the JS callback boundary.
    pub fn to_format_string(&self) -> String {
        match self {
            FormatSpec::Currency { code: Some(c) } => format!("currency:{c}"),
            FormatSpec::Currency { code: None } => "currency".into(),
            FormatSpec::CurrencyCompact { code: Some(c) } => format!("currency_compact:{c}"),
            FormatSpec::CurrencyCompact { code: None } => "currency_compact".into(),
            FormatSpec::Number => "number".into(),
            FormatSpec::NumberCompact => "number_compact".into(),
            FormatSpec::Decimal => "decimal".into(),
            FormatSpec::Percent => "percent".into(),
            FormatSpec::Bytes => "bytes".into(),
            FormatSpec::Date => "date".into(),
            FormatSpec::Datetime => "datetime".into(),
            FormatSpec::Pill => "pill".into(),
            FormatSpec::BarIndicator => "bar_indicator".into(),
            FormatSpec::String => "string".into(),
            FormatSpec::Raw => "raw".into(),
        }
    }
}

/// Format a JSON value according to a parsed `FormatSpec` and a
/// `RenderContext`. Implementations live in their host: the browser
/// supplies a `JsFormatter` (uses `Intl`); the server supplies
/// `SimpleAsciiFormatter` (or, in the future, `IcuFormatter`).
///
/// The trait is `Sync + Send` so a single formatter instance can be
/// shared across template renders.
pub trait Formatter: Send + Sync {
    fn format(&self, value: &Value, spec: &FormatSpec, ctx: &RenderContext) -> String;
}

/// Server-side default formatter. ASCII output for every locale —
/// matches the historical server template output. Suitable for any
/// non-browser embed that doesn't yet have ICU available. Swap to a
/// locale-aware `Formatter` impl when needed; the call sites don't change.
#[derive(Debug, Clone, Copy, Default)]
pub struct SimpleAsciiFormatter;

impl Formatter for SimpleAsciiFormatter {
    fn format(&self, value: &Value, spec: &FormatSpec, ctx: &RenderContext) -> String {
        if value.is_null() {
            return String::new();
        }

        match spec {
            FormatSpec::Currency { code } => match value.as_f64() {
                Some(n) => format_currency_ascii(n, code.as_deref().unwrap_or(&ctx.currency)),
                None => render_raw(value),
            },
            FormatSpec::CurrencyCompact { code } => match value.as_f64() {
                Some(n) => {
                    format_currency_compact_ascii(n, code.as_deref().unwrap_or(&ctx.currency))
                }
                None => render_raw(value),
            },
            FormatSpec::Number => match value.as_f64() {
                Some(n) if n.fract() == 0.0 => format_thousands(n as i64),
                Some(n) => format!("{:.2}", n),
                None => render_raw(value),
            },
            FormatSpec::NumberCompact => match value.as_f64() {
                Some(n) => format_compact_ascii(n),
                None => render_raw(value),
            },
            FormatSpec::Decimal => match value.as_f64() {
                Some(n) => format!("{:.2}", n),
                None => render_raw(value),
            },
            FormatSpec::Percent => match value.as_f64() {
                Some(n) => format!("{:.1}%", n * 100.0),
                None => render_raw(value),
            },
            FormatSpec::Bytes => match value.as_f64() {
                Some(n) => format_bytes_ascii(n),
                None => render_raw(value),
            },
            FormatSpec::Date => match value.as_str() {
                Some(s) => s.split('T').next().unwrap_or(s).to_string(),
                None => render_raw(value),
            },
            FormatSpec::Datetime => match value.as_str() {
                Some(s) => {
                    let trimmed = s.strip_suffix('Z').unwrap_or(s);
                    trimmed
                        .split_once('.')
                        .map(|(head, _)| head.to_string())
                        .unwrap_or_else(|| trimmed.to_string())
                }
                None => render_raw(value),
            },
            FormatSpec::Pill | FormatSpec::BarIndicator | FormatSpec::String | FormatSpec::Raw => {
                render_raw(value)
            }
        }
    }
}

fn render_raw(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

fn format_currency_ascii(n: f64, code: &str) -> String {
    let symbol = currency_symbol(code);
    let sign = if n < 0.0 { "-" } else { "" };
    let abs = n.abs();
    let whole = abs.trunc() as i64;
    let cents = (abs.fract() * 100.0).round() as i64;
    format!("{sign}{symbol}{}.{:02}", format_thousands(whole), cents)
}

fn format_currency_compact_ascii(n: f64, code: &str) -> String {
    let symbol = currency_symbol(code);
    let sign = if n < 0.0 { "-" } else { "" };
    let body = format_compact_ascii(n.abs());
    format!("{sign}{symbol}{body}")
}

/// SI byte formatting — decimal thresholds (10^3 per step), short unit
/// suffix. Values below 1 KB render as integer bytes; everything else as
/// two-decimal places. The JS bridge handles the locale-aware rendering;
/// this is the ASCII fallback used by the server when no `Intl`-backed
/// formatter is installed.
fn format_bytes_ascii(n: f64) -> String {
    if !n.is_finite() {
        return String::new();
    }
    let sign = if n < 0.0 { "-" } else { "" };
    let abs = n.abs();
    let (scaled, suffix) = if abs < 1.0e3 {
        return format!("{sign}{} B", format_thousands(abs.round() as i64));
    } else if abs < 1.0e6 {
        (abs / 1.0e3, "KB")
    } else if abs < 1.0e9 {
        (abs / 1.0e6, "MB")
    } else if abs < 1.0e12 {
        (abs / 1.0e9, "GB")
    } else if abs < 1.0e15 {
        (abs / 1.0e12, "TB")
    } else {
        (abs / 1.0e15, "PB")
    };
    format!("{sign}{:.2} {suffix}", scaled)
}

fn format_compact_ascii(n: f64) -> String {
    let abs = n.abs();
    let sign = if n < 0.0 { "-" } else { "" };
    let (scaled, suffix) = if abs >= 1.0e12 {
        (abs / 1.0e12, "T")
    } else if abs >= 1.0e9 {
        (abs / 1.0e9, "B")
    } else if abs >= 1.0e6 {
        (abs / 1.0e6, "M")
    } else if abs >= 1.0e3 {
        (abs / 1.0e3, "K")
    } else {
        return if abs.fract() == 0.0 {
            format!("{sign}{}", abs as i64)
        } else {
            format!("{sign}{:.1}", abs)
        };
    };
    let rendered = if scaled.fract() == 0.0 {
        format!("{}", scaled as i64)
    } else {
        format!("{:.1}", scaled)
    };
    format!("{sign}{rendered}{suffix}")
}

fn format_thousands(n: i64) -> String {
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

fn currency_symbol(code: &str) -> &'static str {
    match code.to_ascii_uppercase().as_str() {
        "USD" | "CAD" | "AUD" | "NZD" | "HKD" | "SGD" => "$",
        "EUR" => "€",
        "GBP" => "£",
        "JPY" | "CNY" => "¥",
        "INR" => "₹",
        _ => "$",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx() -> RenderContext {
        RenderContext {
            locale: "en-US".into(),
            currency: "USD".into(),
            timezone: "UTC".into(),
        }
    }

    #[test]
    fn parse_format_strings() {
        assert_eq!(
            FormatSpec::parse("currency"),
            FormatSpec::Currency { code: None }
        );
        assert_eq!(
            FormatSpec::parse("currency:eur"),
            FormatSpec::Currency {
                code: Some("eur".into())
            }
        );
        assert_eq!(
            FormatSpec::parse("number_compact"),
            FormatSpec::NumberCompact
        );
        assert_eq!(FormatSpec::parse("bytes"), FormatSpec::Bytes);
        assert_eq!(FormatSpec::Bytes.to_format_string(), "bytes");
        assert_eq!(FormatSpec::parse(""), FormatSpec::Raw);
        assert_eq!(FormatSpec::parse("nonsense"), FormatSpec::Raw);
        assert_eq!(FormatSpec::parse("raw"), FormatSpec::Raw);
    }

    #[test]
    fn known_names_agree_with_parse() {
        // Every advertised name parses to a real spec: only `raw` is allowed
        // to land on `Raw` — any other name doing so would be a silent
        // unknown, exactly the drift this set exists to prevent.
        for name in FormatSpec::KNOWN_NAMES {
            assert!(FormatSpec::is_known_name(name), "{name} should be known");
            let spec = FormatSpec::parse(name);
            if *name == "raw" {
                assert_eq!(spec, FormatSpec::Raw);
            } else {
                assert_ne!(spec, FormatSpec::Raw, "{name} degraded to Raw");
            }
        }
        // Anything outside the set is not a known name.
        assert!(!FormatSpec::is_known_name("foobar"));
        assert!(!FormatSpec::is_known_name(""));
        assert!(!FormatSpec::is_known_name("Currency"));
    }

    #[test]
    fn ascii_currency() {
        let fmt = SimpleAsciiFormatter;
        let spec = FormatSpec::Currency { code: None };
        assert_eq!(fmt.format(&json!(1234.5), &spec, &ctx()), "$1,234.50");
        assert_eq!(fmt.format(&json!(-1234.5), &spec, &ctx()), "-$1,234.50");
        assert_eq!(fmt.format(&Value::Null, &spec, &ctx()), "");
    }

    #[test]
    fn ascii_currency_explicit_code() {
        let fmt = SimpleAsciiFormatter;
        let spec = FormatSpec::Currency {
            code: Some("EUR".into()),
        };
        assert_eq!(fmt.format(&json!(50.0), &spec, &ctx()), "€50.00");
    }

    #[test]
    fn ascii_number_compact() {
        let fmt = SimpleAsciiFormatter;
        let spec = FormatSpec::NumberCompact;
        assert_eq!(fmt.format(&json!(1234), &spec, &ctx()), "1.2K");
        assert_eq!(fmt.format(&json!(1_500_000), &spec, &ctx()), "1.5M");
        assert_eq!(fmt.format(&json!(1_000_000_000), &spec, &ctx()), "1B");
    }

    #[test]
    fn ascii_bytes() {
        let fmt = SimpleAsciiFormatter;
        let spec = FormatSpec::Bytes;
        // Below 1 KB → integer bytes with unit
        assert_eq!(fmt.format(&json!(0), &spec, &ctx()), "0 B");
        assert_eq!(fmt.format(&json!(512), &spec, &ctx()), "512 B");
        // SI thresholds: 1500 B = 1.50 KB; 2.5 GB; etc.
        assert_eq!(fmt.format(&json!(1_500), &spec, &ctx()), "1.50 KB");
        assert_eq!(fmt.format(&json!(2_500_000), &spec, &ctx()), "2.50 MB");
        assert_eq!(
            fmt.format(&json!(2_500_000_000_i64), &spec, &ctx()),
            "2.50 GB"
        );
        // The number used by the seeded System resources report (= 64 GiB)
        // renders as 68.72 GB in SI — the documented behaviour.
        assert_eq!(
            fmt.format(&json!(68_719_476_736_i64), &spec, &ctx()),
            "68.72 GB"
        );
        // Negative + null
        assert_eq!(fmt.format(&json!(-1_500), &spec, &ctx()), "-1.50 KB");
        assert_eq!(fmt.format(&Value::Null, &spec, &ctx()), "");
        // Non-numeric input falls through to raw
        assert_eq!(fmt.format(&json!("abc"), &spec, &ctx()), "abc");
    }

    #[test]
    fn ascii_percent() {
        let fmt = SimpleAsciiFormatter;
        let spec = FormatSpec::Percent;
        assert_eq!(fmt.format(&json!(0.123), &spec, &ctx()), "12.3%");
        assert_eq!(fmt.format(&json!(1.0), &spec, &ctx()), "100.0%");
    }

    #[test]
    fn ascii_date_strips_time() {
        let fmt = SimpleAsciiFormatter;
        let spec = FormatSpec::Date;
        assert_eq!(
            fmt.format(&json!("2026-01-15T08:30:00Z"), &spec, &ctx()),
            "2026-01-15"
        );
    }

    #[test]
    fn ascii_datetime_trims_z_and_fractions() {
        let fmt = SimpleAsciiFormatter;
        let spec = FormatSpec::Datetime;
        assert_eq!(
            fmt.format(&json!("2026-01-15T08:30:00.123Z"), &spec, &ctx()),
            "2026-01-15T08:30:00"
        );
    }

    #[test]
    fn ascii_raw_passthrough() {
        let fmt = SimpleAsciiFormatter;
        assert_eq!(fmt.format(&json!("hi"), &FormatSpec::Raw, &ctx()), "hi");
        assert_eq!(fmt.format(&json!(42), &FormatSpec::Raw, &ctx()), "42");
        assert_eq!(fmt.format(&Value::Null, &FormatSpec::Raw, &ctx()), "");
    }
}

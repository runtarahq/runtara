//! `wasm-bindgen` JS bindings for the report-DSL functions the frontend
//! consumes (Phase 2): template rendering, row-condition evaluation,
//! and one-shot value formatting.
//!
//! The bindings take/return JS values via `serde-wasm-bindgen` so callers
//! pass plain JSON objects on the JS side. Errors are thrown as `Error`
//! objects with a `message` string.
//!
//! ## Formatter callback
//!
//! Locale-aware formatting lives on the host side. The FE registers a
//! global callback `window.__runtaraReportDslFormatValue(value, spec_json,
//! locale, currency, timezone) -> string` before invoking any template
//! render. WASM calls back into that function through a `JsFormatter`
//! that implements the [`crate::Formatter`] trait. Keeps `runtara-report-dsl`
//! locale-agnostic — a future ICU-backed formatter is a drop-in.

#![allow(clippy::unused_unit)] // wasm_bindgen generates extern fns that trip this

use std::sync::Arc;

use serde_json::Value;
use wasm_bindgen::prelude::*;

use crate::format::{FormatSpec, Formatter, RenderContext};

#[wasm_bindgen]
extern "C" {
    /// FE-side callback. Receives a JS-side value plus a serialized
    /// `FormatSpec` (JSON-encoded so additions to the enum don't break
    /// the boundary), returns the formatted string.
    ///
    /// Registered as `window.__runtaraReportDslFormatValue` by
    /// `frontend/src/wasm/runtara-report-dsl/index.ts`.
    #[wasm_bindgen(js_namespace = window, js_name = __runtaraReportDslFormatValue)]
    fn js_format_value_callback(
        value: JsValue,
        spec_json: &str,
        locale: &str,
        currency: &str,
        timezone: &str,
    ) -> String;
}

/// `Formatter` impl that delegates to the JS-side `Intl`-backed callback.
struct JsFormatter;

impl Formatter for JsFormatter {
    fn format(&self, value: &Value, spec: &FormatSpec, ctx: &RenderContext) -> String {
        let js_value = serde_wasm_bindgen::to_value(value).unwrap_or(JsValue::NULL);
        let spec_json = serde_json::to_string(spec).unwrap_or_else(|_| String::from("\"raw\""));
        js_format_value_callback(
            js_value,
            &spec_json,
            &ctx.locale,
            &ctx.currency,
            &ctx.timezone,
        )
    }
}

fn parse_context(ctx: JsValue) -> Result<RenderContext, JsError> {
    if ctx.is_null() || ctx.is_undefined() {
        return Ok(RenderContext::default());
    }
    serde_wasm_bindgen::from_value(ctx).map_err(|e| JsError::new(&e.to_string()))
}

/// Library version. Useful for FE↔BE drift detection.
#[wasm_bindgen(js_name = version)]
pub fn js_version() -> String {
    crate::VERSION.to_string()
}

/// Render a `{{ field | filter }}` template string against a row.
/// Throws on parse or render error. Locale-aware formatting routes back
/// into JS via the registered formatter callback.
#[wasm_bindgen(js_name = renderTemplate)]
pub fn js_render_template(template: &str, row: JsValue, ctx: JsValue) -> Result<String, JsError> {
    let row_json: Value =
        serde_wasm_bindgen::from_value(row).map_err(|e| JsError::new(&e.to_string()))?;
    let render_ctx = parse_context(ctx)?;
    crate::render_template(template, &row_json, &render_ctx, Arc::new(JsFormatter))
        .map_err(|e| JsError::new(&e.to_string()))
}

/// One-shot value formatting outside a template. Same dispatch path as
/// the template filters, so `formatValue(x, 'currency', ctx)` matches
/// `renderTemplate('{{ x | currency }}', { x }, ctx)`.
#[wasm_bindgen(js_name = formatValue)]
pub fn js_format_value(value: JsValue, format: &str, ctx: JsValue) -> Result<String, JsError> {
    let value_json: Value =
        serde_wasm_bindgen::from_value(value).map_err(|e| JsError::new(&e.to_string()))?;
    let render_ctx = parse_context(ctx)?;
    Ok(crate::format_value(
        &value_json,
        format,
        &render_ctx,
        &JsFormatter,
    ))
}

/// Compile-check a template string. Returns `null` on success, throws on
/// parse error. Useful for save-time validation in the FE wizard.
#[wasm_bindgen(js_name = validateTemplate)]
pub fn js_validate_template(template: &str) -> Result<(), JsError> {
    crate::validate_template(template).map_err(|e| JsError::new(&e.to_string()))
}

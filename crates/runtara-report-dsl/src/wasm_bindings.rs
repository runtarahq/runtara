//! `wasm-bindgen` JS bindings for the report-DSL functions the frontend
//! consumes (Phase 2): template rendering and row-condition evaluation.
//!
//! The bindings take/return JS values via `serde-wasm-bindgen` so callers
//! pass plain JSON objects on the JS side. Errors are thrown as `Error`
//! objects with a `message` string.

#![allow(clippy::unused_unit)] // wasm_bindgen generates extern fns that trip this

use wasm_bindgen::prelude::*;

/// Library version. Useful for FE↔BE drift detection.
#[wasm_bindgen(js_name = version)]
pub fn js_version() -> String {
    crate::VERSION.to_string()
}

/// Render a `{{ field | filter }}` template string against a row.
/// Throws on parse or render error.
#[wasm_bindgen(js_name = renderTemplate)]
pub fn js_render_template(template: &str, row: JsValue) -> Result<String, JsError> {
    let row_json: serde_json::Value =
        serde_wasm_bindgen::from_value(row).map_err(|e| JsError::new(&e.to_string()))?;
    crate::render_template(template, &row_json).map_err(|e| JsError::new(&e.to_string()))
}

/// Compile-check a template string. Returns `null` on success, throws on
/// parse error. Useful for save-time validation in the FE wizard.
#[wasm_bindgen(js_name = validateTemplate)]
pub fn js_validate_template(template: &str) -> Result<(), JsError> {
    let env = crate::template::make_environment();
    env.template_from_str(template)
        .map(|_| ())
        .map_err(|e| JsError::new(&e.to_string()))
}

/// Evaluate a row condition (a `ConditionExpression` shape) against a row.
/// Returns true/false. Throws on server-only operators or malformed input.
#[wasm_bindgen(js_name = evaluateRowCondition)]
pub fn js_evaluate_row_condition(expr: JsValue, row: JsValue) -> Result<bool, JsError> {
    let expr_value: serde_json::Value =
        serde_wasm_bindgen::from_value(expr).map_err(|e| JsError::new(&e.to_string()))?;
    let row_value: serde_json::Value =
        serde_wasm_bindgen::from_value(row).map_err(|e| JsError::new(&e.to_string()))?;
    let cond: runtara_dsl::ConditionExpression =
        serde_json::from_value(expr_value).map_err(|e| JsError::new(&e.to_string()))?;
    crate::evaluate_row_condition(&cond, &row_value).map_err(|e| JsError::new(&e.to_string()))
}

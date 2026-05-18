//! Utility agent — math, dates, strings, country codes — as a WebAssembly component.
//!
//! Schema matches the legacy `runtara-agents/src/agents/utils.rs` agent so
//! A/B parity tests can compare results:
//! - `random-double`: Generate a random double between 0 and 1
//! - `random-array`: Generate an array of random integers
//! - `return-input-string`: Return the input string value
//! - `return-input`: Return the input JSON value
//! - `do-nothing`: No-op operation that returns null
//! - `delay-in-ms`: Delay execution for N milliseconds
//! - `calculate`: Evaluate a mathematical expression with variables
//! - `format-date-from-iso`: Format an ISO date to a custom format
//! - `iso-to-unix-timestamp`: Convert ISO date to Unix timestamp
//! - `get-current-unix-timestamp`: Get the current Unix timestamp
//! - `get-current-iso-datetime`: Get the current date/time in ISO format
//! - `get-current-formatted-datetime`: Get the current date/time in a custom format
//! - `country-name-to-iso-code`: Convert country name to ISO code

#![cfg(target_arch = "wasm32")]

#[allow(warnings)]
mod bindings;

use bindings::exports::runtara::agent::capabilities::{
    CapabilityInfo, ConnectionInfo, ErrorInfo, Guest, ModuleInfo,
};
use std::collections::HashMap;

// -----------------------------------------------------------------------------
// Error helpers (mirror crypto agent pattern)
// -----------------------------------------------------------------------------

fn permanent_err(code: &str, message: impl Into<String>) -> ErrorInfo {
    ErrorInfo {
        code: code.into(),
        message: message.into(),
        category: "permanent".into(),
        severity: "error".into(),
        retryable: false,
        retry_after_ms: None,
        attributes: None,
    }
}

fn bad_json(e: serde_json::Error) -> ErrorInfo {
    permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string())
}

fn capability_err(message: impl Into<String>) -> ErrorInfo {
    permanent_err("CAPABILITY_ERROR", message)
}

// -----------------------------------------------------------------------------
// Input/Output types (mirror legacy utils.rs)
// -----------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct RandomDoubleInput {}

#[derive(serde::Deserialize)]
struct ReturnRandomArrayInput {
    size: i32,
}

#[derive(serde::Deserialize)]
struct ReturnStringInput {
    value: String,
}

#[derive(serde::Deserialize)]
struct ReturnInputData {
    value: serde_json::Value,
}

#[derive(serde::Deserialize)]
struct DoNothingInput {}

#[derive(serde::Deserialize)]
struct DelayInMsInput {
    delay_value: u64,
}

#[derive(serde::Deserialize)]
struct CalculateInput {
    expression: String,
    variables: HashMap<String, serde_json::Value>,
    #[serde(default)]
    enable_rounding: bool,
    decimal_places: Option<u32>,
}

#[derive(serde::Deserialize)]
struct FormatDateFromIsoInput {
    iso_date: String,
    target_format: String,
}

#[derive(serde::Deserialize)]
struct IsoToUnixTimestampInput {
    iso_date: String,
}

#[derive(serde::Deserialize)]
struct GetCurrentUnixTimestampInput {}

#[derive(serde::Deserialize)]
struct GetCurrentIsoDatetimeInput {}

#[derive(serde::Deserialize)]
struct GetCurrentFormattedDateTimeInput {
    format: String,
}

#[derive(serde::Deserialize)]
struct CountryNameToIsoCodeInput {
    country_name: String,
    #[serde(default = "default_code_type")]
    code_type: String,
}

fn default_code_type() -> String {
    "alpha2".to_string()
}

// -----------------------------------------------------------------------------
// Component plumbing
// -----------------------------------------------------------------------------

struct Component;

impl Guest for Component {
    fn get_module_info() -> ModuleInfo {
        ModuleInfo {
            id: "utils".into(),
            display_name: "Utils".into(),
            description: "Utility operations: math, dates, strings, and country code lookups."
                .into(),
            has_side_effects: false,
            supports_connections: false,
            integration_ids: vec![],
            secure: false,
        }
    }

    fn list_capabilities() -> Vec<CapabilityInfo> {
        vec![
            CapabilityInfo {
                id: "random-double".into(),
                function_name: "random_double".into(),
                display_name: Some("Random Double".into()),
                description: Some("Generate a random double between 0 and 1".into()),
                has_side_effects: false,
                is_idempotent: false,
                rate_limited: false,
                tags: vec!["utils".into(), "random".into()],
                input_schema: r#"{"type":"object","properties":{}}"#.into(),
                output_schema: r#"{"type":"number"}"#.into(),
                known_errors: vec![],
                compensation_hint: None,
            },
            CapabilityInfo {
                id: "random-array".into(),
                function_name: "random_array".into(),
                display_name: Some("Random Array".into()),
                description: Some("Generate an array of random integers".into()),
                has_side_effects: false,
                is_idempotent: false,
                rate_limited: false,
                tags: vec!["utils".into(), "random".into()],
                input_schema: r#"{"type":"object","required":["size"],"properties":{"size":{"type":"integer","description":"The number of random integers to generate"}}}"#.into(),
                output_schema: r#"{"type":"array","items":{"type":"integer"}}"#.into(),
                known_errors: vec![],
                compensation_hint: None,
            },
            CapabilityInfo {
                id: "return-input-string".into(),
                function_name: "return_input_string".into(),
                display_name: Some("Return String".into()),
                description: Some("Returns the input string value".into()),
                has_side_effects: false,
                is_idempotent: true,
                rate_limited: false,
                tags: vec!["utils".into()],
                input_schema: r#"{"type":"object","required":["value"],"properties":{"value":{"type":"string","description":"The string value to return as output"}}}"#.into(),
                output_schema: r#"{"type":"string"}"#.into(),
                known_errors: vec![],
                compensation_hint: None,
            },
            CapabilityInfo {
                id: "return-input".into(),
                function_name: "return_input".into(),
                display_name: Some("Return Input".into()),
                description: Some("Returns the input JSON value".into()),
                has_side_effects: false,
                is_idempotent: true,
                rate_limited: false,
                tags: vec!["utils".into()],
                input_schema: r#"{"type":"object","required":["value"],"properties":{"value":{"description":"The JSON value to return as output"}}}"#.into(),
                output_schema: r#"{}"#.into(),
                known_errors: vec![],
                compensation_hint: None,
            },
            CapabilityInfo {
                id: "do-nothing".into(),
                function_name: "do_nothing".into(),
                display_name: Some("Do Nothing".into()),
                description: Some("No-op operation that returns null".into()),
                has_side_effects: false,
                is_idempotent: true,
                rate_limited: false,
                tags: vec!["utils".into()],
                input_schema: r#"{"type":"object","properties":{}}"#.into(),
                output_schema: r#"{"type":"null"}"#.into(),
                known_errors: vec![],
                compensation_hint: None,
            },
            CapabilityInfo {
                id: "delay-in-ms".into(),
                function_name: "delay_in_ms".into(),
                display_name: Some("Delay".into()),
                description: Some("Delays execution for the specified number of milliseconds. For durable delays that survive crashes, use the Delay step type instead.".into()),
                has_side_effects: false,
                is_idempotent: true,
                rate_limited: false,
                tags: vec!["utils".into()],
                input_schema: r#"{"type":"object","required":["delay_value"],"properties":{"delay_value":{"type":"integer","description":"The amount of time to pause execution in milliseconds"}}}"#.into(),
                output_schema: r#"{"type":"integer"}"#.into(),
                known_errors: vec![],
                compensation_hint: None,
            },
            CapabilityInfo {
                id: "calculate".into(),
                function_name: "calculate".into(),
                display_name: Some("Calculate".into()),
                description: Some("Evaluate a mathematical expression with variables".into()),
                has_side_effects: false,
                is_idempotent: true,
                rate_limited: false,
                tags: vec!["utils".into(), "math".into()],
                input_schema: r#"{"type":"object","required":["expression","variables"],"properties":{"expression":{"type":"string","description":"Mathematical expression with variables and operators (+, -, *, /, %, parentheses)"},"variables":{"type":"object","description":"Map of variable names to their numeric values (supports numbers and string numbers)"},"enable_rounding":{"type":"boolean","default":false,"description":"Round the result to the specified number of decimal places"},"decimal_places":{"type":"integer","description":"Number of decimal places to round to (maximum 15)"}}}"#.into(),
                output_schema: r#"{"type":"number"}"#.into(),
                known_errors: vec![],
                compensation_hint: None,
            },
            CapabilityInfo {
                id: "format-date-from-iso".into(),
                function_name: "format_date_from_iso".into(),
                display_name: Some("Format Date".into()),
                description: Some("Format an ISO date to a custom format".into()),
                has_side_effects: false,
                is_idempotent: true,
                rate_limited: false,
                tags: vec!["utils".into(), "date".into()],
                input_schema: r#"{"type":"object","required":["iso_date","target_format"],"properties":{"iso_date":{"type":"string","description":"The date/time in ISO 8601 format (e.g., 2024-01-15T10:30:00Z)"},"target_format":{"type":"string","description":"Format pattern using yyyy, MM, dd, HH, mm, ss tokens"}}}"#.into(),
                output_schema: r#"{"type":"string"}"#.into(),
                known_errors: vec![],
                compensation_hint: None,
            },
            CapabilityInfo {
                id: "iso-to-unix-timestamp".into(),
                function_name: "iso_to_unix_timestamp".into(),
                display_name: Some("ISO to Unix".into()),
                description: Some("Convert ISO date to Unix timestamp".into()),
                has_side_effects: false,
                is_idempotent: true,
                rate_limited: false,
                tags: vec!["utils".into(), "date".into()],
                input_schema: r#"{"type":"object","required":["iso_date"],"properties":{"iso_date":{"type":"string","description":"The date/time in ISO 8601 format to convert to Unix timestamp"}}}"#.into(),
                output_schema: r#"{"type":"integer"}"#.into(),
                known_errors: vec![],
                compensation_hint: None,
            },
            CapabilityInfo {
                id: "get-current-unix-timestamp".into(),
                function_name: "get_current_unix_timestamp".into(),
                display_name: Some("Current Unix Timestamp".into()),
                description: Some("Get the current Unix timestamp".into()),
                has_side_effects: false,
                is_idempotent: false,
                rate_limited: false,
                tags: vec!["utils".into(), "date".into()],
                input_schema: r#"{"type":"object","properties":{}}"#.into(),
                output_schema: r#"{"type":"integer"}"#.into(),
                known_errors: vec![],
                compensation_hint: None,
            },
            CapabilityInfo {
                id: "get-current-iso-datetime".into(),
                function_name: "get_current_iso_datetime".into(),
                display_name: Some("Current ISO Datetime".into()),
                description: Some("Get the current date/time in ISO format".into()),
                has_side_effects: false,
                is_idempotent: false,
                rate_limited: false,
                tags: vec!["utils".into(), "date".into()],
                input_schema: r#"{"type":"object","properties":{}}"#.into(),
                output_schema: r#"{"type":"string"}"#.into(),
                known_errors: vec![],
                compensation_hint: None,
            },
            CapabilityInfo {
                id: "get-current-formatted-datetime".into(),
                function_name: "get_current_formatted_datetime".into(),
                display_name: Some("Current Formatted DateTime".into()),
                description: Some("Get the current date/time in a custom format".into()),
                has_side_effects: false,
                is_idempotent: false,
                rate_limited: false,
                tags: vec!["utils".into(), "date".into()],
                input_schema: r#"{"type":"object","required":["format"],"properties":{"format":{"type":"string","description":"Format pattern using yyyy, MM, dd, HH, mm, ss tokens"}}}"#.into(),
                output_schema: r#"{"type":"string"}"#.into(),
                known_errors: vec![],
                compensation_hint: None,
            },
            CapabilityInfo {
                id: "country-name-to-iso-code".into(),
                function_name: "country_name_to_iso_code".into(),
                display_name: Some("Country to ISO Code".into()),
                description: Some("Convert country name to ISO code".into()),
                has_side_effects: false,
                is_idempotent: true,
                rate_limited: false,
                tags: vec!["utils".into(), "country".into()],
                input_schema: r#"{"type":"object","required":["country_name"],"properties":{"country_name":{"type":"string","description":"The full country name or commonly used alternative"},"code_type":{"type":"string","enum":["alpha2","alpha3"],"default":"alpha2","description":"The type of ISO code to return"}}}"#.into(),
                output_schema: r#"{"type":"string"}"#.into(),
                known_errors: vec![],
                compensation_hint: None,
            },
        ]
    }

    fn invoke(
        capability_id: String,
        input: String,
        _connection: Option<ConnectionInfo>,
    ) -> Result<String, ErrorInfo> {
        match capability_id.as_str() {
            "random-double" => cap_random_double(&input),
            "random-array" => cap_random_array(&input),
            "return-input-string" => cap_return_input_string(&input),
            "return-input" => cap_return_input(&input),
            "do-nothing" => cap_do_nothing(&input),
            "delay-in-ms" => cap_delay_in_ms(&input),
            "calculate" => cap_calculate(&input),
            "format-date-from-iso" => cap_format_date_from_iso(&input),
            "iso-to-unix-timestamp" => cap_iso_to_unix_timestamp(&input),
            "get-current-unix-timestamp" => cap_get_current_unix_timestamp(&input),
            "get-current-iso-datetime" => cap_get_current_iso_datetime(&input),
            "get-current-formatted-datetime" => cap_get_current_formatted_datetime(&input),
            "country-name-to-iso-code" => cap_country_name_to_iso_code(&input),
            other => Err(ErrorInfo {
                code: "UNKNOWN_CAPABILITY".into(),
                message: format!("utils agent has no capability `{other}`"),
                category: "permanent".into(),
                severity: "error".into(),
                retryable: false,
                retry_after_ms: None,
                attributes: None,
            }),
        }
    }
}

// -----------------------------------------------------------------------------
// Capability implementations
// -----------------------------------------------------------------------------

fn cap_random_double(input_json: &str) -> Result<String, ErrorInfo> {
    let _input: RandomDoubleInput = serde_json::from_str(input_json).map_err(bad_json)?;
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let v: f64 = rng.r#gen();
    serde_json::to_string(&v).map_err(bad_json)
}

fn cap_random_array(input_json: &str) -> Result<String, ErrorInfo> {
    let input: ReturnRandomArrayInput = serde_json::from_str(input_json).map_err(bad_json)?;
    if input.size < 0 {
        return Err(capability_err("Size cannot be negative"));
    }
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let result: Vec<i64> = (0..input.size)
        .map(|_| rng.gen_range(0..=input.size) as i64)
        .collect();
    serde_json::to_string(&result).map_err(bad_json)
}

fn cap_return_input_string(input_json: &str) -> Result<String, ErrorInfo> {
    let input: ReturnStringInput = serde_json::from_str(input_json).map_err(bad_json)?;
    serde_json::to_string(&input.value).map_err(bad_json)
}

fn cap_return_input(input_json: &str) -> Result<String, ErrorInfo> {
    let input: ReturnInputData = serde_json::from_str(input_json).map_err(bad_json)?;
    serde_json::to_string(&input.value).map_err(bad_json)
}

fn cap_do_nothing(input_json: &str) -> Result<String, ErrorInfo> {
    let _input: DoNothingInput = serde_json::from_str(input_json).map_err(bad_json)?;
    Ok("null".into())
}

fn cap_delay_in_ms(input_json: &str) -> Result<String, ErrorInfo> {
    let input: DelayInMsInput = serde_json::from_str(input_json).map_err(bad_json)?;
    // std::thread::sleep routes to wasi:clocks/monotonic-clock.subscribe-duration under
    // wasm32-wasip2 when compiled with the wasi target. This is the idiomatic approach.
    std::thread::sleep(std::time::Duration::from_millis(input.delay_value));
    serde_json::to_string(&input.delay_value).map_err(bad_json)
}

fn cap_calculate(input_json: &str) -> Result<String, ErrorInfo> {
    let input: CalculateInput = serde_json::from_str(input_json).map_err(bad_json)?;

    let mut variables: HashMap<String, f64> = HashMap::new();
    for (key, value) in input.variables {
        let num = match &value {
            serde_json::Value::Number(n) => n.as_f64().ok_or_else(|| {
                capability_err(format!("Variable '{}' has invalid number: {}", key, n))
            })?,
            serde_json::Value::String(s) => s.parse::<f64>().map_err(|_| {
                capability_err(format!(
                    "Variable '{}' cannot be parsed as number: '{}'",
                    key, s
                ))
            })?,
            serde_json::Value::Null => {
                return Err(capability_err(format!("Variable '{}' is null", key)));
            }
            _ => {
                return Err(capability_err(format!(
                    "Variable '{}' must be a number or string, got: {:?}",
                    key, value
                )));
            }
        };
        variables.insert(key, num);
    }

    let result = calculate_expression(
        &input.expression,
        &variables,
        input.enable_rounding,
        input.decimal_places,
    )
    .map_err(capability_err)?;

    serde_json::to_string(&result).map_err(bad_json)
}

fn cap_format_date_from_iso(input_json: &str) -> Result<String, ErrorInfo> {
    let input: FormatDateFromIsoInput = serde_json::from_str(input_json).map_err(bad_json)?;
    if input.iso_date.trim().is_empty() {
        return Err(capability_err("ISO date cannot be null or empty"));
    }
    if input.target_format.trim().is_empty() {
        return Err(capability_err("Target format cannot be null or empty"));
    }
    let formatted =
        parse_and_format_datetime(&input.iso_date, &input.target_format).map_err(capability_err)?;
    serde_json::to_string(&formatted).map_err(bad_json)
}

fn cap_iso_to_unix_timestamp(input_json: &str) -> Result<String, ErrorInfo> {
    let input: IsoToUnixTimestampInput = serde_json::from_str(input_json).map_err(bad_json)?;
    if input.iso_date.trim().is_empty() {
        return Err(capability_err("ISO date cannot be null or empty"));
    }
    let ts = parse_iso_to_unix(&input.iso_date).map_err(capability_err)?;
    serde_json::to_string(&ts).map_err(bad_json)
}

fn cap_get_current_unix_timestamp(input_json: &str) -> Result<String, ErrorInfo> {
    let _input: GetCurrentUnixTimestampInput =
        serde_json::from_str(input_json).map_err(bad_json)?;
    let ts = current_unix_timestamp();
    serde_json::to_string(&ts).map_err(bad_json)
}

fn cap_get_current_iso_datetime(input_json: &str) -> Result<String, ErrorInfo> {
    let _input: GetCurrentIsoDatetimeInput = serde_json::from_str(input_json).map_err(bad_json)?;
    let ts = current_unix_timestamp() as u64;
    let (year, month, day, hour, minute, second) = unix_to_datetime(ts);
    let iso = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hour, minute, second
    );
    serde_json::to_string(&iso).map_err(bad_json)
}

fn cap_get_current_formatted_datetime(input_json: &str) -> Result<String, ErrorInfo> {
    let input: GetCurrentFormattedDateTimeInput =
        serde_json::from_str(input_json).map_err(bad_json)?;
    if input.format.trim().is_empty() {
        return Err(capability_err("Format cannot be null or empty"));
    }
    let ts = current_unix_timestamp() as u64;
    let formatted = format_timestamp(&ts, &input.format).map_err(capability_err)?;
    serde_json::to_string(&formatted).map_err(bad_json)
}

fn cap_country_name_to_iso_code(input_json: &str) -> Result<String, ErrorInfo> {
    let input: CountryNameToIsoCodeInput = serde_json::from_str(input_json).map_err(bad_json)?;
    if input.country_name.trim().is_empty() {
        return Err(capability_err("Country name cannot be null or empty"));
    }
    let code_type = input.code_type.trim().to_lowercase();
    if code_type != "alpha2" && code_type != "alpha3" {
        return Err(capability_err("Code type must be 'alpha2' or 'alpha3'"));
    }
    let code = find_country_code(&input.country_name, &code_type).map_err(capability_err)?;
    serde_json::to_string(&code).map_err(bad_json)
}

// -----------------------------------------------------------------------------
// Current time — WASI-compatible
// -----------------------------------------------------------------------------

/// Returns the current Unix timestamp in seconds.
///
/// Under wasm32-wasip2, `std::time::SystemTime::now()` routes through
/// `wasi:clocks/wall-clock.now()`. We fall back to computing from the
/// monotonic clock if needed, but the std path is simplest.
fn current_unix_timestamp() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// -----------------------------------------------------------------------------
// Helper Functions — Expression Calculation (ported from legacy utils.rs)
// -----------------------------------------------------------------------------

fn calculate_expression(
    expression: &str,
    variables: &HashMap<String, f64>,
    enable_rounding: bool,
    decimal_places: Option<u32>,
) -> Result<f64, String> {
    let expr = expression.trim();

    if expr.is_empty() {
        return Err("Expression cannot be null or empty".to_string());
    }

    if enable_rounding {
        if let Some(places) = decimal_places {
            if places > 15 {
                return Err("Decimal places cannot exceed 15".to_string());
            }
        }
    }

    let expr_vars = validate_and_extract_variables(expr)?;
    validate_variables_present(&expr_vars, variables)?;

    let substituted = substitute_variables(expr, variables);
    let result = evaluate_simple_expression(&substituted)?;

    if enable_rounding {
        if let Some(places) = decimal_places {
            let multiplier = 10f64.powi(places as i32);
            Ok((result * multiplier).round() / multiplier)
        } else {
            Ok(result)
        }
    } else {
        Ok(result)
    }
}

fn validate_and_extract_variables(expression: &str) -> Result<Vec<String>, String> {
    for ch in expression.chars() {
        if !ch.is_alphanumeric()
            && !matches!(
                ch,
                '+' | '-' | '*' | '/' | '%' | '(' | ')' | '.' | ' ' | '_'
            )
        {
            return Err(format!("Expression contains invalid character: '{}'", ch));
        }
    }

    let mut variables = Vec::new();
    let mut current_var = String::new();
    let mut chars = expression.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch.is_alphabetic() || ch == '_' {
            current_var.push(ch);
            while let Some(&next_ch) = chars.peek() {
                if next_ch.is_alphanumeric() || next_ch == '_' {
                    current_var.push(chars.next().unwrap());
                } else {
                    break;
                }
            }
            if !is_reserved_keyword(&current_var) && !variables.contains(&current_var) {
                variables.push(current_var.clone());
            }
            current_var.clear();
        }
    }

    Ok(variables)
}

fn is_reserved_keyword(word: &str) -> bool {
    matches!(
        word.to_lowercase().as_str(),
        "true" | "false" | "null" | "and" | "or" | "not" | "in" | "is"
    )
}

fn validate_variables_present(
    expr_vars: &[String],
    variables: &HashMap<String, f64>,
) -> Result<(), String> {
    for var in expr_vars {
        if !variables.contains_key(var) {
            return Err(format!(
                "Variable '{}' is not present in the variables map",
                var
            ));
        }
    }
    Ok(())
}

fn substitute_variables(expression: &str, variables: &HashMap<String, f64>) -> String {
    let mut result = String::new();
    let mut chars = expression.chars().peekable();
    let mut current_word = String::new();

    while let Some(ch) = chars.next() {
        if ch.is_alphabetic() || ch == '_' {
            current_word.push(ch);
            while let Some(&next_ch) = chars.peek() {
                if next_ch.is_alphanumeric() || next_ch == '_' {
                    current_word.push(chars.next().unwrap());
                } else {
                    break;
                }
            }
            if let Some(&value) = variables.get(&current_word) {
                result.push_str(&value.to_string());
            } else {
                result.push_str(&current_word);
            }
            current_word.clear();
        } else {
            result.push(ch);
        }
    }

    result
}

fn evaluate_simple_expression(expr: &str) -> Result<f64, String> {
    let expr = expr.replace(' ', "");
    parse_addition(&expr, &mut 0)
}

fn parse_addition(expr: &str, pos: &mut usize) -> Result<f64, String> {
    let mut result = parse_multiplication(expr, pos)?;

    while *pos < expr.len() {
        let ch = expr.chars().nth(*pos).unwrap();
        if ch == '+' {
            *pos += 1;
            result += parse_multiplication(expr, pos)?;
        } else if ch == '-' {
            *pos += 1;
            result -= parse_multiplication(expr, pos)?;
        } else {
            break;
        }
    }

    Ok(result)
}

fn parse_multiplication(expr: &str, pos: &mut usize) -> Result<f64, String> {
    let mut result = parse_unary(expr, pos)?;

    while *pos < expr.len() {
        let ch = expr.chars().nth(*pos).unwrap();
        if ch == '*' {
            *pos += 1;
            result *= parse_unary(expr, pos)?;
        } else if ch == '/' {
            *pos += 1;
            let divisor = parse_unary(expr, pos)?;
            if divisor == 0.0 {
                return Err("Division by zero".to_string());
            }
            result /= divisor;
        } else if ch == '%' {
            *pos += 1;
            result %= parse_unary(expr, pos)?;
        } else {
            break;
        }
    }

    Ok(result)
}

fn parse_unary(expr: &str, pos: &mut usize) -> Result<f64, String> {
    if *pos >= expr.len() {
        return Err("Unexpected end of expression".to_string());
    }

    let ch = expr.chars().nth(*pos).unwrap();

    if ch == '-' {
        *pos += 1;
        return Ok(-parse_primary(expr, pos)?);
    } else if ch == '+' {
        *pos += 1;
        return parse_primary(expr, pos);
    }

    parse_primary(expr, pos)
}

fn parse_primary(expr: &str, pos: &mut usize) -> Result<f64, String> {
    if *pos >= expr.len() {
        return Err("Unexpected end of expression".to_string());
    }

    let ch = expr.chars().nth(*pos).unwrap();

    if ch == '(' {
        *pos += 1;
        let result = parse_addition(expr, pos)?;
        if *pos >= expr.len() || expr.chars().nth(*pos).unwrap() != ')' {
            return Err("Mismatched parentheses".to_string());
        }
        *pos += 1;
        return Ok(result);
    }

    let start = *pos;
    while *pos < expr.len() {
        let c = expr.chars().nth(*pos).unwrap();
        if c.is_numeric() || c == '.' {
            *pos += 1;
        } else {
            break;
        }
    }

    if start == *pos {
        return Err(format!("Expected number at position {}", pos));
    }

    let num_str = &expr[start..*pos];
    num_str
        .parse::<f64>()
        .map_err(|_| format!("Invalid number: {}", num_str))
}

// -----------------------------------------------------------------------------
// Helper Functions — Date/Time (ported from legacy utils.rs)
// -----------------------------------------------------------------------------

fn parse_iso_to_unix(iso_date: &str) -> Result<i64, String> {
    let date_str = iso_date.trim().trim_end_matches('Z');
    let parts: Vec<&str> = date_str.split('T').collect();

    if parts.len() != 2 {
        return Err(format!("Invalid ISO date format: {}", iso_date));
    }

    let date_parts: Vec<&str> = parts[0].split('-').collect();
    let time_parts: Vec<&str> = parts[1].split(':').collect();

    if date_parts.len() != 3 || time_parts.len() != 3 {
        return Err(format!("Invalid ISO date format: {}", iso_date));
    }

    let year: i32 = date_parts[0]
        .parse()
        .map_err(|_| "Invalid year".to_string())?;
    let month: u32 = date_parts[1]
        .parse()
        .map_err(|_| "Invalid month".to_string())?;
    let day: u32 = date_parts[2]
        .parse()
        .map_err(|_| "Invalid day".to_string())?;
    let hour: u32 = time_parts[0]
        .parse()
        .map_err(|_| "Invalid hour".to_string())?;
    let minute: u32 = time_parts[1]
        .parse()
        .map_err(|_| "Invalid minute".to_string())?;
    let second: u32 = time_parts[2]
        .parse()
        .map_err(|_| "Invalid second".to_string())?;

    Ok(calculate_unix_timestamp(
        year, month, day, hour, minute, second,
    ))
}

fn calculate_unix_timestamp(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> i64 {
    const DAYS_IN_MONTH: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

    let mut days: i64 = 0;

    for y in 1970..year {
        days += if is_leap_year(y) { 366 } else { 365 };
    }

    for m in 1..month {
        days += DAYS_IN_MONTH[(m - 1) as usize] as i64;
        if m == 2 && is_leap_year(year) {
            days += 1;
        }
    }

    days += (day - 1) as i64;

    days * 86400 + hour as i64 * 3600 + minute as i64 * 60 + second as i64
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

fn unix_to_datetime(timestamp: u64) -> (i32, u32, u32, u32, u32, u32) {
    const DAYS_IN_MONTH: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

    let total_seconds = timestamp;
    let days = total_seconds / 86400;
    let remaining = total_seconds % 86400;

    let hour = (remaining / 3600) as u32;
    let minute = ((remaining % 3600) / 60) as u32;
    let second = (remaining % 60) as u32;

    let mut year: i32 = 1970;
    let mut remaining_days = days;

    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 } as u64;
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }

    let mut month = 1u32;
    let mut day = remaining_days as u32 + 1;

    for (m, &base_days) in DAYS_IN_MONTH.iter().enumerate() {
        let mut days_in_month = base_days;
        if m == 1 && is_leap_year(year) {
            days_in_month = 29;
        }

        if day <= days_in_month {
            month = m as u32 + 1;
            break;
        }
        day -= days_in_month;
    }

    (year, month, day, hour, minute, second)
}

fn parse_and_format_datetime(iso_date: &str, format: &str) -> Result<String, String> {
    let timestamp = parse_iso_to_unix(iso_date)?;
    format_timestamp(&(timestamp as u64), format)
}

fn format_timestamp(timestamp: &u64, format: &str) -> Result<String, String> {
    let (year, month, day, hour, minute, second) = unix_to_datetime(*timestamp);

    let mut result = format.to_string();
    result = result.replace("yyyy", &format!("{:04}", year));
    result = result.replace("yy", &format!("{:02}", year % 100));
    result = result.replace("MM", &format!("{:02}", month));
    result = result.replace("dd", &format!("{:02}", day));
    result = result.replace("HH", &format!("{:02}", hour));
    result = result.replace("mm", &format!("{:02}", minute));
    result = result.replace("ss", &format!("{:02}", second));

    Ok(result)
}

// -----------------------------------------------------------------------------
// Helper Functions — Country Codes (ported from legacy utils.rs)
// -----------------------------------------------------------------------------

fn find_country_code(country_name: &str, code_type: &str) -> Result<String, String> {
    let mappings = initialize_country_mappings();
    let normalized_input = normalize_country_name(country_name);

    for (name, codes) in &mappings {
        if normalize_country_name(name) == normalized_input {
            return Ok(if code_type == "alpha2" {
                codes[0].to_string()
            } else {
                codes[1].to_string()
            });
        }
    }

    for codes in mappings.values() {
        for alt_name in codes.iter().skip(2) {
            if normalize_country_name(alt_name) == normalized_input {
                return Ok(if code_type == "alpha2" {
                    codes[0].to_string()
                } else {
                    codes[1].to_string()
                });
            }
        }
    }

    let threshold = std::cmp::max(3, country_name.len() / 4);
    let mut best_match: Option<(&str, &Vec<&str>)> = None;
    let mut min_distance = usize::MAX;

    for (name, codes) in &mappings {
        let distance = levenshtein_distance(&normalized_input, &normalize_country_name(name));
        if distance < min_distance && distance <= threshold {
            min_distance = distance;
            best_match = Some((name, codes));
        }

        for alt_name in codes.iter().skip(2) {
            let distance =
                levenshtein_distance(&normalized_input, &normalize_country_name(alt_name));
            if distance < min_distance && distance <= threshold {
                min_distance = distance;
                best_match = Some((name, codes));
            }
        }
    }

    if let Some((_name, codes)) = best_match {
        return Ok(if code_type == "alpha2" {
            codes[0].to_string()
        } else {
            codes[1].to_string()
        });
    }

    Err(format!("Country not found: {}", country_name))
}

fn normalize_country_name(name: &str) -> String {
    name.to_lowercase()
        .trim()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn levenshtein_distance(s1: &str, s2: &str) -> usize {
    let len1 = s1.len();
    let len2 = s2.len();
    let mut matrix = vec![vec![0usize; len2 + 1]; len1 + 1];

    for (i, row) in matrix.iter_mut().enumerate().take(len1 + 1) {
        row[0] = i;
    }
    for (j, cell) in matrix[0].iter_mut().enumerate().take(len2 + 1) {
        *cell = j;
    }

    let s1_chars: Vec<char> = s1.chars().collect();
    let s2_chars: Vec<char> = s2.chars().collect();

    for i in 1..=len1 {
        for j in 1..=len2 {
            let cost = if s1_chars[i - 1] == s2_chars[j - 1] {
                0
            } else {
                1
            };
            matrix[i][j] = std::cmp::min(
                std::cmp::min(matrix[i - 1][j] + 1, matrix[i][j - 1] + 1),
                matrix[i - 1][j - 1] + cost,
            );
        }
    }

    matrix[len1][len2]
}

fn initialize_country_mappings() -> HashMap<&'static str, Vec<&'static str>> {
    let mut mappings = HashMap::new();

    // Format: Country Name -> [alpha2, alpha3, alternative names...]
    mappings.insert("Afghanistan", vec!["AF", "AFG"]);
    mappings.insert("Albania", vec!["AL", "ALB"]);
    mappings.insert("Algeria", vec!["DZ", "DZA"]);
    mappings.insert("Andorra", vec!["AD", "AND"]);
    mappings.insert("Angola", vec!["AO", "AGO"]);
    mappings.insert("Argentina", vec!["AR", "ARG"]);
    mappings.insert("Armenia", vec!["AM", "ARM"]);
    mappings.insert("Australia", vec!["AU", "AUS"]);
    mappings.insert("Austria", vec!["AT", "AUT"]);
    mappings.insert("Azerbaijan", vec!["AZ", "AZE"]);
    mappings.insert("Bahamas", vec!["BS", "BHS", "The Bahamas"]);
    mappings.insert("Bahrain", vec!["BH", "BHR"]);
    mappings.insert("Bangladesh", vec!["BD", "BGD"]);
    mappings.insert("Barbados", vec!["BB", "BRB"]);
    mappings.insert("Belarus", vec!["BY", "BLR"]);
    mappings.insert("Belgium", vec!["BE", "BEL"]);
    mappings.insert("Belize", vec!["BZ", "BLZ"]);
    mappings.insert("Benin", vec!["BJ", "BEN"]);
    mappings.insert("Bhutan", vec!["BT", "BTN"]);
    mappings.insert("Bolivia", vec!["BO", "BOL"]);
    mappings.insert(
        "Bosnia and Herzegovina",
        vec!["BA", "BIH", "Bosnia", "Herzegovina"],
    );
    mappings.insert("Botswana", vec!["BW", "BWA"]);
    mappings.insert("Brazil", vec!["BR", "BRA", "Brasil"]);
    mappings.insert("Brunei", vec!["BN", "BRN", "Brunei Darussalam"]);
    mappings.insert("Bulgaria", vec!["BG", "BGR"]);
    mappings.insert("Burkina Faso", vec!["BF", "BFA"]);
    mappings.insert("Burundi", vec!["BI", "BDI"]);
    mappings.insert("Cambodia", vec!["KH", "KHM"]);
    mappings.insert("Cameroon", vec!["CM", "CMR"]);
    mappings.insert("Canada", vec!["CA", "CAN"]);
    mappings.insert("Cape Verde", vec!["CV", "CPV", "Cabo Verde"]);
    mappings.insert("Central African Republic", vec!["CF", "CAF"]);
    mappings.insert("Chad", vec!["TD", "TCD"]);
    mappings.insert("Chile", vec!["CL", "CHL"]);
    mappings.insert(
        "China",
        vec!["CN", "CHN", "People's Republic of China", "PRC"],
    );
    mappings.insert("Colombia", vec!["CO", "COL"]);
    mappings.insert("Comoros", vec!["KM", "COM"]);
    mappings.insert("Congo", vec!["CG", "COG", "Republic of the Congo"]);
    mappings.insert(
        "Congo Democratic Republic",
        vec!["CD", "COD", "DRC", "Democratic Republic of the Congo"],
    );
    mappings.insert("Costa Rica", vec!["CR", "CRI"]);
    mappings.insert("Croatia", vec!["HR", "HRV"]);
    mappings.insert("Cuba", vec!["CU", "CUB"]);
    mappings.insert("Cyprus", vec!["CY", "CYP"]);
    mappings.insert("Czech Republic", vec!["CZ", "CZE", "Czechia"]);
    mappings.insert("Denmark", vec!["DK", "DNK"]);
    mappings.insert("Djibouti", vec!["DJ", "DJI"]);
    mappings.insert("Dominica", vec!["DM", "DMA"]);
    mappings.insert("Dominican Republic", vec!["DO", "DOM"]);
    mappings.insert("Ecuador", vec!["EC", "ECU"]);
    mappings.insert("Egypt", vec!["EG", "EGY"]);
    mappings.insert("El Salvador", vec!["SV", "SLV"]);
    mappings.insert("Equatorial Guinea", vec!["GQ", "GNQ"]);
    mappings.insert("Eritrea", vec!["ER", "ERI"]);
    mappings.insert("Estonia", vec!["EE", "EST"]);
    mappings.insert("Eswatini", vec!["SZ", "SWZ", "Swaziland"]);
    mappings.insert("Ethiopia", vec!["ET", "ETH"]);
    mappings.insert("Fiji", vec!["FJ", "FJI"]);
    mappings.insert("Finland", vec!["FI", "FIN"]);
    mappings.insert("France", vec!["FR", "FRA"]);
    mappings.insert("Gabon", vec!["GA", "GAB"]);
    mappings.insert("Gambia", vec!["GM", "GMB", "The Gambia"]);
    mappings.insert("Georgia", vec!["GE", "GEO"]);
    mappings.insert("Germany", vec!["DE", "DEU", "Deutschland"]);
    mappings.insert("Ghana", vec!["GH", "GHA"]);
    mappings.insert("Greece", vec!["GR", "GRC"]);
    mappings.insert("Grenada", vec!["GD", "GRD"]);
    mappings.insert("Guatemala", vec!["GT", "GTM"]);
    mappings.insert("Guinea", vec!["GN", "GIN"]);
    mappings.insert("Guinea-Bissau", vec!["GW", "GNB"]);
    mappings.insert("Guyana", vec!["GY", "GUY"]);
    mappings.insert("Haiti", vec!["HT", "HTI"]);
    mappings.insert("Honduras", vec!["HN", "HND"]);
    mappings.insert("Hungary", vec!["HU", "HUN"]);
    mappings.insert("Iceland", vec!["IS", "ISL"]);
    mappings.insert("India", vec!["IN", "IND"]);
    mappings.insert("Indonesia", vec!["ID", "IDN"]);
    mappings.insert("Iran", vec!["IR", "IRN", "Islamic Republic of Iran"]);
    mappings.insert("Iraq", vec!["IQ", "IRQ"]);
    mappings.insert("Ireland", vec!["IE", "IRL", "Republic of Ireland"]);
    mappings.insert("Israel", vec!["IL", "ISR"]);
    mappings.insert("Italy", vec!["IT", "ITA", "Italia"]);
    mappings.insert("Ivory Coast", vec!["CI", "CIV", "Cote d'Ivoire"]);
    mappings.insert("Jamaica", vec!["JM", "JAM"]);
    mappings.insert("Japan", vec!["JP", "JPN"]);
    mappings.insert("Jordan", vec!["JO", "JOR"]);
    mappings.insert("Kazakhstan", vec!["KZ", "KAZ"]);
    mappings.insert("Kenya", vec!["KE", "KEN"]);
    mappings.insert("Kiribati", vec!["KI", "KIR"]);
    mappings.insert("Kuwait", vec!["KW", "KWT"]);
    mappings.insert("Kyrgyzstan", vec!["KG", "KGZ"]);
    mappings.insert("Laos", vec!["LA", "LAO"]);
    mappings.insert("Latvia", vec!["LV", "LVA"]);
    mappings.insert("Lebanon", vec!["LB", "LBN"]);
    mappings.insert("Lesotho", vec!["LS", "LSO"]);
    mappings.insert("Liberia", vec!["LR", "LBR"]);
    mappings.insert("Libya", vec!["LY", "LBY"]);
    mappings.insert("Liechtenstein", vec!["LI", "LIE"]);
    mappings.insert("Lithuania", vec!["LT", "LTU"]);
    mappings.insert("Luxembourg", vec!["LU", "LUX"]);
    mappings.insert("Madagascar", vec!["MG", "MDG"]);
    mappings.insert("Malawi", vec!["MW", "MWI"]);
    mappings.insert("Malaysia", vec!["MY", "MYS"]);
    mappings.insert("Maldives", vec!["MV", "MDV"]);
    mappings.insert("Mali", vec!["ML", "MLI"]);
    mappings.insert("Malta", vec!["MT", "MLT"]);
    mappings.insert("Marshall Islands", vec!["MH", "MHL"]);
    mappings.insert("Mauritania", vec!["MR", "MRT"]);
    mappings.insert("Mauritius", vec!["MU", "MUS"]);
    mappings.insert("Mexico", vec!["MX", "MEX"]);
    mappings.insert("Micronesia", vec!["FM", "FSM"]);
    mappings.insert("Moldova", vec!["MD", "MDA", "Republic of Moldova"]);
    mappings.insert("Monaco", vec!["MC", "MCO"]);
    mappings.insert("Mongolia", vec!["MN", "MNG"]);
    mappings.insert("Montenegro", vec!["ME", "MNE"]);
    mappings.insert("Morocco", vec!["MA", "MAR"]);
    mappings.insert("Mozambique", vec!["MZ", "MOZ"]);
    mappings.insert("Myanmar", vec!["MM", "MMR", "Burma"]);
    mappings.insert("Namibia", vec!["NA", "NAM"]);
    mappings.insert("Nauru", vec!["NR", "NRU"]);
    mappings.insert("Nepal", vec!["NP", "NPL"]);
    mappings.insert("Netherlands", vec!["NL", "NLD", "Holland"]);
    mappings.insert("New Zealand", vec!["NZ", "NZL"]);
    mappings.insert("Nicaragua", vec!["NI", "NIC"]);
    mappings.insert("Niger", vec!["NE", "NER"]);
    mappings.insert("Nigeria", vec!["NG", "NGA"]);
    mappings.insert("North Korea", vec!["KP", "PRK", "DPRK"]);
    mappings.insert("North Macedonia", vec!["MK", "MKD", "Macedonia"]);
    mappings.insert("Norway", vec!["NO", "NOR"]);
    mappings.insert("Oman", vec!["OM", "OMN"]);
    mappings.insert("Pakistan", vec!["PK", "PAK"]);
    mappings.insert("Palau", vec!["PW", "PLW"]);
    mappings.insert("Palestine", vec!["PS", "PSE", "Palestinian Territory"]);
    mappings.insert("Panama", vec!["PA", "PAN"]);
    mappings.insert("Papua New Guinea", vec!["PG", "PNG"]);
    mappings.insert("Paraguay", vec!["PY", "PRY"]);
    mappings.insert("Peru", vec!["PE", "PER"]);
    mappings.insert("Philippines", vec!["PH", "PHL"]);
    mappings.insert("Poland", vec!["PL", "POL"]);
    mappings.insert("Portugal", vec!["PT", "PRT"]);
    mappings.insert("Qatar", vec!["QA", "QAT"]);
    mappings.insert("Romania", vec!["RO", "ROU"]);
    mappings.insert("Russia", vec!["RU", "RUS", "Russian Federation"]);
    mappings.insert("Rwanda", vec!["RW", "RWA"]);
    mappings.insert("Saint Kitts and Nevis", vec!["KN", "KNA"]);
    mappings.insert("Saint Lucia", vec!["LC", "LCA"]);
    mappings.insert("Saint Vincent and the Grenadines", vec!["VC", "VCT"]);
    mappings.insert("Samoa", vec!["WS", "WSM"]);
    mappings.insert("San Marino", vec!["SM", "SMR"]);
    mappings.insert("Sao Tome and Principe", vec!["ST", "STP"]);
    mappings.insert("Saudi Arabia", vec!["SA", "SAU", "KSA"]);
    mappings.insert("Senegal", vec!["SN", "SEN"]);
    mappings.insert("Serbia", vec!["RS", "SRB"]);
    mappings.insert("Seychelles", vec!["SC", "SYC"]);
    mappings.insert("Sierra Leone", vec!["SL", "SLE"]);
    mappings.insert("Singapore", vec!["SG", "SGP"]);
    mappings.insert("Slovakia", vec!["SK", "SVK"]);
    mappings.insert("Slovenia", vec!["SI", "SVN"]);
    mappings.insert("Solomon Islands", vec!["SB", "SLB"]);
    mappings.insert("Somalia", vec!["SO", "SOM"]);
    mappings.insert("South Africa", vec!["ZA", "ZAF"]);
    mappings.insert("South Korea", vec!["KR", "KOR", "Republic of Korea"]);
    mappings.insert("South Sudan", vec!["SS", "SSD"]);
    mappings.insert("Spain", vec!["ES", "ESP"]);
    mappings.insert("Sri Lanka", vec!["LK", "LKA"]);
    mappings.insert("Sudan", vec!["SD", "SDN"]);
    mappings.insert("Suriname", vec!["SR", "SUR"]);
    mappings.insert("Sweden", vec!["SE", "SWE"]);
    mappings.insert("Switzerland", vec!["CH", "CHE"]);
    mappings.insert("Syria", vec!["SY", "SYR", "Syrian Arab Republic"]);
    mappings.insert("Taiwan", vec!["TW", "TWN", "Republic of China"]);
    mappings.insert("Tajikistan", vec!["TJ", "TJK"]);
    mappings.insert("Tanzania", vec!["TZ", "TZA"]);
    mappings.insert("Thailand", vec!["TH", "THA"]);
    mappings.insert("Timor-Leste", vec!["TL", "TLS", "East Timor"]);
    mappings.insert("Togo", vec!["TG", "TGO"]);
    mappings.insert("Tonga", vec!["TO", "TON"]);
    mappings.insert("Trinidad and Tobago", vec!["TT", "TTO"]);
    mappings.insert("Tunisia", vec!["TN", "TUN"]);
    mappings.insert("Turkey", vec!["TR", "TUR"]);
    mappings.insert("Turkmenistan", vec!["TM", "TKM"]);
    mappings.insert("Tuvalu", vec!["TV", "TUV"]);
    mappings.insert("Uganda", vec!["UG", "UGA"]);
    mappings.insert("Ukraine", vec!["UA", "UKR"]);
    mappings.insert("United Arab Emirates", vec!["AE", "ARE", "UAE"]);
    mappings.insert("United Kingdom", vec!["GB", "GBR", "UK", "Britain"]);
    mappings.insert(
        "United States",
        vec!["US", "USA", "United States of America", "America"],
    );
    mappings.insert("Uruguay", vec!["UY", "URY"]);
    mappings.insert("Uzbekistan", vec!["UZ", "UZB"]);
    mappings.insert("Vanuatu", vec!["VU", "VUT"]);
    mappings.insert("Vatican City", vec!["VA", "VAT", "Holy See"]);
    mappings.insert("Venezuela", vec!["VE", "VEN"]);
    mappings.insert("Vietnam", vec!["VN", "VNM"]);
    mappings.insert("Yemen", vec!["YE", "YEM"]);
    mappings.insert("Zambia", vec!["ZM", "ZMB"]);
    mappings.insert("Zimbabwe", vec!["ZW", "ZWE"]);

    mappings
}

bindings::export!(Component with_types_in bindings);

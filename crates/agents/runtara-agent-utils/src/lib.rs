//! Utility agent — math, dates, strings, country codes — as a WebAssembly component.
//!
//! Capability metadata travels through `#[capability_input]` / `#[capability]`
//! annotations on the same Rust types and functions that the wasm cdylib's
//! `invoke` dispatcher calls into. The workspace binary
//! `runtara-agent-bundle-emit` reads these macro-emitted `&'static` statics on
//! the host architecture and writes `runtara_agent_utils.meta.json` next to
//! the `.wasm` — the JSON is a build artifact, never hand-edited.
//!
//! Schema matches the legacy `runtara-agents/src/agents/utils.rs` agent so
//! A/B parity tests can compare results.

use rand::Rng;
use runtara_agent_macro::{CapabilityInput, capability};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

// -----------------------------------------------------------------------------
// Inputs (with capability macros so meta.json can be derived)
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Random Double Input")]
pub struct RandomDoubleInput {}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Random Array Input")]
pub struct ReturnRandomArrayInput {
    #[field(
        display_name = "Array Size",
        description = "The number of random integers to generate",
        example = "5"
    )]
    pub size: i32,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "String Input")]
pub struct ReturnStringInput {
    #[field(
        display_name = "Value",
        description = "The string value to return as output",
        example = "Hello World"
    )]
    pub value: String,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "JSON Value Input")]
pub struct ReturnInputData {
    #[field(
        display_name = "Value",
        description = "The JSON value to return as output"
    )]
    pub value: Value,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "No Operation Input")]
pub struct DoNothingInput {}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delay Input")]
pub struct DelayInMsInput {
    #[field(
        display_name = "Delay (milliseconds)",
        description = "The amount of time to pause execution in milliseconds",
        example = "1000"
    )]
    pub delay_value: u64,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Calculate Expression Input")]
pub struct CalculateInput {
    #[field(
        display_name = "Expression",
        description = "Mathematical expression with variables and operators (+, -, *, /, %, parentheses)",
        example = "(x + y) * 2"
    )]
    pub expression: String,

    #[field(
        display_name = "Variables",
        description = "Map of variable names to their numeric values (supports numbers and string numbers)"
    )]
    pub variables: HashMap<String, Value>,

    #[field(
        display_name = "Enable Rounding",
        description = "Round the result to the specified number of decimal places",
        default = "false"
    )]
    #[serde(default)]
    pub enable_rounding: bool,

    #[field(
        display_name = "Decimal Places",
        description = "Number of decimal places to round to (maximum 15)",
        example = "2"
    )]
    pub decimal_places: Option<u32>,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Format Date Input")]
pub struct FormatDateFromIsoInput {
    #[field(
        display_name = "ISO Date",
        description = "The date/time in ISO 8601 format (e.g., 2024-01-15T10:30:00Z)",
        example = "2024-01-15T10:30:00Z"
    )]
    pub iso_date: String,

    #[field(
        display_name = "Target Format",
        description = "Format pattern using yyyy, MM, dd, HH, mm, ss tokens",
        example = "yyyy-MM-dd HH:mm:ss"
    )]
    pub target_format: String,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "ISO to Unix Timestamp Input")]
pub struct IsoToUnixTimestampInput {
    #[field(
        display_name = "ISO Date",
        description = "The date/time in ISO 8601 format to convert to Unix timestamp",
        example = "2024-01-15T10:30:00Z"
    )]
    pub iso_date: String,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Current Unix Timestamp Input")]
pub struct GetCurrentUnixTimestampInput {}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Current ISO Datetime Input")]
pub struct GetCurrentIsoDatetimeInput {}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Current Formatted DateTime Input")]
pub struct GetCurrentFormattedDateTimeInput {
    #[field(
        display_name = "Format",
        description = "Format pattern using yyyy, MM, dd, HH, mm, ss tokens",
        example = "yyyy-MM-dd"
    )]
    pub format: String,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Country Name to ISO Code Input")]
pub struct CountryNameToIsoCodeInput {
    #[field(
        display_name = "Country Name",
        description = "The full country name or commonly used alternative",
        example = "United States"
    )]
    pub country_name: String,

    #[field(
        display_name = "Code Type",
        description = "The type of ISO code to return (alpha2 or alpha3)",
        example = "alpha2",
        default = "alpha2"
    )]
    #[serde(default = "default_code_type")]
    pub code_type: String,
}

fn default_code_type() -> String {
    "alpha2".to_string()
}

// -----------------------------------------------------------------------------
// Capabilities — annotated for metadata; the `__executor_*` fns the macro emits
// are what the wasm Guest impl dispatches to.
// -----------------------------------------------------------------------------

#[capability(
    module = "utils",
    module_display_name = "Utils",
    module_description = "Utility operations: random numbers, math, dates, country codes, delays.",
    display_name = "Random Double",
    description = "Generate a random double between 0 and 1"
)]
pub fn random_double(_input: RandomDoubleInput) -> Result<f64, String> {
    let mut rng = rand::thread_rng();
    Ok(rng.r#gen())
}

#[capability(
    module = "utils",
    display_name = "Random Array",
    description = "Generate an array of random integers"
)]
pub fn random_array(input: ReturnRandomArrayInput) -> Result<Vec<i64>, String> {
    if input.size < 0 {
        return Err("Size cannot be negative".to_string());
    }

    let size = input.size as usize;
    let mut rng = rand::thread_rng();
    let mut result = Vec::with_capacity(size);

    for _ in 0..size {
        let random_val: i32 = rng.gen_range(0..=input.size);
        result.push(random_val as i64);
    }

    Ok(result)
}

#[capability(
    module = "utils",
    display_name = "Return String",
    description = "Returns the input string value"
)]
pub fn return_input_string(input: ReturnStringInput) -> Result<String, String> {
    Ok(input.value)
}

#[capability(
    module = "utils",
    display_name = "Return Input",
    description = "Returns the input JSON value"
)]
pub fn return_input(input: ReturnInputData) -> Result<Value, String> {
    Ok(input.value)
}

#[capability(
    module = "utils",
    display_name = "Do Nothing",
    description = "No-op operation that returns null"
)]
pub fn do_nothing(_input: DoNothingInput) -> Result<Value, String> {
    Ok(Value::Null)
}

#[capability(
    module = "utils",
    display_name = "Delay",
    description = "Delays execution for the specified number of milliseconds. For durable delays that survive crashes, use the Delay step type instead."
)]
pub fn delay_in_ms(input: DelayInMsInput) -> Result<u64, String> {
    // std::thread::sleep routes to wasi:clocks/monotonic-clock.subscribe-duration
    // under wasm32-wasip2 — idiomatic and host-portable.
    std::thread::sleep(std::time::Duration::from_millis(input.delay_value));
    Ok(input.delay_value)
}

#[capability(
    module = "utils",
    display_name = "Calculate",
    description = "Evaluate a mathematical expression with variables"
)]
pub fn calculate(input: CalculateInput) -> Result<f64, String> {
    let mut variables: HashMap<String, f64> = HashMap::new();
    for (key, value) in input.variables {
        let num = match &value {
            Value::Number(n) => n
                .as_f64()
                .ok_or_else(|| format!("Variable '{}' has invalid number: {}", key, n))?,
            Value::String(s) => s
                .parse::<f64>()
                .map_err(|_| format!("Variable '{}' cannot be parsed as number: '{}'", key, s))?,
            Value::Null => {
                return Err(format!("Variable '{}' is null", key));
            }
            _ => {
                return Err(format!(
                    "Variable '{}' must be a number or string, got: {:?}",
                    key, value
                ));
            }
        };
        variables.insert(key, num);
    }

    calculate_expression(
        &input.expression,
        &variables,
        input.enable_rounding,
        input.decimal_places,
    )
}

#[capability(
    module = "utils",
    display_name = "Format Date",
    description = "Format an ISO date to a custom format"
)]
pub fn format_date_from_iso(input: FormatDateFromIsoInput) -> Result<String, String> {
    if input.iso_date.trim().is_empty() {
        return Err("ISO date cannot be null or empty".to_string());
    }

    if input.target_format.trim().is_empty() {
        return Err("Target format cannot be null or empty".to_string());
    }

    parse_and_format_datetime(&input.iso_date, &input.target_format)
}

#[capability(
    module = "utils",
    display_name = "ISO to Unix",
    description = "Convert ISO date to Unix timestamp"
)]
pub fn iso_to_unix_timestamp(input: IsoToUnixTimestampInput) -> Result<i64, String> {
    if input.iso_date.trim().is_empty() {
        return Err("ISO date cannot be null or empty".to_string());
    }

    parse_iso_to_unix(&input.iso_date)
}

#[capability(
    module = "utils",
    display_name = "Current Unix Timestamp",
    description = "Get the current Unix timestamp"
)]
pub fn get_current_unix_timestamp(_input: GetCurrentUnixTimestampInput) -> Result<i64, String> {
    Ok(current_unix_timestamp())
}

#[capability(
    module = "utils",
    display_name = "Current ISO Datetime",
    description = "Get the current date/time in ISO format"
)]
pub fn get_current_iso_datetime(_input: GetCurrentIsoDatetimeInput) -> Result<String, String> {
    let (year, month, day, hour, minute, second) = unix_to_datetime(current_unix_timestamp());
    Ok(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hour, minute, second
    ))
}

#[capability(
    module = "utils",
    display_name = "Current Formatted DateTime",
    description = "Get the current date/time in a custom format"
)]
pub fn get_current_formatted_datetime(
    input: GetCurrentFormattedDateTimeInput,
) -> Result<String, String> {
    if input.format.trim().is_empty() {
        return Err("Format cannot be null or empty".to_string());
    }

    format_timestamp(current_unix_timestamp(), &input.format)
}

#[capability(
    module = "utils",
    display_name = "Country to ISO Code",
    description = "Convert country name to ISO code"
)]
pub fn country_name_to_iso_code(input: CountryNameToIsoCodeInput) -> Result<String, String> {
    if input.country_name.trim().is_empty() {
        return Err("Country name cannot be null or empty".to_string());
    }

    let code_type = input.code_type.trim().to_lowercase();
    if code_type != "alpha2" && code_type != "alpha3" {
        return Err("Code type must be 'alpha2' or 'alpha3'".to_string());
    }

    find_country_code(&input.country_name, &code_type)
}

// -----------------------------------------------------------------------------
// Current time — WASI-compatible
// -----------------------------------------------------------------------------

/// Returns the current Unix timestamp in seconds.
///
/// Under wasm32-wasip2, `std::time::SystemTime::now()` routes through
/// `wasi:clocks/wall-clock.now()`. On the host this uses the system clock.
fn current_unix_timestamp() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// -----------------------------------------------------------------------------
// Helper Functions — Expression Calculation
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

    if enable_rounding
        && let Some(places) = decimal_places
        && places > 15
    {
        return Err("Decimal places cannot exceed 15".to_string());
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
// Helper Functions — Date/Time
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

/// Days from 1970-01-01 to a proleptic Gregorian date, in constant time.
///
/// This is Howard Hinnant's `days_from_civil`. The previous implementation
/// counted one loop iteration per year since 1970, and nothing bounds the year
/// a caller can supply: `parse_iso_to_unix` accepts any `i32`, so a date like
/// `2147483647-01-01T00:00:00Z` spun roughly two billion iterations inside the
/// guest with no host call and no yield to cancel at. Closed-form arithmetic
/// removes that cooperation gap instead of trying to interrupt it, and it is
/// also correct before 1970, where the ascending loop simply never ran.
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = i64::from(year) - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146097 + day_of_era - 719468
}

/// The proleptic Gregorian date a day offset from 1970-01-01 falls on.
///
/// Hinnant's `civil_from_days`, the inverse of [`days_from_civil`]. It replaces
/// a loop that walked forward one year at a time; a `u64` timestamp built from
/// a negative `i64` made that loop run for hundreds of billions of iterations.
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let days = days + 719468;
    let era = if days >= 0 { days } else { days - 146096 } / 146097;
    let day_of_era = days - era * 146097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_position = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_position + 2) / 5 + 1;
    let month = month_position + if month_position < 10 { 3 } else { -9 };
    (
        (year + i64::from(month <= 2)) as i32,
        month as u32,
        day as u32,
    )
}

fn calculate_unix_timestamp(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> i64 {
    days_from_civil(year, month, day) * 86400
        + hour as i64 * 3600
        + minute as i64 * 60
        + second as i64
}

fn unix_to_datetime(timestamp: i64) -> (i32, u32, u32, u32, u32, u32) {
    let days = timestamp.div_euclid(86400);
    let remaining = timestamp.rem_euclid(86400);
    let (year, month, day) = civil_from_days(days);
    (
        year,
        month,
        day,
        (remaining / 3600) as u32,
        ((remaining % 3600) / 60) as u32,
        (remaining % 60) as u32,
    )
}

fn parse_and_format_datetime(iso_date: &str, format: &str) -> Result<String, String> {
    let timestamp = parse_iso_to_unix(iso_date)?;
    format_timestamp(timestamp, format)
}

fn format_timestamp(timestamp: i64, format: &str) -> Result<String, String> {
    let (year, month, day, hour, minute, second) = unix_to_datetime(timestamp);

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
// Helper Functions — Country Codes
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

// -----------------------------------------------------------------------------
// AgentInfo assembler (host-only; the wasm binary doesn't need it)
// -----------------------------------------------------------------------------

/// Build the canonical `AgentInfo` for this agent by walking the macro-emitted
/// `&'static` statics. The workspace `runtara-agent-bundle-emit` binary calls
/// this on the host architecture and writes the JSON to disk; the wasm binary
/// itself never executes this code, so we cfg-gate it out to keep the
/// component small.
#[cfg(not(target_arch = "wasm32"))]
pub fn agent_info() -> runtara_dsl::agent_meta::AgentInfo {
    use runtara_dsl::agent_meta::{
        AgentInfo, CapabilityMeta, InputTypeMeta, OutputTypeMeta, capability_to_api_with_types,
    };
    use std::collections::HashMap;

    let caps: &[&'static CapabilityMeta] = &[
        &__CAPABILITY_META_RANDOM_DOUBLE,
        &__CAPABILITY_META_RANDOM_ARRAY,
        &__CAPABILITY_META_RETURN_INPUT_STRING,
        &__CAPABILITY_META_RETURN_INPUT,
        &__CAPABILITY_META_DO_NOTHING,
        &__CAPABILITY_META_DELAY_IN_MS,
        &__CAPABILITY_META_CALCULATE,
        &__CAPABILITY_META_FORMAT_DATE_FROM_ISO,
        &__CAPABILITY_META_ISO_TO_UNIX_TIMESTAMP,
        &__CAPABILITY_META_GET_CURRENT_UNIX_TIMESTAMP,
        &__CAPABILITY_META_GET_CURRENT_ISO_DATETIME,
        &__CAPABILITY_META_GET_CURRENT_FORMATTED_DATETIME,
        &__CAPABILITY_META_COUNTRY_NAME_TO_ISO_CODE,
    ];
    let input_types: HashMap<&'static str, &'static InputTypeMeta> = [
        (
            "RandomDoubleInput",
            &__INPUT_META_RandomDoubleInput as &InputTypeMeta,
        ),
        (
            "ReturnRandomArrayInput",
            &__INPUT_META_ReturnRandomArrayInput,
        ),
        ("ReturnStringInput", &__INPUT_META_ReturnStringInput),
        ("ReturnInputData", &__INPUT_META_ReturnInputData),
        ("DoNothingInput", &__INPUT_META_DoNothingInput),
        ("DelayInMsInput", &__INPUT_META_DelayInMsInput),
        ("CalculateInput", &__INPUT_META_CalculateInput),
        (
            "FormatDateFromIsoInput",
            &__INPUT_META_FormatDateFromIsoInput,
        ),
        (
            "IsoToUnixTimestampInput",
            &__INPUT_META_IsoToUnixTimestampInput,
        ),
        (
            "GetCurrentUnixTimestampInput",
            &__INPUT_META_GetCurrentUnixTimestampInput,
        ),
        (
            "GetCurrentIsoDatetimeInput",
            &__INPUT_META_GetCurrentIsoDatetimeInput,
        ),
        (
            "GetCurrentFormattedDateTimeInput",
            &__INPUT_META_GetCurrentFormattedDateTimeInput,
        ),
        (
            "CountryNameToIsoCodeInput",
            &__INPUT_META_CountryNameToIsoCodeInput,
        ),
    ]
    .into_iter()
    .collect();
    // No struct outputs — every capability returns a primitive (f64, i64, u64,
    // String, Vec<i64>, or serde_json::Value). `capability_to_api` handles those
    // via `rust_to_json_schema_type`, so we pass an empty output-type registry.
    let output_types: HashMap<&'static str, &'static OutputTypeMeta> = HashMap::new();

    let capabilities = caps
        .iter()
        .map(|cap| {
            capability_to_api_with_types(
                cap,
                input_types.get(cap.input_type).copied(),
                output_types.get(cap.output_type).copied(),
                &output_types,
            )
        })
        .collect();

    AgentInfo {
        id: "utils".into(),
        name: "Utils".into(),
        description: "Utility operations: random numbers, math, dates, country codes, delays."
            .into(),
        has_side_effects: false,
        supports_connections: false,
        integration_ids: vec![],
        capabilities,
    }
}

// -----------------------------------------------------------------------------
// Wasm component plumbing
// -----------------------------------------------------------------------------

runtara_agent_macro::agent_component!(
    agent = "utils",
    capabilities = [
        random_double,
        random_array,
        return_input_string,
        return_input,
        do_nothing,
        delay_in_ms,
        calculate,
        format_date_from_iso,
        iso_to_unix_timestamp,
        get_current_unix_timestamp,
        get_current_iso_datetime,
        get_current_formatted_datetime,
        country_name_to_iso_code,
    ],
);

#[cfg(test)]
mod tests {
    use super::*;

    fn iso(value: &str) -> i64 {
        parse_iso_to_unix(value).unwrap()
    }

    #[test]
    fn epoch_and_well_known_instants_round_trip() {
        for (text, seconds) in [
            ("1970-01-01T00:00:00Z", 0),
            ("2000-01-01T00:00:00Z", 946_684_800),
            ("2001-09-09T01:46:40Z", 1_000_000_000),
            ("2024-02-29T12:34:56Z", 1_709_210_096),
            ("2038-01-19T03:14:08Z", 2_147_483_648),
        ] {
            assert_eq!(iso(text), seconds, "{text}");
            assert_eq!(
                format_timestamp(seconds, "yyyy-MM-ddTHH:mm:ss").unwrap(),
                text.trim_end_matches('Z'),
                "{text}"
            );
        }
    }

    #[test]
    fn leap_day_and_century_rules_hold_in_both_directions() {
        // 1900 is not a leap year and 2000 is, which is what separates the
        // Gregorian rule from a plain four-year cycle.
        assert_eq!(unix_to_datetime(iso("1900-03-01T00:00:00Z")).1, 3);
        assert_eq!(
            unix_to_datetime(iso("2000-02-29T00:00:00Z")),
            (2000, 2, 29, 0, 0, 0)
        );
        assert_eq!(
            iso("2000-03-01T00:00:00Z") - iso("2000-02-28T00:00:00Z"),
            2 * 86400
        );
        assert_eq!(
            iso("1900-03-01T00:00:00Z") - iso("1900-02-28T00:00:00Z"),
            86400
        );
    }

    #[test]
    fn dates_before_the_epoch_are_negative_and_reversible() {
        // The old ascending year loop never ran for these, so every pre-1970
        // date reported a positive timestamp.
        assert_eq!(iso("1969-12-31T23:59:59Z"), -1);
        assert_eq!(iso("1960-01-01T00:00:00Z"), -315_619_200);
        assert_eq!(unix_to_datetime(-315_619_200), (1960, 1, 1, 0, 0, 0));
        assert_eq!(
            format_timestamp(-1, "yyyy-MM-dd HH:mm:ss").unwrap(),
            "1969-12-31 23:59:59"
        );
    }

    #[test]
    fn every_day_of_two_leap_cycles_survives_a_round_trip() {
        let mut day = days_from_civil(1968, 1, 1);
        let last = days_from_civil(1977, 1, 1);
        while day < last {
            let (year, month, date) = civil_from_days(day);
            assert_eq!(days_from_civil(year, month, date), day);
            day += 1;
        }
    }

    #[test]
    fn extreme_years_return_immediately_instead_of_looping() {
        // Both directions used to walk one iteration per year, so these inputs
        // burned billions of uninterruptible iterations inside the guest.
        let far = iso("2147483647-01-01T00:00:00Z");
        assert!(far > 0);
        assert_eq!(unix_to_datetime(far).0, 2_147_483_647);
        let ancient = days_from_civil(-2_000_000_000, 6, 15) * 86400;
        assert_eq!(unix_to_datetime(ancient), (-2_000_000_000, 6, 15, 0, 0, 0));
        assert!(format_timestamp(far, "yyyy").is_ok());
    }
}

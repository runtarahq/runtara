// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Utility agents for workflow execution
//!
//! This module provides utility operations that can be used in workflows:
//! - Random number generation
//! - String and object manipulation
//! - Delays
//! - Arithmetic calculations
//! - Date/time formatting and conversion
//! - Country code lookups
//!
//! All operations accept Rust data structures directly (no CloudEvents wrapper)

use chrono::Utc;
use rand::Rng;
use runtara_agent_macro::{CapabilityInput, capability};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

// ============================================================================
// Input/Output Types
// ============================================================================

/// Input for generating a random double number
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Random Double Input")]
pub struct RandomDoubleInput {}

/// Input for generating an array of random integers
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Random Array Input")]
pub struct ReturnRandomArrayInput {
    /// Number of random integers to generate in the array
    #[field(
        display_name = "Array Size",
        description = "The number of random integers to generate",
        example = "5"
    )]
    pub size: i32,
}

/// Input for returning a string value
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "String Input")]
pub struct ReturnStringInput {
    /// The string value to return
    #[field(
        display_name = "Value",
        description = "The string value to return as output",
        example = "Hello World"
    )]
    pub value: String,
}

/// Input for returning a JSON value
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "JSON Value Input")]
pub struct ReturnInputData {
    /// The JSON value to return
    #[field(
        display_name = "Value",
        description = "The JSON value to return as output"
    )]
    pub value: Value,
}

/// Input for a no-op operation
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "No Operation Input")]
pub struct DoNothingInput {}

/// Input for delaying execution
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delay Input")]
pub struct DelayInMsInput {
    /// Time to wait in milliseconds
    #[field(
        display_name = "Delay (milliseconds)",
        description = "The amount of time to pause execution in milliseconds",
        example = "1000"
    )]
    pub delay_value: u64,
}

/// Input for calculating mathematical expressions
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Calculate Expression Input")]
pub struct CalculateInput {
    /// The mathematical expression to evaluate
    #[field(
        display_name = "Expression",
        description = "Mathematical expression with variables and operators (+, -, *, /, %, parentheses)",
        example = "(x + y) * 2"
    )]
    pub expression: String,

    /// Variable values used in the expression
    #[field(
        display_name = "Variables",
        description = "Map of variable names to their numeric values (supports numbers and string numbers)"
    )]
    pub variables: HashMap<String, serde_json::Value>,

    /// Whether to round the result to decimal places
    #[field(
        display_name = "Enable Rounding",
        description = "Round the result to the specified number of decimal places",
        default = "false"
    )]
    #[serde(default)]
    pub enable_rounding: bool,

    /// Number of decimal places for rounding
    #[field(
        display_name = "Decimal Places",
        description = "Number of decimal places to round to (maximum 15)",
        example = "2"
    )]
    pub decimal_places: Option<u32>,
}

/// Input for formatting a date from ISO format
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Format Date Input")]
pub struct FormatDateFromIsoInput {
    /// Date in ISO 8601 format
    #[field(
        display_name = "ISO Date",
        description = "The date/time in ISO 8601 format (e.g., 2024-01-15T10:30:00Z)",
        example = "2024-01-15T10:30:00Z"
    )]
    pub iso_date: String,

    /// Desired date format pattern
    #[field(
        display_name = "Target Format",
        description = "Format pattern using yyyy, MM, dd, HH, mm, ss tokens",
        example = "yyyy-MM-dd HH:mm:ss"
    )]
    pub target_format: String,
}

/// Input for converting ISO date to Unix timestamp
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "ISO to Unix Timestamp Input")]
pub struct IsoToUnixTimestampInput {
    /// Date in ISO 8601 format
    #[field(
        display_name = "ISO Date",
        description = "The date/time in ISO 8601 format to convert to Unix timestamp",
        example = "2024-01-15T10:30:00Z"
    )]
    pub iso_date: String,
}

/// Input for getting the current Unix timestamp
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Current Unix Timestamp Input")]
pub struct GetCurrentUnixTimestampInput {}

/// Input for getting the current date/time in ISO format
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Current ISO Datetime Input")]
pub struct GetCurrentIsoDatetimeInput {}

/// Input for getting the current date/time in a custom format
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Current Formatted DateTime Input")]
pub struct GetCurrentFormattedDateTimeInput {
    /// Date/time format pattern
    #[field(
        display_name = "Format",
        description = "Format pattern using yyyy, MM, dd, HH, mm, ss tokens",
        example = "yyyy-MM-dd"
    )]
    pub format: String,
}

/// Input for converting country name to ISO code
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Country Name to ISO Code Input")]
pub struct CountryNameToIsoCodeInput {
    /// Full country name or alternative name
    #[field(
        display_name = "Country Name",
        description = "The full country name or commonly used alternative",
        example = "United States"
    )]
    pub country_name: String,

    /// ISO code format type
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

// ============================================================================
// Operations
// ============================================================================

/// Returns a random double number between 0.0 and 1.0
#[capability(
    module = "utils",
    display_name = "Random Double",
    description = "Generate a random double between 0 and 1"
)]
pub fn random_double(_input: RandomDoubleInput) -> Result<f64, String> {
    let mut rng = rand::thread_rng();
    Ok(rng.r#gen())
}

/// Returns an array of random integers with a specified size
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

/// Returns an input string
#[capability(
    module = "utils",
    display_name = "Return String",
    description = "Returns the input string value"
)]
pub fn return_input_string(input: ReturnStringInput) -> Result<String, String> {
    Ok(input.value)
}

/// Returns an input object from the 'value' field
#[capability(
    module = "utils",
    display_name = "Return Input",
    description = "Returns the input JSON value"
)]
pub fn return_input(input: ReturnInputData) -> Result<Value, String> {
    Ok(input.value)
}

/// Void operation - does nothing, returns null
#[capability(
    module = "utils",
    display_name = "Do Nothing",
    description = "No-op operation that returns null"
)]
pub fn do_nothing(_input: DoNothingInput) -> Result<Value, String> {
    Ok(Value::Null)
}

/// Delays for specified amount of milliseconds, returns the requested delay value
#[capability(
    module = "utils",
    display_name = "Delay",
    description = "Pause execution for specified milliseconds"
)]
pub async fn delay_in_ms(input: DelayInMsInput) -> Result<u64, String> {
    tokio::time::sleep(Duration::from_millis(input.delay_value)).await;
    Ok(input.delay_value)
}

/// Calculates an arithmetic expression with variables
#[capability(
    module = "utils",
    display_name = "Calculate",
    description = "Evaluate a mathematical expression with variables"
)]
pub fn calculate(input: CalculateInput) -> Result<f64, String> {
    // Convert Value variables to f64, coercing strings to numbers
    let mut variables: HashMap<String, f64> = HashMap::new();
    for (key, value) in input.variables {
        let num = match &value {
            serde_json::Value::Number(n) => n
                .as_f64()
                .ok_or_else(|| format!("Variable '{}' has invalid number: {}", key, n))?,
            serde_json::Value::String(s) => s
                .parse::<f64>()
                .map_err(|_| format!("Variable '{}' cannot be parsed as number: '{}'", key, s))?,
            serde_json::Value::Null => {
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

/// Format date from ISO format to a specified format
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

    // Parse ISO datetime and format it
    parse_and_format_datetime(&input.iso_date, &input.target_format)
}

/// Convert ISO date to Unix timestamp (seconds since epoch)
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

/// Return current Unix timestamp (seconds since epoch)
#[capability(
    module = "utils",
    display_name = "Current Unix Timestamp",
    description = "Get the current Unix timestamp"
)]
pub fn get_current_unix_timestamp(_input: GetCurrentUnixTimestampInput) -> Result<i64, String> {
    Ok(Utc::now().timestamp())
}

/// Return current date/time in ISO format (RFC3339)
#[capability(
    module = "utils",
    display_name = "Current ISO Datetime",
    description = "Get the current date/time in ISO format"
)]
pub fn get_current_iso_datetime(_input: GetCurrentIsoDatetimeInput) -> Result<String, String> {
    Ok(Utc::now().to_rfc3339())
}

/// Return current date/time in a specified format
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

    let timestamp = Utc::now().timestamp() as u64;
    format_timestamp(&timestamp, &input.format)
}

/// Convert country name to ISO country code (alpha2 or alpha3)
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

// ============================================================================
// Helper Functions - Expression Calculation
// ============================================================================

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

    // Validate and extract variables
    let expr_vars = validate_and_extract_variables(expr)?;

    // Validate all variables are present
    validate_variables_present(&expr_vars, variables)?;

    // Substitute variables and evaluate
    let substituted = substitute_variables(expr, variables);
    let result = evaluate_simple_expression(&substituted)?;

    // Apply rounding if enabled
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
    // Check for invalid characters
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

    // Extract variable names (alphanumeric identifiers starting with letter or underscore)
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

            // Check if it's not a reserved keyword
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
            // Continue collecting the identifier
            while let Some(&next_ch) = chars.peek() {
                if next_ch.is_alphanumeric() || next_ch == '_' {
                    current_word.push(chars.next().unwrap());
                } else {
                    break;
                }
            }

            // Check if this word is a variable
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
    // Simple expression evaluator using operator precedence
    // Supports: +, -, *, /, %, parentheses

    let expr = expr.replace(" ", "");
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

    // Parse number
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

// ============================================================================
// Helper Functions - Date/Time
// ============================================================================

fn parse_iso_to_unix(iso_date: &str) -> Result<i64, String> {
    // Simple ISO 8601 parser for basic formats like "2024-01-15T10:30:00"
    // Supports: YYYY-MM-DDTHH:MM:SS and YYYY-MM-DDTHH:MM:SSZ

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

    let year: i32 = date_parts[0].parse().map_err(|_| "Invalid year")?;
    let month: u32 = date_parts[1].parse().map_err(|_| "Invalid month")?;
    let day: u32 = date_parts[2].parse().map_err(|_| "Invalid day")?;
    let hour: u32 = time_parts[0].parse().map_err(|_| "Invalid hour")?;
    let minute: u32 = time_parts[1].parse().map_err(|_| "Invalid minute")?;
    let second: u32 = time_parts[2].parse().map_err(|_| "Invalid second")?;

    // Calculate Unix timestamp (simplified, assumes UTC)
    let timestamp = calculate_unix_timestamp(year, month, day, hour, minute, second);
    Ok(timestamp)
}

fn calculate_unix_timestamp(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> i64 {
    // Days in each month (non-leap year)
    const DAYS_IN_MONTH: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

    let mut days: i64 = 0;

    // Add days for complete years since 1970
    for y in 1970..year {
        days += if is_leap_year(y) { 366 } else { 365 };
    }

    // Add days for complete months in current year
    for m in 1..month {
        days += DAYS_IN_MONTH[(m - 1) as usize] as i64;
        if m == 2 && is_leap_year(year) {
            days += 1;
        }
    }

    // Add remaining days
    days += (day - 1) as i64;

    // Convert to seconds and add time
    days * 86400 + hour as i64 * 3600 + minute as i64 * 60 + second as i64
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

#[allow(dead_code)]
fn format_unix_to_iso(timestamp: u64) -> String {
    let (year, month, day, hour, minute, second) = unix_to_datetime(timestamp);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        year, month, day, hour, minute, second
    )
}

fn unix_to_datetime(timestamp: u64) -> (i32, u32, u32, u32, u32, u32) {
    const DAYS_IN_MONTH: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

    let total_seconds = timestamp;
    let days = total_seconds / 86400;
    let remaining = total_seconds % 86400;

    let hour = (remaining / 3600) as u32;
    let minute = ((remaining % 3600) / 60) as u32;
    let second = (remaining % 60) as u32;

    let mut year = 1970;
    let mut remaining_days = days;

    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }

    let mut month = 1;
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

    // Simple format string replacement (supports common patterns)
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

// ============================================================================
// Helper Functions - Country Codes
// ============================================================================

fn find_country_code(country_name: &str, code_type: &str) -> Result<String, String> {
    let mappings = initialize_country_mappings();
    let normalized_input = normalize_country_name(country_name);

    // Try exact match first
    for (name, codes) in &mappings {
        if normalize_country_name(name) == normalized_input {
            return Ok(if code_type == "alpha2" {
                codes[0].to_string()
            } else {
                codes[1].to_string()
            });
        }
    }

    // Try alternative names
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

    // Try fuzzy matching with Levenshtein distance
    let threshold = std::cmp::max(3, country_name.len() / 4);
    let mut best_match: Option<(&str, &Vec<&str>)> = None;
    let mut min_distance = usize::MAX;

    for (name, codes) in &mappings {
        let distance = levenshtein_distance(&normalized_input, &normalize_country_name(name));
        if distance < min_distance && distance <= threshold {
            min_distance = distance;
            best_match = Some((name, codes));
        }

        // Check alternative names
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
    let mut matrix = vec![vec![0; len2 + 1]; len1 + 1];

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_random_double() {
        let input = RandomDoubleInput {};
        let result = random_double(input).unwrap();
        assert!((0.0..=1.0).contains(&result));
    }

    #[test]
    fn test_random_array() {
        let input = ReturnRandomArrayInput { size: 5 };
        let result = random_array(input).unwrap();
        assert_eq!(result.len(), 5);
    }

    #[test]
    fn test_random_array_negative_size() {
        let input = ReturnRandomArrayInput { size: -1 };
        let result = random_array(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_return_input_string() {
        let input = ReturnStringInput {
            value: "Hello".to_string(),
        };
        assert_eq!(return_input_string(input).unwrap(), "Hello");
    }

    #[test]
    fn test_do_nothing() {
        let input = DoNothingInput {};
        let result = do_nothing(input).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[tokio::test]
    async fn test_delay_in_ms() {
        let input = DelayInMsInput { delay_value: 10 };
        let start = std::time::Instant::now();
        let result = delay_in_ms(input).await.unwrap();
        let elapsed = start.elapsed().as_millis();

        assert_eq!(result, 10);
        assert!(elapsed >= 10);
    }

    #[test]
    fn test_calculate_simple() {
        let mut variables = HashMap::new();
        variables.insert("a".to_string(), serde_json::json!(10.0));
        variables.insert("b".to_string(), serde_json::json!(5.0));

        let input = CalculateInput {
            expression: "a + b".to_string(),
            variables,
            enable_rounding: false,
            decimal_places: None,
        };

        let result = calculate(input).unwrap();
        assert_eq!(result, 15.0);
    }

    #[test]
    fn test_calculate_with_string_numbers() {
        let mut variables = HashMap::new();
        variables.insert("a".to_string(), serde_json::json!("10.5"));
        variables.insert("b".to_string(), serde_json::json!("5.5"));

        let input = CalculateInput {
            expression: "a + b".to_string(),
            variables,
            enable_rounding: false,
            decimal_places: None,
        };

        let result = calculate(input).unwrap();
        assert_eq!(result, 16.0);
    }

    #[test]
    fn test_calculate_with_rounding() {
        let mut variables = HashMap::new();
        variables.insert("a".to_string(), serde_json::json!(10.5555));

        let input = CalculateInput {
            expression: "a".to_string(),
            variables,
            enable_rounding: true,
            decimal_places: Some(2),
        };

        let result = calculate(input).unwrap();
        assert_eq!(result, 10.56);
    }

    #[test]
    fn test_iso_to_unix_timestamp() {
        let input = IsoToUnixTimestampInput {
            iso_date: "2024-01-15T10:30:00".to_string(),
        };

        let result = iso_to_unix_timestamp(input).unwrap();
        assert!(result > 0);
    }

    #[test]
    fn test_country_name_to_iso_code() {
        let input = CountryNameToIsoCodeInput {
            country_name: "United States".to_string(),
            code_type: "alpha2".to_string(),
        };

        let result = country_name_to_iso_code(input).unwrap();
        assert_eq!(result, "US");
    }

    #[test]
    fn test_country_name_fuzzy_match() {
        let input = CountryNameToIsoCodeInput {
            country_name: "Germny".to_string(), // typo
            code_type: "alpha2".to_string(),
        };

        let result = country_name_to_iso_code(input).unwrap();
        assert_eq!(result, "DE");
    }
}

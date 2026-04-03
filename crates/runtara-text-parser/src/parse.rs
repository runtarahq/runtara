use chrono::NaiveDate;
use regex::Regex;
use runtara_dsl::{SchemaField, SchemaFieldType};
use serde_json::{Value, json};

/// Create a plain string SchemaField with no constraints.
fn plain_string_field() -> SchemaField {
    SchemaField {
        field_type: SchemaFieldType::String,
        description: None,
        required: false,
        default: None,
        example: None,
        items: None,
        enum_values: None,
        label: None,
        placeholder: None,
        order: None,
        format: None,
        min: None,
        max: None,
        pattern: None,
        properties: None,
        visible_when: None,
    }
}

/// Result of parsing user text against a schema field.
#[derive(Debug, Clone)]
pub enum ParseResult {
    /// Successfully parsed into a JSON value.
    Ok(Value),
    /// Parse failed — contains a human-readable hint for the retry prompt.
    Retry(String),
}

/// Parse raw text input against a `SchemaField` definition.
pub fn parse_text(input: &str, field: &SchemaField) -> ParseResult {
    let trimmed = input.trim();

    match field.field_type {
        SchemaFieldType::String => parse_string(trimmed, field),
        SchemaFieldType::Integer => parse_integer(trimmed, field),
        SchemaFieldType::Number => parse_number(trimmed, field),
        SchemaFieldType::Boolean => parse_boolean(trimmed),
        SchemaFieldType::Array => parse_array(trimmed, field),
        SchemaFieldType::Object => parse_object(trimmed, field),
        SchemaFieldType::File => ParseResult::Retry(
            "File upload is not supported in text channels. Please use the web interface.".into(),
        ),
    }
}

// ---------------------------------------------------------------------------
// String
// ---------------------------------------------------------------------------

fn parse_string(input: &str, field: &SchemaField) -> ParseResult {
    // Enum selection
    if let Some(enum_values) = &field.enum_values {
        return parse_enum(input, enum_values);
    }

    // Format-specific parsing
    if let Some(format) = &field.format {
        return match format.as_str() {
            "date" => parse_date(input),
            "datetime" => parse_datetime(input),
            "email" => parse_email(input),
            "url" => parse_url(input),
            "tel" => parse_tel(input),
            "color" => parse_color(input),
            // textarea, markdown, password — accept as-is
            _ => validate_string_constraints(input, field),
        };
    }

    validate_string_constraints(input, field)
}

fn validate_string_constraints(input: &str, field: &SchemaField) -> ParseResult {
    let len = input.len() as f64;

    if let Some(min) = field.min
        && len < min
    {
        return ParseResult::Retry(format!("Must be at least {} characters", min as u64));
    }
    if let Some(max) = field.max
        && len > max
    {
        return ParseResult::Retry(format!("Must be at most {} characters", max as u64));
    }
    if let Some(pattern) = &field.pattern
        && let Ok(re) = Regex::new(pattern)
        && !re.is_match(input)
    {
        let hint = field
            .placeholder
            .as_deref()
            .map(|p| format!("Must match the required format, e.g. {}", p))
            .unwrap_or_else(|| format!("Must match pattern: {}", pattern));
        return ParseResult::Retry(hint);
    }

    ParseResult::Ok(Value::String(input.to_string()))
}

// ---------------------------------------------------------------------------
// Enum
// ---------------------------------------------------------------------------

fn parse_enum(input: &str, enum_values: &[Value]) -> ParseResult {
    let lower = input.to_lowercase();
    let labels: Vec<String> = enum_values
        .iter()
        .map(|v| match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .collect();

    // 1. Numeric index (1-based)
    if let Ok(idx) = lower.parse::<usize>()
        && idx >= 1
        && idx <= enum_values.len()
    {
        return ParseResult::Ok(enum_values[idx - 1].clone());
    }

    // 2. Exact match (case-insensitive)
    for (i, label) in labels.iter().enumerate() {
        if label.to_lowercase() == lower {
            return ParseResult::Ok(enum_values[i].clone());
        }
    }

    // 3. Unique prefix match
    let prefix_matches: Vec<usize> = labels
        .iter()
        .enumerate()
        .filter(|(_, l)| l.to_lowercase().starts_with(&lower))
        .map(|(i, _)| i)
        .collect();

    if prefix_matches.len() == 1 {
        return ParseResult::Ok(enum_values[prefix_matches[0]].clone());
    }

    // Build retry hint with numbered list
    let options: Vec<String> = labels
        .iter()
        .enumerate()
        .map(|(i, l)| format!("{}) {}", i + 1, l))
        .collect();
    ParseResult::Retry(format!("Pick one: {}", options.join(", ")))
}

// ---------------------------------------------------------------------------
// Date / DateTime
// ---------------------------------------------------------------------------

fn parse_date(input: &str) -> ParseResult {
    let lower = input.to_lowercase();

    // Relative dates
    let today = chrono::Local::now().date_naive();
    match lower.as_str() {
        "today" => return ParseResult::Ok(Value::String(today.to_string())),
        "tomorrow" => {
            return ParseResult::Ok(Value::String((today + chrono::Days::new(1)).to_string()));
        }
        "yesterday" => {
            return ParseResult::Ok(Value::String((today - chrono::Days::new(1)).to_string()));
        }
        _ => {}
    }

    // ISO 8601: 2026-03-25
    if let Ok(d) = NaiveDate::parse_from_str(input, "%Y-%m-%d") {
        return ParseResult::Ok(Value::String(d.to_string()));
    }

    // US format: 03/25/2026 or 03-25-2026
    if let Ok(d) = NaiveDate::parse_from_str(input, "%m/%d/%Y") {
        return ParseResult::Ok(Value::String(d.to_string()));
    }
    if let Ok(d) = NaiveDate::parse_from_str(input, "%m-%d-%Y") {
        return ParseResult::Ok(Value::String(d.to_string()));
    }

    // European format: 25/03/2026 or 25.03.2026
    if let Ok(d) = NaiveDate::parse_from_str(input, "%d/%m/%Y") {
        return ParseResult::Ok(Value::String(d.to_string()));
    }
    if let Ok(d) = NaiveDate::parse_from_str(input, "%d.%m.%Y") {
        return ParseResult::Ok(Value::String(d.to_string()));
    }

    // Month name: "March 25, 2026" or "25 March 2026"
    if let Ok(d) = NaiveDate::parse_from_str(input, "%B %d, %Y") {
        return ParseResult::Ok(Value::String(d.to_string()));
    }
    if let Ok(d) = NaiveDate::parse_from_str(input, "%d %B %Y") {
        return ParseResult::Ok(Value::String(d.to_string()));
    }
    // Abbreviated: "Mar 25, 2026"
    if let Ok(d) = NaiveDate::parse_from_str(input, "%b %d, %Y") {
        return ParseResult::Ok(Value::String(d.to_string()));
    }

    ParseResult::Retry("Couldn't parse as a date. Try YYYY-MM-DD format, e.g. 2026-03-25".into())
}

fn parse_datetime(input: &str) -> ParseResult {
    use chrono::NaiveDateTime;

    // ISO 8601: 2026-03-25T15:00:00
    if let Ok(dt) = NaiveDateTime::parse_from_str(input, "%Y-%m-%dT%H:%M:%S") {
        return ParseResult::Ok(Value::String(dt.format("%Y-%m-%dT%H:%M:%S").to_string()));
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(input, "%Y-%m-%dT%H:%M") {
        return ParseResult::Ok(Value::String(dt.format("%Y-%m-%dT%H:%M:%S").to_string()));
    }

    // Space separator: "2026-03-25 15:00"
    if let Ok(dt) = NaiveDateTime::parse_from_str(input, "%Y-%m-%d %H:%M:%S") {
        return ParseResult::Ok(Value::String(dt.format("%Y-%m-%dT%H:%M:%S").to_string()));
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(input, "%Y-%m-%d %H:%M") {
        return ParseResult::Ok(Value::String(dt.format("%Y-%m-%dT%H:%M:%S").to_string()));
    }

    // Date-only → append T00:00:00
    if let ParseResult::Ok(Value::String(date_str)) = parse_date(input) {
        return ParseResult::Ok(Value::String(format!("{}T00:00:00", date_str)));
    }

    ParseResult::Retry("Need a date and time. Try: 2026-03-25 15:00".into())
}

// ---------------------------------------------------------------------------
// Email / URL / Tel / Color
// ---------------------------------------------------------------------------

fn parse_email(input: &str) -> ParseResult {
    let lower = input.to_lowercase();
    // Practical email validation: has @, has domain with dot
    let re = Regex::new(r"^[^\s@]+@[^\s@]+\.[^\s@]+$").unwrap();
    if re.is_match(&lower) {
        ParseResult::Ok(Value::String(lower))
    } else {
        ParseResult::Retry("That doesn't look like a valid email. Try: name@example.com".into())
    }
}

fn parse_url(input: &str) -> ParseResult {
    let url = if !input.contains("://") {
        format!("https://{}", input)
    } else {
        input.to_string()
    };

    // Minimal URL validation: scheme://host
    let re = Regex::new(r"^https?://[^\s/$.?#].[^\s]*$").unwrap();
    if re.is_match(&url) {
        ParseResult::Ok(Value::String(url))
    } else {
        ParseResult::Retry("Need a valid URL, e.g. https://example.com".into())
    }
}

fn parse_tel(input: &str) -> ParseResult {
    // Strip formatting characters, keep + prefix and digits
    let cleaned: String = input
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '+')
        .collect();

    let digit_count = cleaned.chars().filter(|c| c.is_ascii_digit()).count();
    if digit_count >= 7 {
        ParseResult::Ok(Value::String(cleaned))
    } else {
        ParseResult::Retry("Need a phone number with at least 7 digits".into())
    }
}

fn parse_color(input: &str) -> ParseResult {
    let lower = input.to_lowercase().trim().to_string();

    // Hex color
    let hex_re = Regex::new(r"^#?([0-9a-f]{6}|[0-9a-f]{3})$").unwrap();
    if hex_re.is_match(&lower) {
        let hex = if lower.starts_with('#') {
            lower
        } else {
            format!("#{}", lower)
        };
        return ParseResult::Ok(Value::String(hex));
    }

    // Named colors
    let named = match lower.as_str() {
        "red" => Some("#ff0000"),
        "green" => Some("#00ff00"),
        "blue" => Some("#0000ff"),
        "white" => Some("#ffffff"),
        "black" => Some("#000000"),
        "yellow" => Some("#ffff00"),
        "orange" => Some("#ffa500"),
        "purple" => Some("#800080"),
        "pink" => Some("#ffc0cb"),
        "gray" | "grey" => Some("#808080"),
        "brown" => Some("#a52a2a"),
        "cyan" => Some("#00ffff"),
        _ => None,
    };

    if let Some(hex) = named {
        return ParseResult::Ok(Value::String(hex.to_string()));
    }

    ParseResult::Retry("Need a color. Try #FF0000 or a name like red, blue, green".into())
}

// ---------------------------------------------------------------------------
// Integer / Number
// ---------------------------------------------------------------------------

fn parse_integer(input: &str, field: &SchemaField) -> ParseResult {
    match input.parse::<i64>() {
        Ok(n) => {
            if let Some(min) = field.min
                && (n as f64) < min
            {
                return ParseResult::Retry(format!("Must be at least {}", min as i64));
            }
            if let Some(max) = field.max
                && (n as f64) > max
            {
                return ParseResult::Retry(format!("Must be at most {}", max as i64));
            }
            ParseResult::Ok(json!(n))
        }
        Err(_) => {
            let hint = match (field.min, field.max) {
                (Some(min), Some(max)) => {
                    format!(
                        "Need a whole number between {} and {}",
                        min as i64, max as i64
                    )
                }
                _ => "Need a whole number".into(),
            };
            ParseResult::Retry(hint)
        }
    }
}

fn parse_number(input: &str, field: &SchemaField) -> ParseResult {
    match input.parse::<f64>() {
        Ok(n) if n.is_finite() => {
            if let Some(min) = field.min
                && n < min
            {
                return ParseResult::Retry(format!("Must be at least {}", min));
            }
            if let Some(max) = field.max
                && n > max
            {
                return ParseResult::Retry(format!("Must be at most {}", max));
            }
            ParseResult::Ok(json!(n))
        }
        _ => {
            let hint = match (field.min, field.max) {
                (Some(min), Some(max)) => format!("Need a number between {} and {}", min, max),
                _ => "Need a number".into(),
            };
            ParseResult::Retry(hint)
        }
    }
}

// ---------------------------------------------------------------------------
// Boolean
// ---------------------------------------------------------------------------

fn parse_boolean(input: &str) -> ParseResult {
    let lower = input.to_lowercase();
    match lower.as_str() {
        "yes" | "y" | "true" | "1" | "ok" | "si" | "da" | "tak" => {
            ParseResult::Ok(Value::Bool(true))
        }
        "no" | "n" | "false" | "0" | "nope" | "nie" | "nein" => ParseResult::Ok(Value::Bool(false)),
        _ => ParseResult::Retry("Please answer yes or no".into()),
    }
}

// ---------------------------------------------------------------------------
// Array
// ---------------------------------------------------------------------------

fn parse_array(input: &str, field: &SchemaField) -> ParseResult {
    let parts: Vec<&str> = input
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    if parts.is_empty() {
        return ParseResult::Retry("Enter values separated by commas".into());
    }

    let item_field = field
        .items
        .as_deref()
        .cloned()
        .unwrap_or_else(plain_string_field);

    let mut values = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        match parse_text(part, &item_field) {
            ParseResult::Ok(v) => values.push(v),
            ParseResult::Retry(hint) => {
                return ParseResult::Retry(format!("Item {} (\"{}\"): {}", i + 1, part, hint));
            }
        }
    }

    ParseResult::Ok(Value::Array(values))
}

// ---------------------------------------------------------------------------
// Object
// ---------------------------------------------------------------------------

fn parse_object(input: &str, field: &SchemaField) -> ParseResult {
    // If the field has properties, we can't parse a flat text string into
    // a multi-field object — that requires sequential prompting at a higher level.
    // Here we only handle the "no properties" case (raw JSON).
    if field.properties.is_some() {
        return ParseResult::Retry(
            "This field requires multiple values. They will be requested one at a time.".into(),
        );
    }

    // Try parsing as JSON
    match serde_json::from_str::<Value>(input) {
        Ok(v) if v.is_object() => ParseResult::Ok(v),
        _ => ParseResult::Retry("Need a valid JSON object, e.g. {\"key\": \"value\"}".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_dsl::SchemaFieldType;

    fn string_field() -> SchemaField {
        SchemaField {
            required: true,
            ..plain_string_field()
        }
    }

    fn assert_ok(result: &ParseResult) -> &Value {
        match result {
            ParseResult::Ok(v) => v,
            ParseResult::Retry(hint) => panic!("Expected Ok, got Retry: {}", hint),
        }
    }

    fn assert_retry(result: &ParseResult) -> &str {
        match result {
            ParseResult::Retry(hint) => hint,
            ParseResult::Ok(v) => panic!("Expected Retry, got Ok: {}", v),
        }
    }

    // -- String tests --

    #[test]
    fn plain_string_accepted() {
        let field = string_field();
        let r = parse_text("hello world", &field);
        assert_eq!(assert_ok(&r), "hello world");
    }

    #[test]
    fn string_min_length() {
        let mut field = string_field();
        field.min = Some(5.0);
        assert_retry(&parse_text("hi", &field));
        assert_ok(&parse_text("hello", &field));
    }

    #[test]
    fn string_max_length() {
        let mut field = string_field();
        field.max = Some(3.0);
        assert_retry(&parse_text("toolong", &field));
        assert_ok(&parse_text("ok", &field));
    }

    #[test]
    fn string_pattern() {
        let mut field = string_field();
        field.pattern = Some(r"^[A-Z]{3}\d{4}$".into());
        assert_ok(&parse_text("ABC1234", &field));
        assert_retry(&parse_text("abc1234", &field));
        assert_retry(&parse_text("AB123", &field));
    }

    // -- Enum tests --

    #[test]
    fn enum_exact_match() {
        let mut field = string_field();
        field.enum_values = Some(vec![json!("low"), json!("medium"), json!("high")]);
        assert_eq!(assert_ok(&parse_text("medium", &field)), "medium");
        assert_eq!(assert_ok(&parse_text("MEDIUM", &field)), "medium");
    }

    #[test]
    fn enum_numeric_index() {
        let mut field = string_field();
        field.enum_values = Some(vec![json!("low"), json!("medium"), json!("high")]);
        assert_eq!(assert_ok(&parse_text("2", &field)), "medium");
        assert_eq!(assert_ok(&parse_text("3", &field)), "high");
    }

    #[test]
    fn enum_prefix_match() {
        let mut field = string_field();
        field.enum_values = Some(vec![
            json!("shopify"),
            json!("woocommerce"),
            json!("amazon"),
        ]);
        assert_eq!(assert_ok(&parse_text("shop", &field)), "shopify");
        assert_eq!(assert_ok(&parse_text("woo", &field)), "woocommerce");
    }

    #[test]
    fn enum_ambiguous_prefix_retry() {
        let mut field = string_field();
        field.enum_values = Some(vec![json!("shipping"), json!("shopping")]);
        assert_retry(&parse_text("sh", &field));
    }

    #[test]
    fn enum_no_match_retry() {
        let mut field = string_field();
        field.enum_values = Some(vec![json!("low"), json!("high")]);
        let result = parse_text("unknown", &field);
        let hint = assert_retry(&result);
        assert!(hint.contains("Pick one"));
    }

    // -- Date tests --

    #[test]
    fn date_iso() {
        let mut field = string_field();
        field.format = Some("date".into());
        assert_eq!(assert_ok(&parse_text("2026-03-25", &field)), "2026-03-25");
    }

    #[test]
    fn date_relative_today() {
        let mut field = string_field();
        field.format = Some("date".into());
        let today = chrono::Local::now().date_naive().to_string();
        assert_eq!(assert_ok(&parse_text("today", &field)), today.as_str());
    }

    #[test]
    fn date_month_name() {
        let mut field = string_field();
        field.format = Some("date".into());
        assert_eq!(
            assert_ok(&parse_text("March 25, 2026", &field)),
            "2026-03-25"
        );
    }

    #[test]
    fn date_invalid() {
        let mut field = string_field();
        field.format = Some("date".into());
        assert_retry(&parse_text("not a date", &field));
    }

    // -- DateTime tests --

    #[test]
    fn datetime_iso() {
        let mut field = string_field();
        field.format = Some("datetime".into());
        assert_eq!(
            assert_ok(&parse_text("2026-03-25T15:00:00", &field)),
            "2026-03-25T15:00:00"
        );
    }

    #[test]
    fn datetime_space_separator() {
        let mut field = string_field();
        field.format = Some("datetime".into());
        assert_eq!(
            assert_ok(&parse_text("2026-03-25 15:00", &field)),
            "2026-03-25T15:00:00"
        );
    }

    #[test]
    fn datetime_date_only_fallback() {
        let mut field = string_field();
        field.format = Some("datetime".into());
        assert_eq!(
            assert_ok(&parse_text("2026-03-25", &field)),
            "2026-03-25T00:00:00"
        );
    }

    // -- Email tests --

    #[test]
    fn email_valid() {
        let mut field = string_field();
        field.format = Some("email".into());
        assert_eq!(
            assert_ok(&parse_text("John@Example.COM", &field)),
            "john@example.com"
        );
    }

    #[test]
    fn email_invalid() {
        let mut field = string_field();
        field.format = Some("email".into());
        assert_retry(&parse_text("notanemail", &field));
    }

    // -- URL tests --

    #[test]
    fn url_with_scheme() {
        let mut field = string_field();
        field.format = Some("url".into());
        assert_eq!(
            assert_ok(&parse_text("https://example.com", &field)),
            "https://example.com"
        );
    }

    #[test]
    fn url_auto_scheme() {
        let mut field = string_field();
        field.format = Some("url".into());
        assert_eq!(
            assert_ok(&parse_text("example.com", &field)),
            "https://example.com"
        );
    }

    // -- Tel tests --

    #[test]
    fn tel_with_formatting() {
        let mut field = string_field();
        field.format = Some("tel".into());
        assert_eq!(
            assert_ok(&parse_text("+1 (555) 123-4567", &field)),
            "+15551234567"
        );
    }

    #[test]
    fn tel_too_short() {
        let mut field = string_field();
        field.format = Some("tel".into());
        assert_retry(&parse_text("123", &field));
    }

    // -- Color tests --

    #[test]
    fn color_hex() {
        let mut field = string_field();
        field.format = Some("color".into());
        assert_eq!(assert_ok(&parse_text("#ff0000", &field)), "#ff0000");
        assert_eq!(assert_ok(&parse_text("00ff00", &field)), "#00ff00");
    }

    #[test]
    fn color_named() {
        let mut field = string_field();
        field.format = Some("color".into());
        assert_eq!(assert_ok(&parse_text("red", &field)), "#ff0000");
    }

    // -- Integer tests --

    #[test]
    fn integer_valid() {
        let field = SchemaField {
            field_type: SchemaFieldType::Integer,
            min: Some(1.0),
            max: Some(1000.0),
            ..string_field()
        };
        assert_eq!(assert_ok(&parse_text("42", &field)), 42);
    }

    #[test]
    fn integer_out_of_range() {
        let field = SchemaField {
            field_type: SchemaFieldType::Integer,
            min: Some(1.0),
            max: Some(10.0),
            ..string_field()
        };
        assert_retry(&parse_text("99", &field));
        assert_retry(&parse_text("0", &field));
    }

    #[test]
    fn integer_not_a_number() {
        let field = SchemaField {
            field_type: SchemaFieldType::Integer,
            ..string_field()
        };
        assert_retry(&parse_text("abc", &field));
    }

    // -- Number tests --

    #[test]
    #[allow(clippy::approx_constant)]
    fn number_valid() {
        let field = SchemaField {
            field_type: SchemaFieldType::Number,
            ..string_field()
        };
        let result = parse_text("3.14", &field);
        let v = assert_ok(&result);
        assert!((v.as_f64().unwrap() - 3.14_f64).abs() < f64::EPSILON);
    }

    #[test]
    fn number_bounds() {
        let field = SchemaField {
            field_type: SchemaFieldType::Number,
            min: Some(0.01),
            max: Some(99.99),
            ..string_field()
        };
        assert_retry(&parse_text("0", &field));
        assert_retry(&parse_text("100", &field));
        assert_ok(&parse_text("50.5", &field));
    }

    // -- Boolean tests --

    #[test]
    fn boolean_affirmative() {
        let field = SchemaField {
            field_type: SchemaFieldType::Boolean,
            ..string_field()
        };
        assert_eq!(assert_ok(&parse_text("yes", &field)), true);
        assert_eq!(assert_ok(&parse_text("Y", &field)), true);
        assert_eq!(assert_ok(&parse_text("true", &field)), true);
        assert_eq!(assert_ok(&parse_text("tak", &field)), true);
    }

    #[test]
    fn boolean_negative() {
        let field = SchemaField {
            field_type: SchemaFieldType::Boolean,
            ..string_field()
        };
        assert_eq!(assert_ok(&parse_text("no", &field)), false);
        assert_eq!(assert_ok(&parse_text("N", &field)), false);
        assert_eq!(assert_ok(&parse_text("nie", &field)), false);
    }

    #[test]
    fn boolean_invalid() {
        let field = SchemaField {
            field_type: SchemaFieldType::Boolean,
            ..string_field()
        };
        assert_retry(&parse_text("maybe", &field));
    }

    // -- Array tests --

    #[test]
    fn array_of_strings() {
        let field = SchemaField {
            field_type: SchemaFieldType::Array,
            ..string_field()
        };
        let r = parse_text("urgent, fragile, oversized", &field);
        let arr = assert_ok(&r).as_array().unwrap().clone();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0], "urgent");
        assert_eq!(arr[1], "fragile");
        assert_eq!(arr[2], "oversized");
    }

    #[test]
    fn array_of_integers() {
        let field = SchemaField {
            field_type: SchemaFieldType::Array,
            items: Some(Box::new(SchemaField {
                field_type: SchemaFieldType::Integer,
                ..string_field()
            })),
            ..string_field()
        };
        let r = parse_text("1, 2, 3", &field);
        let arr = assert_ok(&r).as_array().unwrap().clone();
        assert_eq!(arr, vec![json!(1), json!(2), json!(3)]);
    }

    #[test]
    fn array_item_parse_error() {
        let field = SchemaField {
            field_type: SchemaFieldType::Array,
            items: Some(Box::new(SchemaField {
                field_type: SchemaFieldType::Integer,
                ..string_field()
            })),
            ..string_field()
        };
        let result = parse_text("1, abc, 3", &field);
        let hint = assert_retry(&result);
        assert!(hint.contains("Item 2"));
    }

    // -- Object tests --

    #[test]
    fn object_raw_json() {
        let field = SchemaField {
            field_type: SchemaFieldType::Object,
            ..string_field()
        };
        let r = parse_text(r#"{"key": "value"}"#, &field);
        let obj = assert_ok(&r);
        assert_eq!(obj["key"], "value");
    }

    #[test]
    fn object_with_properties_defers() {
        let mut props = std::collections::HashMap::new();
        props.insert("name".into(), string_field());
        let field = SchemaField {
            field_type: SchemaFieldType::Object,
            properties: Some(props),
            ..string_field()
        };
        assert_retry(&parse_text("anything", &field));
    }

    // -- File tests --

    #[test]
    fn file_not_supported() {
        let field = SchemaField {
            field_type: SchemaFieldType::File,
            ..string_field()
        };
        let result = parse_text("anything", &field);
        let hint = assert_retry(&result);
        assert!(hint.contains("not supported"));
    }
}

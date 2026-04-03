use runtara_dsl::{SchemaField, SchemaFieldType};
use serde_json::Value;

/// Build a human-readable prompt for a schema field.
///
/// The prompt guides a text-channel user to provide the right value.
/// It includes the label, type hint, constraints, and format examples.
pub fn build_prompt(field_name: &str, field: &SchemaField) -> String {
    let label = field.label.as_deref().unwrap_or(field_name).to_string();

    let mut parts = Vec::new();

    // Main label
    parts.push(format!("**{}**", label));

    // Type/format hint
    if let Some(hint) = type_hint(field) {
        parts.push(format!("({})", hint));
    }

    // Optional marker
    if !field.required {
        if let Some(default) = &field.default {
            let default_str = format_default(default);
            parts.push(format!("[optional, default: {}]", default_str));
        } else {
            parts.push("[optional, /skip to skip]".into());
        }
    }

    let mut prompt = parts.join(" ");

    // Enum options — numbered list
    if let Some(enum_values) = &field.enum_values {
        let options: Vec<String> = enum_values
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let label = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                format!("  {}) {}", i + 1, label)
            })
            .collect();
        prompt.push('\n');
        prompt.push_str(&options.join("\n"));
    }

    // Description — on the next line
    if let Some(desc) = &field.description {
        prompt.push('\n');
        prompt.push_str(&format!("_{}_", desc));
    }

    prompt.push(':');
    prompt
}

/// Generate a concise type hint for parenthetical display.
fn type_hint(field: &SchemaField) -> Option<String> {
    // Enum fields get their hint from the numbered list, not here.
    if field.enum_values.is_some() {
        return None;
    }

    match field.field_type {
        SchemaFieldType::String => string_format_hint(field),
        SchemaFieldType::Integer => Some(integer_hint(field)),
        SchemaFieldType::Number => Some(number_hint(field)),
        SchemaFieldType::Boolean => Some("yes/no".into()),
        SchemaFieldType::Array => Some(array_hint(field)),
        SchemaFieldType::Object => Some("multiple fields".into()),
        SchemaFieldType::File => Some("file upload — not available in text channels".into()),
    }
}

fn string_format_hint(field: &SchemaField) -> Option<String> {
    if let Some(format) = &field.format {
        let hint = match format.as_str() {
            "date" => {
                let example = field.placeholder.as_deref().unwrap_or("YYYY-MM-DD");
                format!("date, e.g. {}", example)
            }
            "datetime" => {
                let example = field.placeholder.as_deref().unwrap_or("YYYY-MM-DD HH:MM");
                format!("date & time, e.g. {}", example)
            }
            "email" => {
                let example = field.placeholder.as_deref().unwrap_or("name@example.com");
                format!("email, e.g. {}", example)
            }
            "url" => {
                let example = field
                    .placeholder
                    .as_deref()
                    .unwrap_or("https://example.com");
                format!("URL, e.g. {}", example)
            }
            "tel" => {
                let example = field.placeholder.as_deref().unwrap_or("+1 555 000 0000");
                format!("phone, e.g. {}", example)
            }
            "color" => "hex color, e.g. #FF0000 or name".into(),
            "textarea" | "markdown" => return None, // No special hint needed
            "password" => "text".into(),
            other => other.to_string(),
        };
        return Some(hint);
    }

    // Pattern hint
    if field.pattern.is_some() {
        if let Some(placeholder) = &field.placeholder {
            return Some(format!("e.g. {}", placeholder));
        }
        return Some("text".into());
    }

    // Length constraints
    match (field.min, field.max) {
        (Some(min), Some(max)) if (min - max).abs() < f64::EPSILON => {
            Some(format!("exactly {} chars", min as u64))
        }
        (Some(min), Some(max)) => Some(format!("{}-{} chars", min as u64, max as u64)),
        (Some(min), None) => Some(format!("min {} chars", min as u64)),
        (None, Some(max)) => Some(format!("max {} chars", max as u64)),
        (None, None) => None, // Plain text, no hint needed
    }
}

fn integer_hint(field: &SchemaField) -> String {
    match (field.min, field.max) {
        (Some(min), Some(max)) => format!("whole number, {}-{}", min as i64, max as i64),
        (Some(min), None) => format!("whole number, min {}", min as i64),
        (None, Some(max)) => format!("whole number, max {}", max as i64),
        (None, None) => "whole number".into(),
    }
}

fn number_hint(field: &SchemaField) -> String {
    match (field.min, field.max) {
        (Some(min), Some(max)) => format!("number, {}-{}", min, max),
        (Some(min), None) => format!("number, min {}", min),
        (None, Some(max)) => format!("number, max {}", max),
        (None, None) => "number".into(),
    }
}

fn array_hint(field: &SchemaField) -> String {
    let item_type = field
        .items
        .as_ref()
        .map(|i| match i.field_type {
            SchemaFieldType::String => "text",
            SchemaFieldType::Integer => "numbers",
            SchemaFieldType::Number => "numbers",
            SchemaFieldType::Boolean => "yes/no",
            _ => "values",
        })
        .unwrap_or("values");
    format!("comma-separated {}", item_type)
}

fn format_default(value: &Value) -> String {
    match value {
        Value::String(s) if s.is_empty() => "empty".into(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => if *b { "yes" } else { "no" }.into(),
        Value::Number(n) => n.to_string(),
        Value::Null => "none".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn base_field(ft: SchemaFieldType) -> SchemaField {
        SchemaField {
            field_type: ft,
            description: None,
            required: true,
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

    #[test]
    fn plain_string_prompt() {
        let field = base_field(SchemaFieldType::String);
        let prompt = build_prompt("full_name", &field);
        assert!(prompt.starts_with("**full_name**"));
        assert!(prompt.ends_with(':'));
    }

    #[test]
    fn string_with_label() {
        let mut field = base_field(SchemaFieldType::String);
        field.label = Some("Full Name".into());
        let prompt = build_prompt("full_name", &field);
        assert!(prompt.starts_with("**Full Name**"));
    }

    #[test]
    fn date_prompt() {
        let mut field = base_field(SchemaFieldType::String);
        field.format = Some("date".into());
        field.label = Some("Delivery date".into());
        let prompt = build_prompt("delivery_date", &field);
        assert!(prompt.contains("**Delivery date**"));
        assert!(prompt.contains("YYYY-MM-DD"));
    }

    #[test]
    fn enum_prompt_numbered() {
        let mut field = base_field(SchemaFieldType::String);
        field.label = Some("Priority".into());
        field.enum_values = Some(vec![
            serde_json::json!("low"),
            serde_json::json!("medium"),
            serde_json::json!("high"),
        ]);
        let prompt = build_prompt("priority", &field);
        assert!(prompt.contains("1) low"));
        assert!(prompt.contains("2) medium"));
        assert!(prompt.contains("3) high"));
    }

    #[test]
    fn integer_with_bounds() {
        let mut field = base_field(SchemaFieldType::Integer);
        field.label = Some("Quantity".into());
        field.min = Some(1.0);
        field.max = Some(1000.0);
        let prompt = build_prompt("quantity", &field);
        assert!(prompt.contains("1-1000"));
    }

    #[test]
    fn boolean_prompt() {
        let mut field = base_field(SchemaFieldType::Boolean);
        field.label = Some("Approve?".into());
        field.description = Some("Check to approve this order".into());
        let prompt = build_prompt("approved", &field);
        assert!(prompt.contains("yes/no"));
        assert!(prompt.contains("_Check to approve this order_"));
    }

    #[test]
    fn optional_with_default() {
        let mut field = base_field(SchemaFieldType::String);
        field.required = false;
        field.label = Some("Notes".into());
        field.default = Some(serde_json::json!("N/A"));
        let prompt = build_prompt("notes", &field);
        assert!(prompt.contains("optional"));
        assert!(prompt.contains("default: N/A"));
    }

    #[test]
    fn optional_without_default() {
        let mut field = base_field(SchemaFieldType::String);
        field.required = false;
        field.label = Some("Notes".into());
        let prompt = build_prompt("notes", &field);
        assert!(prompt.contains("/skip"));
    }

    #[test]
    fn array_prompt() {
        let field = SchemaField {
            field_type: SchemaFieldType::Array,
            label: Some("Tags".into()),
            items: Some(Box::new(base_field(SchemaFieldType::String))),
            ..base_field(SchemaFieldType::Array)
        };
        let prompt = build_prompt("tags", &field);
        assert!(prompt.contains("comma-separated"));
    }

    #[test]
    fn email_prompt() {
        let mut field = base_field(SchemaFieldType::String);
        field.format = Some("email".into());
        field.label = Some("Email".into());
        let prompt = build_prompt("email", &field);
        assert!(prompt.contains("email"));
        assert!(prompt.contains("name@example.com"));
    }

    #[test]
    fn string_with_pattern_and_placeholder() {
        let mut field = base_field(SchemaFieldType::String);
        field.pattern = Some(r"^[A-Z]{3}\d{4}$".into());
        field.placeholder = Some("ABC1234".into());
        field.label = Some("SKU".into());
        let prompt = build_prompt("sku", &field);
        assert!(prompt.contains("e.g. ABC1234"));
    }

    #[test]
    fn description_on_next_line() {
        let mut field = base_field(SchemaFieldType::Integer);
        field.label = Some("Count".into());
        field.description = Some("Number of items to process".into());
        let prompt = build_prompt("count", &field);
        let lines: Vec<&str> = prompt.lines().collect();
        assert!(lines.len() >= 2);
        assert!(lines.last().unwrap().contains("Number of items to process"));
    }
}

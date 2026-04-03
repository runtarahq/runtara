mod parse;
mod prompt;

pub use parse::{ParseResult, parse_text};
pub use prompt::build_prompt;

use runtara_dsl::{SchemaField, VisibleWhen};
use serde_json::{Map, Value};
use std::collections::HashMap;

/// Error from the field collection process.
#[derive(Debug)]
pub enum CollectError {
    /// User exceeded max retries for a field.
    MaxRetries { field: String, hint: String },
    /// User cancelled the form.
    Cancelled,
    /// Channel-level error (disconnect, timeout, etc.)
    ChannelError(String),
}

impl std::fmt::Display for CollectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MaxRetries { field, hint } => {
                write!(f, "Max retries for field '{}': {}", field, hint)
            }
            Self::Cancelled => write!(f, "Input cancelled by user"),
            Self::ChannelError(msg) => write!(f, "Channel error: {}", msg),
        }
    }
}

/// Sort schema fields by `order` (ascending), then alphabetically by key.
pub fn sort_fields(schema: &HashMap<String, SchemaField>) -> Vec<(&String, &SchemaField)> {
    let mut fields: Vec<_> = schema.iter().collect();
    fields.sort_by(|(ka, a), (kb, b)| {
        let oa = a.order.unwrap_or(i32::MAX);
        let ob = b.order.unwrap_or(i32::MAX);
        oa.cmp(&ob).then_with(|| ka.cmp(kb))
    });
    fields
}

/// Evaluate a `visible_when` condition against already-collected values.
pub fn evaluate_visible_when(vw: &VisibleWhen, collected: &Map<String, Value>) -> bool {
    let sibling = collected.get(&vw.field);

    if let Some(equals) = &vw.equals {
        sibling == Some(equals)
    } else if let Some(not_equals) = &vw.not_equals {
        sibling != Some(not_equals)
    } else {
        // No condition specified — always visible.
        true
    }
}

/// Build a structured payload from a schema with only a single field,
/// parsing the raw text directly. Returns `None` if the schema has != 1 field.
pub fn try_single_field_parse(
    schema: &HashMap<String, SchemaField>,
    input: &str,
) -> Option<(String, ParseResult)> {
    if schema.len() != 1 {
        return None;
    }
    let (name, field) = schema.iter().next().unwrap();
    Some((name.clone(), parse_text(input, field)))
}

/// Check if a schema represents a "simple message" form
/// (empty, or a single field named "message" with type string).
pub fn is_message_schema(schema: &HashMap<String, SchemaField>) -> bool {
    if schema.is_empty() {
        return true;
    }
    if schema.len() == 1
        && let Some(field) = schema.get("message")
    {
        return field.field_type == runtara_dsl::SchemaFieldType::String
            && field.enum_values.is_none()
            && field.format.is_none();
    }
    false
}

// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Input mapping code generation.
//!
//! Generates code that maps data from various sources to step inputs.

use proc_macro2::TokenStream;
use quote::quote;

use super::context::EmitContext;
use super::json_to_tokens;
use runtara_dsl::{ImmediateValue, InputMapping, MappingValue, ReferenceValue, ValueType};

/// Emit code for a complete input mapping.
///
/// Generates code that creates a JSON object from the mapping definition,
/// resolving references and embedding immediate values.
///
/// Dotted keys are expanded into nested structures:
/// - `"variables.source"` becomes `{"variables": {"source": value}}`
/// - `"outputs.name"` becomes `{"outputs": {"name": value}}`
pub fn emit_input_mapping(
    mapping: &InputMapping,
    ctx: &mut EmitContext,
    source_var: &proc_macro2::Ident,
) -> TokenStream {
    if mapping.is_empty() {
        return quote! {
            serde_json::Value::Object(serde_json::Map::new())
        };
    }

    let dest_var = ctx.temp_var("mapping_result");

    let field_assignments: Vec<TokenStream> = mapping
        .iter()
        .map(|(key, value)| {
            let value_tokens = emit_mapping_value(value, ctx, source_var);
            emit_nested_insert(&dest_var, key, value_tokens)
        })
        .collect();

    quote! {
        {
            let mut #dest_var = serde_json::Map::new();
            #(#field_assignments)*
            serde_json::Value::Object(#dest_var)
        }
    }
}

/// Emit code to insert a value at a potentially nested path.
///
/// For simple keys like "value", emits: `map.insert("value", value_tokens);`
/// For dotted keys like "variables.source", emits code that creates/updates
/// nested objects to produce: `{"variables": {"source": value_tokens}}`
fn emit_nested_insert(
    dest_var: &proc_macro2::Ident,
    key: &str,
    value_tokens: TokenStream,
) -> TokenStream {
    let parts: Vec<&str> = key.split('.').collect();

    if parts.len() == 1 {
        // Simple key - direct insert
        quote! {
            #dest_var.insert(#key.to_string(), #value_tokens);
        }
    } else {
        // Dotted key - need to create nested structure
        // e.g., "variables.source" -> {"variables": {"source": value}}
        let first_key = parts[0];
        let rest_parts: Vec<&str> = parts[1..].to_vec();

        // Build the nested path insertion
        emit_nested_path_insert(dest_var, first_key, &rest_parts, value_tokens)
    }
}

/// Emit code to insert a value at a nested path within an object.
fn emit_nested_path_insert(
    dest_var: &proc_macro2::Ident,
    first_key: &str,
    rest_parts: &[&str],
    value_tokens: TokenStream,
) -> TokenStream {
    // Generate the full path for building nested objects
    let mut path_tokens = Vec::new();
    let mut current_path = String::new();

    for (i, part) in rest_parts.iter().enumerate() {
        if i > 0 {
            current_path.push('.');
        }
        current_path.push_str(part);
        path_tokens.push(part.to_string());
    }

    // Build a nested setter that ensures intermediate objects exist
    // We generate code like:
    // {
    //     let nested_val = value_tokens;
    //     let entry = dest_var.entry("first_key").or_insert(serde_json::Value::Object(serde_json::Map::new()));
    //     if let serde_json::Value::Object(ref mut nested_map) = entry {
    //         // For each intermediate key, ensure nested object exists
    //         let final_map = ...;
    //         final_map.insert("last_key", nested_val);
    //     }
    // }

    if rest_parts.len() == 1 {
        // Only one more level: variables.source -> insert "source" into "variables" object
        let last_key = rest_parts[0];
        quote! {
            {
                let nested_val = #value_tokens;
                let entry = #dest_var.entry(#first_key.to_string()).or_insert(serde_json::Value::Object(serde_json::Map::new()));
                // In Rust 2024, match ergonomics automatically binds nested_map as &mut Map
                if let serde_json::Value::Object(nested_map) = entry {
                    nested_map.insert(#last_key.to_string(), nested_val);
                }
            }
        }
    } else {
        // Multiple levels: we need to traverse/create each level
        // For simplicity, build the innermost value first, then wrap it
        let path_parts: Vec<String> = rest_parts.iter().map(|s| s.to_string()).collect();

        quote! {
            {
                let nested_val = #value_tokens;
                // Ensure the first level exists
                let entry = #dest_var.entry(#first_key.to_string()).or_insert(serde_json::Value::Object(serde_json::Map::new()));
                // In Rust 2024, match ergonomics automatically binds level_0 as &mut Map
                if let serde_json::Value::Object(level_0) = entry {
                    // Navigate/create nested levels
                    let path_parts: Vec<&str> = vec![#(#path_parts),*];
                    let mut current_map = level_0;
                    for (i, part) in path_parts.iter().enumerate() {
                        if i == path_parts.len() - 1 {
                            // Last part - insert the value
                            current_map.insert(part.to_string(), nested_val.clone());
                        } else {
                            // Intermediate part - ensure object exists
                            let next_entry = current_map.entry(part.to_string()).or_insert(serde_json::Value::Object(serde_json::Map::new()));
                            if let serde_json::Value::Object(next_map) = next_entry {
                                current_map = next_map;
                            } else {
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Emit code for a single mapping value.
pub fn emit_mapping_value(
    value: &MappingValue,
    ctx: &mut EmitContext,
    source_var: &proc_macro2::Ident,
) -> TokenStream {
    match value {
        MappingValue::Reference(ref_val) => emit_reference_value(ref_val, ctx, source_var),
        MappingValue::Immediate(imm_val) => emit_immediate_value(imm_val),
    }
}

/// Emit code for a reference value (data lookup).
fn emit_reference_value(
    ref_val: &ReferenceValue,
    _ctx: &EmitContext,
    source_var: &proc_macro2::Ident,
) -> TokenStream {
    let path = &ref_val.value;
    let json_pointer = path_to_json_pointer(path);

    // Generate the base lookup, using default value if provided
    let lookup = if let Some(default_val) = &ref_val.default {
        let default_tokens = super::json_to_tokens(default_val);
        quote! {
            {
                let looked_up = #source_var.pointer(#json_pointer).cloned();
                match looked_up {
                    Some(serde_json::Value::Null) | None => #default_tokens,
                    Some(v) => v,
                }
            }
        }
    } else {
        quote! {
            #source_var.pointer(#json_pointer).cloned().unwrap_or(serde_json::Value::Null)
        }
    };

    // Apply type conversion if specified
    match &ref_val.type_hint {
        Some(ValueType::String) => {
            quote! {
                {
                    let val = #lookup;
                    match &val {
                        serde_json::Value::String(s) => serde_json::Value::String(s.clone()),
                        serde_json::Value::Number(n) => serde_json::Value::String(n.to_string()),
                        serde_json::Value::Bool(b) => serde_json::Value::String(b.to_string()),
                        serde_json::Value::Null => serde_json::Value::String("".to_string()),
                        _ => serde_json::Value::String(val.to_string()),
                    }
                }
            }
        }
        Some(ValueType::Integer) => {
            quote! {
                {
                    let val = #lookup;
                    match &val {
                        serde_json::Value::Number(n) => {
                            if let Some(i) = n.as_i64() {
                                serde_json::Value::Number(serde_json::Number::from(i))
                            } else if let Some(f) = n.as_f64() {
                                serde_json::Value::Number(serde_json::Number::from(f as i64))
                            } else {
                                serde_json::Value::Number(serde_json::Number::from(0))
                            }
                        }
                        serde_json::Value::String(s) => {
                            s.parse::<i64>()
                                .map(|i| serde_json::Value::Number(serde_json::Number::from(i)))
                                .unwrap_or(serde_json::Value::Number(serde_json::Number::from(0)))
                        }
                        serde_json::Value::Bool(b) => {
                            serde_json::Value::Number(serde_json::Number::from(if *b { 1 } else { 0 }))
                        }
                        _ => serde_json::Value::Number(serde_json::Number::from(0)),
                    }
                }
            }
        }
        Some(ValueType::Number) => {
            quote! {
                {
                    let val = #lookup;
                    match &val {
                        serde_json::Value::Number(n) => {
                            if let Some(f) = n.as_f64() {
                                serde_json::Number::from_f64(f)
                                    .map(serde_json::Value::Number)
                                    .unwrap_or(serde_json::Value::Number(serde_json::Number::from(0)))
                            } else {
                                serde_json::Value::Number(serde_json::Number::from(0))
                            }
                        }
                        serde_json::Value::String(s) => {
                            s.parse::<f64>()
                                .ok()
                                .and_then(|f| serde_json::Number::from_f64(f))
                                .map(serde_json::Value::Number)
                                .unwrap_or(serde_json::Value::Number(serde_json::Number::from(0)))
                        }
                        _ => serde_json::Value::Number(serde_json::Number::from(0)),
                    }
                }
            }
        }
        Some(ValueType::Boolean) => {
            quote! {
                {
                    let val = #lookup;
                    match &val {
                        serde_json::Value::Bool(b) => serde_json::Value::Bool(*b),
                        serde_json::Value::String(s) => {
                            serde_json::Value::Bool(s == "true" || s == "1")
                        }
                        serde_json::Value::Number(n) => {
                            serde_json::Value::Bool(n.as_i64().map(|i| i != 0).unwrap_or(false))
                        }
                        serde_json::Value::Null => serde_json::Value::Bool(false),
                        serde_json::Value::Array(a) => serde_json::Value::Bool(!a.is_empty()),
                        serde_json::Value::Object(o) => serde_json::Value::Bool(!o.is_empty()),
                    }
                }
            }
        }
        Some(ValueType::Json) | Some(ValueType::File) | None => {
            // Pass through as-is (File is a JSON object with content, filename, mimeType)
            lookup
        }
    }
}

/// Emit code for an immediate (literal) value.
fn emit_immediate_value(imm_val: &ImmediateValue) -> TokenStream {
    json_to_tokens(&imm_val.value)
}

/// Convert a dot-notation path to a JSON pointer.
///
/// Examples:
/// - "data.user.name" -> "/data/user/name"
/// - "steps.step1.outputs.items" -> "/steps/step1/outputs/items"
/// - "steps['step-1'].outputs" -> "/steps/step-1/outputs"
/// - "instances[0].properties" -> "/instances/0/properties"
fn path_to_json_pointer(path: &str) -> String {
    // Handle bracket notation: steps['step-1'] -> steps/step-1
    let normalized = path
        .replace("['", ".")
        .replace("']", "")
        .replace("[\"", ".")
        .replace("\"]", "");

    // Handle numeric array indices: instances[0] -> instances.0
    let with_array_indices = {
        let mut result = String::new();
        let mut chars = normalized.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '[' {
                // Collect the index
                let mut index = String::new();
                while let Some(&next_c) = chars.peek() {
                    if next_c == ']' {
                        chars.next(); // consume ']'
                        break;
                    }
                    index.push(chars.next().unwrap());
                }
                // If it's a numeric index, add as .index
                if index.chars().all(|c| c.is_ascii_digit()) {
                    result.push('.');
                    result.push_str(&index);
                } else {
                    // Non-numeric, keep original bracket notation
                    result.push('[');
                    result.push_str(&index);
                    result.push(']');
                }
            } else {
                result.push(c);
            }
        }
        result
    };

    // Convert dots to slashes
    let parts: Vec<&str> = with_array_indices.split('.').collect();
    format!("/{}", parts.join("/"))
}

/// Emit code to build the step inputs data source.
/// This combines data, variables, and steps context into a single source object.
pub fn emit_build_source(ctx: &EmitContext) -> TokenStream {
    let inputs = &ctx.inputs_var;
    let steps_context = &ctx.steps_context_var;

    quote! {
        {
            let mut source_map = serde_json::Map::new();
            source_map.insert("data".to_string(), (*#inputs.data).clone());
            source_map.insert("variables".to_string(), (*#inputs.variables).clone());
            source_map.insert("steps".to_string(), serde_json::Value::Object(#steps_context.clone()));
            source_map.insert("scenario".to_string(), serde_json::json!({
                "inputs": {
                    "data": &*#inputs.data,
                    "variables": &*#inputs.variables
                }
            }));
            serde_json::Value::Object(source_map)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_to_json_pointer() {
        assert_eq!(path_to_json_pointer("data"), "/data");
        assert_eq!(path_to_json_pointer("data.user.name"), "/data/user/name");
        assert_eq!(
            path_to_json_pointer("steps.step1.outputs"),
            "/steps/step1/outputs"
        );
        assert_eq!(
            path_to_json_pointer("steps['step-1'].outputs"),
            "/steps/step-1/outputs"
        );
        // Test numeric array indices
        assert_eq!(
            path_to_json_pointer("instances[0].properties.name"),
            "/instances/0/properties/name"
        );
        assert_eq!(
            path_to_json_pointer("steps.get-markup.outputs.instances[0].properties.markup"),
            "/steps/get-markup/outputs/instances/0/properties/markup"
        );
    }
}

// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Input mapping code generation.
//!
//! Generates code that maps data from various sources to step inputs.

use proc_macro2::TokenStream;
use quote::quote;

use super::context::EmitContext;
use super::json_to_tokens;
use runtara_dsl::{
    CompositeInner, CompositeValue, ImmediateValue, InputMapping, MappingValue, ReferenceValue,
    ValueType,
};

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
        MappingValue::Composite(comp_val) => emit_composite_value(comp_val, ctx, source_var),
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

/// Emit code for a composite value (nested object or array with MappingValues).
///
/// Generates code that builds a JSON object or array at runtime, where each
/// field/element can be a reference, immediate, or another composite.
fn emit_composite_value(
    comp_val: &CompositeValue,
    ctx: &mut EmitContext,
    source_var: &proc_macro2::Ident,
) -> TokenStream {
    match &comp_val.value {
        CompositeInner::Object(map) => {
            if map.is_empty() {
                return quote! { serde_json::Value::Object(serde_json::Map::new()) };
            }
            let field_assignments: Vec<TokenStream> = map
                .iter()
                .map(|(key, value)| {
                    let value_tokens = emit_mapping_value(value, ctx, source_var);
                    quote! { obj.insert(#key.to_string(), #value_tokens); }
                })
                .collect();
            quote! {
                {
                    let mut obj = serde_json::Map::new();
                    #(#field_assignments)*
                    serde_json::Value::Object(obj)
                }
            }
        }
        CompositeInner::Array(arr) => {
            if arr.is_empty() {
                return quote! { serde_json::Value::Array(vec![]) };
            }
            let element_tokens: Vec<TokenStream> = arr
                .iter()
                .map(|value| emit_mapping_value(value, ctx, source_var))
                .collect();
            quote! {
                serde_json::Value::Array(vec![#(#element_tokens),*])
            }
        }
    }
}

/// Convert a dot-notation path to a JSON pointer.
///
/// Examples:
/// - "data.user.name" -> "/data/user/name"
/// - "steps.step1.outputs.items" -> "/steps/step1/outputs/items"
/// - "steps['step-1'].outputs" -> "/steps/step-1/outputs"
/// - "instances[0].properties" -> "/instances/0/properties"
pub fn path_to_json_pointer(path: &str) -> String {
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
    use proc_macro2::{Ident, Span};

    // ==========================================
    // Tests for path_to_json_pointer
    // ==========================================

    #[test]
    fn test_path_to_json_pointer_simple() {
        assert_eq!(path_to_json_pointer("data"), "/data");
    }

    #[test]
    fn test_path_to_json_pointer_nested() {
        assert_eq!(path_to_json_pointer("data.user.name"), "/data/user/name");
    }

    #[test]
    fn test_path_to_json_pointer_steps_context() {
        assert_eq!(
            path_to_json_pointer("steps.step1.outputs"),
            "/steps/step1/outputs"
        );
    }

    #[test]
    fn test_path_to_json_pointer_bracket_notation() {
        assert_eq!(
            path_to_json_pointer("steps['step-1'].outputs"),
            "/steps/step-1/outputs"
        );
    }

    #[test]
    fn test_path_to_json_pointer_double_quote_bracket() {
        assert_eq!(
            path_to_json_pointer("steps[\"step-2\"].value"),
            "/steps/step-2/value"
        );
    }

    #[test]
    fn test_path_to_json_pointer_numeric_array_index() {
        assert_eq!(
            path_to_json_pointer("instances[0].properties.name"),
            "/instances/0/properties/name"
        );
    }

    #[test]
    fn test_path_to_json_pointer_complex_path() {
        assert_eq!(
            path_to_json_pointer("steps.get-markup.outputs.instances[0].properties.markup"),
            "/steps/get-markup/outputs/instances/0/properties/markup"
        );
    }

    #[test]
    fn test_path_to_json_pointer_multiple_array_indices() {
        assert_eq!(
            path_to_json_pointer("data[0].items[1].value"),
            "/data/0/items/1/value"
        );
    }

    #[test]
    fn test_path_to_json_pointer_deeply_nested() {
        assert_eq!(path_to_json_pointer("a.b.c.d.e.f.g"), "/a/b/c/d/e/f/g");
    }

    // ==========================================
    // Tests for emit_immediate_value
    // ==========================================

    #[test]
    fn test_emit_immediate_value_string() {
        let imm = ImmediateValue {
            value: serde_json::json!("hello"),
        };
        let tokens = emit_immediate_value(&imm);
        let code = tokens.to_string();
        assert!(code.contains("hello"));
        assert!(code.contains("String"));
    }

    #[test]
    fn test_emit_immediate_value_number() {
        let imm = ImmediateValue {
            value: serde_json::json!(42),
        };
        let tokens = emit_immediate_value(&imm);
        let code = tokens.to_string();
        assert!(code.contains("42"));
        assert!(code.contains("Number"));
    }

    #[test]
    fn test_emit_immediate_value_boolean() {
        let imm = ImmediateValue {
            value: serde_json::json!(true),
        };
        let tokens = emit_immediate_value(&imm);
        let code = tokens.to_string();
        assert!(code.contains("true"));
        assert!(code.contains("Bool"));
    }

    #[test]
    fn test_emit_immediate_value_null() {
        let imm = ImmediateValue {
            value: serde_json::Value::Null,
        };
        let tokens = emit_immediate_value(&imm);
        let code = tokens.to_string();
        assert!(code.contains("Null"));
    }

    #[test]
    fn test_emit_immediate_value_array() {
        let imm = ImmediateValue {
            value: serde_json::json!([1, 2, 3]),
        };
        let tokens = emit_immediate_value(&imm);
        let code = tokens.to_string();
        assert!(code.contains("from_str"));
        assert!(code.contains("[1,2,3]"));
    }

    #[test]
    fn test_emit_immediate_value_object() {
        let imm = ImmediateValue {
            value: serde_json::json!({"key": "value"}),
        };
        let tokens = emit_immediate_value(&imm);
        let code = tokens.to_string();
        assert!(code.contains("from_str"));
        assert!(code.contains("key"));
    }

    // ==========================================
    // Tests for emit_reference_value
    // ==========================================

    #[test]
    fn test_emit_reference_value_basic() {
        let ref_val = ReferenceValue {
            value: "data.user.name".to_string(),
            type_hint: None,
            default: None,
        };
        let ctx = EmitContext::new(false);
        let source_var = Ident::new("source", Span::call_site());
        let tokens = emit_reference_value(&ref_val, &ctx, &source_var);
        let code = tokens.to_string();

        assert!(code.contains("source"));
        assert!(code.contains("pointer"));
        assert!(code.contains("/data/user/name"));
    }

    #[test]
    fn test_emit_reference_value_with_default() {
        let ref_val = ReferenceValue {
            value: "data.optional".to_string(),
            type_hint: None,
            default: Some(serde_json::json!("default_value")),
        };
        let ctx = EmitContext::new(false);
        let source_var = Ident::new("source", Span::call_site());
        let tokens = emit_reference_value(&ref_val, &ctx, &source_var);
        let code = tokens.to_string();

        assert!(code.contains("default_value"));
        assert!(code.contains("Null"));
        assert!(code.contains("None"));
    }

    #[test]
    fn test_emit_reference_value_type_hint_string() {
        let ref_val = ReferenceValue {
            value: "data.value".to_string(),
            type_hint: Some(ValueType::String),
            default: None,
        };
        let ctx = EmitContext::new(false);
        let source_var = Ident::new("source", Span::call_site());
        let tokens = emit_reference_value(&ref_val, &ctx, &source_var);
        let code = tokens.to_string();

        // String type hint should generate conversion logic
        assert!(code.contains("String"));
        assert!(code.contains("to_string"));
    }

    #[test]
    fn test_emit_reference_value_type_hint_integer() {
        let ref_val = ReferenceValue {
            value: "data.count".to_string(),
            type_hint: Some(ValueType::Integer),
            default: None,
        };
        let ctx = EmitContext::new(false);
        let source_var = Ident::new("source", Span::call_site());
        let tokens = emit_reference_value(&ref_val, &ctx, &source_var);
        let code = tokens.to_string();

        // Integer type hint should generate i64 conversion
        assert!(code.contains("as_i64"));
        assert!(code.contains("parse"));
    }

    #[test]
    fn test_emit_reference_value_type_hint_number() {
        let ref_val = ReferenceValue {
            value: "data.price".to_string(),
            type_hint: Some(ValueType::Number),
            default: None,
        };
        let ctx = EmitContext::new(false);
        let source_var = Ident::new("source", Span::call_site());
        let tokens = emit_reference_value(&ref_val, &ctx, &source_var);
        let code = tokens.to_string();

        // Number type hint should generate f64 conversion
        assert!(code.contains("as_f64"));
        assert!(code.contains("from_f64"));
    }

    #[test]
    fn test_emit_reference_value_type_hint_boolean() {
        let ref_val = ReferenceValue {
            value: "data.enabled".to_string(),
            type_hint: Some(ValueType::Boolean),
            default: None,
        };
        let ctx = EmitContext::new(false);
        let source_var = Ident::new("source", Span::call_site());
        let tokens = emit_reference_value(&ref_val, &ctx, &source_var);
        let code = tokens.to_string();

        // Boolean type hint should generate boolean conversion
        assert!(code.contains("Bool"));
        assert!(code.contains("\"true\""));
        assert!(code.contains("\"1\""));
    }

    #[test]
    fn test_emit_reference_value_type_hint_json() {
        let ref_val = ReferenceValue {
            value: "data.payload".to_string(),
            type_hint: Some(ValueType::Json),
            default: None,
        };
        let ctx = EmitContext::new(false);
        let source_var = Ident::new("source", Span::call_site());
        let tokens = emit_reference_value(&ref_val, &ctx, &source_var);
        let code = tokens.to_string();

        // Json type hint should pass through as-is (no conversion)
        assert!(code.contains("pointer"));
        assert!(code.contains("unwrap_or"));
    }

    // ==========================================
    // Tests for emit_mapping_value
    // ==========================================

    #[test]
    fn test_emit_mapping_value_reference() {
        let map_val = MappingValue::Reference(ReferenceValue {
            value: "data.field".to_string(),
            type_hint: None,
            default: None,
        });
        let mut ctx = EmitContext::new(false);
        let source_var = Ident::new("src", Span::call_site());
        let tokens = emit_mapping_value(&map_val, &mut ctx, &source_var);
        let code = tokens.to_string();

        assert!(code.contains("src"));
        assert!(code.contains("/data/field"));
    }

    #[test]
    fn test_emit_mapping_value_immediate() {
        let map_val = MappingValue::Immediate(ImmediateValue {
            value: serde_json::json!("constant"),
        });
        let mut ctx = EmitContext::new(false);
        let source_var = Ident::new("src", Span::call_site());
        let tokens = emit_mapping_value(&map_val, &mut ctx, &source_var);
        let code = tokens.to_string();

        assert!(code.contains("constant"));
    }

    // ==========================================
    // Tests for emit_input_mapping
    // ==========================================

    #[test]
    fn test_emit_input_mapping_empty() {
        let mapping: InputMapping = std::collections::HashMap::new();
        let mut ctx = EmitContext::new(false);
        let source_var = Ident::new("source", Span::call_site());
        let tokens = emit_input_mapping(&mapping, &mut ctx, &source_var);
        let code = tokens.to_string();

        assert!(code.contains("serde_json :: Value :: Object"));
        assert!(code.contains("serde_json :: Map :: new ()"));
    }

    #[test]
    fn test_emit_input_mapping_single_field() {
        let mut mapping: InputMapping = std::collections::HashMap::new();
        mapping.insert(
            "output".to_string(),
            MappingValue::Reference(ReferenceValue {
                value: "data.input".to_string(),
                type_hint: None,
                default: None,
            }),
        );
        let mut ctx = EmitContext::new(false);
        let source_var = Ident::new("source", Span::call_site());
        let tokens = emit_input_mapping(&mapping, &mut ctx, &source_var);
        let code = tokens.to_string();

        assert!(code.contains("output"));
        assert!(code.contains("insert"));
        assert!(code.contains("/data/input"));
    }

    #[test]
    fn test_emit_input_mapping_multiple_fields() {
        let mut mapping: InputMapping = std::collections::HashMap::new();
        mapping.insert(
            "field1".to_string(),
            MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!("value1"),
            }),
        );
        mapping.insert(
            "field2".to_string(),
            MappingValue::Reference(ReferenceValue {
                value: "data.source".to_string(),
                type_hint: None,
                default: None,
            }),
        );
        let mut ctx = EmitContext::new(false);
        let source_var = Ident::new("source", Span::call_site());
        let tokens = emit_input_mapping(&mapping, &mut ctx, &source_var);
        let code = tokens.to_string();

        assert!(code.contains("field1"));
        assert!(code.contains("field2"));
        assert!(code.contains("value1"));
    }

    // ==========================================
    // Tests for emit_nested_insert (dotted keys)
    // ==========================================

    #[test]
    fn test_emit_nested_insert_simple_key() {
        let dest_var = Ident::new("dest", Span::call_site());
        let value_tokens = quote! { serde_json::Value::String("test".to_string()) };
        let tokens = emit_nested_insert(&dest_var, "simple", value_tokens);
        let code = tokens.to_string();

        assert!(code.contains("dest . insert"));
        assert!(code.contains("simple"));
    }

    #[test]
    fn test_emit_nested_insert_dotted_key_two_levels() {
        let dest_var = Ident::new("dest", Span::call_site());
        let value_tokens = quote! { serde_json::Value::String("test".to_string()) };
        let tokens = emit_nested_insert(&dest_var, "variables.source", value_tokens);
        let code = tokens.to_string();

        // Should create nested object structure
        assert!(code.contains("variables"));
        assert!(code.contains("source"));
        assert!(code.contains("entry"));
        assert!(code.contains("Object"));
    }

    #[test]
    fn test_emit_nested_insert_dotted_key_three_levels() {
        let dest_var = Ident::new("dest", Span::call_site());
        let value_tokens = quote! { serde_json::Value::Bool(true) };
        let tokens = emit_nested_insert(&dest_var, "a.b.c", value_tokens);
        let code = tokens.to_string();

        // Should handle multiple levels of nesting
        assert!(code.contains("path_parts"));
        assert!(code.contains("a"));
    }

    // ==========================================
    // Tests for emit_build_source
    // ==========================================

    #[test]
    fn test_emit_build_source() {
        let ctx = EmitContext::new(false);
        let tokens = emit_build_source(&ctx);
        let code = tokens.to_string();

        // Should build source with data, variables, steps, and scenario
        assert!(code.contains("data"));
        assert!(code.contains("variables"));
        assert!(code.contains("steps"));
        assert!(code.contains("scenario"));
        assert!(code.contains("source_map"));
    }

    #[test]
    fn test_emit_build_source_uses_context_vars() {
        let ctx = EmitContext::new(false);
        let tokens = emit_build_source(&ctx);
        let code = tokens.to_string();

        // Should use the context's input and steps_context variables
        assert!(code.contains("inputs"));
        assert!(code.contains("steps_context"));
    }

    #[test]
    fn test_emit_build_source_includes_scenario_inputs() {
        let ctx = EmitContext::new(false);
        let tokens = emit_build_source(&ctx);
        let code = tokens.to_string();

        // Should include scenario.inputs structure
        assert!(code.contains("inputs"));
        assert!(
            code.contains("data") && code.contains("variables"),
            "Should have both data and variables in scenario.inputs"
        );
    }

    // ==========================================
    // Tests for type conversion in references
    // ==========================================

    #[test]
    fn test_emit_reference_string_conversion_from_number() {
        let ref_val = ReferenceValue {
            value: "data.count".to_string(),
            type_hint: Some(ValueType::String),
            default: None,
        };
        let ctx = EmitContext::new(false);
        let source_var = Ident::new("source", Span::call_site());
        let tokens = emit_reference_value(&ref_val, &ctx, &source_var);
        let code = tokens.to_string();

        // String conversion should handle Number -> String
        assert!(code.contains("Number (n)"));
        assert!(code.contains("n . to_string ()"));
    }

    #[test]
    fn test_emit_reference_string_conversion_from_bool() {
        let ref_val = ReferenceValue {
            value: "data.flag".to_string(),
            type_hint: Some(ValueType::String),
            default: None,
        };
        let ctx = EmitContext::new(false);
        let source_var = Ident::new("source", Span::call_site());
        let tokens = emit_reference_value(&ref_val, &ctx, &source_var);
        let code = tokens.to_string();

        // String conversion should handle Bool -> String
        assert!(code.contains("Bool (b)"));
        assert!(code.contains("b . to_string ()"));
    }

    #[test]
    fn test_emit_reference_integer_from_string() {
        let ref_val = ReferenceValue {
            value: "data.str_num".to_string(),
            type_hint: Some(ValueType::Integer),
            default: None,
        };
        let ctx = EmitContext::new(false);
        let source_var = Ident::new("source", Span::call_site());
        let tokens = emit_reference_value(&ref_val, &ctx, &source_var);
        let code = tokens.to_string();

        // Integer conversion should parse string
        assert!(code.contains("parse :: < i64 >"));
    }

    #[test]
    fn test_emit_reference_boolean_from_string() {
        let ref_val = ReferenceValue {
            value: "data.str_bool".to_string(),
            type_hint: Some(ValueType::Boolean),
            default: None,
        };
        let ctx = EmitContext::new(false);
        let source_var = Ident::new("source", Span::call_site());
        let tokens = emit_reference_value(&ref_val, &ctx, &source_var);
        let code = tokens.to_string();

        // Boolean conversion should check "true" and "1" strings
        assert!(code.contains("== \"true\""));
        assert!(code.contains("== \"1\""));
    }

    #[test]
    fn test_emit_reference_boolean_truthy_checks() {
        let ref_val = ReferenceValue {
            value: "data.value".to_string(),
            type_hint: Some(ValueType::Boolean),
            default: None,
        };
        let ctx = EmitContext::new(false);
        let source_var = Ident::new("source", Span::call_site());
        let tokens = emit_reference_value(&ref_val, &ctx, &source_var);
        let code = tokens.to_string();

        // Boolean should check various truthy values
        assert!(code.contains("Array"), "Should check arrays for emptiness");
        assert!(
            code.contains("Object"),
            "Should check objects for emptiness"
        );
        assert!(code.contains("Null"), "Should handle null as false");
    }

    // ==========================================
    // Tests for default value handling
    // ==========================================

    #[test]
    fn test_emit_reference_default_used_on_null() {
        let ref_val = ReferenceValue {
            value: "data.nullable".to_string(),
            type_hint: None,
            default: Some(serde_json::json!("fallback")),
        };
        let ctx = EmitContext::new(false);
        let source_var = Ident::new("source", Span::call_site());
        let tokens = emit_reference_value(&ref_val, &ctx, &source_var);
        let code = tokens.to_string();

        // Default should be used when value is Null or None
        assert!(code.contains("Null"));
        assert!(code.contains("None"));
        assert!(code.contains("fallback"));
    }

    #[test]
    fn test_emit_reference_default_complex_value() {
        let ref_val = ReferenceValue {
            value: "data.config".to_string(),
            type_hint: None,
            default: Some(serde_json::json!({"enabled": true, "count": 5})),
        };
        let ctx = EmitContext::new(false);
        let source_var = Ident::new("source", Span::call_site());
        let tokens = emit_reference_value(&ref_val, &ctx, &source_var);
        let code = tokens.to_string();

        assert!(code.contains("enabled"));
        assert!(code.contains("count"));
    }

    // ==========================================
    // Tests for edge cases
    // ==========================================

    #[test]
    fn test_path_to_json_pointer_empty() {
        // Empty string becomes just "/"
        assert_eq!(path_to_json_pointer(""), "/");
    }

    #[test]
    fn test_path_to_json_pointer_single_char() {
        assert_eq!(path_to_json_pointer("x"), "/x");
    }

    #[test]
    fn test_emit_mapping_generates_unique_temp_vars() {
        let mut mapping: InputMapping = std::collections::HashMap::new();
        mapping.insert(
            "field".to_string(),
            MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!("value"),
            }),
        );
        let mut ctx = EmitContext::new(false);
        let source_var = Ident::new("source", Span::call_site());

        let tokens1 = emit_input_mapping(&mapping, &mut ctx, &source_var);
        let tokens2 = emit_input_mapping(&mapping, &mut ctx, &source_var);

        let code1 = tokens1.to_string();
        let code2 = tokens2.to_string();

        // Each call should use different temp variable names
        assert!(code1.contains("mapping_result_1"));
        assert!(code2.contains("mapping_result_2"));
    }

    // ==========================================
    // Tests for emit_composite_value
    // ==========================================

    #[test]
    fn test_emit_composite_value_empty_object() {
        let comp_val = CompositeValue {
            value: CompositeInner::Object(std::collections::HashMap::new()),
        };
        let mut ctx = EmitContext::new(false);
        let source_var = Ident::new("source", Span::call_site());
        let tokens = emit_composite_value(&comp_val, &mut ctx, &source_var);
        let code = tokens.to_string();

        assert!(code.contains("Object"));
        assert!(code.contains("Map :: new"));
    }

    #[test]
    fn test_emit_composite_value_empty_array() {
        let comp_val = CompositeValue {
            value: CompositeInner::Array(vec![]),
        };
        let mut ctx = EmitContext::new(false);
        let source_var = Ident::new("source", Span::call_site());
        let tokens = emit_composite_value(&comp_val, &mut ctx, &source_var);
        let code = tokens.to_string();

        assert!(code.contains("Array"));
        assert!(code.contains("vec !"));
    }

    #[test]
    fn test_emit_composite_value_object_with_immediate() {
        let mut map = std::collections::HashMap::new();
        map.insert(
            "name".to_string(),
            MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!("John"),
            }),
        );
        let comp_val = CompositeValue {
            value: CompositeInner::Object(map),
        };
        let mut ctx = EmitContext::new(false);
        let source_var = Ident::new("source", Span::call_site());
        let tokens = emit_composite_value(&comp_val, &mut ctx, &source_var);
        let code = tokens.to_string();

        assert!(code.contains("name"));
        assert!(code.contains("John"));
        assert!(code.contains("insert"));
    }

    #[test]
    fn test_emit_composite_value_object_with_reference() {
        let mut map = std::collections::HashMap::new();
        map.insert(
            "userId".to_string(),
            MappingValue::Reference(ReferenceValue {
                value: "data.user.id".to_string(),
                type_hint: None,
                default: None,
            }),
        );
        let comp_val = CompositeValue {
            value: CompositeInner::Object(map),
        };
        let mut ctx = EmitContext::new(false);
        let source_var = Ident::new("source", Span::call_site());
        let tokens = emit_composite_value(&comp_val, &mut ctx, &source_var);
        let code = tokens.to_string();

        assert!(code.contains("userId"));
        assert!(code.contains("/data/user/id"));
        assert!(code.contains("pointer"));
    }

    #[test]
    fn test_emit_composite_value_object_mixed() {
        let mut map = std::collections::HashMap::new();
        map.insert(
            "name".to_string(),
            MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!("John"),
            }),
        );
        map.insert(
            "userId".to_string(),
            MappingValue::Reference(ReferenceValue {
                value: "data.user.id".to_string(),
                type_hint: None,
                default: None,
            }),
        );
        let comp_val = CompositeValue {
            value: CompositeInner::Object(map),
        };
        let mut ctx = EmitContext::new(false);
        let source_var = Ident::new("source", Span::call_site());
        let tokens = emit_composite_value(&comp_val, &mut ctx, &source_var);
        let code = tokens.to_string();

        // Should have both immediate and reference
        assert!(code.contains("name"));
        assert!(code.contains("John"));
        assert!(code.contains("userId"));
        assert!(code.contains("/data/user/id"));
    }

    #[test]
    fn test_emit_composite_value_array_with_references() {
        let comp_val = CompositeValue {
            value: CompositeInner::Array(vec![
                MappingValue::Reference(ReferenceValue {
                    value: "data.items[0]".to_string(),
                    type_hint: None,
                    default: None,
                }),
                MappingValue::Reference(ReferenceValue {
                    value: "data.items[1]".to_string(),
                    type_hint: None,
                    default: None,
                }),
            ]),
        };
        let mut ctx = EmitContext::new(false);
        let source_var = Ident::new("source", Span::call_site());
        let tokens = emit_composite_value(&comp_val, &mut ctx, &source_var);
        let code = tokens.to_string();

        assert!(code.contains("Array"));
        assert!(code.contains("/data/items/0"));
        assert!(code.contains("/data/items/1"));
    }

    #[test]
    fn test_emit_composite_value_array_mixed() {
        let comp_val = CompositeValue {
            value: CompositeInner::Array(vec![
                MappingValue::Reference(ReferenceValue {
                    value: "data.first".to_string(),
                    type_hint: None,
                    default: None,
                }),
                MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!("static"),
                }),
                MappingValue::Reference(ReferenceValue {
                    value: "data.last".to_string(),
                    type_hint: None,
                    default: None,
                }),
            ]),
        };
        let mut ctx = EmitContext::new(false);
        let source_var = Ident::new("source", Span::call_site());
        let tokens = emit_composite_value(&comp_val, &mut ctx, &source_var);
        let code = tokens.to_string();

        assert!(code.contains("Array"));
        assert!(code.contains("/data/first"));
        assert!(code.contains("static"));
        assert!(code.contains("/data/last"));
    }

    #[test]
    fn test_emit_composite_value_nested_composite() {
        // Nested composite: object containing an array
        let inner_array = CompositeValue {
            value: CompositeInner::Array(vec![MappingValue::Reference(ReferenceValue {
                value: "data.items[0]".to_string(),
                type_hint: None,
                default: None,
            })]),
        };
        let mut outer_map = std::collections::HashMap::new();
        outer_map.insert("items".to_string(), MappingValue::Composite(inner_array));
        let comp_val = CompositeValue {
            value: CompositeInner::Object(outer_map),
        };
        let mut ctx = EmitContext::new(false);
        let source_var = Ident::new("source", Span::call_site());
        let tokens = emit_composite_value(&comp_val, &mut ctx, &source_var);
        let code = tokens.to_string();

        // Should have nested structure
        assert!(code.contains("items"));
        assert!(code.contains("Array"));
        assert!(code.contains("/data/items/0"));
    }

    #[test]
    fn test_emit_mapping_value_composite() {
        let mut map = std::collections::HashMap::new();
        map.insert(
            "field".to_string(),
            MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!("value"),
            }),
        );
        let comp_val = MappingValue::Composite(CompositeValue {
            value: CompositeInner::Object(map),
        });
        let mut ctx = EmitContext::new(false);
        let source_var = Ident::new("src", Span::call_site());
        let tokens = emit_mapping_value(&comp_val, &mut ctx, &source_var);
        let code = tokens.to_string();

        assert!(code.contains("field"));
        assert!(code.contains("value"));
    }
}

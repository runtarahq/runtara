// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared condition expression emitters for AST-based code generation.
//!
//! These functions generate Rust `TokenStream` code that evaluates
//! `ConditionExpression` trees at runtime. Used by both Conditional and
//! Switch step emitters.

use proc_macro2::TokenStream;
use quote::quote;

use super::context::EmitContext;
use super::mapping::emit_mapping_value;
use runtara_dsl::{ConditionArgument, ConditionExpression, ConditionOperation, ConditionOperator};

/// Emit code that evaluates a `ConditionExpression` to a `bool`.
pub fn emit_condition_expression(
    expr: &ConditionExpression,
    ctx: &mut EmitContext,
    source_var: &proc_macro2::Ident,
) -> TokenStream {
    match expr {
        ConditionExpression::Operation(op) => emit_operation(op, ctx, source_var),
        ConditionExpression::Value(mapping_value) => {
            // A direct value - evaluate as truthy
            let value_code = emit_mapping_value(mapping_value, ctx, source_var);
            quote! {
                {
                    let val = #value_code;
                    is_truthy(&val)
                }
            }
        }
    }
}

/// Emit code for a `ConditionOperation`.
pub fn emit_operation(
    op: &ConditionOperation,
    ctx: &mut EmitContext,
    source_var: &proc_macro2::Ident,
) -> TokenStream {
    let arguments = &op.arguments;

    match op.op {
        // Logical operators
        ConditionOperator::And => {
            if arguments.is_empty() {
                return quote! { true };
            }
            let arg_codes: Vec<TokenStream> = arguments
                .iter()
                .map(|arg| emit_argument_as_bool(arg, ctx, source_var))
                .collect();
            quote! { #(#arg_codes)&&* }
        }
        ConditionOperator::Or => {
            if arguments.is_empty() {
                return quote! { false };
            }
            let arg_codes: Vec<TokenStream> = arguments
                .iter()
                .map(|arg| emit_argument_as_bool(arg, ctx, source_var))
                .collect();
            quote! { #(#arg_codes)||* }
        }
        ConditionOperator::Not => {
            if arguments.is_empty() {
                return quote! { true };
            }
            let arg_code = emit_argument_as_bool(&arguments[0], ctx, source_var);
            quote! { !(#arg_code) }
        }

        // Comparison operators
        ConditionOperator::Gt => emit_comparison(arguments, ctx, source_var, quote! { > }),
        ConditionOperator::Gte => emit_comparison(arguments, ctx, source_var, quote! { >= }),
        ConditionOperator::Lt => emit_comparison(arguments, ctx, source_var, quote! { < }),
        ConditionOperator::Lte => emit_comparison(arguments, ctx, source_var, quote! { <= }),
        ConditionOperator::Eq => emit_equality(arguments, ctx, source_var, false),
        ConditionOperator::Ne => emit_equality(arguments, ctx, source_var, true),

        // String operators
        ConditionOperator::StartsWith => emit_starts_with(arguments, ctx, source_var),
        ConditionOperator::EndsWith => emit_ends_with(arguments, ctx, source_var),

        // Array operators
        ConditionOperator::Contains => emit_contains(arguments, ctx, source_var),
        ConditionOperator::In => emit_in(arguments, ctx, source_var),
        ConditionOperator::NotIn => {
            let in_code = emit_in(arguments, ctx, source_var);
            quote! { !(#in_code) }
        }

        // Utility operators
        ConditionOperator::Length => {
            // When used as a boolean, non-zero length is truthy
            if arguments.is_empty() {
                return quote! { false };
            }
            let arg_code = emit_argument_as_value(&arguments[0], ctx, source_var);
            quote! {
                {
                    let val = #arg_code;
                    let len: i64 = match &val {
                        serde_json::Value::String(s) => s.len() as i64,
                        serde_json::Value::Array(a) => a.len() as i64,
                        serde_json::Value::Object(o) => o.len() as i64,
                        serde_json::Value::Null => 0,
                        _ => 1,
                    };
                    len > 0
                }
            }
        }
        ConditionOperator::IsDefined => {
            if arguments.is_empty() {
                return quote! { false };
            }
            let arg_code = emit_argument_as_value(&arguments[0], ctx, source_var);
            quote! { !#arg_code.is_null() }
        }
        ConditionOperator::IsNotEmpty => {
            if arguments.is_empty() {
                return quote! { false };
            }
            let arg_code = emit_argument_as_value(&arguments[0], ctx, source_var);
            quote! {
                {
                    let val = #arg_code;
                    match &val {
                        serde_json::Value::Array(a) => !a.is_empty(),
                        serde_json::Value::String(s) => !s.is_empty(),
                        serde_json::Value::Object(o) => !o.is_empty(),
                        serde_json::Value::Null => false,
                        _ => true,
                    }
                }
            }
        }
        ConditionOperator::IsEmpty => {
            if arguments.is_empty() {
                return quote! { true };
            }
            let arg_code = emit_argument_as_value(&arguments[0], ctx, source_var);
            quote! {
                {
                    let val = #arg_code;
                    match &val {
                        serde_json::Value::Array(a) => a.is_empty(),
                        serde_json::Value::String(s) => s.is_empty(),
                        serde_json::Value::Object(o) => o.is_empty(),
                        serde_json::Value::Null => true,
                        _ => false,
                    }
                }
            }
        }
    }
}

/// Emit code for a `ConditionArgument` that returns a `bool`.
pub fn emit_argument_as_bool(
    arg: &ConditionArgument,
    ctx: &mut EmitContext,
    source_var: &proc_macro2::Ident,
) -> TokenStream {
    match arg {
        ConditionArgument::Expression(expr) => emit_condition_expression(expr, ctx, source_var),
        ConditionArgument::Value(mapping_value) => {
            let value_code = emit_mapping_value(mapping_value, ctx, source_var);
            quote! {
                {
                    let val = #value_code;
                    is_truthy(&val)
                }
            }
        }
    }
}

/// Emit code for a `ConditionArgument` that returns a `Value`.
pub fn emit_argument_as_value(
    arg: &ConditionArgument,
    ctx: &mut EmitContext,
    source_var: &proc_macro2::Ident,
) -> TokenStream {
    match arg {
        ConditionArgument::Expression(expr) => {
            // Evaluate expression and wrap result in Value
            let bool_code = emit_condition_expression(expr, ctx, source_var);
            quote! { serde_json::Value::Bool(#bool_code) }
        }
        ConditionArgument::Value(mapping_value) => {
            emit_mapping_value(mapping_value, ctx, source_var)
        }
    }
}

// ============================================================================
// Operator Implementations
// ============================================================================

/// Emit comparison code (GT, GTE, LT, LTE).
fn emit_comparison(
    arguments: &[ConditionArgument],
    ctx: &mut EmitContext,
    source_var: &proc_macro2::Ident,
    op: TokenStream,
) -> TokenStream {
    if arguments.len() < 2 {
        return quote! { false };
    }

    let left_code = emit_argument_as_value(&arguments[0], ctx, source_var);
    let right_code = emit_argument_as_value(&arguments[1], ctx, source_var);

    quote! {
        {
            let left_val = #left_code;
            let right_val = #right_code;
            let left_num = to_number(&left_val);
            let right_num = to_number(&right_val);
            match (left_num, right_num) {
                (Some(l), Some(r)) => l #op r,
                _ => false,
            }
        }
    }
}

/// Emit equality/inequality code.
fn emit_equality(
    arguments: &[ConditionArgument],
    ctx: &mut EmitContext,
    source_var: &proc_macro2::Ident,
    negate: bool,
) -> TokenStream {
    if arguments.len() < 2 {
        return quote! { false };
    }

    let left_code = emit_argument_as_value(&arguments[0], ctx, source_var);
    let right_code = emit_argument_as_value(&arguments[1], ctx, source_var);

    let eq_check = quote! {
        {
            let left_val = #left_code;
            let right_val = #right_code;
            values_equal(&left_val, &right_val)
        }
    };

    if negate {
        quote! { !(#eq_check) }
    } else {
        eq_check
    }
}

/// Emit CONTAINS code (array contains value).
fn emit_contains(
    arguments: &[ConditionArgument],
    ctx: &mut EmitContext,
    source_var: &proc_macro2::Ident,
) -> TokenStream {
    if arguments.len() < 2 {
        return quote! { false };
    }

    let array_code = emit_argument_as_value(&arguments[0], ctx, source_var);
    let value_code = emit_argument_as_value(&arguments[1], ctx, source_var);

    quote! {
        {
            let arr_val = #array_code;
            let search_val = #value_code;
            if let Some(arr) = arr_val.as_array() {
                arr.iter().any(|item| values_equal(item, &search_val))
            } else {
                false
            }
        }
    }
}

/// Emit IN code (value in array).
fn emit_in(
    arguments: &[ConditionArgument],
    ctx: &mut EmitContext,
    source_var: &proc_macro2::Ident,
) -> TokenStream {
    if arguments.len() < 2 {
        return quote! { false };
    }

    let value_code = emit_argument_as_value(&arguments[0], ctx, source_var);
    let array_code = emit_argument_as_value(&arguments[1], ctx, source_var);

    quote! {
        {
            let search_val = #value_code;
            let arr_val = #array_code;
            if let Some(arr) = arr_val.as_array() {
                arr.iter().any(|item| values_equal(&search_val, item))
            } else {
                false
            }
        }
    }
}

/// Emit STARTS_WITH code (string starts with prefix).
fn emit_starts_with(
    arguments: &[ConditionArgument],
    ctx: &mut EmitContext,
    source_var: &proc_macro2::Ident,
) -> TokenStream {
    if arguments.len() < 2 {
        return quote! { false };
    }

    let string_code = emit_argument_as_value(&arguments[0], ctx, source_var);
    let prefix_code = emit_argument_as_value(&arguments[1], ctx, source_var);

    quote! {
        {
            let str_val = #string_code;
            let prefix_val = #prefix_code;
            match (str_val.as_str(), prefix_val.as_str()) {
                (Some(s), Some(p)) => s.starts_with(p),
                _ => false,
            }
        }
    }
}

/// Emit ENDS_WITH code (string ends with suffix).
fn emit_ends_with(
    arguments: &[ConditionArgument],
    ctx: &mut EmitContext,
    source_var: &proc_macro2::Ident,
) -> TokenStream {
    if arguments.len() < 2 {
        return quote! { false };
    }

    let string_code = emit_argument_as_value(&arguments[0], ctx, source_var);
    let suffix_code = emit_argument_as_value(&arguments[1], ctx, source_var);

    quote! {
        {
            let str_val = #string_code;
            let suffix_val = #suffix_code;
            match (str_val.as_str(), suffix_val.as_str()) {
                (Some(s), Some(suf)) => s.ends_with(suf),
                _ => false,
            }
        }
    }
}

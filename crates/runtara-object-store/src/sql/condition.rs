//! Condition building for SQL WHERE clauses
//!
//! Converts JSON condition structures to SQL WHERE clauses.

use crate::instance::Condition;
use crate::schema::Schema;
use crate::sql::sanitize::quote_identifier;
use crate::types::ColumnType;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConditionSubquery {
    pub schema: String,
    pub select: String,
    #[serde(default, rename = "connectionId")]
    pub connection_id: Option<String>,
    #[serde(default)]
    pub condition: Option<Condition>,
}

#[derive(Clone, Copy)]
struct ConditionBuildContext<'a> {
    subquery_schemas: Option<&'a HashMap<String, Schema>>,
    allow_subqueries: bool,
}

/// Map camelCase system field names to their snake_case SQL column equivalents.
pub(crate) fn field_to_sql(field: &str) -> &str {
    match field {
        "createdAt" => "created_at",
        "updatedAt" => "updated_at",
        _ => field,
    }
}

/// Resolve the SQL cast type for a field based on schema column definitions.
///
/// System fields are handled first (id → text, created_at/updated_at → timestamptz).
/// Then schema columns are looked up by name and mapped to their SQL cast type.
/// Falls back to "text" for unknown fields.
pub(crate) fn resolve_sql_cast(field: &str, schema: &Schema) -> &'static str {
    // System fields (already in SQL name form after field_to_sql)
    match field {
        "id" => return "text",
        "created_at" | "updated_at" => return "timestamptz",
        _ => {}
    }

    // Look up in schema columns
    if let Some(col) = schema.columns.iter().find(|c| c.name == field) {
        match &col.column_type {
            ColumnType::String | ColumnType::Enum { .. } => "text",
            ColumnType::Integer => "bigint",
            ColumnType::Decimal { .. } => "numeric",
            ColumnType::Boolean => "boolean",
            ColumnType::Timestamp => "timestamptz",
            ColumnType::Json => "text",
            // tsvector columns are only meaningful inside MATCH / TS_RANK;
            // the generic comparison arms reject this cast.
            ColumnType::Tsvector { .. } => "tsvector",
            // Vector columns are only meaningful inside the distance ExprFns
            // (COSINE_DISTANCE / L2_DISTANCE / INNER_PRODUCT). Any condition
            // operator that reaches `resolve_sql_cast` for a vector column is
            // semantically wrong and the SQL it emits will fail at execution.
            ColumnType::Vector { .. } => "vector",
        }
    } else {
        "text"
    }
}

/// Look up a column's `text_search_configuration` (e.g. `"english"`). Used by
/// MATCH / TS_RANK to render `plainto_tsquery('<lang>', $N)` against the
/// column's declared language. Returns `"english"` for unknown / non-tsvector
/// fields.
pub(crate) fn resolve_tsvector_language(field: &str, schema: &Schema) -> String {
    if let Some(col) = schema.columns.iter().find(|c| c.name == field)
        && let ColumnType::Tsvector { language, .. } = &col.column_type
    {
        return language.clone();
    }
    "english".to_string()
}

/// Build SQL WHERE clause from condition structure
///
/// Returns (clause, params) tuple where:
/// - `clause` is the SQL WHERE condition string with parameter placeholders ($1, $2, etc.)
/// - `params` is a vector of parameter values to bind
///
/// # Arguments
/// * `condition` - The condition structure to convert
/// * `param_offset` - Starting parameter number (mutated to track next available)
///
/// # Supported Operations
/// - Logical: AND, OR, NOT
/// - Comparison: EQ, NE, GT, LT, GTE, LTE
/// - String: CONTAINS (LIKE with wildcards)
/// - Array: IN, NOT_IN
/// - Nullability: IS_EMPTY, IS_NOT_EMPTY, IS_DEFINED
pub fn build_condition_clause(
    condition: &Condition,
    param_offset: &mut i32,
    schema: &Schema,
) -> Result<(String, Vec<serde_json::Value>), String> {
    build_condition_clause_at(
        condition,
        param_offset,
        schema,
        "condition",
        ConditionBuildContext {
            subquery_schemas: None,
            allow_subqueries: false,
        },
    )
}

pub fn build_condition_clause_with_subqueries(
    condition: &Condition,
    param_offset: &mut i32,
    schema: &Schema,
    subquery_schemas: &HashMap<String, Schema>,
) -> Result<(String, Vec<serde_json::Value>), String> {
    build_condition_clause_at(
        condition,
        param_offset,
        schema,
        "condition",
        ConditionBuildContext {
            subquery_schemas: Some(subquery_schemas),
            allow_subqueries: true,
        },
    )
}

pub fn collect_condition_subquery_schema_names(
    condition: &Condition,
) -> Result<HashSet<String>, String> {
    let mut schemas = HashSet::new();
    collect_condition_subquery_schema_names_at(condition, false, &mut schemas, "condition")?;
    Ok(schemas)
}

fn argument_path(condition_path: &str, index: usize) -> String {
    format!("{}.arguments[{}]", condition_path, index)
}

fn required_arguments<'a>(
    condition: &'a Condition,
    condition_path: &str,
    op: &str,
    expected_shape: &str,
) -> Result<&'a Vec<serde_json::Value>, String> {
    condition.arguments.as_ref().ok_or_else(|| {
        format!(
            "{}: {} operation requires arguments ({})",
            condition_path, op, expected_shape
        )
    })
}

fn validate_argument_count(
    args: &[serde_json::Value],
    condition_path: &str,
    op: &str,
    expected: usize,
    expected_shape: &str,
) -> Result<(), String> {
    if args.len() != expected {
        return Err(format!(
            "{}: {} operation requires exactly {} argument{} ({})",
            condition_path,
            op,
            expected,
            if expected == 1 { "" } else { "s" },
            expected_shape
        ));
    }
    Ok(())
}

fn validate_field_name<'a>(
    args: &'a [serde_json::Value],
    index: usize,
    condition_path: &str,
    description: &str,
) -> Result<&'a str, String> {
    let path = argument_path(condition_path, index);
    let raw_field = args[index].as_str().ok_or_else(|| {
        format!(
            "{}: {} argument must be a {} string",
            path,
            ordinal_argument(index),
            description
        )
    })?;

    if raw_field.is_empty() {
        return Err(format!("{}: field name string cannot be empty", path));
    }

    if !raw_field
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return Err(format!(
            "{}: field name string contains invalid characters; expected alphanumeric, underscore, or hyphen",
            path
        ));
    }

    Ok(raw_field)
}

fn ordinal_argument(index: usize) -> &'static str {
    match index {
        0 => "first",
        1 => "second",
        2 => "third",
        _ => "argument",
    }
}

fn deserialize_condition_arg(value: &serde_json::Value, path: &str) -> Result<Condition, String> {
    serde_json::from_value::<Condition>(value.clone()).map_err(|err| {
        format!(
            "{}: expected condition object with op and optional arguments; {}",
            path, err
        )
    })
}

fn parse_subquery_operand(
    value: &serde_json::Value,
    path: &str,
) -> Result<Option<ConditionSubquery>, String> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    let Some(subquery) = object.get("subquery") else {
        return Ok(None);
    };
    if object.len() != 1 {
        return Err(format!(
            "{}: subquery operand must contain only the 'subquery' key",
            path
        ));
    }
    let parsed = serde_json::from_value::<ConditionSubquery>(subquery.clone()).map_err(|err| {
        format!(
            "{}.subquery: expected {{schema, select, condition?}}; {}",
            path, err
        )
    })?;
    if parsed.schema.trim().is_empty() {
        return Err(format!("{}.subquery.schema: schema cannot be empty", path));
    }
    if parsed.select.trim().is_empty() {
        return Err(format!("{}.subquery.select: select cannot be empty", path));
    }
    Ok(Some(parsed))
}

fn collect_condition_subquery_schema_names_at(
    condition: &Condition,
    inside_subquery: bool,
    schemas: &mut HashSet<String>,
    condition_path: &str,
) -> Result<(), String> {
    let Some(arguments) = condition.arguments.as_ref() else {
        return Ok(());
    };

    for (index, argument) in arguments.iter().enumerate() {
        let argument_path = argument_path(condition_path, index);
        if let Some(subquery) = parse_subquery_operand(argument, &argument_path)? {
            if inside_subquery {
                return Err(format!(
                    "{}: nested subqueries are not supported",
                    argument_path
                ));
            }
            schemas.insert(subquery.schema.clone());
            if let Some(child) = subquery.condition.as_ref() {
                collect_condition_subquery_schema_names_at(child, true, schemas, &argument_path)?;
            }
            continue;
        }

        if let Ok(child) = serde_json::from_value::<Condition>(argument.clone()) {
            collect_condition_subquery_schema_names_at(
                &child,
                inside_subquery,
                schemas,
                &argument_path,
            )?;
        }
    }

    Ok(())
}

fn validate_field_name_from_str<'a>(
    raw_field: &'a str,
    path: &str,
    description: &str,
) -> Result<&'a str, String> {
    if raw_field.is_empty() {
        return Err(format!("{}: {} string cannot be empty", path, description));
    }

    if !raw_field
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return Err(format!(
            "{}: {} string contains invalid characters; expected alphanumeric, underscore, or hyphen",
            path, description
        ));
    }

    Ok(raw_field)
}

fn schema_has_field(schema: &Schema, field: &str) -> bool {
    matches!(field, "id" | "created_at" | "updated_at")
        || schema.columns.iter().any(|column| column.name == field)
}

fn build_subquery_condition_clause(
    outer_field: &str,
    subquery: &ConditionSubquery,
    param_offset: &mut i32,
    schema: &Schema,
    condition_path: &str,
    negate: bool,
    context: ConditionBuildContext<'_>,
) -> Result<(String, Vec<serde_json::Value>), String> {
    if !context.allow_subqueries {
        return Err(format!(
            "{}: subquery operands require a same-store condition context",
            argument_path(condition_path, 1)
        ));
    }

    let subquery_schemas = context.subquery_schemas.ok_or_else(|| {
        format!(
            "{}: subquery operands require resolved same-store schemas",
            argument_path(condition_path, 1)
        )
    })?;
    let subquery_schema = subquery_schemas.get(&subquery.schema).ok_or_else(|| {
        format!(
            "{}.subquery.schema: schema '{}' was not resolved for this condition",
            argument_path(condition_path, 1),
            subquery.schema
        )
    })?;

    let parent_cast = resolve_sql_cast(outer_field, schema);
    if !schema_has_field(schema, outer_field) {
        return Err(format!(
            "{}: unknown field '{}'",
            argument_path(condition_path, 0),
            outer_field
        ));
    }

    let select_field = field_to_sql(validate_field_name_from_str(
        &subquery.select,
        &format!("{}.subquery.select", argument_path(condition_path, 1)),
        "select field name",
    )?);
    if !schema_has_field(subquery_schema, select_field) {
        return Err(format!(
            "{}.subquery.select: unknown field '{}' on schema '{}'",
            argument_path(condition_path, 1),
            subquery.select,
            subquery.schema
        ));
    }
    let subquery_cast = resolve_sql_cast(select_field, subquery_schema);
    if parent_cast != subquery_cast {
        return Err(format!(
            "{}.subquery.select: field '{}' has SQL cast {}, but parent field '{}' has SQL cast {}",
            argument_path(condition_path, 1),
            subquery.select,
            subquery_cast,
            outer_field,
            parent_cast
        ));
    }

    let (subquery_where, params) = if let Some(condition) = subquery.condition.as_ref() {
        build_condition_clause_at(
            condition,
            param_offset,
            subquery_schema,
            &format!("{}.subquery.condition", argument_path(condition_path, 1)),
            ConditionBuildContext {
                subquery_schemas: context.subquery_schemas,
                allow_subqueries: false,
            },
        )?
    } else {
        ("TRUE".to_string(), Vec::new())
    };

    let subquery_sql = format!(
        "SELECT {}::{} FROM {} WHERE deleted = FALSE AND ({})",
        quote_identifier(select_field),
        subquery_cast,
        quote_identifier(&subquery_schema.table_name),
        subquery_where
    );
    let in_clause = format!(
        "{}::{} IN ({})",
        quote_identifier(outer_field),
        parent_cast,
        subquery_sql
    );
    let clause = if negate {
        format!("NOT ({})", in_clause)
    } else {
        in_clause
    };

    Ok((clause, params))
}

fn build_condition_clause_at(
    condition: &Condition,
    param_offset: &mut i32,
    schema: &Schema,
    condition_path: &str,
    context: ConditionBuildContext<'_>,
) -> Result<(String, Vec<serde_json::Value>), String> {
    let op = condition.op.to_uppercase();
    let args = condition.arguments.as_ref();

    let mut params = Vec::new();

    match op.as_str() {
        "AND" => {
            if let Some(args) = args {
                let mut clauses = Vec::new();
                for (i, arg) in args.iter().enumerate() {
                    let child_path = argument_path(condition_path, i);
                    let sub_condition = deserialize_condition_arg(arg, &child_path)?;
                    let (clause, mut sub_params) = build_condition_clause_at(
                        &sub_condition,
                        param_offset,
                        schema,
                        &child_path,
                        context,
                    )?;
                    clauses.push(format!("({})", clause));
                    params.append(&mut sub_params);
                }
                if clauses.is_empty() {
                    return Err(format!(
                        "{}: AND operation requires at least one condition argument",
                        condition_path
                    ));
                }
                Ok((clauses.join(" AND "), params))
            } else {
                Err(format!(
                    "{}: AND operation requires arguments (condition, ...)",
                    condition_path
                ))
            }
        }
        "OR" => {
            if let Some(args) = args {
                let mut clauses = Vec::new();
                for (i, arg) in args.iter().enumerate() {
                    let child_path = argument_path(condition_path, i);
                    let sub_condition = deserialize_condition_arg(arg, &child_path)?;
                    let (clause, mut sub_params) = build_condition_clause_at(
                        &sub_condition,
                        param_offset,
                        schema,
                        &child_path,
                        context,
                    )?;
                    clauses.push(format!("({})", clause));
                    params.append(&mut sub_params);
                }
                if clauses.is_empty() {
                    return Err(format!(
                        "{}: OR operation requires at least one condition argument",
                        condition_path
                    ));
                }
                Ok((clauses.join(" OR "), params))
            } else {
                Err(format!(
                    "{}: OR operation requires arguments (condition, ...)",
                    condition_path
                ))
            }
        }
        "NOT" => {
            if let Some(args) = args {
                if args.len() != 1 {
                    return Err(format!(
                        "{}: NOT operation requires exactly one argument (condition)",
                        condition_path
                    ));
                }
                let child_path = argument_path(condition_path, 0);
                let sub_condition = deserialize_condition_arg(&args[0], &child_path)?;
                let (clause, sub_params) = build_condition_clause_at(
                    &sub_condition,
                    param_offset,
                    schema,
                    &child_path,
                    context,
                )?;
                params.extend(sub_params);
                Ok((format!("NOT ({})", clause), params))
            } else {
                Err(format!(
                    "{}: NOT operation requires arguments (condition)",
                    condition_path
                ))
            }
        }
        "EQ" | "NE" | "GT" | "LT" | "GTE" | "LTE" => {
            if let Some(args) = args {
                if args.len() != 2 {
                    return Err(format!(
                        "{}: {} operation requires exactly 2 arguments (field, value)",
                        condition_path, op
                    ));
                }
                let raw_field = validate_field_name(args, 0, condition_path, "field name")?;
                let value = &args[1];
                if parse_subquery_operand(value, &argument_path(condition_path, 1))?.is_some() {
                    return Err(format!(
                        "{}: subquery operands are only supported as the second argument of IN or NOT_IN",
                        argument_path(condition_path, 1)
                    ));
                }

                // Map camelCase system fields to snake_case SQL columns
                let field = field_to_sql(raw_field);

                let operator = match op.as_str() {
                    "EQ" => "=",
                    "NE" => "!=",
                    "GT" => ">",
                    "LT" => "<",
                    "GTE" => ">=",
                    "LTE" => "<=",
                    _ => unreachable!(),
                };

                // Handle NULL values specially - use IS NULL / IS NOT NULL
                if value.is_null() {
                    let null_operator = match op.as_str() {
                        "EQ" => "IS NULL",
                        "NE" => "IS NOT NULL",
                        _ => {
                            return Err(format!(
                                "{}: {} operation with NULL value is not supported",
                                argument_path(condition_path, 1),
                                op
                            ));
                        }
                    };
                    return Ok((format!("\"{}\" {}", field, null_operator), params));
                }

                // Convert value to string for comparison
                let value_str = match value {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    serde_json::Value::Null => "null".to_string(),
                    _ => value.to_string(),
                };

                params.push(serde_json::Value::String(value_str));

                let cast = resolve_sql_cast(field, schema);
                let clause = format!(
                    "\"{}\"::{} {} ${}::{}",
                    field, cast, operator, param_offset, cast
                );
                *param_offset += 1;

                Ok((clause, params))
            } else {
                Err(format!(
                    "{}: {} operation requires arguments (field, value)",
                    condition_path, op
                ))
            }
        }
        "CONTAINS" => {
            if let Some(args) = args {
                if args.len() != 2 {
                    return Err(format!(
                        "{}: CONTAINS operation requires exactly 2 arguments (field, string)",
                        condition_path
                    ));
                }
                let raw_field = validate_field_name(args, 0, condition_path, "field name")?;
                let value = args[1].as_str().ok_or_else(|| {
                    format!(
                        "{}: second argument must be a string value",
                        argument_path(condition_path, 1)
                    )
                })?;

                let field = field_to_sql(raw_field);

                params.push(serde_json::Value::String(format!("%{}%", value)));

                let clause = format!("\"{}\"::text LIKE ${}::text", field, param_offset);
                *param_offset += 1;

                Ok((clause, params))
            } else {
                Err(format!(
                    "{}: CONTAINS operation requires arguments (field, string)",
                    condition_path
                ))
            }
        }
        "IN" => {
            if let Some(args) = args {
                if args.len() != 2 {
                    return Err(format!(
                        "{}: IN operation requires exactly 2 arguments (field, array)",
                        condition_path
                    ));
                }
                let raw_field = validate_field_name(args, 0, condition_path, "field name")?;
                let field = field_to_sql(raw_field);

                if let Some(subquery) =
                    parse_subquery_operand(&args[1], &argument_path(condition_path, 1))?
                {
                    return build_subquery_condition_clause(
                        field,
                        &subquery,
                        param_offset,
                        schema,
                        condition_path,
                        false,
                        context,
                    );
                }

                let values = args[1].as_array().ok_or_else(|| {
                    format!(
                        "{}: second argument must be an array of values or a subquery operand",
                        argument_path(condition_path, 1)
                    )
                })?;

                params.push(serde_json::Value::Array(values.clone()));

                let clause = format!(
                    "\"{}\"::text = ANY(SELECT jsonb_array_elements_text(${}::jsonb))",
                    field, param_offset
                );
                *param_offset += 1;

                Ok((clause, params))
            } else {
                Err(format!(
                    "{}: IN operation requires arguments (field, array)",
                    condition_path
                ))
            }
        }
        "NOT_IN" => {
            if let Some(args) = args {
                if args.len() != 2 {
                    return Err(format!(
                        "{}: NOT_IN operation requires exactly 2 arguments (field, array)",
                        condition_path
                    ));
                }
                let raw_field = validate_field_name(args, 0, condition_path, "field name")?;
                let field = field_to_sql(raw_field);

                if let Some(subquery) =
                    parse_subquery_operand(&args[1], &argument_path(condition_path, 1))?
                {
                    return build_subquery_condition_clause(
                        field,
                        &subquery,
                        param_offset,
                        schema,
                        condition_path,
                        true,
                        context,
                    );
                }

                let values = args[1].as_array().ok_or_else(|| {
                    format!(
                        "{}: second argument must be an array of values or a subquery operand",
                        argument_path(condition_path, 1)
                    )
                })?;

                params.push(serde_json::Value::Array(values.clone()));

                let clause = format!(
                    "NOT (\"{}\"::text = ANY(SELECT jsonb_array_elements_text(${}::jsonb)))",
                    field, param_offset
                );
                *param_offset += 1;

                Ok((clause, params))
            } else {
                Err(format!(
                    "{}: NOT_IN operation requires arguments (field, array)",
                    condition_path
                ))
            }
        }
        "IS_EMPTY" => {
            if let Some(args) = args {
                if args.len() != 1 {
                    return Err(format!(
                        "{}: IS_EMPTY operation requires exactly 1 argument (field)",
                        condition_path
                    ));
                }
                let raw_field = validate_field_name(args, 0, condition_path, "field name")?;

                let field = field_to_sql(raw_field);

                let clause = format!("(\"{}\" IS NULL OR \"{}\"::text = '')", field, field);

                Ok((clause, params))
            } else {
                Err(format!(
                    "{}: IS_EMPTY operation requires arguments (field)",
                    condition_path
                ))
            }
        }
        "IS_NOT_EMPTY" => {
            if let Some(args) = args {
                if args.len() != 1 {
                    return Err(format!(
                        "{}: IS_NOT_EMPTY operation requires exactly 1 argument (field)",
                        condition_path
                    ));
                }
                let raw_field = validate_field_name(args, 0, condition_path, "field name")?;

                let field = field_to_sql(raw_field);

                let clause = format!("(\"{}\" IS NOT NULL AND \"{}\"::text != '')", field, field);

                Ok((clause, params))
            } else {
                Err(format!(
                    "{}: IS_NOT_EMPTY operation requires arguments (field)",
                    condition_path
                ))
            }
        }
        "IS_DEFINED" => {
            if let Some(args) = args {
                if args.len() != 1 {
                    return Err(format!(
                        "{}: IS_DEFINED operation requires exactly 1 argument (field)",
                        condition_path
                    ));
                }
                let raw_field = validate_field_name(args, 0, condition_path, "field name")?;

                let field = field_to_sql(raw_field);

                let clause = format!("\"{}\" IS NOT NULL", field);

                Ok((clause, params))
            } else {
                Err(format!(
                    "{}: IS_DEFINED operation requires arguments (field)",
                    condition_path
                ))
            }
        }
        "SIMILARITY_GTE" => {
            let args = required_arguments(
                condition,
                condition_path,
                "SIMILARITY_GTE",
                "field, value, threshold",
            )?;
            validate_argument_count(
                args,
                condition_path,
                "SIMILARITY_GTE",
                3,
                "field, value, threshold",
            )?;
            let raw_field = validate_field_name(args, 0, condition_path, "field name")?;
            let field = field_to_sql(raw_field);

            // Reject SIMILARITY_GTE on non-text columns (covers schema columns
            // *and* system fields like `created_at`/`updated_at`).
            let cast = resolve_sql_cast(field, schema);
            if cast != "text" {
                return Err(format!(
                    "{}: SIMILARITY_GTE requires a string/enum column; '{}' has SQL cast {}",
                    argument_path(condition_path, 0),
                    field,
                    cast
                ));
            }

            let value = args[1].as_str().ok_or_else(|| {
                format!(
                    "{}: second argument must be a string value",
                    argument_path(condition_path, 1)
                )
            })?;
            let threshold = args[2].as_f64().ok_or_else(|| {
                format!(
                    "{}: third argument must be a numeric threshold",
                    argument_path(condition_path, 2)
                )
            })?;
            if !(0.0..=1.0).contains(&threshold) || threshold.is_nan() {
                return Err(format!(
                    "{}: SIMILARITY_GTE threshold must be between 0.0 and 1.0, got {}",
                    argument_path(condition_path, 2),
                    threshold
                ));
            }

            params.push(serde_json::Value::String(value.to_string()));
            let clause = format!(
                "similarity(\"{}\"::text, ${}::text) >= {:.6}",
                field, param_offset, threshold
            );
            *param_offset += 1;
            Ok((clause, params))
        }
        op_name @ ("COSINE_DISTANCE_LTE" | "L2_DISTANCE_LTE") => {
            let pg_op = if op_name == "COSINE_DISTANCE_LTE" {
                "<=>"
            } else {
                "<->"
            };
            let args = required_arguments(
                condition,
                condition_path,
                op_name,
                "vector_field, vector_literal, threshold",
            )?;
            validate_argument_count(
                args,
                condition_path,
                op_name,
                3,
                "vector_field, vector_literal, threshold",
            )?;
            let raw_field = validate_field_name(args, 0, condition_path, "vector field name")?;
            let field = field_to_sql(raw_field);

            // First arg must reference a vector column. Pull the dimension so
            // we can validate the literal against it.
            let dimension = match schema.columns.iter().find(|c| c.name == field) {
                Some(col) => match &col.column_type {
                    ColumnType::Vector { dimension, .. } => *dimension,
                    other => {
                        return Err(format!(
                            "{}: {} requires a vector column; '{}' has type {:?}",
                            argument_path(condition_path, 0),
                            op_name,
                            field,
                            other
                        ));
                    }
                },
                None => {
                    return Err(format!(
                        "{}: {} unknown column '{}'",
                        argument_path(condition_path, 0),
                        op_name,
                        field
                    ));
                }
            };

            let arr = args[1].as_array().ok_or_else(|| {
                format!(
                    "{}: second argument must be a JSON array of numbers",
                    argument_path(condition_path, 1)
                )
            })?;
            if arr.len() as u32 != dimension {
                return Err(format!(
                    "{}: {} vector literal dimension {} does not match column '{}' dimension {}",
                    argument_path(condition_path, 1),
                    op_name,
                    arr.len(),
                    field,
                    dimension
                ));
            }
            let mut parts: Vec<String> = Vec::with_capacity(arr.len());
            for (i, v) in arr.iter().enumerate() {
                let f = v.as_f64().ok_or_else(|| {
                    format!(
                        "{}[{}]: vector element must be a number",
                        argument_path(condition_path, 1),
                        i
                    )
                })?;
                if !f.is_finite() {
                    return Err(format!(
                        "{}[{}]: vector element must be finite, got {}",
                        argument_path(condition_path, 1),
                        i,
                        f
                    ));
                }
                parts.push(
                    serde_json::Number::from_f64(f)
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| f.to_string()),
                );
            }
            let lit = format!("[{}]", parts.join(","));

            let threshold = args[2].as_f64().ok_or_else(|| {
                format!(
                    "{}: third argument must be a numeric threshold",
                    argument_path(condition_path, 2)
                )
            })?;
            if !threshold.is_finite() || threshold < 0.0 {
                return Err(format!(
                    "{}: {} threshold must be a finite non-negative number, got {}",
                    argument_path(condition_path, 2),
                    op_name,
                    threshold
                ));
            }

            params.push(serde_json::Value::String(lit));
            let clause = format!(
                "(\"{}\" {} ${}::vector) <= {:.6}",
                field, pg_op, param_offset, threshold
            );
            *param_offset += 1;
            Ok((clause, params))
        }
        "MATCH" => {
            let args = required_arguments(condition, condition_path, "MATCH", "field, query")?;
            validate_argument_count(args, condition_path, "MATCH", 2, "field, query")?;
            let raw_field = validate_field_name(args, 0, condition_path, "field name")?;
            let field = field_to_sql(raw_field);

            // Reject MATCH on anything that isn't a tsvector column.
            let cast = resolve_sql_cast(field, schema);
            if cast != "tsvector" {
                return Err(format!(
                    "{}: MATCH requires a tsvector column; '{}' has SQL cast {}",
                    argument_path(condition_path, 0),
                    field,
                    cast
                ));
            }

            let query = args[1].as_str().ok_or_else(|| {
                format!(
                    "{}: second argument must be a query string",
                    argument_path(condition_path, 1)
                )
            })?;

            let language = resolve_tsvector_language(field, schema);
            let lang_lit = language.replace('\'', "''");
            params.push(serde_json::Value::String(query.to_string()));
            let clause = format!(
                "\"{}\" @@ plainto_tsquery('{}', ${})",
                field, lang_lit, param_offset
            );
            *param_offset += 1;
            Ok((clause, params))
        }
        _ => Err(format!("{}: Unsupported operation: {}", condition_path, op)),
    }
}

/// Build ORDER BY clause from sort parameters
///
/// # Arguments
/// * `sort_by` - Optional list of field names to sort by
/// * `sort_order` - Optional list of sort orders ("asc" or "desc")
/// * `schema` - The schema to validate field names against
///
/// # Returns
/// SQL ORDER BY clause string (without "ORDER BY" prefix)
pub fn build_order_by_clause(
    sort_by: &Option<Vec<String>>,
    sort_order: &Option<Vec<String>>,
    schema: &Schema,
) -> Result<String, String> {
    let sort_fields = match sort_by {
        Some(fields) if !fields.is_empty() => fields,
        _ => return Ok("created_at ASC".to_string()), // Default
    };

    let orders = sort_order.as_ref();
    let mut order_parts = Vec::new();

    // System fields that are always available
    let system_fields = ["id", "createdAt", "updatedAt", "created_at", "updated_at"];

    for (i, field) in sort_fields.iter().enumerate() {
        // Validate field exists
        let sql_field = field_to_sql(field);
        let is_system =
            system_fields.contains(&field.as_str()) || system_fields.contains(&sql_field);
        let is_schema_column = schema.columns.iter().any(|c| c.name == *field);

        if !is_system && !is_schema_column {
            return Err(format!(
                "Invalid sort field: '{}'. Must be a system field (id, createdAt, updatedAt) or a schema column.",
                field
            ));
        }

        // Get order (default: ASC)
        let order = orders
            .and_then(|o| o.get(i))
            .map(|s| s.to_uppercase())
            .unwrap_or_else(|| "ASC".to_string());

        if order != "ASC" && order != "DESC" {
            return Err(format!(
                "Invalid sort order: '{}'. Must be 'asc' or 'desc'.",
                order
            ));
        }

        order_parts.push(format!("{} {}", quote_identifier(sql_field), order));
    }

    Ok(order_parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ColumnDefinition;

    fn make_test_schema() -> Schema {
        Schema {
            id: "test-id".to_string(),
            name: "test_schema".to_string(),
            description: None,
            table_name: "test_table".to_string(),
            columns: vec![
                ColumnDefinition::new("name", crate::types::ColumnType::String),
                ColumnDefinition::new("price", crate::types::ColumnType::decimal(10, 2)),
                ColumnDefinition::new("quantity", crate::types::ColumnType::Integer),
                ColumnDefinition::new("active", crate::types::ColumnType::Boolean),
            ],
            indexes: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-01T00:00:00Z".to_string(),
        }
    }

    // ==================== Comparison Operations ====================

    #[test]
    fn test_eq_condition() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "EQ".to_string(),
            arguments: Some(vec![serde_json::json!("name"), serde_json::json!("test")]),
        };

        let mut offset = 1;
        let (clause, params) = build_condition_clause(&condition, &mut offset, &schema).unwrap();

        assert_eq!(clause, "\"name\"::text = $1::text");
        assert_eq!(params.len(), 1);
        assert_eq!(params[0], serde_json::json!("test"));
        assert_eq!(offset, 2);
    }

    #[test]
    fn test_eq_condition_with_number() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "EQ".to_string(),
            arguments: Some(vec![serde_json::json!("age"), serde_json::json!(25)]),
        };

        let mut offset = 1;
        let (clause, params) = build_condition_clause(&condition, &mut offset, &schema).unwrap();

        assert_eq!(clause, "\"age\"::text = $1::text"); // "age" not in schema, falls back to text
        assert_eq!(params[0], serde_json::json!("25")); // Numbers are converted to strings
    }

    #[test]
    fn test_eq_condition_with_boolean() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "EQ".to_string(),
            arguments: Some(vec![serde_json::json!("active"), serde_json::json!(true)]),
        };

        let mut offset = 1;
        let (clause, params) = build_condition_clause(&condition, &mut offset, &schema).unwrap();

        assert_eq!(clause, "\"active\"::boolean = $1::boolean");
        assert_eq!(params[0], serde_json::json!("true"));
    }

    #[test]
    fn test_eq_condition_lowercase_op() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "eq".to_string(), // lowercase
            arguments: Some(vec![serde_json::json!("name"), serde_json::json!("test")]),
        };

        let mut offset = 1;
        let (clause, _) = build_condition_clause(&condition, &mut offset, &schema).unwrap();

        assert!(clause.contains("=")); // Should work with lowercase
    }

    #[test]
    fn test_ne_condition() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "NE".to_string(),
            arguments: Some(vec![
                serde_json::json!("status"),
                serde_json::json!("deleted"),
            ]),
        };

        let mut offset = 1;
        let (clause, params) = build_condition_clause(&condition, &mut offset, &schema).unwrap();

        assert_eq!(clause, "\"status\"::text != $1::text");
        assert_eq!(params[0], serde_json::json!("deleted"));
    }

    #[test]
    fn test_gt_condition() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "GT".to_string(),
            arguments: Some(vec![serde_json::json!("price"), serde_json::json!(100)]),
        };

        let mut offset = 1;
        let (clause, _) = build_condition_clause(&condition, &mut offset, &schema).unwrap();

        assert_eq!(clause, "\"price\"::numeric > $1::numeric");
    }

    #[test]
    fn test_lt_condition() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "LT".to_string(),
            arguments: Some(vec![serde_json::json!("quantity"), serde_json::json!(10)]),
        };

        let mut offset = 1;
        let (clause, _) = build_condition_clause(&condition, &mut offset, &schema).unwrap();

        assert_eq!(clause, "\"quantity\"::bigint < $1::bigint");
    }

    #[test]
    fn test_gte_condition() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "GTE".to_string(),
            arguments: Some(vec![serde_json::json!("score"), serde_json::json!(90)]),
        };

        let mut offset = 1;
        let (clause, _) = build_condition_clause(&condition, &mut offset, &schema).unwrap();

        assert_eq!(clause, "\"score\"::text >= $1::text"); // "score" not in schema
    }

    #[test]
    fn test_lte_condition() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "LTE".to_string(),
            arguments: Some(vec![serde_json::json!("rating"), serde_json::json!(5)]),
        };

        let mut offset = 1;
        let (clause, _) = build_condition_clause(&condition, &mut offset, &schema).unwrap();

        assert_eq!(clause, "\"rating\"::text <= $1::text"); // "rating" not in schema
    }

    // ==================== Logical Operations ====================

    #[test]
    fn test_and_condition() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "AND".to_string(),
            arguments: Some(vec![
                serde_json::json!({"op": "EQ", "arguments": ["field1", "value1"]}),
                serde_json::json!({"op": "EQ", "arguments": ["field2", "value2"]}),
            ]),
        };

        let mut offset = 1;
        let (clause, params) = build_condition_clause(&condition, &mut offset, &schema).unwrap();

        assert!(clause.contains(" AND "));
        assert!(clause.contains("(\"field1\"::text = $1::text)"));
        assert!(clause.contains("(\"field2\"::text = $2::text)"));
        assert_eq!(params.len(), 2);
        assert_eq!(offset, 3);
    }

    #[test]
    fn test_and_with_three_conditions() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "AND".to_string(),
            arguments: Some(vec![
                serde_json::json!({"op": "EQ", "arguments": ["a", "1"]}),
                serde_json::json!({"op": "EQ", "arguments": ["b", "2"]}),
                serde_json::json!({"op": "EQ", "arguments": ["c", "3"]}),
            ]),
        };

        let mut offset = 1;
        let (clause, params) = build_condition_clause(&condition, &mut offset, &schema).unwrap();

        // Count AND occurrences
        let and_count = clause.matches(" AND ").count();
        assert_eq!(and_count, 2);
        assert_eq!(params.len(), 3);
    }

    #[test]
    fn test_or_condition() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "OR".to_string(),
            arguments: Some(vec![
                serde_json::json!({"op": "EQ", "arguments": ["status", "active"]}),
                serde_json::json!({"op": "EQ", "arguments": ["status", "pending"]}),
            ]),
        };

        let mut offset = 1;
        let (clause, params) = build_condition_clause(&condition, &mut offset, &schema).unwrap();

        assert!(clause.contains(" OR "));
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn test_not_condition() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "NOT".to_string(),
            arguments: Some(vec![
                serde_json::json!({"op": "EQ", "arguments": ["deleted", true]}),
            ]),
        };

        let mut offset = 1;
        let (clause, _) = build_condition_clause(&condition, &mut offset, &schema).unwrap();

        assert!(clause.starts_with("NOT ("));
        assert!(clause.ends_with(")"));
    }

    #[test]
    fn test_nested_and_or_conditions() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "AND".to_string(),
            arguments: Some(vec![
                serde_json::json!({"op": "EQ", "arguments": ["type", "product"]}),
                serde_json::json!({
                    "op": "OR",
                    "arguments": [
                        {"op": "EQ", "arguments": ["status", "active"]},
                        {"op": "EQ", "arguments": ["status", "pending"]}
                    ]
                }),
            ]),
        };

        let mut offset = 1;
        let (clause, params) = build_condition_clause(&condition, &mut offset, &schema).unwrap();

        assert!(clause.contains(" AND "));
        assert!(clause.contains(" OR "));
        assert_eq!(params.len(), 3);
    }

    #[test]
    fn test_nested_and_reports_invalid_first_arg_path() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "AND".to_string(),
            arguments: Some(vec![serde_json::json!({
                "op": "EQ",
                "arguments": [123, "value"]
            })]),
        };

        let mut offset = 1;
        let err = build_condition_clause(&condition, &mut offset, &schema).unwrap_err();

        assert!(
            err.contains("condition.arguments[0].arguments[0]"),
            "{}",
            err
        );
        assert!(
            err.contains("first argument must be a field name string"),
            "{}",
            err
        );
    }

    #[test]
    fn test_nested_and_reports_invalid_in_second_arg_path() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "AND".to_string(),
            arguments: Some(vec![serde_json::json!({
                "op": "IN",
                "arguments": ["status", "not_an_array"]
            })]),
        };

        let mut offset = 1;
        let err = build_condition_clause(&condition, &mut offset, &schema).unwrap_err();

        assert!(
            err.contains("condition.arguments[0].arguments[1]"),
            "{}",
            err
        );
        assert!(
            err.contains("second argument must be an array of values"),
            "{}",
            err
        );
    }

    // ==================== String Operations ====================

    #[test]
    fn test_contains_condition() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "CONTAINS".to_string(),
            arguments: Some(vec![serde_json::json!("name"), serde_json::json!("test")]),
        };

        let mut offset = 1;
        let (clause, params) = build_condition_clause(&condition, &mut offset, &schema).unwrap();

        assert_eq!(clause, "\"name\"::text LIKE $1::text");
        assert_eq!(params[0], serde_json::json!("%test%"));
    }

    // ==================== Array Operations ====================

    #[test]
    fn test_in_condition() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "IN".to_string(),
            arguments: Some(vec![
                serde_json::json!("status"),
                serde_json::json!(["active", "pending", "draft"]),
            ]),
        };

        let mut offset = 1;
        let (clause, params) = build_condition_clause(&condition, &mut offset, &schema).unwrap();

        assert!(clause.contains("ANY"));
        assert!(clause.contains("jsonb_array_elements_text"));
        assert_eq!(params[0], serde_json::json!(["active", "pending", "draft"]));
    }

    #[test]
    fn test_not_in_condition() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "NOT_IN".to_string(),
            arguments: Some(vec![
                serde_json::json!("status"),
                serde_json::json!(["deleted", "archived"]),
            ]),
        };

        let mut offset = 1;
        let (clause, params) = build_condition_clause(&condition, &mut offset, &schema).unwrap();

        assert!(clause.starts_with("NOT"));
        assert!(clause.contains("ANY"));
        assert_eq!(params[0], serde_json::json!(["deleted", "archived"]));
    }

    #[test]
    fn test_in_condition_with_subquery() {
        let schema = make_test_schema();
        let product_schema = Schema {
            id: "product-id".to_string(),
            name: "products".to_string(),
            description: None,
            table_name: "product_table".to_string(),
            columns: vec![
                ColumnDefinition::new("name", crate::types::ColumnType::String),
                ColumnDefinition::new("category", crate::types::ColumnType::String),
            ],
            indexes: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-01T00:00:00Z".to_string(),
        };
        let subquery_schemas = HashMap::from([("products".to_string(), product_schema)]);
        let condition = Condition {
            op: "IN".to_string(),
            arguments: Some(vec![
                serde_json::json!("name"),
                serde_json::json!({
                    "subquery": {
                        "schema": "products",
                        "select": "name",
                        "condition": {
                            "op": "EQ",
                            "arguments": ["category", "networking"]
                        }
                    }
                }),
            ]),
        };

        let mut offset = 1;
        let (clause, params) = build_condition_clause_with_subqueries(
            &condition,
            &mut offset,
            &schema,
            &subquery_schemas,
        )
        .unwrap();

        assert_eq!(
            clause,
            "\"name\"::text IN (SELECT \"name\"::text FROM \"product_table\" WHERE deleted = FALSE AND (\"category\"::text = $1::text))"
        );
        assert_eq!(params, vec![serde_json::json!("networking")]);
        assert_eq!(offset, 2);
    }

    #[test]
    fn test_collect_condition_subquery_schema_names_rejects_nested_subquery() {
        let condition = Condition {
            op: "IN".to_string(),
            arguments: Some(vec![
                serde_json::json!("name"),
                serde_json::json!({
                    "subquery": {
                        "schema": "products",
                        "select": "name",
                        "condition": {
                            "op": "IN",
                            "arguments": [
                                "category",
                                {
                                    "subquery": {
                                        "schema": "categories",
                                        "select": "id"
                                    }
                                }
                            ]
                        }
                    }
                }),
            ]),
        };

        let err = collect_condition_subquery_schema_names(&condition).unwrap_err();
        assert!(err.contains("nested subqueries are not supported"));
    }

    #[test]
    fn test_eq_condition_rejects_subquery_operand() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "EQ".to_string(),
            arguments: Some(vec![
                serde_json::json!("name"),
                serde_json::json!({
                    "subquery": {
                        "schema": "products",
                        "select": "name"
                    }
                }),
            ]),
        };

        let mut offset = 1;
        let err = build_condition_clause(&condition, &mut offset, &schema).unwrap_err();
        assert!(err.contains("only supported as the second argument of IN or NOT_IN"));
    }

    // ==================== Nullability Operations ====================

    #[test]
    fn test_is_empty_condition() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "IS_EMPTY".to_string(),
            arguments: Some(vec![serde_json::json!("description")]),
        };

        let mut offset = 1;
        let (clause, params) = build_condition_clause(&condition, &mut offset, &schema).unwrap();

        assert_eq!(
            clause,
            "(\"description\" IS NULL OR \"description\"::text = '')"
        );
        assert!(params.is_empty()); // No params for IS_EMPTY
        assert_eq!(offset, 1); // Offset unchanged
    }

    #[test]
    fn test_is_not_empty_condition() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "IS_NOT_EMPTY".to_string(),
            arguments: Some(vec![serde_json::json!("email")]),
        };

        let mut offset = 1;
        let (clause, params) = build_condition_clause(&condition, &mut offset, &schema).unwrap();

        assert_eq!(clause, "(\"email\" IS NOT NULL AND \"email\"::text != '')");
        assert!(params.is_empty());
    }

    #[test]
    fn test_is_defined_condition() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "IS_DEFINED".to_string(),
            arguments: Some(vec![serde_json::json!("optional_field")]),
        };

        let mut offset = 1;
        let (clause, params) = build_condition_clause(&condition, &mut offset, &schema).unwrap();

        assert_eq!(clause, "\"optional_field\" IS NOT NULL");
        assert!(params.is_empty());
    }

    // ==================== Parameter Offset Tracking ====================

    #[test]
    fn test_param_offset_tracking() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "AND".to_string(),
            arguments: Some(vec![
                serde_json::json!({"op": "EQ", "arguments": ["a", "1"]}),
                serde_json::json!({"op": "EQ", "arguments": ["b", "2"]}),
                serde_json::json!({"op": "EQ", "arguments": ["c", "3"]}),
            ]),
        };

        let mut offset = 5; // Start at 5
        let (clause, params) = build_condition_clause(&condition, &mut offset, &schema).unwrap();

        assert!(clause.contains("$5"));
        assert!(clause.contains("$6"));
        assert!(clause.contains("$7"));
        assert_eq!(params.len(), 3);
        assert_eq!(offset, 8);
    }

    // ==================== Error Cases ====================

    #[test]
    fn test_unsupported_operation() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "INVALID_OP".to_string(),
            arguments: Some(vec![serde_json::json!("field")]),
        };

        let mut offset = 1;
        let result = build_condition_clause(&condition, &mut offset, &schema);

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unsupported operation"));
    }

    #[test]
    fn test_and_no_arguments() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "AND".to_string(),
            arguments: None,
        };

        let mut offset = 1;
        let result = build_condition_clause(&condition, &mut offset, &schema);

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("requires arguments"));
    }

    #[test]
    fn test_eq_wrong_argument_count() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "EQ".to_string(),
            arguments: Some(vec![serde_json::json!("field_only")]),
        };

        let mut offset = 1;
        let result = build_condition_clause(&condition, &mut offset, &schema);

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("requires exactly 2 arguments"));
    }

    #[test]
    fn test_not_wrong_argument_count() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "NOT".to_string(),
            arguments: Some(vec![
                serde_json::json!({"op": "EQ", "arguments": ["a", "1"]}),
                serde_json::json!({"op": "EQ", "arguments": ["b", "2"]}),
            ]),
        };

        let mut offset = 1;
        let result = build_condition_clause(&condition, &mut offset, &schema);

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("requires exactly one argument")
        );
    }

    #[test]
    fn test_in_second_arg_not_array() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "IN".to_string(),
            arguments: Some(vec![
                serde_json::json!("status"),
                serde_json::json!("not_an_array"),
            ]),
        };

        let mut offset = 1;
        let result = build_condition_clause(&condition, &mut offset, &schema);

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must be an array"));
    }

    #[test]
    fn test_contains_second_arg_not_string() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "CONTAINS".to_string(),
            arguments: Some(vec![serde_json::json!("field"), serde_json::json!(123)]),
        };

        let mut offset = 1;
        let result = build_condition_clause(&condition, &mut offset, &schema);

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must be a string"));
    }

    #[test]
    fn test_invalid_field_name_special_chars() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "EQ".to_string(),
            arguments: Some(vec![
                serde_json::json!("field; DROP TABLE"),
                serde_json::json!("value"),
            ]),
        };

        let mut offset = 1;
        let result = build_condition_clause(&condition, &mut offset, &schema);

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid characters"));
    }

    #[test]
    fn test_field_name_with_hyphen_is_valid() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "EQ".to_string(),
            arguments: Some(vec![
                serde_json::json!("my-field"),
                serde_json::json!("value"),
            ]),
        };

        let mut offset = 1;
        let result = build_condition_clause(&condition, &mut offset, &schema);

        assert!(result.is_ok());
    }

    #[test]
    fn test_field_name_with_underscore_is_valid() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "EQ".to_string(),
            arguments: Some(vec![
                serde_json::json!("my_field"),
                serde_json::json!("value"),
            ]),
        };

        let mut offset = 1;
        let result = build_condition_clause(&condition, &mut offset, &schema);

        assert!(result.is_ok());
    }

    // ==================== Schema-Aware Type Casting ====================

    #[test]
    fn test_eq_integer_column_uses_bigint_cast() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "EQ".to_string(),
            arguments: Some(vec![serde_json::json!("quantity"), serde_json::json!(42)]),
        };

        let mut offset = 1;
        let (clause, _) = build_condition_clause(&condition, &mut offset, &schema).unwrap();

        assert_eq!(clause, "\"quantity\"::bigint = $1::bigint");
    }

    #[test]
    fn test_gt_decimal_column_uses_numeric_cast() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "GT".to_string(),
            arguments: Some(vec![serde_json::json!("price"), serde_json::json!(99.99)]),
        };

        let mut offset = 1;
        let (clause, _) = build_condition_clause(&condition, &mut offset, &schema).unwrap();

        assert_eq!(clause, "\"price\"::numeric > $1::numeric");
    }

    #[test]
    fn test_eq_boolean_column_uses_boolean_cast() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "EQ".to_string(),
            arguments: Some(vec![serde_json::json!("active"), serde_json::json!(true)]),
        };

        let mut offset = 1;
        let (clause, _) = build_condition_clause(&condition, &mut offset, &schema).unwrap();

        assert_eq!(clause, "\"active\"::boolean = $1::boolean");
    }

    #[test]
    fn test_eq_unknown_column_falls_back_to_text() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "EQ".to_string(),
            arguments: Some(vec![
                serde_json::json!("unknown_field"),
                serde_json::json!("value"),
            ]),
        };

        let mut offset = 1;
        let (clause, _) = build_condition_clause(&condition, &mut offset, &schema).unwrap();

        assert_eq!(clause, "\"unknown_field\"::text = $1::text");
    }

    #[test]
    fn test_system_field_created_at_uses_timestamptz() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "GT".to_string(),
            arguments: Some(vec![
                serde_json::json!("createdAt"),
                serde_json::json!("2024-01-01T00:00:00Z"),
            ]),
        };

        let mut offset = 1;
        let (clause, _) = build_condition_clause(&condition, &mut offset, &schema).unwrap();

        assert_eq!(clause, "\"created_at\"::timestamptz > $1::timestamptz");
    }

    #[test]
    fn test_system_field_id_uses_text() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "EQ".to_string(),
            arguments: Some(vec![
                serde_json::json!("id"),
                serde_json::json!("some-uuid"),
            ]),
        };

        let mut offset = 1;
        let (clause, _) = build_condition_clause(&condition, &mut offset, &schema).unwrap();

        assert_eq!(clause, "\"id\"::text = $1::text");
    }

    // ==================== build_order_by_clause Tests ====================

    #[test]
    fn test_order_by_default() {
        let schema = make_test_schema();
        let result = build_order_by_clause(&None, &None, &schema).unwrap();

        assert_eq!(result, "created_at ASC");
    }

    #[test]
    fn test_order_by_empty_fields() {
        let schema = make_test_schema();
        let result = build_order_by_clause(&Some(vec![]), &None, &schema).unwrap();

        assert_eq!(result, "created_at ASC");
    }

    #[test]
    fn test_order_by_single_field_asc() {
        let schema = make_test_schema();
        let result = build_order_by_clause(
            &Some(vec!["name".to_string()]),
            &Some(vec!["asc".to_string()]),
            &schema,
        )
        .unwrap();

        assert_eq!(result, "\"name\" ASC");
    }

    #[test]
    fn test_order_by_single_field_desc() {
        let schema = make_test_schema();
        let result = build_order_by_clause(
            &Some(vec!["price".to_string()]),
            &Some(vec!["desc".to_string()]),
            &schema,
        )
        .unwrap();

        assert_eq!(result, "\"price\" DESC");
    }

    #[test]
    fn test_order_by_multiple_fields() {
        let schema = make_test_schema();
        let result = build_order_by_clause(
            &Some(vec!["name".to_string(), "price".to_string()]),
            &Some(vec!["asc".to_string(), "desc".to_string()]),
            &schema,
        )
        .unwrap();

        assert_eq!(result, "\"name\" ASC, \"price\" DESC");
    }

    #[test]
    fn test_order_by_system_field_created_at() {
        let schema = make_test_schema();
        let result =
            build_order_by_clause(&Some(vec!["createdAt".to_string()]), &None, &schema).unwrap();

        assert_eq!(result, "\"created_at\" ASC"); // camelCase -> snake_case
    }

    #[test]
    fn test_order_by_system_field_updated_at() {
        let schema = make_test_schema();
        let result = build_order_by_clause(
            &Some(vec!["updatedAt".to_string()]),
            &Some(vec!["desc".to_string()]),
            &schema,
        )
        .unwrap();

        assert_eq!(result, "\"updated_at\" DESC");
    }

    #[test]
    fn test_order_by_system_field_id() {
        let schema = make_test_schema();
        let result = build_order_by_clause(&Some(vec!["id".to_string()]), &None, &schema).unwrap();

        assert_eq!(result, "\"id\" ASC");
    }

    #[test]
    fn test_order_by_default_order_asc() {
        let schema = make_test_schema();
        let result = build_order_by_clause(
            &Some(vec!["name".to_string()]),
            &None, // No order specified
            &schema,
        )
        .unwrap();

        assert_eq!(result, "\"name\" ASC"); // Default is ASC
    }

    #[test]
    fn test_order_by_invalid_field() {
        let schema = make_test_schema();
        let result =
            build_order_by_clause(&Some(vec!["nonexistent_field".to_string()]), &None, &schema);

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid sort field"));
    }

    #[test]
    fn test_order_by_invalid_order() {
        let schema = make_test_schema();
        let result = build_order_by_clause(
            &Some(vec!["name".to_string()]),
            &Some(vec!["invalid".to_string()]),
            &schema,
        );

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid sort order"));
    }

    // ==================== MATCH ====================

    fn schema_with_tsv() -> Schema {
        Schema {
            id: "test-id".to_string(),
            name: "fts_schema".to_string(),
            description: None,
            table_name: "fts_table".to_string(),
            columns: vec![
                ColumnDefinition::new("name", crate::types::ColumnType::String),
                ColumnDefinition::new(
                    "name_tsv",
                    crate::types::ColumnType::Tsvector {
                        source_column: "name".to_string(),
                        language: "english".to_string(),
                    },
                )
                .not_null(),
            ],
            indexes: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn test_match_basic() {
        let schema = schema_with_tsv();
        let condition = Condition {
            op: "MATCH".to_string(),
            arguments: Some(vec![
                serde_json::json!("name_tsv"),
                serde_json::json!("blue jacket"),
            ]),
        };
        let mut offset = 1;
        let (clause, params) = build_condition_clause(&condition, &mut offset, &schema).unwrap();
        assert_eq!(clause, "\"name_tsv\" @@ plainto_tsquery('english', $1)");
        assert_eq!(params, vec![serde_json::json!("blue jacket")]);
        assert_eq!(offset, 2);
    }

    #[test]
    fn test_match_rejects_non_tsvector_column() {
        let schema = schema_with_tsv();
        let condition = Condition {
            op: "MATCH".to_string(),
            arguments: Some(vec![serde_json::json!("name"), serde_json::json!("blue")]),
        };
        let mut offset = 1;
        let err = build_condition_clause(&condition, &mut offset, &schema).unwrap_err();
        assert!(err.contains("tsvector"), "{}", err);
    }

    #[test]
    fn test_match_wrong_arity() {
        let schema = schema_with_tsv();
        let condition = Condition {
            op: "MATCH".to_string(),
            arguments: Some(vec![serde_json::json!("name_tsv")]),
        };
        let mut offset = 1;
        let err = build_condition_clause(&condition, &mut offset, &schema).unwrap_err();
        assert!(err.contains("exactly 2 arguments"), "{}", err);
    }

    // ==================== SIMILARITY_GTE ====================

    #[test]
    fn test_similarity_gte_basic() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "SIMILARITY_GTE".to_string(),
            arguments: Some(vec![
                serde_json::json!("name"),
                serde_json::json!("blue jacket"),
                serde_json::json!(0.3),
            ]),
        };
        let mut offset = 1;
        let (clause, params) = build_condition_clause(&condition, &mut offset, &schema).unwrap();
        assert_eq!(clause, "similarity(\"name\"::text, $1::text) >= 0.300000");
        assert_eq!(params, vec![serde_json::json!("blue jacket")]);
        assert_eq!(offset, 2);
    }

    #[test]
    fn test_similarity_gte_threshold_out_of_range() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "SIMILARITY_GTE".to_string(),
            arguments: Some(vec![
                serde_json::json!("name"),
                serde_json::json!("x"),
                serde_json::json!(1.5),
            ]),
        };
        let mut offset = 1;
        let err = build_condition_clause(&condition, &mut offset, &schema).unwrap_err();
        assert!(err.contains("between 0.0 and 1.0"), "{}", err);
    }

    #[test]
    fn test_similarity_gte_rejects_non_text_column() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "SIMILARITY_GTE".to_string(),
            arguments: Some(vec![
                serde_json::json!("quantity"),
                serde_json::json!("x"),
                serde_json::json!(0.3),
            ]),
        };
        let mut offset = 1;
        let err = build_condition_clause(&condition, &mut offset, &schema).unwrap_err();
        assert!(err.contains("string/enum column"), "{}", err);
    }

    #[test]
    fn test_similarity_gte_wrong_arity() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "SIMILARITY_GTE".to_string(),
            arguments: Some(vec![serde_json::json!("name"), serde_json::json!("x")]),
        };
        let mut offset = 1;
        let err = build_condition_clause(&condition, &mut offset, &schema).unwrap_err();
        assert!(err.contains("exactly 3 arguments"), "{}", err);
    }

    #[test]
    fn test_similarity_gte_param_offset_after_other_clause() {
        let schema = make_test_schema();
        let condition = Condition {
            op: "AND".to_string(),
            arguments: Some(vec![
                serde_json::json!({"op": "EQ", "arguments": ["name", "active"]}),
                serde_json::json!({
                    "op": "SIMILARITY_GTE",
                    "arguments": ["name", "blue", 0.3]
                }),
            ]),
        };
        let mut offset = 1;
        let (clause, params) = build_condition_clause(&condition, &mut offset, &schema).unwrap();
        assert!(clause.contains("$1::text"));
        assert!(clause.contains("similarity(\"name\"::text, $2::text)"));
        assert_eq!(params.len(), 2);
        assert_eq!(offset, 3);
    }

    #[test]
    fn test_order_by_mixed_schema_and_system_fields() {
        let schema = make_test_schema();
        let result = build_order_by_clause(
            &Some(vec![
                "name".to_string(),
                "createdAt".to_string(),
                "price".to_string(),
            ]),
            &Some(vec![
                "asc".to_string(),
                "desc".to_string(),
                "asc".to_string(),
            ]),
            &schema,
        )
        .unwrap();

        assert_eq!(result, "\"name\" ASC, \"created_at\" DESC, \"price\" ASC");
    }
}

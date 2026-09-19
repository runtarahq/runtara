//! Shared columnar/object bulk input normalization.
#[derive(Debug, thiserror::Error)]
pub enum BulkInputError {
    #[error("{0}")]
    ValidationError(String),
}

pub fn normalize_bulk_create_inputs(
    instances: Option<&[serde_json::Value]>,
    columns: Option<&[String]>,
    rows: Option<&[Vec<serde_json::Value>]>,
    constants: &serde_json::Map<String, serde_json::Value>,
    nullify_empty_strings: bool,
    schema: &crate::Schema,
) -> Result<Vec<serde_json::Value>, BulkInputError> {
    match (instances, columns, rows) {
        (Some(inst), None, None) => Ok(inst.to_vec()),

        (None, Some(cols), Some(rows)) => {
            build_columnar_instances(cols, rows, constants, nullify_empty_strings, schema)
        }

        (None, Some(_), None) | (None, None, Some(_)) => Err(BulkInputError::ValidationError(
            "columnar form requires both `columns` and `rows`".to_string(),
        )),

        (Some(_), Some(_), _) | (Some(_), _, Some(_)) => Err(BulkInputError::ValidationError(
            "provide either `instances` or `columns` + `rows`, not both".to_string(),
        )),

        (None, None, None) => Err(BulkInputError::ValidationError(
            "must provide either `instances` or `columns` + `rows`".to_string(),
        )),
    }
}

fn build_columnar_instances(
    columns: &[String],
    rows: &[Vec<serde_json::Value>],
    constants: &serde_json::Map<String, serde_json::Value>,
    nullify_empty_strings: bool,
    schema: &crate::Schema,
) -> Result<Vec<serde_json::Value>, BulkInputError> {
    // Pre-compute which columns should nullify empty strings (non-string,
    // non-enum columns). Only populated when the flag is on.
    let nullify_cols: std::collections::HashSet<&str> = if nullify_empty_strings {
        schema
            .columns
            .iter()
            .filter(|c| {
                !matches!(
                    c.column_type,
                    crate::ColumnType::String | crate::ColumnType::Enum { .. }
                )
            })
            .map(|c| c.name.as_str())
            .collect()
    } else {
        std::collections::HashSet::new()
    };

    let mut result = Vec::with_capacity(rows.len());
    for (idx, row) in rows.iter().enumerate() {
        if row.len() != columns.len() {
            return Err(BulkInputError::ValidationError(format!(
                "row {} has {} cells, expected {} to match `columns`",
                idx,
                row.len(),
                columns.len()
            )));
        }
        let mut obj = constants.clone();
        for (col, val) in columns.iter().zip(row.iter()) {
            let val = if nullify_cols.contains(col.as_str()) {
                match val {
                    serde_json::Value::String(s) if s.is_empty() => serde_json::Value::Null,
                    other => other.clone(),
                }
            } else {
                val.clone()
            };
            obj.insert(col.clone(), val);
        }
        result.push(serde_json::Value::Object(obj));
    }
    Ok(result)
}

use crate::config::StoreConfig;
use crate::instance::{
    BulkCreateOptions, BulkCreateResult, BulkRowError, ConflictMode, ValidationMode,
};
use crate::planning::validate_instance_for_insert;
use crate::sql::quote_identifier;
use crate::{ColumnDefinition, Schema};
use runtara_database_contract::{SqlValue, Statement};
type ValidatedRow = (String, serde_json::Map<String, serde_json::Value>);

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct BulkPlanError(String);
impl BulkPlanError {
    fn validation(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

pub struct BulkCreatePlan {
    pub statements: Vec<Statement>,
    pub result: BulkCreateResult,
    validated_len: i64,
    count_conflicts: bool,
}
impl BulkCreatePlan {
    pub fn finish(mut self, affected: i64) -> BulkCreateResult {
        if self.count_conflicts && affected < self.validated_len {
            self.result.skipped_count += self.validated_len - affected;
        }
        self.result.created_count = affected;
        self.result
    }
}

pub fn plan_bulk_create(
    config: &StoreConfig,
    schema: &Schema,
    instances: Vec<serde_json::Value>,
    opts: BulkCreateOptions,
    mut new_id: impl FnMut() -> String,
) -> Result<BulkCreatePlan, BulkPlanError> {
    let mut result = BulkCreateResult::default();
    if instances.is_empty() {
        return Ok(BulkCreatePlan {
            statements: vec![],
            result,
            validated_len: 0,
            count_conflicts: false,
        });
    }

    if instances.len() > config.bulk_request_limit {
        return Err(BulkPlanError::validation(format!(
            "bulk request size {} exceeds limit of {}",
            instances.len(),
            config.bulk_request_limit
        )));
    }

    // Validate conflict_columns up front — required for Skip/Upsert.
    let conflict_cols: Option<&[String]> = match &opts.conflict_mode {
        ConflictMode::Error => None,
        ConflictMode::Skip { conflict_columns } | ConflictMode::Upsert { conflict_columns } => {
            if conflict_columns.is_empty() {
                return Err(BulkPlanError::validation(
                    "`conflict_columns` must be non-empty when on_conflict is 'skip' or 'upsert'",
                ));
            }
            Some(conflict_columns.as_slice())
        }
    };

    if let Some(cols) = conflict_cols {
        let known: std::collections::HashSet<&str> =
            schema.columns.iter().map(|c| c.name.as_str()).collect();
        for name in cols {
            if name != "id" && !known.contains(name.as_str()) {
                return Err(BulkPlanError::validation(format!(
                    "Conflict column '{}' does not exist in schema",
                    name
                )));
            }
        }
    }

    // Per-row validation — separate into valid / invalid.
    let mut validated: Vec<(String, serde_json::Map<String, serde_json::Value>)> =
        Vec::with_capacity(instances.len());
    for (idx, instance) in instances.into_iter().enumerate() {
        match validate_instance_for_insert(schema, &instance) {
            Ok(obj) => {
                let instance_id = new_id();
                validated.push((instance_id, obj));
            }
            Err(reason) => match opts.validation_mode {
                ValidationMode::Stop => {
                    return Err(BulkPlanError::validation(format!(
                        "Instance at index {}: {}",
                        idx, reason
                    )));
                }
                ValidationMode::Skip => {
                    result.skipped_count += 1;
                    result.errors.push(BulkRowError { index: idx, reason });
                }
            },
        }
    }

    if validated.is_empty() {
        return Ok(BulkCreatePlan {
            statements: vec![],
            result,
            validated_len: 0,
            count_conflicts: false,
        });
    }

    // Captured before `validated` is moved into conflict groups.
    let validated_len = validated.len() as i64;

    // Compute chunk size under Postgres' ~32k-param limit.
    let params_per_row = 1 + schema.columns.len();
    let chunk_size = (32000 / params_per_row.max(1)).max(1);

    // Column list (shared across chunks).
    let mut column_names = Vec::new();
    if config.auto_columns.id {
        column_names.push("id".to_string());
    }
    for col in &schema.columns {
        if col.column_type.is_generated() {
            continue;
        }
        column_names.push(quote_identifier(&col.name));
    }

    // Build (ON CONFLICT clause, rows) groups. Error and Skip share a
    // single clause across all rows. Upsert groups rows by their UPDATE
    // signature so each group's DO UPDATE SET only touches the columns
    // present in those rows' payloads.
    let conflict_groups: Vec<(String, Vec<ValidatedRow>)> = match &opts.conflict_mode {
        ConflictMode::Error => vec![(String::new(), validated)],
        ConflictMode::Skip { conflict_columns } => {
            let cols: Vec<String> = conflict_columns
                .iter()
                .map(|c| quote_identifier(c))
                .collect();
            let clause = format!(" ON CONFLICT {} DO NOTHING", live_conflict_target(&cols));
            vec![(clause, validated)]
        }
        ConflictMode::Upsert { conflict_columns } => {
            let cols: Vec<String> = conflict_columns
                .iter()
                .map(|c| quote_identifier(c))
                .collect();
            let conflict_target = live_conflict_target(&cols);
            let conflict_set: std::collections::HashSet<&str> =
                conflict_columns.iter().map(String::as_str).collect();
            let mut row_groups: std::collections::HashMap<Vec<String>, Vec<ValidatedRow>> =
                std::collections::HashMap::new();
            for (id, props) in validated {
                let sig = update_signature(schema, &props, &conflict_set);
                row_groups.entry(sig).or_default().push((id, props));
            }
            let updated_at_bump = config.auto_columns.updated_at;
            row_groups
                .into_iter()
                .map(|(signature, rows)| {
                    let mut update_sets: Vec<String> = signature
                        .iter()
                        .map(|name| {
                            let q = quote_identifier(name);
                            format!("{} = EXCLUDED.{}", q, q)
                        })
                        .collect();
                    if updated_at_bump {
                        update_sets.push("updated_at = NOW()".to_string());
                    }
                    let clause = if update_sets.is_empty() {
                        // Group has nothing to update — skip conflicts.
                        format!(" ON CONFLICT {} DO NOTHING", conflict_target)
                    } else {
                        format!(
                            " ON CONFLICT {} DO UPDATE SET {}",
                            conflict_target,
                            update_sets.join(", ")
                        )
                    };
                    (clause, rows)
                })
                .collect()
        }
    };

    let mut statements = Vec::new();

    for (on_conflict_clause, group_rows) in conflict_groups {
        for chunk in group_rows.chunks(chunk_size) {
            let mut placeholders = Vec::new();
            let mut param_idx = 1;
            for (_, properties_obj) in chunk {
                let mut row_placeholders = Vec::new();
                if config.auto_columns.id {
                    row_placeholders.push(format!("${}", param_idx));
                    param_idx += 1;
                }
                for col in &schema.columns {
                    if col.column_type.is_generated() {
                        continue;
                    }
                    match classify_slot(col, properties_obj) {
                        Slot::Default => row_placeholders.push("DEFAULT".to_string()),
                        Slot::TypedNull | Slot::Value(_) => {
                            row_placeholders.push(format!("${}", param_idx));
                            param_idx += 1;
                        }
                    }
                }
                placeholders.push(format!("({})", row_placeholders.join(", ")));
            }

            let insert_sql = format!(
                "INSERT INTO {} ({}) VALUES {}{}",
                quote_identifier(&schema.table_name),
                column_names.join(", "),
                placeholders.join(", "),
                on_conflict_clause,
            );

            let mut params = Vec::new();
            for (instance_id, properties_obj) in chunk {
                if config.auto_columns.id {
                    params.push(SqlValue::Text(instance_id.clone()));
                }
                for col in &schema.columns {
                    if col.column_type.is_generated() {
                        continue;
                    }
                    match classify_slot(col, properties_obj) {
                        Slot::Default => (),
                        Slot::TypedNull => params.push(SqlValue::Null(
                            crate::mapping::sql_type(&col.column_type)
                                .map_err(BulkPlanError::validation)?,
                        )),
                        Slot::Value(value) => params.push(
                            crate::mapping::object_param(&col.column_type, value)
                                .map_err(BulkPlanError::validation)?,
                        ),
                    }
                }
            }
            statements.push(Statement {
                sql: insert_sql,
                params,
                returning: None,
            });
        }
    }

    Ok(BulkCreatePlan {
        statements,
        result,
        validated_len,
        count_conflicts: matches!(opts.conflict_mode, ConflictMode::Skip { .. }),
    })
}

/// Per-column slot in a bulk-insert VALUES tuple.
///
/// Preserves the distinction between "payload omitted this key" and "payload
/// set this key to null" — which Postgres cares about for (a) firing declared
/// `DEFAULT` clauses and (b) writing SQL NULL vs JSONB `null` on JSON columns.
pub enum Slot<'a> {
    /// Key absent and the column declares a DB default — emit literal `DEFAULT`.
    Default,
    /// Key absent with no default — emit `$N`, bind typed `None::<T>` (SQL NULL).
    TypedNull,
    /// Key present (including explicit null) — emit `$N`, bind via the typed value mapper.
    Value(&'a serde_json::Value),
}

/// Classify a (column, row-payload) pair into the correct [`Slot`] variant.
pub fn classify_slot<'a>(
    col: &ColumnDefinition,
    properties_obj: &'a serde_json::Map<String, serde_json::Value>,
) -> Slot<'a> {
    match properties_obj.get(&col.name) {
        None if col.default_value.is_some() => Slot::Default,
        None => Slot::TypedNull,
        Some(v) => Slot::Value(v),
    }
}

/// Compute a row's UPDATE signature: the schema column names, in schema order,
/// that are both (a) not in the conflict-column set and (b) present in the
/// payload. Used by the upsert paths to group rows so each group's
/// `ON CONFLICT ... DO UPDATE SET` only touches columns the caller actually
/// provided — absent columns keep their stored value (or fall back to
/// `DO NOTHING` if the group has nothing to update).
pub fn update_signature(
    schema: &Schema,
    properties_obj: &serde_json::Map<String, serde_json::Value>,
    conflict_cols: &std::collections::HashSet<&str>,
) -> Vec<String> {
    schema
        .columns
        .iter()
        .filter(|col| !conflict_cols.contains(col.name.as_str()))
        .filter(|col| properties_obj.contains_key(&col.name))
        .map(|col| col.name.clone())
        .collect()
}

pub fn live_conflict_target(quoted_columns: &[String]) -> String {
    format!("({}) WHERE deleted = FALSE", quoted_columns.join(", "))
}

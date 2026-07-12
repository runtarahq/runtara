//! Shared form schema, condition state, and value validation.
//!
//! Domain DSLs compose these types instead of defining their own field/control
//! vocabularies. Backend callers use these functions directly; browser callers
//! receive the same behavior through the validation WASM crate.

use std::collections::{HashMap, HashSet};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::condition_eval::{ConditionEvaluationError, evaluate_condition};
use crate::{ConditionExpression, SchemaField, SchemaFieldType};

pub const FORM_SCHEMA_VERSION: u32 = 1;

fn default_schema_version() -> u32 {
    FORM_SCHEMA_VERSION
}

fn default_allow_unknown_fields() -> bool {
    true
}

#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct FormDefinition {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub fields: HashMap<String, FormField>,
    #[serde(default)]
    pub sections: Vec<FormSection>,
    #[serde(default = "default_allow_unknown_fields")]
    pub allow_unknown_fields: bool,
}

impl Default for FormDefinition {
    fn default() -> Self {
        Self {
            schema_version: FORM_SCHEMA_VERSION,
            fields: HashMap::new(),
            sections: Vec::new(),
            allow_unknown_fields: true,
        }
    }
}

#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct FormField {
    #[serde(flatten)]
    pub schema: SchemaField,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<FormControl>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    #[serde(default, skip_serializing_if = "FormConditions::is_empty")]
    pub conditions: FormConditions,
    #[serde(default)]
    pub access: FieldAccessMode,
    #[serde(default, skip_serializing_if = "is_false")]
    pub secret: bool,
}

#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct FormSection {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub order: i32,
    #[serde(default, skip_serializing_if = "is_false")]
    pub advanced: bool,
    #[serde(default, skip_serializing_if = "FormConditions::is_empty")]
    pub conditions: FormConditions,
}

#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum FieldAccessMode {
    #[default]
    ReadWrite,
    Read,
    Write,
}

#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct FormControl {
    pub kind: ControlKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<FormOption>,
    /// Domain-owned key used to resolve dynamic choices. The shared engine
    /// deliberately does not interpret the key or define a query language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_resolver: Option<String>,
    /// Sibling fields whose values affect `option_resolver` results.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub option_dependencies: Vec<String>,
}

#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ControlKind {
    Text,
    Textarea,
    SecretTextarea,
    Password,
    Number,
    Toggle,
    Select,
    MultiSelect,
    Radio,
    Date,
    Datetime,
    DateRange,
    NumberRange,
    Tags,
    KeyValue,
    Lookup,
    File,
}

#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct FormOption {
    pub value: Value,
    pub label: String,
}

#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct FormConditions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "utoipa", schema(no_recursion))]
    pub visible: Option<ConditionExpression>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "utoipa", schema(no_recursion))]
    pub enabled: Option<ConditionExpression>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "utoipa", schema(no_recursion))]
    pub required: Option<ConditionExpression>,
}

impl FormConditions {
    pub fn is_empty(&self) -> bool {
        self.visible.is_none() && self.enabled.is_none() && self.required.is_none()
    }
}

#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum FormIssueSeverity {
    Error,
    Warning,
}

#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct FormIssue {
    pub code: String,
    pub path: String,
    pub message: String,
    pub severity: FormIssueSeverity,
}

#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct FormFieldState {
    pub visible: bool,
    pub enabled: bool,
    pub required: bool,
}

#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct FormAnalysis {
    pub valid: bool,
    pub fields: HashMap<String, FormFieldState>,
    pub issues: Vec<FormIssue>,
}

/// Validate a form definition independently of any submitted value.
pub fn validate_form_definition(definition: &FormDefinition) -> Vec<FormIssue> {
    let mut issues = Vec::new();

    if definition.schema_version != FORM_SCHEMA_VERSION {
        issues.push(error(
            "UNSUPPORTED_FORM_SCHEMA_VERSION",
            "schemaVersion",
            format!(
                "Unsupported form schema version {}; expected {}",
                definition.schema_version, FORM_SCHEMA_VERSION
            ),
        ));
    }

    let mut sections = HashSet::new();
    for (index, section) in definition.sections.iter().enumerate() {
        let path = format!("sections[{index}]");
        if section.id.trim().is_empty() {
            issues.push(error(
                "EMPTY_FORM_SECTION_ID",
                format!("{path}.id"),
                "Form section id cannot be empty",
            ));
        } else if !sections.insert(section.id.as_str()) {
            issues.push(error(
                "DUPLICATE_FORM_SECTION_ID",
                format!("{path}.id"),
                format!("Duplicate form section id '{}'", section.id),
            ));
        }
        if section.conditions.required.is_some() {
            issues.push(error(
                "SECTION_CANNOT_BE_REQUIRED",
                format!("{path}.conditions.required"),
                "Required conditions are only valid on fields",
            ));
        }
        validate_conditions(
            &section.conditions,
            &format!("{path}.conditions"),
            &mut issues,
        );
    }

    for (name, field) in &definition.fields {
        let path = format!("fields.{name}");
        if name.trim().is_empty() {
            issues.push(error(
                "EMPTY_FORM_FIELD_NAME",
                &path,
                "Form field name cannot be empty",
            ));
        }
        if let Some(section) = field.section.as_deref()
            && !sections.contains(section)
        {
            issues.push(error(
                "UNKNOWN_FORM_SECTION",
                format!("{path}.section"),
                format!("Field references unknown form section '{section}'"),
            ));
        }
        if field.secret && field.access != FieldAccessMode::Write {
            issues.push(error(
                "SECRET_FIELD_MUST_BE_WRITE",
                format!("{path}.access"),
                "Secret fields must use write access",
            ));
        }
        if field.secret
            && let Some(control) = &field.control
            && !matches!(
                control.kind,
                ControlKind::Password | ControlKind::SecretTextarea
            )
        {
            issues.push(error(
                "SECRET_FIELD_CONTROL_UNMASKED",
                format!("{path}.control.kind"),
                "Secret fields must use password or secret_textarea controls",
            ));
        }
        validate_control(field, &path, &mut issues);
        if let Some(pattern) = field.schema.pattern.as_deref()
            && let Err(regex_error) = Regex::new(pattern)
        {
            issues.push(error(
                "FORM_PATTERN_INVALID",
                format!("{path}.pattern"),
                format!("Invalid regular expression: {regex_error}"),
            ));
        }
        validate_conditions(
            &field.conditions,
            &format!("{path}.conditions"),
            &mut issues,
        );
    }

    issues
}

/// Evaluate effective state and validate submitted data.
pub fn analyze_form(definition: &FormDefinition, data: &Value) -> FormAnalysis {
    let mut issues = validate_form_definition(definition);
    let fields = evaluate_field_states(definition, data, &mut issues);

    let Some(object) = data.as_object() else {
        issues.push(error(
            "FORM_DATA_NOT_OBJECT",
            "data",
            "Form data must be a JSON object",
        ));
        return FormAnalysis {
            valid: false,
            fields,
            issues,
        };
    };

    if !definition.allow_unknown_fields {
        for name in object.keys() {
            if !definition.fields.contains_key(name) {
                issues.push(error(
                    "UNKNOWN_FORM_FIELD",
                    format!("data.{name}"),
                    format!("Unknown form field '{name}'"),
                ));
            }
        }
    }

    for (name, field) in &definition.fields {
        let state = fields.get(name).copied().unwrap_or(FormFieldState {
            visible: false,
            enabled: false,
            required: false,
        });
        match object.get(name) {
            Some(Value::String(value))
                if state.required && field.schema.default.is_none() && value.trim().is_empty() =>
            {
                issues.push(error(
                    "REQUIRED_FORM_FIELD_MISSING",
                    format!("data.{name}"),
                    format!(
                        "{} is required",
                        field.schema.label.as_deref().unwrap_or(name)
                    ),
                ));
            }
            Some(value) => {
                validate_field_value(&field.schema, value, &format!("data.{name}"), &mut issues)
            }
            None if state.required && field.schema.default.is_none() => issues.push(error(
                "REQUIRED_FORM_FIELD_MISSING",
                format!("data.{name}"),
                format!(
                    "{} is required",
                    field.schema.label.as_deref().unwrap_or(name)
                ),
            )),
            None => {}
        }
    }

    FormAnalysis {
        valid: !issues
            .iter()
            .any(|issue| issue.severity == FormIssueSeverity::Error),
        fields,
        issues,
    }
}

/// Build a canonical form definition from connection descriptor metadata.
///
/// This is the compatibility adapter for the existing `ConnectionParams`
/// mini-DSL. New UI metadata is emitted by the derive macro, while field type,
/// default, enum, secret, and optionality metadata retain their existing wire
/// meaning.
pub fn connection_form_definition(meta: &crate::agent_meta::ConnectionTypeMeta) -> FormDefinition {
    let mut fields = HashMap::new();
    let mut section_ids = HashSet::new();

    for field in meta.fields {
        let field_type = connection_field_type(field.type_name);
        let section = field
            .section
            .unwrap_or(if field.is_secret {
                "credentials"
            } else {
                "configuration"
            })
            .to_string();
        section_ids.insert(section.clone());

        let control_kind = field.control.or_else(|| {
            if field.is_secret {
                Some(ControlKind::Password)
            } else if field.enum_values.is_some() {
                Some(if field_type == SchemaFieldType::Array {
                    ControlKind::MultiSelect
                } else {
                    ControlKind::Select
                })
            } else {
                None
            }
        });
        let options = field
            .enum_values
            .unwrap_or_default()
            .iter()
            .map(|value| FormOption {
                value: Value::String((*value).to_string()),
                label: humanize_identifier(value),
            })
            .collect();

        fields.insert(
            field.name.to_string(),
            FormField {
                schema: SchemaField {
                    field_type: field_type.clone(),
                    description: field.description.map(str::to_string),
                    required: field.is_required || !field.is_optional,
                    default: field
                        .default_value
                        .map(|value| connection_default_value(value, &field_type)),
                    example: None,
                    items: (field_type == SchemaFieldType::Array)
                        .then(|| Box::new(empty_schema_field(SchemaFieldType::String))),
                    enum_values: field.enum_values.map(|values| {
                        values
                            .iter()
                            .map(|value| Value::String((*value).to_string()))
                            .collect()
                    }),
                    label: field.display_name.map(str::to_string),
                    placeholder: field.placeholder.map(str::to_string),
                    order: None,
                    format: field.is_url.then(|| "url".to_string()),
                    min: None,
                    max: None,
                    pattern: None,
                    properties: None,
                    visible_when: None,
                    nullable: None,
                },
                control: control_kind.map(|kind| FormControl {
                    kind,
                    options,
                    option_resolver: None,
                    option_dependencies: Vec::new(),
                }),
                section: Some(section),
                conditions: FormConditions {
                    visible: field.conditions.visible.map(|factory| factory()),
                    enabled: field.conditions.enabled.map(|factory| factory()),
                    required: field.conditions.required.map(|factory| factory()),
                },
                access: field.access,
                secret: field.is_secret,
            },
        );
    }

    let mut sections: Vec<FormSection> = section_ids
        .into_iter()
        .map(|id| FormSection {
            label: match id.as_str() {
                "configuration" => "Connection details".to_string(),
                "credentials" => "Credentials".to_string(),
                _ => humanize_identifier(&id),
            },
            order: if id == "configuration" { 0 } else { 100 },
            advanced: false,
            description: None,
            conditions: FormConditions::default(),
            id,
        })
        .collect();
    sections.sort_by(|left, right| left.order.cmp(&right.order).then(left.id.cmp(&right.id)));

    FormDefinition {
        fields,
        sections,
        allow_unknown_fields: false,
        ..FormDefinition::default()
    }
}

/// Build a canonical equality condition against a sibling form field.
///
/// Connection condition factories use this helper so descriptor code remains
/// readable while still producing the exact shared `ConditionExpression` AST.
pub fn field_equals(field: impl Into<String>, value: impl Into<Value>) -> ConditionExpression {
    ConditionExpression::Operation(crate::ConditionOperation {
        op: crate::ConditionOperator::Eq,
        arguments: vec![
            crate::ConditionArgument::Value(crate::MappingValue::Reference(
                crate::ReferenceValue {
                    value: field.into(),
                    type_hint: None,
                    default: None,
                },
            )),
            crate::ConditionArgument::Value(crate::MappingValue::Immediate(
                crate::ImmediateValue {
                    value: value.into(),
                },
            )),
        ],
    })
}

/// Invert a canonical form condition without adding an inverse UI effect.
pub fn not(condition: ConditionExpression) -> ConditionExpression {
    ConditionExpression::Operation(crate::ConditionOperation {
        op: crate::ConditionOperator::Not,
        arguments: vec![crate::ConditionArgument::Expression(Box::new(condition))],
    })
}

/// Build a canonical condition that is true when a sibling field is present.
pub fn field_is_defined(field: impl Into<String>) -> ConditionExpression {
    ConditionExpression::Operation(crate::ConditionOperation {
        op: crate::ConditionOperator::IsDefined,
        arguments: vec![crate::ConditionArgument::Value(
            crate::MappingValue::Reference(crate::ReferenceValue {
                value: field.into(),
                type_hint: None,
                default: None,
            }),
        )],
    })
}

/// Compose canonical conditions with `AND`.
pub fn all(conditions: impl IntoIterator<Item = ConditionExpression>) -> ConditionExpression {
    ConditionExpression::Operation(crate::ConditionOperation {
        op: crate::ConditionOperator::And,
        arguments: conditions
            .into_iter()
            .map(|condition| crate::ConditionArgument::Expression(Box::new(condition)))
            .collect(),
    })
}

/// Compose canonical conditions with `OR`.
pub fn any(conditions: impl IntoIterator<Item = ConditionExpression>) -> ConditionExpression {
    ConditionExpression::Operation(crate::ConditionOperation {
        op: crate::ConditionOperator::Or,
        arguments: conditions
            .into_iter()
            .map(|condition| crate::ConditionArgument::Expression(Box::new(condition)))
            .collect(),
    })
}

/// Normalize the workflow DSL's existing flat schema map into a transient
/// canonical form definition. The workflow schema itself is not mutated or
/// persisted in a new shape.
pub fn schema_fields_form_definition(fields: &HashMap<String, SchemaField>) -> FormDefinition {
    FormDefinition {
        fields: fields
            .iter()
            .map(|(name, source)| {
                let mut schema = source.clone();
                let visible = schema
                    .visible_when
                    .take()
                    .and_then(|condition| legacy_visible_condition(&condition));
                (
                    name.clone(),
                    FormField {
                        schema,
                        control: None,
                        section: None,
                        conditions: FormConditions {
                            visible,
                            ..FormConditions::default()
                        },
                        access: FieldAccessMode::ReadWrite,
                        secret: false,
                    },
                )
            })
            .collect(),
        allow_unknown_fields: false,
        ..FormDefinition::default()
    }
}

fn legacy_visible_condition(condition: &crate::VisibleWhen) -> Option<ConditionExpression> {
    let mut clauses = Vec::new();
    if let Some(value) = &condition.equals {
        clauses.push(serde_json::json!({
            "type": "operation",
            "op": "EQ",
            "arguments": [
                {"valueType": "reference", "value": condition.field},
                {"valueType": "immediate", "value": value}
            ]
        }));
    }
    if let Some(value) = &condition.not_equals {
        clauses.push(serde_json::json!({
            "type": "operation",
            "op": "NE",
            "arguments": [
                {"valueType": "reference", "value": condition.field},
                {"valueType": "immediate", "value": value}
            ]
        }));
    }
    let value = match clauses.len() {
        0 => return None,
        1 => clauses.pop().expect("one condition clause"),
        _ => serde_json::json!({
            "type": "operation",
            "op": "AND",
            "arguments": clauses
        }),
    };
    serde_json::from_value(value).ok()
}

fn connection_field_type(type_name: &str) -> SchemaFieldType {
    let normalized = type_name.to_ascii_lowercase().replace(' ', "");
    match normalized.as_str() {
        "bool" | "boolean" => SchemaFieldType::Boolean,
        "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32" | "i64" | "usize" | "isize" => {
            SchemaFieldType::Integer
        }
        "f32" | "f64" | "number" => SchemaFieldType::Number,
        value if value.starts_with("vec<") || value == "array" || value == "string[]" => {
            SchemaFieldType::Array
        }
        value if value.starts_with("hashmap<") || value.starts_with("map<") => {
            SchemaFieldType::Object
        }
        _ => SchemaFieldType::String,
    }
}

fn connection_default_value(value: &str, field_type: &SchemaFieldType) -> Value {
    match field_type {
        SchemaFieldType::Boolean => Value::Bool(value.eq_ignore_ascii_case("true")),
        SchemaFieldType::Integer => value
            .parse::<i64>()
            .map(Value::from)
            .unwrap_or_else(|_| Value::String(value.to_string())),
        SchemaFieldType::Number => value
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .unwrap_or_else(|| Value::String(value.to_string())),
        SchemaFieldType::Array => Value::Array(
            value
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(|item| Value::String(item.to_string()))
                .collect(),
        ),
        _ => Value::String(value.to_string()),
    }
}

fn empty_schema_field(field_type: SchemaFieldType) -> SchemaField {
    SchemaField {
        field_type,
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
        nullable: None,
    }
}

fn humanize_identifier(value: &str) -> String {
    value
        .split(['_', '-', ' '])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn evaluate_field_states(
    definition: &FormDefinition,
    data: &Value,
    issues: &mut Vec<FormIssue>,
) -> HashMap<String, FormFieldState> {
    let mut sections = HashMap::new();
    for (index, section) in definition.sections.iter().enumerate() {
        sections.insert(
            section.id.as_str(),
            (
                evaluate_optional_condition(
                    section.conditions.visible.as_ref(),
                    data,
                    true,
                    &format!("sections[{index}].conditions.visible"),
                    issues,
                ),
                evaluate_optional_condition(
                    section.conditions.enabled.as_ref(),
                    data,
                    true,
                    &format!("sections[{index}].conditions.enabled"),
                    issues,
                ),
            ),
        );
    }

    definition
        .fields
        .iter()
        .map(|(name, field)| {
            let path = format!("fields.{name}.conditions");
            let (section_visible, section_enabled) = field
                .section
                .as_deref()
                .and_then(|section| sections.get(section).copied())
                .unwrap_or((true, true));
            let visible = section_visible
                && evaluate_optional_condition(
                    field.conditions.visible.as_ref(),
                    data,
                    true,
                    &format!("{path}.visible"),
                    issues,
                );
            let enabled = section_enabled
                && evaluate_optional_condition(
                    field.conditions.enabled.as_ref(),
                    data,
                    true,
                    &format!("{path}.enabled"),
                    issues,
                );
            let conditionally_required = evaluate_optional_condition(
                field.conditions.required.as_ref(),
                data,
                false,
                &format!("{path}.required"),
                issues,
            );
            (
                name.clone(),
                FormFieldState {
                    visible,
                    enabled,
                    required: visible && (field.schema.required || conditionally_required),
                },
            )
        })
        .collect()
}

fn evaluate_optional_condition(
    condition: Option<&ConditionExpression>,
    data: &Value,
    default: bool,
    path: &str,
    issues: &mut Vec<FormIssue>,
) -> bool {
    let Some(condition) = condition else {
        return default;
    };
    match evaluate_condition(condition, data) {
        Ok(value) => value,
        Err(condition_error) => {
            issues.push(condition_issue(path, condition_error));
            false
        }
    }
}

fn validate_conditions(conditions: &FormConditions, path: &str, issues: &mut Vec<FormIssue>) {
    for (name, condition) in [
        ("visible", conditions.visible.as_ref()),
        ("enabled", conditions.enabled.as_ref()),
        ("required", conditions.required.as_ref()),
    ] {
        if let Some(condition) = condition
            && let Err(condition_error) =
                evaluate_condition(condition, &Value::Object(Default::default()))
        {
            issues.push(condition_issue(&format!("{path}.{name}"), condition_error));
        }
    }
}

fn validate_control(field: &FormField, path: &str, issues: &mut Vec<FormIssue>) {
    let Some(control) = &field.control else {
        return;
    };
    let compatible = match control.kind {
        ControlKind::Number => matches!(
            field.schema.field_type,
            SchemaFieldType::Integer | SchemaFieldType::Number
        ),
        ControlKind::Toggle => matches!(field.schema.field_type, SchemaFieldType::Boolean),
        ControlKind::Tags
        | ControlKind::MultiSelect
        | ControlKind::DateRange
        | ControlKind::NumberRange => {
            matches!(field.schema.field_type, SchemaFieldType::Array)
        }
        ControlKind::KeyValue => matches!(field.schema.field_type, SchemaFieldType::Object),
        ControlKind::File => matches!(field.schema.field_type, SchemaFieldType::File),
        _ => matches!(field.schema.field_type, SchemaFieldType::String),
    };
    if !compatible {
        issues.push(error(
            "FORM_CONTROL_TYPE_MISMATCH",
            format!("{path}.control.kind"),
            format!(
                "Control {:?} is incompatible with field type {}",
                control.kind, field.schema.field_type
            ),
        ));
    }
}

fn validate_field_value(
    schema: &SchemaField,
    value: &Value,
    path: &str,
    issues: &mut Vec<FormIssue>,
) {
    if value.is_null() {
        if schema.nullable.unwrap_or(false) {
            return;
        }
        issues.push(error(
            "FORM_FIELD_NULL_NOT_ALLOWED",
            path,
            "Field cannot be null",
        ));
        return;
    }

    if !value_matches_type(value, &schema.field_type) {
        issues.push(error(
            "FORM_FIELD_TYPE_MISMATCH",
            path,
            format!("Expected {}", schema.field_type),
        ));
        return;
    }

    if let Some(values) = &schema.enum_values
        && !values.iter().any(|allowed| allowed == value)
    {
        issues.push(error(
            "FORM_FIELD_NOT_IN_ENUM",
            path,
            "Value is not one of the allowed options",
        ));
    }

    match value {
        Value::String(value) => {
            let length = value.chars().count() as f64;
            validate_min_max(schema, length, "length", path, issues);
            if let Some(pattern) = schema.pattern.as_deref()
                && let Ok(pattern) = Regex::new(pattern)
                && !pattern.is_match(value)
            {
                issues.push(error(
                    "FORM_FIELD_PATTERN_MISMATCH",
                    path,
                    "Field does not match the required pattern",
                ));
            }
            if let Some(format) = schema.format.as_deref()
                && !value_matches_format(value, format)
            {
                issues.push(error(
                    "FORM_FIELD_FORMAT_INVALID",
                    path,
                    format!("Field is not a valid {format}"),
                ));
            }
        }
        Value::Number(value) => {
            if let Some(number) = value.as_f64() {
                validate_min_max(schema, number, "value", path, issues);
            }
        }
        Value::Array(values) => {
            if let Some(items) = &schema.items {
                for (index, value) in values.iter().enumerate() {
                    validate_field_value(items, value, &format!("{path}[{index}]"), issues);
                }
            }
        }
        Value::Object(values) => {
            if let Some(properties) = &schema.properties {
                for (name, property) in properties {
                    match values.get(name) {
                        Some(value) => {
                            validate_field_value(property, value, &format!("{path}.{name}"), issues)
                        }
                        None if property.required => issues.push(error(
                            "REQUIRED_FORM_FIELD_MISSING",
                            format!("{path}.{name}"),
                            format!("{} is required", property.label.as_deref().unwrap_or(name)),
                        )),
                        None => {}
                    }
                }
            }
        }
        _ => {}
    }
}

fn validate_min_max(
    schema: &SchemaField,
    actual: f64,
    noun: &str,
    path: &str,
    issues: &mut Vec<FormIssue>,
) {
    if let Some(minimum) = schema.min
        && actual < minimum
    {
        issues.push(error(
            "FORM_FIELD_BELOW_MINIMUM",
            path,
            format!("Field {noun} must be at least {minimum}"),
        ));
    }
    if let Some(maximum) = schema.max
        && actual > maximum
    {
        issues.push(error(
            "FORM_FIELD_ABOVE_MAXIMUM",
            path,
            format!("Field {noun} must be at most {maximum}"),
        ));
    }
}

fn value_matches_type(value: &Value, field_type: &SchemaFieldType) -> bool {
    match field_type {
        SchemaFieldType::String => value.is_string(),
        SchemaFieldType::Integer => value.as_i64().is_some() || value.as_u64().is_some(),
        SchemaFieldType::Number => value.is_number(),
        SchemaFieldType::Boolean => value.is_boolean(),
        SchemaFieldType::Array => value.is_array(),
        SchemaFieldType::Object | SchemaFieldType::File => value.is_object(),
    }
}

fn value_matches_format(value: &str, format: &str) -> bool {
    match format {
        "email" => Regex::new(r"^[^\s@]+@[^\s@]+\.[^\s@]+$")
            .expect("static email regex")
            .is_match(value),
        "url" => Regex::new(r"^[A-Za-z][A-Za-z0-9+.-]*://[^\s/]+(?:/[^\s]*)?$")
            .expect("static URL regex")
            .is_match(value),
        "date" => valid_date(value),
        "datetime" | "date-time" => valid_datetime(value),
        // Presentation-only formats do not add value constraints.
        "text" | "textarea" | "markdown" | "password" | "tel" | "color" => true,
        // SchemaField documents unknown formats as renderer hints with a text
        // fallback, so they remain validation-neutral for compatibility.
        _ => true,
    }
}

fn valid_date(value: &str) -> bool {
    let mut parts = value.split('-');
    let (Some(year), Some(month), Some(day), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    if year.len() != 4 || month.len() != 2 || day.len() != 2 {
        return false;
    }
    let (Ok(year), Ok(month), Ok(day)) = (
        year.parse::<u32>(),
        month.parse::<u32>(),
        day.parse::<u32>(),
    ) else {
        return false;
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=max_day).contains(&day)
}

fn valid_datetime(value: &str) -> bool {
    let Some((date, time)) = value.split_once('T') else {
        return false;
    };
    if !valid_date(date) {
        return false;
    }
    Regex::new(r"^(?:[01]\d|2[0-3]):[0-5]\d(?::[0-5]\d(?:\.\d+)?)?(?:Z|[+-][0-2]\d:[0-5]\d)?$")
        .expect("static datetime regex")
        .is_match(time)
}

fn condition_issue(path: &str, condition_error: ConditionEvaluationError) -> FormIssue {
    error(
        "FORM_CONDITION_INVALID",
        path,
        format!("Invalid form condition: {condition_error}"),
    )
}

fn error(
    code: impl Into<String>,
    path: impl Into<String>,
    message: impl Into<String>,
) -> FormIssue {
    FormIssue {
        code: code.into(),
        path: path.into(),
        message: message.into(),
        severity: FormIssueSeverity::Error,
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn production_only() -> ConditionExpression {
        field_equals("environment", "production")
    }

    fn schema(field_type: SchemaFieldType) -> SchemaField {
        SchemaField {
            field_type,
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
            nullable: None,
        }
    }

    fn field(field_type: SchemaFieldType) -> FormField {
        FormField {
            schema: schema(field_type),
            control: None,
            section: None,
            conditions: FormConditions::default(),
            access: FieldAccessMode::ReadWrite,
            secret: false,
        }
    }

    fn condition(value: Value) -> ConditionExpression {
        serde_json::from_value(value).expect("valid condition fixture")
    }

    #[test]
    fn access_modes_have_compact_unambiguous_wire_values() {
        assert_eq!(
            serde_json::to_value(FieldAccessMode::ReadWrite).unwrap(),
            json!("read_write")
        );
        assert_eq!(
            serde_json::to_value(FieldAccessMode::Read).unwrap(),
            json!("read")
        );
        assert_eq!(
            serde_json::to_value(FieldAccessMode::Write).unwrap(),
            json!("write")
        );
    }

    #[test]
    fn dynamic_option_metadata_is_domain_neutral_and_round_trips() {
        let control: FormControl = serde_json::from_value(json!({
            "kind": "lookup",
            "optionResolver": "object-model.resources",
            "optionDependencies": ["company_id"]
        }))
        .unwrap();

        assert_eq!(
            control.option_resolver.as_deref(),
            Some("object-model.resources")
        );
        assert_eq!(control.option_dependencies, ["company_id"]);
        assert_eq!(
            serde_json::to_value(control).unwrap(),
            json!({
                "kind": "lookup",
                "optionResolver": "object-model.resources",
                "optionDependencies": ["company_id"]
            })
        );
    }

    #[test]
    fn secret_fields_must_be_write_only_at_the_api_boundary() {
        let mut secret = field(SchemaFieldType::String);
        secret.secret = true;
        let definition = FormDefinition {
            fields: HashMap::from([("token".to_string(), secret)]),
            ..FormDefinition::default()
        };

        let issues = validate_form_definition(&definition);
        assert!(
            issues
                .iter()
                .any(|issue| issue.code == "SECRET_FIELD_MUST_BE_WRITE")
        );
    }

    #[test]
    fn visible_condition_controls_required_validation() {
        let mut token = field(SchemaFieldType::String);
        token.schema.required = true;
        token.conditions.visible = Some(condition(json!({
            "type": "operation",
            "op": "EQ",
            "arguments": [
                { "valueType": "reference", "value": "auth_mode" },
                { "valueType": "immediate", "value": "bearer" }
            ]
        })));
        let definition = FormDefinition {
            fields: HashMap::from([
                ("auth_mode".to_string(), field(SchemaFieldType::String)),
                ("token".to_string(), token),
            ]),
            ..FormDefinition::default()
        };

        let hidden = analyze_form(&definition, &json!({ "auth_mode": "none" }));
        assert!(hidden.valid, "{:?}", hidden.issues);
        assert!(!hidden.fields["token"].visible);
        assert!(!hidden.fields["token"].required);

        let shown = analyze_form(&definition, &json!({ "auth_mode": "bearer" }));
        assert!(!shown.valid);
        assert!(shown.fields["token"].visible);
        assert!(shown.fields["token"].required);
        assert!(
            shown
                .issues
                .iter()
                .any(|issue| issue.code == "REQUIRED_FORM_FIELD_MISSING")
        );
    }

    #[test]
    fn effective_required_state_rejects_blank_strings() {
        let mut password = field(SchemaFieldType::String);
        password.conditions.required = Some(condition(json!({
            "type": "operation",
            "op": "EQ",
            "arguments": [
                { "valueType": "reference", "value": "auth_mode" },
                { "valueType": "immediate", "value": "password" }
            ]
        })));
        let definition = FormDefinition {
            fields: HashMap::from([
                ("auth_mode".to_string(), field(SchemaFieldType::String)),
                ("password".to_string(), password),
            ]),
            ..FormDefinition::default()
        };

        let required = analyze_form(
            &definition,
            &json!({ "auth_mode": "password", "password": "  " }),
        );
        assert!(!required.valid);
        assert!(required.fields["password"].required);
        assert!(required.issues.iter().any(|issue| {
            issue.code == "REQUIRED_FORM_FIELD_MISSING" && issue.path == "data.password"
        }));

        let optional = analyze_form(
            &definition,
            &json!({ "auth_mode": "private_key", "password": "" }),
        );
        assert!(optional.valid, "{:?}", optional.issues);
    }

    #[test]
    fn section_conditions_flow_into_field_state() {
        let section_condition = condition(json!({
            "type": "operation",
            "op": "EQ",
            "arguments": [
                { "valueType": "reference", "value": "advanced" },
                { "valueType": "immediate", "value": true }
            ]
        }));
        let section = FormSection {
            id: "advanced".to_string(),
            label: "Advanced".to_string(),
            description: None,
            order: 10,
            advanced: true,
            conditions: FormConditions {
                visible: Some(section_condition),
                ..FormConditions::default()
            },
        };
        let mut endpoint = field(SchemaFieldType::String);
        endpoint.section = Some("advanced".to_string());
        let definition = FormDefinition {
            fields: HashMap::from([("endpoint".to_string(), endpoint)]),
            sections: vec![section],
            ..FormDefinition::default()
        };

        assert!(
            !analyze_form(&definition, &json!({ "advanced": false })).fields["endpoint"].visible
        );
        assert!(analyze_form(&definition, &json!({ "advanced": true })).fields["endpoint"].visible);
    }

    #[test]
    fn validates_nested_values_and_limits() {
        let mut name = schema(SchemaFieldType::String);
        name.required = true;
        name.min = Some(3.0);
        let mut profile = field(SchemaFieldType::Object);
        profile.schema.properties = Some(HashMap::from([("name".to_string(), name)]));
        let definition = FormDefinition {
            fields: HashMap::from([("profile".to_string(), profile)]),
            allow_unknown_fields: false,
            ..FormDefinition::default()
        };

        let analysis = analyze_form(&definition, &json!({ "profile": { "name": "Al" } }));
        assert!(!analysis.valid);
        assert!(
            analysis
                .issues
                .iter()
                .any(|issue| issue.path == "data.profile.name"
                    && issue.code == "FORM_FIELD_BELOW_MINIMUM")
        );
    }

    #[test]
    fn connection_metadata_normalizes_to_canonical_form() {
        use crate::agent_meta::{
            ConnectionFieldBehavior, ConnectionFieldConditions, ConnectionFieldMeta,
            ConnectionTypeMeta,
        };

        static FIELDS: &[ConnectionFieldMeta] = &[
            ConnectionFieldMeta {
                name: "environment",
                type_name: "String",
                is_optional: false,
                display_name: Some("Environment"),
                description: None,
                placeholder: None,
                default_value: Some("sandbox"),
                is_secret: false,
                enum_values: Some(&["sandbox", "production"]),
                is_url: false,
                is_required: false,
                control: None,
                section: None,
                access: FieldAccessMode::ReadWrite,
                conditions: ConnectionFieldConditions {
                    visible: Some(production_only),
                    enabled: None,
                    required: None,
                },
                behavior: ConnectionFieldBehavior {
                    clearable: false,
                    requires_reauthorization: false,
                },
            },
            ConnectionFieldMeta {
                name: "private_key",
                type_name: "String",
                is_optional: true,
                display_name: Some("Private Key"),
                description: None,
                placeholder: None,
                default_value: None,
                is_secret: true,
                enum_values: None,
                is_url: false,
                is_required: false,
                control: Some(ControlKind::SecretTextarea),
                section: None,
                access: FieldAccessMode::Write,
                conditions: ConnectionFieldConditions {
                    visible: None,
                    enabled: None,
                    required: None,
                },
                behavior: ConnectionFieldBehavior {
                    clearable: false,
                    requires_reauthorization: false,
                },
            },
        ];
        let meta = ConnectionTypeMeta {
            integration_id: "fixture",
            display_name: "Fixture",
            description: None,
            category: None,
            service_id: None,
            auth_type: None,
            fields: FIELDS,
            oauth_config: None,
        };

        let definition = connection_form_definition(&meta);
        assert_eq!(
            definition.fields["environment"].section.as_deref(),
            Some("configuration")
        );
        assert!(matches!(
            definition.fields["environment"]
                .control
                .as_ref()
                .map(|control| control.kind),
            Some(ControlKind::Select)
        ));
        assert!(
            definition.fields["environment"]
                .conditions
                .visible
                .is_some()
        );
        assert!(
            !analyze_form(&definition, &json!({ "environment": "sandbox" })).fields["environment"]
                .visible
        );
        assert!(
            analyze_form(&definition, &json!({ "environment": "production" })).fields
                ["environment"]
                .visible
        );
        assert_eq!(
            definition.fields["private_key"].access,
            FieldAccessMode::Write
        );
        assert!(definition.fields["private_key"].secret);
        assert!(matches!(
            definition.fields["private_key"]
                .control
                .as_ref()
                .map(|control| control.kind),
            Some(ControlKind::SecretTextarea)
        ));
        assert!(validate_form_definition(&definition).is_empty());
    }

    #[test]
    fn workflow_schema_normalization_preserves_schema_and_upgrades_visibility() {
        let mut reason = schema(SchemaFieldType::String);
        reason.required = true;
        reason.visible_when = Some(crate::VisibleWhen {
            field: "mode".to_string(),
            equals: Some(json!("manual")),
            not_equals: None,
        });
        let source = HashMap::from([
            ("mode".to_string(), schema(SchemaFieldType::String)),
            ("reason".to_string(), reason),
        ]);

        let definition = schema_fields_form_definition(&source);
        assert!(source["reason"].visible_when.is_some());
        assert!(definition.fields["reason"].schema.visible_when.is_none());
        let analysis = analyze_form(&definition, &json!({"mode": "automatic"}));
        assert!(analysis.valid);
        assert!(!analysis.fields["reason"].visible);
        assert!(!analysis.fields["reason"].required);
    }

    #[test]
    fn validates_patterns_and_supported_formats() {
        let mut code = field(SchemaFieldType::String);
        code.schema.pattern = Some(r"^[A-Z]{3}\d{4}$".to_string());
        let mut email = field(SchemaFieldType::String);
        email.schema.format = Some("email".to_string());
        let mut date = field(SchemaFieldType::String);
        date.schema.format = Some("date".to_string());
        let definition = FormDefinition {
            fields: HashMap::from([
                ("code".to_string(), code),
                ("email".to_string(), email),
                ("date".to_string(), date),
            ]),
            ..FormDefinition::default()
        };

        assert!(
            analyze_form(
                &definition,
                &json!({"code": "ABC1234", "email": "a@b.example", "date": "2024-02-29"})
            )
            .valid
        );
        let invalid = analyze_form(
            &definition,
            &json!({"code": "abc", "email": "not-email", "date": "2023-02-29"}),
        );
        assert!(!invalid.valid);
        assert!(
            invalid
                .issues
                .iter()
                .any(|issue| issue.code == "FORM_FIELD_PATTERN_MISMATCH")
        );
        assert_eq!(
            invalid
                .issues
                .iter()
                .filter(|issue| issue.code == "FORM_FIELD_FORMAT_INVALID")
                .count(),
            2
        );

        let mut malformed = field(SchemaFieldType::String);
        malformed.schema.pattern = Some("[".to_string());
        let issues = validate_form_definition(&FormDefinition {
            fields: HashMap::from([("bad".to_string(), malformed)]),
            ..FormDefinition::default()
        });
        assert!(
            issues
                .iter()
                .any(|issue| issue.code == "FORM_PATTERN_INVALID")
        );
    }
}

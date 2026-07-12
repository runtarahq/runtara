//! Shared form schema, condition state, and value validation.
//!
//! Domain DSLs compose these types instead of defining their own field/control
//! vocabularies. Backend callers use these functions directly; browser callers
//! receive the same behavior through the validation WASM crate.

use std::collections::{HashMap, HashSet};

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
            Some(value) => {
                validate_field_value(&field.schema, value, &format!("data.{name}"), &mut issues)
            }
            None if state.required => issues.push(error(
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
        ControlKind::Number | ControlKind::NumberRange => matches!(
            field.schema.field_type,
            SchemaFieldType::Integer | SchemaFieldType::Number
        ),
        ControlKind::Toggle => matches!(field.schema.field_type, SchemaFieldType::Boolean),
        ControlKind::Tags | ControlKind::MultiSelect => {
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
}

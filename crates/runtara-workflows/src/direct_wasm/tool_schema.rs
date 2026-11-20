// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! What an AI Agent step's model is shown of each tool: its name, a
//! description, and the JSON Schema of the arguments it sends.
//!
//! A tool's arguments become what its target runs with: an Agent step's
//! capability input, or an EmbedWorkflow step's child input data. So the
//! schema is that input's, less what the model is not to give:
//!
//! - a field the host fills (`_`-prefixed, such as `_connection`; `timeout_ms`
//!   when the tool step sets a timeout);
//! - a connection (a `connection` input is a connection id, which the model
//!   cannot know);
//! - a field the tool step maps as a whole, which is a fixed argument
//!   (docs/ai-agent-tool-schemas.md).
//!
//! When the input is not known (no catalog, or a target that takes no
//! arguments), the tool is shown an empty object, as before.
//!
//! The schemas hold no boolean where a subschema goes: strict tool-schema
//! validators refuse `additionalProperties: false` / `true` ("Unrecognized
//! schema"), so such a boolean is written `{}` (anything is accepted, which is
//! what the model's arguments are today).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use runtara_dsl::agent_meta::{
    AgentCatalog, CapabilityField, CapabilityInfo, FieldTypeInfo, OutputField,
};
use runtara_dsl::{ExecutionGraph, InputMapping, SchemaField, SchemaFieldType, Step};
use serde_json::{Map, Value, json};

/// The input a child workflow takes, for an EmbedWorkflow step used as a
/// tool, by the EmbedWorkflow step's id.
#[derive(Debug, Clone, Default)]
pub(super) struct ChildToolInput {
    pub(super) name: Option<String>,
    pub(super) description: Option<String>,
    pub(super) input_schema: HashMap<String, SchemaField>,
}

pub(super) type ChildToolInputs = BTreeMap<String, ChildToolInput>;

/// The definition the model is shown for the tool `label`, whose edge leads to
/// `target`.
pub(super) fn tool_definition(
    graph: &ExecutionGraph,
    label: &str,
    target: &str,
    children: &ChildToolInputs,
    agent_catalog: Option<&AgentCatalog>,
) -> Value {
    let step = graph.steps.get(target);
    let named = |name: Option<&String>| name.filter(|name| !name.trim().is_empty()).cloned();
    let (description, parameters) = match step {
        Some(Step::Agent(agent)) => {
            let capability = agent_catalog
                .and_then(|catalog| catalog.capability(&agent.agent_id, &agent.capability_id));
            let description = named(agent.name.as_ref())
                .or_else(|| {
                    capability.and_then(|capability| named(capability.description.as_ref()))
                })
                .or_else(|| {
                    capability.and_then(|capability| named(capability.display_name.as_ref()))
                });
            let parameters = capability.map(|capability| {
                capability_parameters(
                    capability,
                    agent.timeout.is_some(),
                    &fixed_inputs(agent.input_mapping.as_ref()),
                )
            });
            (description, parameters)
        }
        Some(Step::EmbedWorkflow(embed)) => {
            let child = children.get(target);
            let description = named(embed.name.as_ref())
                .or_else(|| child.and_then(|child| named(child.description.as_ref())))
                .or_else(|| child.and_then(|child| named(child.name.as_ref())));
            let parameters = child.map(|child| {
                child_parameters(
                    &child.input_schema,
                    &fixed_inputs(embed.input_mapping.as_ref()),
                )
            });
            (description, parameters)
        }
        Some(other) => (super::manifest::step_name(other).map(str::to_owned), None),
        None => (None, None),
    };
    json!({
        "name": label,
        "description": description.unwrap_or_else(|| label.to_owned()),
        "parameters": parameters.unwrap_or_else(empty_parameters),
    })
}

/// What a tool is shown when its arguments are not known: any object.
pub(super) fn empty_parameters() -> Value {
    json!({
        "type": "object",
        "properties": {},
        // `{}` (not `true`): a bare boolean subschema is rejected by strict
        // LLM tool-schema validators ("Unrecognized schema: true"); `{}` is
        // the provider-safe equivalent.
        "additionalProperties": {}
    })
}

/// The inputs a tool step maps as a whole (`date`, not `data.table`): fixed
/// arguments, not the model's to give.
pub(super) fn fixed_inputs(mapping: Option<&InputMapping>) -> BTreeSet<String> {
    mapping
        .map(|mapping| {
            mapping
                .keys()
                .filter(|key| !key.contains('.'))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Whether the model may give a capability input. It may not give one the
/// host fills, a connection, or a fixed argument.
pub(super) fn shown_to_the_model(
    field: &CapabilityField,
    step_sets_timeout: bool,
    fixed: &BTreeSet<String>,
) -> bool {
    !(field.name.starts_with('_')
        || (step_sets_timeout && field.name == "timeout_ms")
        || field.type_name == "connection"
        || fixed.contains(&field.name))
}

fn capability_parameters(
    capability: &CapabilityInfo,
    step_sets_timeout: bool,
    fixed: &BTreeSet<String>,
) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for field in capability
        .inputs
        .iter()
        .filter(|field| shown_to_the_model(field, step_sets_timeout, fixed))
    {
        properties.insert(field.name.clone(), capability_field_schema(field));
        if field.required {
            required.push(Value::String(field.name.clone()));
        }
    }
    object_schema(properties, required)
}

fn child_parameters(
    input_schema: &HashMap<String, SchemaField>,
    fixed: &BTreeSet<String>,
) -> Value {
    let shown: HashMap<String, SchemaField> = input_schema
        .iter()
        .filter(|(name, field)| {
            !fixed.contains(*name)
                && !name.starts_with('_')
                && field.field_type != SchemaFieldType::Connection
        })
        .map(|(name, field)| (name.clone(), field.clone()))
        .collect();
    let mut schema = runtara_dsl::schema_convert::dsl_schema_to_json_schema(&shown);
    without_boolean_subschemas(&mut schema);
    // Like a capability's input, the child's data takes what the model adds.
    if let Value::Object(object) = &mut schema {
        object.insert("additionalProperties".to_string(), json!({}));
    }
    schema
}

fn object_schema(properties: Map<String, Value>, mut required: Vec<Value>) -> Value {
    let mut schema = Map::new();
    schema.insert("type".to_string(), json!("object"));
    schema.insert("properties".to_string(), Value::Object(properties));
    if !required.is_empty() {
        required.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
        schema.insert("required".to_string(), Value::Array(required));
    }
    schema.insert("additionalProperties".to_string(), json!({}));
    Value::Object(schema)
}

/// The JSON Schema of one capability input.
fn capability_field_schema(field: &CapabilityField) -> Value {
    // A field that brings its own detailed schema (an expression, a typed
    // object) is shown as that schema.
    if let Some(schema) = &field.schema {
        let mut schema = schema.clone();
        without_boolean_subschemas(&mut schema);
        if let Value::Object(object) = &mut schema
            && let Some(description) = field_description(field)
        {
            object
                .entry("description")
                .or_insert(Value::String(description));
        }
        return schema;
    }
    let mut schema = typed(&field.type_name, field.format.as_deref());
    if let Some(description) = field_description(field) {
        schema.insert("description".to_string(), Value::String(description));
    }
    if let Some(values) = &field.enum_values {
        schema.insert(
            "enum".to_string(),
            Value::Array(values.iter().cloned().map(Value::String).collect()),
        );
    }
    if let Some(default) = &field.default_value {
        schema.insert("default".to_string(), default.clone());
    }
    if let Some(items) = &field.items {
        schema.insert("items".to_string(), field_type_schema(items));
    }
    Value::Object(schema)
}

fn field_description(field: &CapabilityField) -> Option<String> {
    field
        .description
        .clone()
        .or_else(|| field.display_name.clone())
        .filter(|text| !text.trim().is_empty())
}

fn field_type_schema(info: &FieldTypeInfo) -> Value {
    let mut schema = typed(&info.type_name, info.format.as_deref());
    if let Some(description) = info
        .description
        .clone()
        .or_else(|| info.display_name.clone())
    {
        schema.insert("description".to_string(), Value::String(description));
    }
    if let Some(items) = &info.items {
        schema.insert("items".to_string(), field_type_schema(items));
    }
    if let Some(fields) = &info.fields {
        schema.insert("properties".to_string(), output_fields_schema(fields));
    }
    Value::Object(schema)
}

fn output_fields_schema(fields: &[OutputField]) -> Value {
    let mut properties = Map::new();
    for field in fields {
        let mut schema = typed(&field.type_name, field.format.as_deref());
        if let Some(description) = field
            .description
            .clone()
            .or_else(|| field.display_name.clone())
        {
            schema.insert("description".to_string(), Value::String(description));
        }
        if let Some(items) = &field.items {
            schema.insert("items".to_string(), field_type_schema(items));
        }
        if let Some(fields) = &field.fields {
            schema.insert("properties".to_string(), output_fields_schema(fields));
        }
        properties.insert(field.name.clone(), Value::Object(schema));
    }
    Value::Object(properties)
}

/// `{"type": …}` for a catalog type name. `any`, and a name JSON Schema has
/// no type for, are left untyped; a file is the file object.
fn typed(type_name: &str, format: Option<&str>) -> Map<String, Value> {
    let mut schema = Map::new();
    match type_name {
        "string" | "integer" | "number" | "boolean" | "array" | "object" => {
            schema.insert("type".to_string(), json!(type_name));
        }
        "file" => {
            schema.insert("type".to_string(), json!("object"));
            schema.insert(
                "properties".to_string(),
                json!({
                    "content": {"type": "string", "description": "The file's content, base64"},
                    "filename": {"type": "string"},
                    "mimeType": {"type": "string"}
                }),
            );
        }
        _ => {}
    }
    if let Some(format) = format.filter(|format| !format.is_empty()) {
        schema.insert("format".to_string(), json!(format));
    }
    schema
}

/// Rewrites every boolean where a subschema goes (`additionalProperties`,
/// `items`, …) as `{}`. Booleans that are values (`default`, `enum`, `const`)
/// are left alone.
pub(super) fn without_boolean_subschemas(schema: &mut Value) {
    const SUBSCHEMA: &[&str] = &[
        "additionalProperties",
        "items",
        "additionalItems",
        "contains",
        "propertyNames",
        "not",
        "if",
        "then",
        "else",
        "unevaluatedProperties",
        "unevaluatedItems",
    ];
    const SUBSCHEMA_MAPS: &[&str] = &["properties", "patternProperties", "$defs", "definitions"];
    const SUBSCHEMA_LISTS: &[&str] = &["allOf", "anyOf", "oneOf", "prefixItems"];
    let Value::Object(object) = schema else {
        return;
    };
    for (key, value) in object.iter_mut() {
        let key = key.as_str();
        if SUBSCHEMA.contains(&key) {
            if value.is_boolean() {
                *value = json!({});
            } else {
                without_boolean_subschemas(value);
            }
        } else if SUBSCHEMA_MAPS.contains(&key) {
            if let Value::Object(map) = value {
                for subschema in map.values_mut() {
                    if subschema.is_boolean() {
                        *subschema = json!({});
                    } else {
                        without_boolean_subschemas(subschema);
                    }
                }
            }
        } else if SUBSCHEMA_LISTS.contains(&key)
            && let Value::Array(list) = value
        {
            for subschema in list {
                if subschema.is_boolean() {
                    *subschema = json!({});
                } else {
                    without_boolean_subschemas(subschema);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One agent, `kit`, with a capability `send` whose inputs cover what a
    /// tool may or may not be shown.
    fn catalog() -> AgentCatalog {
        AgentCatalog::from_json(
            r#"[{
              "id": "kit", "name": "Kit", "description": "", "hasSideEffects": true,
              "supportsConnections": true, "integrationIds": [],
              "capabilities": [{
                "id": "send", "name": "send", "displayName": "Send",
                "description": "Send a message", "inputType": "SendInput",
                "output": {"type": "object"},
                "hasSideEffects": true, "isIdempotent": false,
                "rateLimited": false, "trusted": false,
                "inputs": [
                  {"name": "to", "type": "string", "required": true, "description": "Who to send to"},
                  {"name": "mode", "type": "string", "required": false, "enum": ["fast", "slow"], "default": "fast"},
                  {"name": "dryRun", "type": "boolean", "required": false, "default": false},
                  {"name": "tags", "type": "array", "required": false, "items": {"type": "string"}},
                  {"name": "anything", "type": "any", "required": false},
                  {"name": "account", "type": "connection", "required": true},
                  {"name": "timeout_ms", "type": "integer", "required": false},
                  {"name": "_connection", "type": "object", "required": true},
                  {"name": "query", "type": "object", "required": false,
                   "schema": {"type": "object", "additionalProperties": false}}
                ]
              }]
            }]"#,
        )
        .expect("test catalog parses")
    }

    fn graph(tool_step: Value, extra_steps: Value) -> ExecutionGraph {
        let mut steps = json!({
            "ai": {"id": "ai", "stepType": "AiAgent", "connectionId": "c", "config": {
                "systemPrompt": {"valueType": "immediate", "value": "s"},
                "userPrompt": {"valueType": "immediate", "value": "u"}
            }},
            "tool": tool_step,
            "finish": {"id": "finish", "stepType": "Finish"}
        });
        if let (Value::Object(steps), Value::Object(extra)) = (&mut steps, extra_steps) {
            steps.extend(extra);
        }
        serde_json::from_value(json!({
            "entryPoint": "ai",
            "steps": steps,
            "executionPlan": [
                {"fromStep": "ai", "toStep": "tool", "label": "do_it"},
                {"fromStep": "ai", "toStep": "finish", "label": "next"}
            ]
        }))
        .expect("graph parses")
    }

    fn agent_tool(extra: Value) -> Value {
        let mut step =
            json!({"id": "tool", "stepType": "Agent", "agentId": "kit", "capabilityId": "send"});
        if let (Value::Object(step), Value::Object(extra)) = (&mut step, extra) {
            step.extend(extra);
        }
        step
    }

    fn definition(graph: &ExecutionGraph, children: &ChildToolInputs) -> Value {
        tool_definition(graph, "do_it", "tool", children, Some(&catalog()))
    }

    #[test]
    fn an_agent_tool_is_shown_its_capabilitys_input() {
        let tool = definition(
            &graph(agent_tool(json!({})), json!({})),
            &ChildToolInputs::new(),
        );
        assert_eq!(tool["name"], "do_it");
        // No step name: the capability's description.
        assert_eq!(tool["description"], "Send a message");
        let parameters = &tool["parameters"];
        assert_eq!(parameters["type"], "object");
        assert_eq!(parameters["properties"]["to"]["type"], "string");
        assert_eq!(
            parameters["properties"]["to"]["description"],
            "Who to send to"
        );
        assert_eq!(
            parameters["properties"]["mode"]["enum"],
            json!(["fast", "slow"])
        );
        assert_eq!(parameters["properties"]["mode"]["default"], "fast");
        // A boolean that is a value stays.
        assert_eq!(parameters["properties"]["dryRun"]["default"], false);
        assert_eq!(parameters["properties"]["tags"]["items"]["type"], "string");
        // `any` has no type.
        assert_eq!(parameters["properties"]["anything"], json!({}));
        // A detailed schema is shown as itself, with no boolean subschema.
        assert_eq!(
            parameters["properties"]["query"],
            json!({"type": "object", "additionalProperties": {}})
        );
        assert_eq!(parameters["required"], json!(["to"]));
        assert_eq!(parameters["additionalProperties"], json!({}));
    }

    #[test]
    fn what_the_host_fills_and_connections_are_not_shown() {
        let tool = definition(
            &graph(agent_tool(json!({})), json!({})),
            &ChildToolInputs::new(),
        );
        let properties = tool["parameters"]["properties"].as_object().unwrap();
        assert!(!properties.contains_key("_connection"));
        assert!(
            !properties.contains_key("account"),
            "a connection id is not the model's"
        );
        // Without a step timeout, `timeout_ms` is the capability's to take.
        assert!(properties.contains_key("timeout_ms"));

        let tool = definition(
            &graph(agent_tool(json!({"timeout": 5000})), json!({})),
            &ChildToolInputs::new(),
        );
        assert!(
            !tool["parameters"]["properties"]
                .as_object()
                .unwrap()
                .contains_key("timeout_ms"),
            "the step's timeout fills it"
        );
    }

    #[test]
    fn a_mapped_input_is_a_fixed_argument_and_not_shown() {
        let tool = definition(
            &graph(
                agent_tool(json!({"inputMapping": {
                    "to": {"valueType": "immediate", "value": "ops"},
                    "query.limit": {"valueType": "immediate", "value": 5}
                }})),
                json!({}),
            ),
            &ChildToolInputs::new(),
        );
        let properties = tool["parameters"]["properties"].as_object().unwrap();
        assert!(!properties.contains_key("to"));
        // Mapped only in part: the model fills the rest.
        assert!(properties.contains_key("query"));
        assert!(tool["parameters"].get("required").is_none(), "{tool}");
    }

    #[test]
    fn the_step_name_describes_the_tool() {
        let tool = definition(
            &graph(
                agent_tool(json!({"name": "Page the on-call engineer"})),
                json!({}),
            ),
            &ChildToolInputs::new(),
        );
        assert_eq!(tool["description"], "Page the on-call engineer");
    }

    #[test]
    fn an_unknown_capability_or_target_is_shown_any_object() {
        let unknown = graph(
            json!({"id": "tool", "stepType": "Agent", "agentId": "kit", "capabilityId": "missing"}),
            json!({}),
        );
        assert_eq!(
            definition(&unknown, &ChildToolInputs::new())["parameters"],
            empty_parameters()
        );
        // No catalog at all.
        let known = graph(agent_tool(json!({})), json!({}));
        let tool = tool_definition(&known, "do_it", "tool", &ChildToolInputs::new(), None);
        assert_eq!(tool["parameters"], empty_parameters());
        assert_eq!(tool["description"], "do_it");
        // A wait takes no arguments.
        let wait = graph(
            json!({"id": "tool", "stepType": "WaitForSignal", "name": "Ask a person"}),
            json!({}),
        );
        let tool = definition(&wait, &ChildToolInputs::new());
        assert_eq!(tool["parameters"], empty_parameters());
        assert_eq!(tool["description"], "Ask a person");
    }

    #[test]
    fn an_embedded_workflow_tool_is_shown_its_childs_input() {
        let embed = graph(
            json!({"id": "tool", "stepType": "EmbedWorkflow", "childWorkflowId": "wf-weather", "childVersion": "latest",
                   "inputMapping": {"units": {"valueType": "immediate", "value": "metric"}}}),
            json!({}),
        );
        let input_schema: HashMap<String, SchemaField> = serde_json::from_value(json!({
            "city": {"type": "string", "required": true, "description": "City name"},
            "units": {"type": "string", "required": false},
            "account": {"type": "connection", "required": true},
            "where": {"type": "object", "required": false, "properties": {
                "lat": {"type": "number", "required": true}
            }}
        }))
        .expect("schema parses");
        let children = ChildToolInputs::from([(
            "tool".to_string(),
            ChildToolInput {
                name: Some("Weather".to_string()),
                description: Some("Today's weather for a city".to_string()),
                input_schema,
            },
        )]);
        let tool = definition(&embed, &children);
        assert_eq!(tool["description"], "Today's weather for a city");
        let parameters = &tool["parameters"];
        assert_eq!(parameters["properties"]["city"]["type"], "string");
        assert_eq!(
            parameters["properties"]["where"]["properties"]["lat"]["type"],
            "number"
        );
        assert_eq!(parameters["required"], json!(["city"]));
        let properties = parameters["properties"].as_object().unwrap();
        assert!(!properties.contains_key("units"), "mapped: fixed");
        assert!(
            !properties.contains_key("account"),
            "a connection is not the model's"
        );
        // `dsl_schema_to_json_schema` writes `additionalProperties: false`;
        // no boolean subschema is left anywhere.
        let mut cleaned = parameters.clone();
        without_boolean_subschemas(&mut cleaned);
        assert_eq!(&cleaned, parameters);
        assert_eq!(parameters["additionalProperties"], json!({}));
    }

    #[test]
    fn booleans_that_are_values_survive_cleaning() {
        let mut schema = json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "on": {"type": "boolean", "default": true, "enum": [true, false]},
                "list": {"type": "array", "items": true},
                "either": {"anyOf": [false, {"type": "string"}]}
            }
        });
        without_boolean_subschemas(&mut schema);
        assert_eq!(
            schema,
            json!({
                "type": "object",
                "additionalProperties": {},
                "properties": {
                    "on": {"type": "boolean", "default": true, "enum": [true, false]},
                    "list": {"type": "array", "items": {}},
                    "either": {"anyOf": [{}, {"type": "string"}]}
                }
            })
        );
    }
}

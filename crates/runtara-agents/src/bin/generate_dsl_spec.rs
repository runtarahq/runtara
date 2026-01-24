// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! CLI tool to generate the complete DSL specification for LLM scenario generation.
//!
//! This binary outputs a combined specification that includes:
//! - DSL JSON Schema (workflow structure, steps, conditions, mappings)
//! - Agent specifications (available agents, capabilities, inputs/outputs)
//! - Connection types (authentication methods)
//!
//! Usage:
//!   cargo run -p runtara-agents --bin generate_dsl_spec > dsl_spec.json
//!   cargo run -p runtara-agents --bin generate_dsl_spec -- --format markdown > dsl_spec.md
//!   cargo run -p runtara-agents --bin generate_dsl_spec -- --schema-only > schema.json
//!   cargo run -p runtara-agents --bin generate_dsl_spec -- --agents-only > agents.json

use runtara_dsl::DSL_VERSION;
use runtara_dsl::agent_meta::{
    get_agents, get_all_connection_types, validate_agent_metadata_or_panic,
};
use runtara_dsl::spec::{AGENT_VERSION, generate_agent_openapi_spec, generate_dsl_schema};
use serde_json::{Value, json};
use std::env;

// Force linking of runtara_agents to ensure all inventory registrations are included
extern crate runtara_agents;

fn main() {
    // Validate agent metadata before generating specs
    validate_agent_metadata_or_panic();

    let args: Vec<String> = env::args().collect();

    let format = args
        .iter()
        .position(|a| a == "--format")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
        .unwrap_or("json");

    let schema_only = args.iter().any(|a| a == "--schema-only");
    let agents_only = args.iter().any(|a| a == "--agents-only");
    let openapi_only = args.iter().any(|a| a == "--openapi-only");
    let help = args.iter().any(|a| a == "--help" || a == "-h");

    if help {
        print_help();
        return;
    }

    if schema_only {
        let schema = generate_dsl_schema();
        println!("{}", serde_json::to_string_pretty(&schema).unwrap());
        return;
    }

    if agents_only {
        let agents = get_agents();
        let agents_json: Vec<Value> = agents
            .iter()
            .map(|a| serde_json::to_value(a).unwrap())
            .collect();
        println!("{}", serde_json::to_string_pretty(&agents_json).unwrap());
        return;
    }

    if openapi_only {
        let agents = get_agents();
        let agents_json: Vec<Value> = agents
            .iter()
            .map(|a| serde_json::to_value(a).unwrap())
            .collect();
        let openapi = generate_agent_openapi_spec(agents_json);
        println!("{}", serde_json::to_string_pretty(&openapi).unwrap());
        return;
    }

    // Generate combined specification
    let spec = generate_combined_spec();

    match format {
        "markdown" | "md" => {
            println!("{}", generate_markdown(&spec));
        }
        _ => {
            println!("{}", serde_json::to_string_pretty(&spec).unwrap());
        }
    }
}

fn print_help() {
    eprintln!(
        r#"Generate DSL Specification for LLM Scenario Generation

Usage: generate_dsl_spec [OPTIONS]

Options:
  --format <FORMAT>  Output format: json (default) or markdown
  --schema-only      Output only the DSL JSON Schema
  --agents-only      Output only the agents specification
  --openapi-only     Output only the OpenAPI specification
  -h, --help         Show this help message

Examples:
  # Generate complete spec as JSON
  cargo run -p runtara-agents --bin generate_dsl_spec > dsl_spec.json

  # Generate as markdown for documentation
  cargo run -p runtara-agents --bin generate_dsl_spec -- --format markdown > DSL.md

  # Generate just the schema
  cargo run -p runtara-agents --bin generate_dsl_spec -- --schema-only > schema.json
"#
    );
}

fn generate_combined_spec() -> Value {
    let dsl_schema = generate_dsl_schema();
    let agents = get_agents();
    let agents_json: Vec<Value> = agents
        .iter()
        .map(|a| serde_json::to_value(a).unwrap())
        .collect();

    // Collect connection types
    let connection_types: Vec<Value> = get_all_connection_types()
        .map(|ct| {
            let fields: Vec<Value> = ct
                .fields
                .iter()
                .map(|f| {
                    json!({
                        "name": f.name,
                        "type": f.type_name,
                        "required": !f.is_optional,
                        "displayName": f.display_name,
                        "description": f.description,
                        "placeholder": f.placeholder,
                        "default": f.default_value,
                        "isSecret": f.is_secret
                    })
                })
                .collect();

            json!({
                "integrationId": ct.integration_id,
                "displayName": ct.display_name,
                "description": ct.description,
                "category": ct.category,
                "fields": fields
            })
        })
        .collect();

    json!({
        "version": {
            "dsl": DSL_VERSION,
            "agents": AGENT_VERSION
        },
        "dslSchema": dsl_schema,
        "agents": agents_json,
        "connectionTypes": connection_types
    })
}

fn generate_markdown(spec: &Value) -> String {
    let mut md = String::new();

    md.push_str("# Runtara DSL Specification\n\n");
    md.push_str(&format!(
        "DSL Version: {} | Agent Version: {}\n\n",
        spec["version"]["dsl"].as_str().unwrap_or("unknown"),
        spec["version"]["agents"].as_str().unwrap_or("unknown")
    ));

    // Overview
    md.push_str("## Overview\n\n");
    md.push_str("This specification defines the Runtara workflow DSL for creating durable execution scenarios.\n\n");

    // Step Types
    md.push_str("## Step Types\n\n");
    if let Some(step_types) = spec["dslSchema"]["x-step-types"].as_array() {
        for step in step_types {
            let step_type = step["type"].as_str().unwrap_or("Unknown");
            let description = step["description"].as_str().unwrap_or("");
            let category = step["category"].as_str().unwrap_or("unknown");

            md.push_str(&format!("### {}\n\n", step_type));
            md.push_str(&format!("**Category:** {}\n\n", category));
            md.push_str(&format!("{}\n\n", description));
        }
    }

    // Agents
    md.push_str("## Agents\n\n");
    if let Some(agents) = spec["agents"].as_array() {
        for agent in agents {
            let id = agent["id"].as_str().unwrap_or("unknown");
            let name = agent["name"].as_str().unwrap_or("Unknown");
            let description = agent["description"].as_str().unwrap_or("");

            md.push_str(&format!("### {} ({})\n\n", name, id));
            md.push_str(&format!("{}\n\n", description));

            if let Some(caps) = agent["capabilities"].as_array() {
                md.push_str("#### Capabilities\n\n");
                for cap in caps {
                    let cap_id = cap["id"].as_str().unwrap_or("unknown");
                    let cap_desc = cap["description"].as_str().unwrap_or("");

                    md.push_str(&format!("- **{}**: {}\n", cap_id, cap_desc));

                    // Input fields
                    if let Some(inputs) = cap["inputs"].as_array()
                        && !inputs.is_empty()
                    {
                        md.push_str("  - Inputs:\n");
                        for input in inputs {
                            let input_name = input["name"].as_str().unwrap_or("?");
                            let input_type = input["type"].as_str().unwrap_or("any");
                            let required = input["required"].as_bool().unwrap_or(false);
                            let req_str = if required { "required" } else { "optional" };
                            md.push_str(&format!(
                                "    - `{}` ({}, {})\n",
                                input_name, input_type, req_str
                            ));
                        }
                    }
                }
                md.push('\n');
            }
        }
    }

    // Connection Types
    md.push_str("## Connection Types\n\n");
    if let Some(connection_types) = spec["connectionTypes"].as_array() {
        for ct in connection_types {
            let id = ct["integrationId"].as_str().unwrap_or("unknown");
            let name = ct["displayName"].as_str().unwrap_or("Unknown");
            let description = ct["description"].as_str().unwrap_or("");

            md.push_str(&format!("### {} ({})\n\n", name, id));
            if !description.is_empty() {
                md.push_str(&format!("{}\n\n", description));
            }

            if let Some(fields) = ct["fields"].as_array() {
                md.push_str("**Fields:**\n\n");
                for field in fields {
                    let field_name = field["name"].as_str().unwrap_or("?");
                    let field_type = field["type"].as_str().unwrap_or("string");
                    let required = field["required"].as_bool().unwrap_or(false);
                    let is_secret = field["isSecret"].as_bool().unwrap_or(false);
                    let req_str = if required { "required" } else { "optional" };
                    let secret_str = if is_secret { ", secret" } else { "" };

                    md.push_str(&format!(
                        "- `{}` ({}, {}{})\n",
                        field_name, field_type, req_str, secret_str
                    ));
                }
                md.push('\n');
            }
        }
    }

    // JSON Schema reference
    md.push_str("## JSON Schema\n\n");
    md.push_str(
        "The complete JSON Schema is available in the `dslSchema` field of the JSON output.\n\n",
    );
    md.push_str("```bash\n");
    md.push_str(
        "cargo run -p runtara-agents --bin generate_dsl_spec -- --schema-only > schema.json\n",
    );
    md.push_str("```\n");

    md
}

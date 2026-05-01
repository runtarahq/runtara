//! Metadata API endpoints
//!
//! Provides endpoints for workflow metadata including step types and step-specific metadata

use axum::{extract::Query, http::StatusCode, response::Json};
use serde::Deserialize;
use serde_json::{Value, json};
use utoipa::ToSchema;

#[derive(Debug, serde::Serialize, serde::Deserialize, ToSchema)]
pub struct NotImplementedResponse {
    pub success: bool,
    pub message: String,
    pub endpoint: String,
    pub status: u16,
}

/// Get all available workflow step types
#[utoipa::path(
    get,
    path = "/api/runtime/metadata/workflow/step-types",
    tag = "workflow-step-type-api",
    responses(
        (status = 200, description = "Step types retrieved successfully"),
    )
)]
pub async fn get_workflow_step_types_handler() -> (StatusCode, Json<Value>) {
    // Return available step types based on the compiler's supported types
    let step_types = vec![
        json!({
            "type": "Start",
            "name": "Start",
            "description": "Entry point that initializes workflow with input data",
            "category": "control",
            "icon": "play-circle",
            "inputSchema": {
                "data": {
                    "type": "json",
                    "description": "Initial data for workflow (optional, defaults to workflow.inputs.data)",
                    "required": false
                },
                "variables": {
                    "type": "json",
                    "description": "Initial variables for workflow (optional, defaults to workflow.inputs.variables)",
                    "required": false
                }
            },
            "outputSchema": {
                "data": {
                    "type": "json",
                    "description": "Initialized data"
                },
                "variables": {
                    "type": "json",
                    "description": "Initialized variables"
                },
                "status": {
                    "type": "text",
                    "description": "Status of initialization"
                }
            }
        }),
        json!({
            "type": "Finish",
            "name": "Finish",
            "description": "Final step that returns workflow outputs",
            "category": "control",
            "icon": "stop-circle",
            "inputSchema": {
                "outputs": {
                    "type": "json",
                    "description": "The output structure to return (mapped from previous steps)",
                    "required": true
                }
            },
            "outputSchema": {
                "description": "Returns the mapped outputs directly (not wrapped)"
            }
        }),
        json!({
            "type": "Conditional",
            "name": "Conditional",
            "description": "Conditional branching based on expression evaluation",
            "category": "control",
            "icon": "git-branch",
            "hasConditions": true,
            "inputSchema": {
                "condition.expression.op": {
                    "type": "text",
                    "description": "Comparison operator (EQ, NE, GT, LT, GTE, LTE, IN, CONTAINS, AND, OR, NOT, IS_DEFINED)",
                    "required": true,
                    "enum": ["EQ", "NE", "GT", "LT", "GTE", "LTE", "IN", "CONTAINS", "AND", "OR", "NOT", "IS_DEFINED"]
                },
                "condition.expression.arguments[N]": {
                    "type": "any",
                    "description": "Arguments for the operator (N = 0, 1, 2, ...)",
                    "required": true
                }
            },
            "outputSchema": {
                "description": "Boolean result of condition evaluation (true/false)"
            }
        }),
        json!({
            "type": "Switch",
            "name": "Switch",
            "description": "Match a value against multiple cases and return corresponding output. Supports exact matching, array membership (IN), and range comparisons.",
            "category": "control",
            "icon": "git-branch",
            "inputSchema": {
                "value": {
                    "type": "text | int | double | boolean | json",
                    "description": "The value to match against cases",
                    "required": true
                },
                "cases": {
                    "type": "json",
                    "description": "Array of case objects with matchType, match value, and output",
                    "required": true,
                    "structure": {
                        "matchType": {
                            "type": "text",
                            "description": "Type of match to perform",
                            "required": true,
                            "enum": ["exact", "in", "gt", "gte", "lt", "lte", "between", "range"],
                            "details": {
                                "exact": "Equality check (match: primitive value)",
                                "in": "Value in array (match: array of values)",
                                "gt": "Greater than (match: number or string)",
                                "gte": "Greater than or equal (match: number or string)",
                                "lt": "Less than (match: number or string)",
                                "lte": "Less than or equal (match: number or string)",
                                "between": "Inclusive range (match: [min, max] array)",
                                "range": "Custom range with operators (match: {gte?, gt?, lte?, lt?} object)"
                            }
                        },
                        "match": {
                            "type": "any",
                            "description": "Value(s) to match against. Shape depends on matchType"
                        },
                        "output": {
                            "type": "json",
                            "description": "Object to return when this case matches"
                        }
                    },
                    "examples": [
                        {"matchType": "exact", "match": "US", "output": {"zone": "NA"}},
                        {"matchType": "in", "match": ["DE", "FR", "IT"], "output": {"zone": "EU"}},
                        {"matchType": "between", "match": [100, 500], "output": {"tier": "mid"}},
                        {"matchType": "gte", "match": 1000, "output": {"tier": "premium"}},
                        {"matchType": "range", "match": {"gte": 0, "lt": 100}, "output": {"tier": "basic"}}
                    ]
                },
                "default": {
                    "type": "json",
                    "description": "Fallback output when no cases match",
                    "required": true
                }
            },
            "outputSchema": {
                "outputs": {
                    "type": "json",
                    "description": "The output object from the first matching case (or default)"
                }
            }
        }),
        json!({
            "type": "Agent",
            "name": "Agent / Operator",
            "description": "Execute an operator function",
            "category": "operation",
            "icon": "settings",
            "requiresOperator": true,
            "inputSchema": {
                "description": "Input schema depends on the specific operator and operation",
                "dynamic": true
            },
            "outputSchema": {
                "description": "Output schema depends on the specific operator and operation",
                "dynamic": true
            }
        }),
        json!({
            "type": "Split",
            "name": "Split",
            "description": "Iterate over array elements and execute subgraph for each",
            "category": "control",
            "icon": "repeat",
            "hasIterator": true,
            "inputSchema": {
                "array": {
                    "type": "json",
                    "description": "The array to iterate over",
                    "required": true
                },
                "subgraph": {
                    "type": "json",
                    "description": "The subgraph definition to execute for each element",
                    "required": true
                }
            },
            "outputSchema": {
                "outputs": {
                    "type": "json",
                    "description": "Array of outputs from each iteration"
                }
            }
        }),
        json!({
            "type": "EmbedWorkflow",
            "name": "Start Workflow",
            "description": "Start a sub-workflow execution",
            "category": "control",
            "icon": "play",
            "inputSchema": {
                "description": "Inputs for the sub-workflow (mapped from parent workflow)",
                "dynamic": true
            },
            "outputSchema": {
                "result": {
                    "type": "text",
                    "description": "Result from sub-workflow execution"
                }
            }
        }),
    ];

    let response = json!({
        "success": true,
        "stepTypes": step_types,
        "count": step_types.len(),
        "timestamp": chrono::Utc::now().to_rfc3339()
    });
    (StatusCode::OK, Json(response))
}

#[derive(Debug, Deserialize)]
pub struct LlmModelsQuery {
    pub provider: Option<String>,
}

/// Get static LLM model metadata for AI Agent configuration.
pub async fn get_llm_models_handler(
    Query(query): Query<LlmModelsQuery>,
) -> (StatusCode, Json<Value>) {
    match query.provider.as_deref().unwrap_or("bedrock") {
        "bedrock" | "aws_credentials" => {
            let catalog = runtara_ai::providers::bedrock_models::catalog_json();
            (StatusCode::OK, Json(catalog))
        }
        "openai" | "openai_api_key" => (
            StatusCode::OK,
            Json(json!({
                "generatedAt": null,
                "source": "static OpenAI AI Agent defaults",
                "models": [
                    {"provider": "OpenAI", "modelName": "GPT-4.1", "modelId": "gpt-4.1", "recommendedForAiAgent": true},
                    {"provider": "OpenAI", "modelName": "GPT-4.1 Mini", "modelId": "gpt-4.1-mini", "recommendedForAiAgent": true},
                    {"provider": "OpenAI", "modelName": "GPT-4o", "modelId": "gpt-4o", "recommendedForAiAgent": true},
                    {"provider": "OpenAI", "modelName": "GPT-4o Mini", "modelId": "gpt-4o-mini", "recommendedForAiAgent": true}
                ]
            })),
        ),
        other => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": format!("Unsupported LLM provider: {}", other),
                "models": []
            })),
        ),
    }
}

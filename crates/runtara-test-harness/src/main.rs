// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runtara Test Harness
//!
//! A binary for testing agent capabilities in isolation. This binary links against
//! `runtara-workflow-stdlib` (which includes all agents) and can execute any
//! capability given an agent ID, capability ID, and input.
//!
//! The test harness runs inside OCI containers, matching the production
//! workflow execution environment.
//!
//! ## Input Format (via `INPUT_JSON` env var)
//!
//! ```json
//! {
//!   "agent_id": "http",
//!   "capability_id": "http-request",
//!   "input": {
//!     "url": "/api/users",
//!     "method": "GET"
//!   },
//!   "connection": {
//!     "integration_id": "bearer",
//!     "parameters": {
//!       "base_url": "https://api.example.com",
//!       "token": "secret"
//!     }
//!   }
//! }
//! ```
//!
//! ## Output Format (written to `output.json`)
//!
//! Same format as workflow instances (`InstanceOutput`).

use runtara_dsl::agent_meta::{clear_current_input, set_current_input};
use runtara_workflow_stdlib::prelude::*;
use serde::{Deserialize, Serialize};
use std::process::ExitCode;

/// Test request input format.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TestRequest {
    /// Agent module name (e.g., "http", "utils", "transform")
    agent_id: String,

    /// Capability ID (e.g., "http-request", "random-double")
    capability_id: String,

    /// Capability input as JSON
    input: serde_json::Value,

    /// Optional connection credentials
    #[serde(default)]
    connection: Option<serde_json::Value>,
}

fn main() -> ExitCode {
    // Build tokio runtime (same as workflows)
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("Failed to create tokio runtime: {}", e);
            let _ = write_failed(format!("Failed to create runtime: {}", e));
            return ExitCode::FAILURE;
        }
    };

    rt.block_on(async_main())
}

async fn async_main() -> ExitCode {
    // Parse INPUT_JSON from environment
    let input_json = match std::env::var("INPUT_JSON") {
        Ok(json) => json,
        Err(_) => {
            let _ = write_failed("INPUT_JSON environment variable not set");
            return ExitCode::FAILURE;
        }
    };

    let request: TestRequest = match serde_json::from_str(&input_json) {
        Ok(req) => req,
        Err(e) => {
            let _ = write_failed(format!("Failed to parse INPUT_JSON: {}", e));
            return ExitCode::FAILURE;
        }
    };

    // Execute the capability
    match execute_capability_test(&request).await {
        Ok(output) => {
            let _ = write_completed(output);
            ExitCode::SUCCESS
        }
        Err(error) => {
            let _ = write_failed(&error);
            ExitCode::FAILURE
        }
    }
}

/// Execute a capability test.
async fn execute_capability_test(
    request: &TestRequest,
) -> std::result::Result<serde_json::Value, String> {
    // Build input with connection injected as _connection field
    let mut input = request.input.clone();
    if let Some(conn) = &request.connection {
        if let serde_json::Value::Object(ref mut map) = input {
            map.insert("_connection".to_string(), conn.clone());
        }
    }

    // Set current input for connection resolution (thread-local storage)
    set_current_input(&input);

    // Execute the capability via the registry
    let result = registry::execute_capability(&request.agent_id, &request.capability_id, input);

    // Clear thread-local input
    clear_current_input();

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_test_request() {
        let json = r#"{
            "agent_id": "utils",
            "capability_id": "random-double",
            "input": {}
        }"#;

        let request: TestRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.agent_id, "utils");
        assert_eq!(request.capability_id, "random-double");
        assert!(request.connection.is_none());
    }

    #[test]
    fn test_parse_test_request_with_connection() {
        let json = r#"{
            "agent_id": "http",
            "capability_id": "http-request",
            "input": {"url": "/api", "method": "GET"},
            "connection": {
                "integration_id": "bearer",
                "parameters": {"base_url": "https://api.example.com", "token": "secret"}
            }
        }"#;

        let request: TestRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.agent_id, "http");
        assert!(request.connection.is_some());
    }

    #[test]
    fn test_connection_injection() {
        let request = TestRequest {
            agent_id: "test".to_string(),
            capability_id: "test-cap".to_string(),
            input: json!({"key": "value"}),
            connection: Some(json!({"integration_id": "test"})),
        };

        // Simulate connection injection
        let mut input = request.input.clone();
        if let Some(conn) = &request.connection {
            if let serde_json::Value::Object(ref mut map) = input {
                map.insert("_connection".to_string(), conn.clone());
            }
        }

        // Verify _connection was injected
        assert!(input.get("_connection").is_some());
        assert_eq!(input["key"], "value");
        assert_eq!(input["_connection"]["integration_id"], "test");
    }

    #[test]
    fn test_connection_injection_no_connection() {
        let request = TestRequest {
            agent_id: "test".to_string(),
            capability_id: "test-cap".to_string(),
            input: json!({"key": "value"}),
            connection: None,
        };

        // Simulate connection injection
        let mut input = request.input.clone();
        if let Some(conn) = &request.connection {
            if let serde_json::Value::Object(ref mut map) = input {
                map.insert("_connection".to_string(), conn.clone());
            }
        }

        // Verify _connection was NOT injected
        assert!(input.get("_connection").is_none());
        assert_eq!(input["key"], "value");
    }

    #[tokio::test]
    async fn test_execute_utils_random_double() {
        let request = TestRequest {
            agent_id: "utils".to_string(),
            capability_id: "random-double".to_string(),
            input: json!({}),
            connection: None,
        };

        let result = execute_capability_test(&request).await;
        assert!(result.is_ok());

        let value = result.unwrap();
        // random-double returns a number between 0 and 1
        let num = value.as_f64().unwrap();
        assert!(num >= 0.0 && num <= 1.0);
    }

    #[tokio::test]
    async fn test_execute_transform_extract() {
        let request = TestRequest {
            agent_id: "transform".to_string(),
            capability_id: "extract".to_string(),
            input: json!({
                "value": [
                    {"name": "Alice", "age": 30},
                    {"name": "Bob", "age": 25}
                ],
                "property_path": "name"
            }),
            connection: None,
        };

        let result = execute_capability_test(&request).await;
        assert!(result.is_ok());

        let output = result.unwrap();
        assert_eq!(output["count"], 2);
        assert_eq!(output["values"][0], "Alice");
        assert_eq!(output["values"][1], "Bob");
    }

    #[tokio::test]
    async fn test_execute_unknown_capability() {
        let request = TestRequest {
            agent_id: "nonexistent".to_string(),
            capability_id: "fake-capability".to_string(),
            input: json!({}),
            connection: None,
        };

        let result = execute_capability_test(&request).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_serialize_test_request() {
        let request = TestRequest {
            agent_id: "utils".to_string(),
            capability_id: "random-double".to_string(),
            input: json!({"min": 0, "max": 100}),
            connection: Some(json!({"integration_id": "test"})),
        };

        let serialized = serde_json::to_string(&request).unwrap();
        let deserialized: TestRequest = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized.agent_id, request.agent_id);
        assert_eq!(deserialized.capability_id, request.capability_id);
        assert_eq!(deserialized.input, request.input);
        assert_eq!(deserialized.connection, request.connection);
    }
}

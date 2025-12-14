// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Error Example - Demonstrates error handling and failure scenarios.
//!
//! This example shows:
//! - Sending failed events with error messages
//! - Error recovery strategies (retry with backoff)
//! - Timeout handling
//! - Different failure modes and how to handle them
//!
//! Run with: cargo run -p durable-example --bin error_example

use runtara_sdk::RuntaraSdk;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{error, info, warn};

/// State tracking retry attempts.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RetryState {
    /// Current operation being attempted
    operation: String,
    /// Number of retry attempts made
    attempt: u32,
    /// Maximum retry attempts
    max_attempts: u32,
    /// Last error message
    last_error: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    info!("=== Error Example: Error Handling & Recovery ===");

    // Instance IDs can be any non-empty string - descriptive names are encouraged
    let instance_id = format!("error-example-{}", uuid::Uuid::new_v4());
    let tenant_id = "demo-tenant";

    info!(instance_id = %instance_id, "Creating SDK instance");

    let mut sdk = match RuntaraSdk::localhost(&instance_id, tenant_id) {
        Ok(sdk) => sdk,
        Err(e) => {
            warn!("Failed to create SDK: {}. Running in demo mode.", e);
            demonstrate_error_handling();
            return Ok(());
        }
    };

    // Connect to runtara-core
    match sdk.connect().await {
        Ok(_) => info!("Connected to runtara-core"),
        Err(e) => {
            warn!("Failed to connect: {}. Running in demo mode.", e);
            demonstrate_error_handling();
            return Ok(());
        }
    }

    sdk.register(None).await?;

    // Demonstrate different error scenarios
    info!("--- Scenario 1: Recoverable Error with Retry ---");
    match scenario_recoverable_error(&sdk).await {
        Ok(result) => info!("Scenario 1 succeeded: {}", result),
        Err(e) => error!("Scenario 1 failed: {}", e),
    }

    info!("--- Scenario 2: Unrecoverable Error ---");
    match scenario_unrecoverable_error(&sdk).await {
        Ok(_) => info!("Scenario 2 succeeded (unexpected)"),
        Err(e) => {
            error!("Scenario 2 failed (expected): {}", e);
            // In a real workflow, we would call sdk.failed() here
            // sdk.failed(&e.to_string()).await?;
        }
    }

    info!("--- Scenario 3: Timeout Handling ---");
    match scenario_timeout_handling().await {
        Ok(result) => info!("Scenario 3 succeeded: {}", result),
        Err(e) => error!("Scenario 3 failed: {}", e),
    }

    info!("--- Scenario 4: Partial Failure with Checkpoint ---");
    match scenario_partial_failure(&sdk).await {
        Ok(result) => info!("Scenario 4 completed: {}", result),
        Err(e) => error!("Scenario 4 failed: {}", e),
    }

    // Complete with summary
    let output = serde_json::json!({
        "status": "completed",
        "scenarios_demonstrated": 4,
        "message": "Error handling demonstration complete"
    });
    let output_bytes = serde_json::to_vec(&output)?;

    sdk.completed(&output_bytes).await?;

    info!("=== Error Example Complete ===");

    Ok(())
}

/// Scenario 1: Recoverable error with exponential backoff retry.
async fn scenario_recoverable_error(
    _sdk: &RuntaraSdk,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut state = RetryState {
        operation: "external_api_call".to_string(),
        attempt: 0,
        max_attempts: 3,
        last_error: None,
    };

    loop {
        state.attempt += 1;
        info!(
            attempt = state.attempt,
            max = state.max_attempts,
            "Attempting operation"
        );

        // Simulate an operation that might fail
        match simulate_flaky_operation(state.attempt).await {
            Ok(result) => {
                info!("Operation succeeded on attempt {}", state.attempt);
                return Ok(result);
            }
            Err(e) => {
                state.last_error = Some(e.to_string());
                warn!(
                    attempt = state.attempt,
                    error = %e,
                    "Operation failed"
                );

                if state.attempt >= state.max_attempts {
                    // Max retries reached - fail permanently
                    return Err(format!(
                        "Operation failed after {} attempts: {}",
                        state.attempt, e
                    )
                    .into());
                }

                // Calculate exponential backoff delay
                let delay_ms = 100 * 2u64.pow(state.attempt - 1);
                info!(delay_ms = delay_ms, "Retrying after backoff");

                // In a real scenario, use sdk.sleep() for durable sleep
                // For short retries, tokio::sleep is fine
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
        }
    }
}

/// Simulates an operation that fails the first 2 times, succeeds on the 3rd.
async fn simulate_flaky_operation(attempt: u32) -> Result<String, Box<dyn std::error::Error>> {
    tokio::time::sleep(Duration::from_millis(100)).await;

    if attempt < 3 {
        Err(format!("Transient error (attempt {})", attempt).into())
    } else {
        Ok("Success after retries".to_string())
    }
}

/// Scenario 2: Unrecoverable error that should fail the workflow.
async fn scenario_unrecoverable_error(
    _sdk: &RuntaraSdk,
) -> Result<String, Box<dyn std::error::Error>> {
    // Simulate an unrecoverable error (e.g., invalid input, permission denied)
    let result: Result<String, _> = Err("Permission denied: cannot access resource");

    match result {
        Ok(v) => Ok(v),
        Err(e) => {
            // This is an unrecoverable error - no point retrying
            // Log and propagate the error
            error!(error = e, "Unrecoverable error encountered");

            // In a real workflow:
            // sdk.failed(&format!("Unrecoverable error: {}", e)).await?;
            // return Err(...);

            Err(e.into())
        }
    }
}

/// Scenario 3: Timeout handling for long-running operations.
async fn scenario_timeout_handling() -> Result<String, Box<dyn std::error::Error>> {
    let timeout_duration = Duration::from_millis(500);

    info!(
        timeout_ms = timeout_duration.as_millis(),
        "Starting operation with timeout"
    );

    // Use tokio::timeout to wrap potentially long operations
    match tokio::time::timeout(timeout_duration, simulate_slow_operation()).await {
        Ok(result) => {
            // Operation completed within timeout
            result
        }
        Err(_) => {
            // Timeout exceeded
            warn!("Operation timed out after {:?}", timeout_duration);

            // Options:
            // 1. Retry with longer timeout
            // 2. Use cached/default value
            // 3. Fail the workflow

            // For this demo, we'll use a fallback value
            Ok("Fallback value (operation timed out)".to_string())
        }
    }
}

/// Simulates a slow operation that takes 200ms.
async fn simulate_slow_operation() -> Result<String, Box<dyn std::error::Error>> {
    tokio::time::sleep(Duration::from_millis(200)).await;
    Ok("Slow operation completed".to_string())
}

/// Scenario 4: Partial failure with checkpoint for recovery.
async fn scenario_partial_failure(sdk: &RuntaraSdk) -> Result<String, Box<dyn std::error::Error>> {
    let items = vec!["item-1", "item-2", "item-3", "item-4", "item-5"];
    let mut processed = Vec::new();
    let mut failed_items = Vec::new();

    for (i, item) in items.iter().enumerate() {
        info!(item = %item, "Processing item");

        // Simulate some items failing
        let result = if *item == "item-3" {
            Err("Simulated failure for item-3")
        } else {
            Ok(format!("Processed {}", item))
        };

        match result {
            Ok(output) => {
                processed.push(output);

                // Checkpoint after each successful item
                let checkpoint_id = format!("after-{}", item);
                let state = serde_json::json!({
                    "processed": processed,
                    "failed": failed_items,
                    "current_index": i + 1,
                });
                let state_bytes = serde_json::to_vec(&state)?;
                let _ = sdk.checkpoint(&checkpoint_id, &state_bytes).await?;
            }
            Err(e) => {
                warn!(item = %item, error = %e, "Item processing failed");
                failed_items.push((*item, e.to_string()));

                // Continue processing other items (partial failure tolerance)
                // Alternative: fail fast by returning error here
            }
        }
    }

    // Report final status
    let summary = format!(
        "Processed {} items, {} failed",
        processed.len(),
        failed_items.len()
    );

    if !failed_items.is_empty() {
        warn!(
            failed_count = failed_items.len(),
            "Some items failed but workflow continued"
        );
    }

    Ok(summary)
}

/// Demonstrates error handling without an actual SDK connection.
fn demonstrate_error_handling() {
    println!("\n--- Demo Mode: Error Handling Patterns ---\n");

    println!("Pattern 1: Retry with Exponential Backoff");
    println!("  let mut attempt = 0;");
    println!("  loop {{");
    println!("      attempt += 1;");
    println!("      match operation().await {{");
    println!("          Ok(result) => return Ok(result),");
    println!("          Err(e) if attempt < max_attempts => {{");
    println!("              let delay = 100 * 2u64.pow(attempt - 1);");
    println!("              tokio::time::sleep(Duration::from_millis(delay)).await;");
    println!("          }}");
    println!("          Err(e) => {{");
    println!("              sdk.failed(&e.to_string()).await?;");
    println!("              return Err(e);");
    println!("          }}");
    println!("      }}");
    println!("  }}\n");

    println!("Pattern 2: Unrecoverable Error");
    println!("  if let Err(e) = validate_input(&data) {{");
    println!("      sdk.failed(&format!(\"Invalid input: {{}}\", e)).await?;");
    println!("      return Err(e);");
    println!("  }}\n");

    println!("Pattern 3: Timeout Handling");
    println!("  match tokio::time::timeout(Duration::from_secs(30), operation()).await {{");
    println!("      Ok(result) => result,");
    println!("      Err(_) => {{");
    println!("          sdk.failed(\"Operation timed out\").await?;");
    println!("          return Err(TimeoutError);");
    println!("      }}");
    println!("  }}\n");

    println!("Pattern 4: Partial Failure with Continue");
    println!("  let mut failed_items = Vec::new();");
    println!("  for item in items {{");
    println!("      match process(item).await {{");
    println!("          Ok(_) => {{ /* checkpoint */ }}");
    println!("          Err(e) => {{");
    println!("              failed_items.push((item, e));");
    println!("              continue;  // Process remaining items");
    println!("          }}");
    println!("      }}");
    println!("  }}");
    println!("  if !failed_items.is_empty() {{");
    println!("      // Report partial failure in output");
    println!("  }}\n");

    println!("Best Practices:");
    println!("  1. Distinguish recoverable vs unrecoverable errors");
    println!("  2. Use exponential backoff for retries");
    println!("  3. Set reasonable timeouts");
    println!("  4. Checkpoint before risky operations");
    println!("  5. Include error details in sdk.failed() message\n");

    println!("--- End Demo Mode ---\n");
}

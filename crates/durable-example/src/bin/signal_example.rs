// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Signal Example - Demonstrates handling cancel, pause, and resume signals.
//!
//! This example shows:
//! - Processing loop with `check_cancelled()` calls
//! - Manual signal polling with `poll_signal()`
//! - Signal acknowledgment
//! - Graceful shutdown on cancel
//! - Pause/resume handling with checkpoint
//!
//! Run with: cargo run -p durable-example --bin signal_example

use runtara_sdk::{RuntaraSdk, SdkError, SignalType};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// State preserved during pause/resume cycles.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TaskState {
    /// Items processed so far
    processed_count: usize,
    /// Total items to process
    total_items: usize,
    /// Whether we're currently paused
    is_paused: bool,
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

    info!("=== Signal Example: Cancel, Pause, Resume Handling ===");

    // Instance IDs can be any non-empty string - descriptive names are encouraged
    let instance_id = format!("signal-example-{}", uuid::Uuid::new_v4());
    let tenant_id = "demo-tenant";

    info!(instance_id = %instance_id, "Creating SDK instance");

    let mut sdk = match RuntaraSdk::localhost(&instance_id, tenant_id) {
        Ok(sdk) => sdk,
        Err(e) => {
            warn!("Failed to create SDK: {}. Running in demo mode.", e);
            demonstrate_signal_handling();
            return Ok(());
        }
    };

    // Connect to runtara-core
    match sdk.connect().await {
        Ok(_) => info!("Connected to runtara-core"),
        Err(e) => {
            warn!("Failed to connect: {}. Running in demo mode.", e);
            demonstrate_signal_handling();
            return Ok(());
        }
    }

    // Initialize state (checkpointing in loop handles resume)
    let mut state = TaskState {
        processed_count: 0,
        total_items: 20,
        is_paused: false,
    };

    sdk.register(None).await?;

    info!(
        processed = state.processed_count,
        total = state.total_items,
        "Starting processing"
    );

    // Main processing loop with signal handling
    while state.processed_count < state.total_items {
        // Method 1: Simple cancellation check
        // This is the most common pattern - check at each iteration
        match sdk.check_cancelled().await {
            Ok(()) => {} // Not cancelled, continue
            Err(SdkError::Cancelled) => {
                info!("Received cancellation signal - shutting down gracefully");

                // Acknowledge the cancellation
                sdk.acknowledge_signal(SignalType::Cancel, true).await?;

                // Save final state before exiting using checkpoint()
                let state_bytes = serde_json::to_vec(&state)?;
                let _ = sdk.checkpoint("cancelled", &state_bytes).await?;

                // Report failure with cancellation reason
                sdk.failed("Cancelled by user request").await?;

                info!("Graceful shutdown complete");
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        }

        // Method 2: Manual signal polling for more control
        // Use this when you need to handle pause/resume
        if let Some(signal) = sdk.poll_signal().await? {
            match signal.signal_type {
                SignalType::Cancel => {
                    info!("Cancel signal received via poll");
                    sdk.acknowledge_signal(SignalType::Cancel, true).await?;
                    sdk.failed("Cancelled").await?;
                    return Ok(());
                }
                SignalType::Pause => {
                    info!("Pause signal received - suspending");

                    // Save current state using checkpoint()
                    state.is_paused = true;
                    let state_bytes = serde_json::to_vec(&state)?;
                    let _ = sdk.checkpoint("paused", &state_bytes).await?;

                    // Acknowledge pause
                    sdk.acknowledge_signal(SignalType::Pause, true).await?;

                    // Send suspended event
                    sdk.suspended().await?;

                    info!("Instance suspended - waiting for resume");

                    // In a real scenario, the instance might exit here
                    // and be resumed later by runtara-core
                    // For demo, we'll wait for resume signal

                    // Wait for resume (polling)
                    loop {
                        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                        if let Some(resume_signal) = sdk.poll_signal_now().await? {
                            if resume_signal.signal_type == SignalType::Resume {
                                info!("Resume signal received - continuing");
                                sdk.acknowledge_signal(SignalType::Resume, true).await?;
                                state.is_paused = false;
                                break;
                            }
                        }
                        // For demo, auto-resume after a few iterations
                        info!("Still paused... (demo will auto-resume)");
                        break; // Auto-break for demo
                    }
                }
                SignalType::Resume => {
                    info!("Resume signal received (unexpected - not paused)");
                    sdk.acknowledge_signal(SignalType::Resume, true).await?;
                }
            }
        }

        // Simulate processing work
        info!(
            item = state.processed_count + 1,
            total = state.total_items,
            "Processing item"
        );
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        state.processed_count += 1;

        // Checkpoint periodically - sdk.checkpoint() saves state
        if state.processed_count % 5 == 0 {
            let checkpoint_id = format!("item-{}", state.processed_count);
            let state_bytes = serde_json::to_vec(&state)?;

            // checkpoint() saves state and returns None for fresh save
            // (If resuming, it would return Some(existing_state))
            let _ = sdk.checkpoint(&checkpoint_id, &state_bytes).await?;

            info!(checkpoint_id = %checkpoint_id, "Checkpoint saved");
        }
    }

    // Complete successfully
    let output = serde_json::json!({
        "status": "success",
        "processed_count": state.processed_count,
        "total_items": state.total_items,
    });
    let output_bytes = serde_json::to_vec(&output)?;

    sdk.completed(&output_bytes).await?;

    info!("=== Signal Example Complete ===");
    info!(
        "Processed {} items with signal handling",
        state.processed_count
    );

    Ok(())
}

/// Demonstrates signal handling without an actual SDK connection.
fn demonstrate_signal_handling() {
    println!("\n--- Demo Mode: Signal Handling ---\n");

    println!("Signal Types:");
    println!("  - Cancel: Request to stop execution");
    println!("  - Pause:  Request to suspend execution");
    println!("  - Resume: Request to continue after pause\n");

    println!("Method 1: Simple Cancellation Check");
    println!("  for item in items {{");
    println!("      sdk.check_cancelled().await?;  // Returns Err(Cancelled) if cancelled");
    println!("      // process item...");
    println!("  }}\n");

    println!("Method 2: Manual Signal Polling");
    println!("  if let Some(signal) = sdk.poll_signal().await? {{");
    println!("      match signal.signal_type {{");
    println!("          SignalType::Cancel => {{");
    println!("              sdk.acknowledge_signal(SignalType::Cancel, true).await?;");
    println!("              sdk.failed(\"Cancelled\").await?;");
    println!("              return Ok(());");
    println!("          }}");
    println!("          SignalType::Pause => {{");
    println!("              sdk.checkpoint(\"paused\", &state).await?;");
    println!("              sdk.acknowledge_signal(SignalType::Pause, true).await?;");
    println!("              sdk.suspended().await?;");
    println!("          }}");
    println!("          SignalType::Resume => {{");
    println!("              sdk.acknowledge_signal(SignalType::Resume, true).await?;");
    println!("          }}");
    println!("      }}");
    println!("  }}\n");

    println!("Signal Acknowledgment:");
    println!("  - true:  Signal was handled successfully");
    println!("  - false: Signal could not be handled (retry may occur)\n");

    println!("Best Practices:");
    println!("  1. Check for cancellation at each iteration of long loops");
    println!("  2. Save checkpoint before acknowledging pause");
    println!("  3. Clean up resources before exiting on cancel");
    println!("  4. poll_signal() is rate-limited; poll_signal_now() ignores rate limit\n");

    println!("--- End Demo Mode ---\n");
}

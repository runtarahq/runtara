// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Sleep Example - Demonstrates durable sleep pattern for long-running tasks.
//!
//! This example shows:
//! - Starting a multi-phase task
//! - Using `sdk.sleep()` between phases
//! - Handling deferred sleep (when instance should exit)
//! - State preservation across sleep/wake cycles
//!
//! Run with: cargo run -p durable-example --bin sleep_example

use runtara_sdk::RuntaraSdk;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{info, warn};

/// State preserved across sleep/wake cycles.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkflowState {
    /// Current phase of the multi-phase workflow
    current_phase: u32,
    /// Total phases to complete
    total_phases: u32,
    /// Results from each completed phase
    phase_results: Vec<String>,
    /// Timestamp when workflow started
    started_at: String,
}

impl WorkflowState {
    fn new(total_phases: u32) -> Self {
        Self {
            current_phase: 0,
            total_phases,
            phase_results: Vec::new(),
            started_at: chrono_now(),
        }
    }
}

/// Get current timestamp as string (simple implementation without chrono dependency)
fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}s since epoch", duration.as_secs())
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

    info!("=== Sleep Example: Durable Sleep Pattern ===");

    // Instance IDs can be any non-empty string - descriptive names are encouraged
    let instance_id = format!("sleep-example-{}", uuid::Uuid::new_v4());
    let tenant_id = "demo-tenant";

    info!(instance_id = %instance_id, "Creating SDK instance");

    let mut sdk = match RuntaraSdk::localhost(&instance_id, tenant_id) {
        Ok(sdk) => sdk,
        Err(e) => {
            warn!("Failed to create SDK: {}. Running in demo mode.", e);
            demonstrate_sleep_workflow();
            return Ok(());
        }
    };

    // Connect to runtara-core
    match sdk.connect().await {
        Ok(_) => info!("Connected to runtara-core"),
        Err(e) => {
            warn!("Failed to connect: {}. Running in demo mode.", e);
            demonstrate_sleep_workflow();
            return Ok(());
        }
    }

    // Initialize state (checkpointing in loop handles resume)
    info!("Starting new workflow");
    let mut state = WorkflowState::new(4); // 4-phase workflow

    sdk.register(None).await?;

    info!(
        current_phase = state.current_phase,
        total_phases = state.total_phases,
        "Workflow state"
    );

    // Process phases with sleep between them
    while state.current_phase < state.total_phases {
        let phase = state.current_phase;
        info!(phase = phase, "Starting phase");

        // Execute phase work
        let result = execute_phase(phase).await;
        state.phase_results.push(result.clone());

        info!(phase = phase, result = %result, "Phase completed");

        state.current_phase += 1;

        // If not the last phase, sleep before continuing
        if state.current_phase < state.total_phases {
            // Checkpoint ID for when we wake up
            let wake_checkpoint_id = format!("phase-{}-complete", phase);
            let state_bytes = serde_json::to_vec(&state)?;

            // Sleep durations vary by phase to demonstrate different behaviors:
            // - Short sleeps (< 30s): Handled in-process, returns immediately
            // - Long sleeps (>= 30s): Deferred - instance should exit and be woken later
            let sleep_duration = match phase {
                0 => Duration::from_secs(2),  // Short sleep - in-process
                1 => Duration::from_secs(3),  // Short sleep - in-process
                2 => Duration::from_secs(60), // Long sleep - would be deferred
                _ => Duration::from_secs(1),
            };

            info!(
                phase = phase,
                sleep_seconds = sleep_duration.as_secs(),
                "Requesting sleep between phases"
            );

            let sleep_result = sdk
                .sleep(sleep_duration, &wake_checkpoint_id, &state_bytes)
                .await?;

            if sleep_result.deferred {
                // Long sleep: Core will wake us later
                // Instance should exit gracefully
                info!(
                    checkpoint_id = %wake_checkpoint_id,
                    "Sleep deferred - instance exiting. Will resume from checkpoint."
                );

                // In a real scenario, the instance would exit here.
                // runtara-core will restart the instance after the sleep duration
                // and we'll resume from the checkpoint.
                println!("\n[DEFERRED SLEEP] Instance would exit now.");
                println!(
                    "After {} seconds, runtara-core would:",
                    sleep_duration.as_secs()
                );
                println!("  1. Restart the instance");
                println!("  2. Instance loads checkpoint '{}'", wake_checkpoint_id);
                println!("  3. Workflow resumes from phase {}\n", state.current_phase);

                // For demo purposes, we'll continue anyway
                // In production: return Ok(());
            } else {
                // Short sleep: Completed in-process
                info!("Sleep completed in-process, continuing");
            }
        }

        // Save progress checkpoint using checkpoint()
        // (checkpoint() saves state - returns None for fresh save, Some for existing)
        let checkpoint_id = format!("phase-{}-complete", phase);
        let state_bytes = serde_json::to_vec(&state)?;
        let _ = sdk.checkpoint(&checkpoint_id, &state_bytes).await?;
    }

    // All phases complete
    let output = serde_json::json!({
        "status": "success",
        "phases_completed": state.total_phases,
        "started_at": state.started_at,
        "phase_results": state.phase_results,
    });
    let output_bytes = serde_json::to_vec(&output)?;

    sdk.completed(&output_bytes).await?;

    info!("=== Sleep Example Complete ===");
    info!(
        "Completed {} phases with durable sleep between them",
        state.total_phases
    );

    Ok(())
}

/// Simulate executing a workflow phase.
async fn execute_phase(phase: u32) -> String {
    // Simulate work
    tokio::time::sleep(Duration::from_millis(500)).await;

    match phase {
        0 => "Initialized resources".to_string(),
        1 => "Processed input data".to_string(),
        2 => "Generated report".to_string(),
        3 => "Sent notifications".to_string(),
        _ => format!("Completed phase {}", phase),
    }
}

/// Demonstrates the sleep workflow without an actual SDK connection.
fn demonstrate_sleep_workflow() {
    println!("\n--- Demo Mode: Durable Sleep Pattern ---\n");

    println!("Durable Sleep Overview:");
    println!("  - Workflows often need to wait (rate limits, scheduling, etc.)");
    println!("  - Regular tokio::sleep loses state if process crashes");
    println!("  - Durable sleep saves state, allows instance to exit, then resume\n");

    println!("1. Request Sleep with Checkpoint:");
    println!("   let sleep_result = sdk.sleep(");
    println!("       Duration::from_secs(3600),  // 1 hour");
    println!("       \"after-sleep\",              // checkpoint ID");
    println!("       &state_bytes,               // state to restore");
    println!("   ).await?;\n");

    println!("2. Handle Sleep Result:");
    println!("   if sleep_result.deferred {{");
    println!("       // Long sleep - instance should exit");
    println!("       // runtara-core will wake us after duration");
    println!("       return Ok(());");
    println!("   }}");
    println!("   // Short sleep - completed in-process, continue\n");

    println!("3. On Wake (Instance Restart):");
    println!("   // checkpoint() handles resume automatically:");
    println!("   // Returns Some(existing_state) if checkpoint exists");
    println!("   // Returns None if fresh execution\n");

    println!("Sleep Behavior:");
    println!("   | Duration     | Behavior                              |");
    println!("   |--------------|---------------------------------------|");
    println!("   | < 30 seconds | In-process (blocks, returns deferred=false) |");
    println!("   | >= 30 seconds| Deferred (save state, exit, wake later)     |\n");

    println!("Use Cases:");
    println!("   - Rate limit backoff (wait 5 minutes before retry)");
    println!("   - Scheduled workflows (run daily at midnight)");
    println!("   - Long-running batch jobs with delays\n");

    println!("--- End Demo Mode ---\n");
}

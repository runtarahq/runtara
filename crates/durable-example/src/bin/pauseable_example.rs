// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Pauseable Example - Demonstrates pause/resume with checkpoint-based signals.
//!
//! This example shows:
//! - Processing items in a loop with checkpoints
//! - Receiving pause/cancel signals via checkpoint response
//! - Exiting cleanly when paused
//! - Resuming from the last checkpoint when restarted
//!
//! Run with:
//!   cargo run -p durable-example --bin pauseable_example
//!
//! Test pause:
//!   1. Start the example
//!   2. Send a pause signal via management API
//!   3. Example will exit after current checkpoint
//!   4. Restart with same RUNTARA_INSTANCE_ID to resume

use runtara_sdk::RuntaraSdk;
use serde::{Deserialize, Serialize};
use std::env;
use std::process::ExitCode;
use tracing::{info, warn};

/// State that gets checkpointed between processing steps.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProcessingState {
    /// Current step index
    current_step: usize,
    /// Items that have been successfully processed
    processed_items: Vec<String>,
    /// Running total
    total_value: i64,
}

impl ProcessingState {
    fn new() -> Self {
        Self {
            current_step: 0,
            processed_items: Vec::new(),
            total_value: 0,
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    match run().await {
        Ok(completed) => {
            if completed {
                info!("Workflow completed successfully");
                ExitCode::SUCCESS
            } else {
                info!("Workflow paused - will resume on restart");
                ExitCode::SUCCESS
            }
        }
        Err(e) => {
            tracing::error!("Workflow failed: {}", e);
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<bool, Box<dyn std::error::Error>> {
    info!("=== Pauseable Example: Checkpoint-based Pause/Resume ===");

    // Items to process (simulating a long-running job)
    let items_to_process: Vec<String> = (1..=20).map(|i| format!("item-{:03}", i)).collect();

    // Get instance ID from environment or generate one
    let instance_id = env::var("RUNTARA_INSTANCE_ID")
        .unwrap_or_else(|_| format!("pauseable-{}", uuid::Uuid::new_v4()));
    let tenant_id = env::var("RUNTARA_TENANT_ID").unwrap_or_else(|_| "demo-tenant".to_string());

    info!(instance_id = %instance_id, tenant_id = %tenant_id, "Starting workflow");

    let mut sdk = match RuntaraSdk::localhost(&instance_id, &tenant_id) {
        Ok(sdk) => sdk,
        Err(e) => {
            warn!("Failed to create SDK: {}. Running in demo mode.", e);
            return demo_mode(&items_to_process);
        }
    };

    // Connect to runtara-core
    match sdk.connect().await {
        Ok(_) => info!("Connected to runtara-core"),
        Err(e) => {
            warn!("Failed to connect: {}. Running in demo mode.", e);
            return demo_mode(&items_to_process);
        }
    }

    // Register (no checkpoint_id - sdk.checkpoint() handles resume logic)
    sdk.register(None).await?;

    info!(total_items = items_to_process.len(), "Starting processing");

    // Start with empty state
    let mut state = ProcessingState::new();

    // Process items with checkpoint-based pause detection
    for (i, item) in items_to_process.iter().enumerate() {
        let checkpoint_id = format!("step-{}", i);

        // Serialize current state
        let state_bytes = serde_json::to_vec(&state)?;

        // checkpoint() now returns CheckpointResult with signal info
        let result = sdk.checkpoint(&checkpoint_id, &state_bytes).await?;

        // Check for pause/cancel signals FIRST (before processing)
        if result.should_cancel() {
            info!(checkpoint_id = %checkpoint_id, "Cancel signal received - terminating");
            sdk.failed("Cancelled by signal").await?;
            return Err("Cancelled".into());
        }

        if result.should_pause() {
            info!(checkpoint_id = %checkpoint_id, "Pause signal received - suspending");
            // State is already saved in checkpoint, just exit cleanly
            sdk.suspended().await?;
            return Ok(false); // Not completed, but not an error
        }

        // Check if we're resuming from an existing checkpoint
        if let Some(existing_state) = result.existing_state() {
            state = serde_json::from_slice(existing_state)?;
            info!(
                checkpoint_id = %checkpoint_id,
                item = %item,
                processed_count = state.processed_items.len(),
                "Resuming from checkpoint - skipping already processed"
            );
            continue;
        }

        // Fresh execution - process the item
        info!(
            step = i,
            item = %item,
            total = items_to_process.len(),
            "Processing item"
        );

        // Simulate processing work (longer delay to give time for pause signals)
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        // Simulate computing a value
        let item_value = (i as i64 + 1) * 10;

        // Update state AFTER processing
        state.current_step = i + 1;
        state.processed_items.push(item.clone());
        state.total_value += item_value;

        info!(
            checkpoint_id = %checkpoint_id,
            processed_count = state.processed_items.len(),
            total_value = state.total_value,
            "Step completed"
        );
    }

    // Prepare final output
    let output = serde_json::json!({
        "status": "completed",
        "total_processed": state.processed_items.len(),
        "total_value": state.total_value,
        "processed_items": state.processed_items,
    });
    let output_bytes = serde_json::to_vec(&output)?;

    sdk.completed(&output_bytes).await?;

    info!("=== Pauseable Example Complete ===");
    info!(
        "Final: {} items processed, total value: {}",
        state.processed_items.len(),
        state.total_value
    );

    Ok(true) // Completed
}

/// Demo mode without SDK connection
fn demo_mode(items: &[String]) -> Result<bool, Box<dyn std::error::Error>> {
    println!("\n--- Demo Mode: Pauseable Workflow ---\n");

    println!("This example demonstrates:");
    println!("1. Checkpoint-based pause signal detection");
    println!("2. Clean exit on pause for later resume");
    println!("3. Resume from last checkpoint\n");

    println!("Code pattern:");
    println!("  let result = sdk.checkpoint(&checkpoint_id, &state_bytes).await?;");
    println!();
    println!("  // Check for pause/cancel BEFORE processing");
    println!("  if result.should_pause() {{");
    println!("      sdk.suspended().await?;");
    println!("      return Ok(false);  // Exit cleanly");
    println!("  }}");
    println!();
    println!("  // Resume from existing checkpoint");
    println!("  if let Some(existing) = result.existing_state() {{");
    println!("      state = deserialize(existing)?;");
    println!("      continue;");
    println!("  }}");
    println!();
    println!("  // Process item...");
    println!();

    println!("Simulated processing {} items:", items.len());
    for (i, item) in items.iter().take(5).enumerate() {
        println!("  [{}] Processing {} ...", i, item);
    }
    println!("  ... ({} more items)", items.len().saturating_sub(5));

    println!("\n--- End Demo Mode ---\n");
    Ok(true)
}

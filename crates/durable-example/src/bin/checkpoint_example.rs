// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Checkpoint Example - Demonstrates checkpointing for durability.
//!
//! This example shows:
//! - Processing items in a loop with durable checkpoints
//! - Using sdk.checkpoint() which handles both save and resume
//! - Automatic resumption from where processing left off
//!
//! Run with: cargo run -p durable-example --bin checkpoint_example

use runtara_sdk::RuntaraSdk;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// State that gets checkpointed between processing steps.
/// This allows the workflow to resume from where it left off.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProcessingState {
    /// Items that have been successfully processed
    processed_items: Vec<String>,
    /// Running total or accumulated result
    accumulated_value: i64,
}

impl ProcessingState {
    fn new() -> Self {
        Self {
            processed_items: Vec::new(),
            accumulated_value: 0,
        }
    }
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

    info!("=== Checkpoint Example: Durability with Checkpoints ===");

    // Items to process (simulating a batch job)
    let items_to_process = vec![
        "order-001",
        "order-002",
        "order-003",
        "order-004",
        "order-005",
        "order-006",
        "order-007",
        "order-008",
        "order-009",
        "order-010",
    ];

    // Instance IDs can be any non-empty string - descriptive names are encouraged
    let instance_id = format!("checkpoint-example-{}", uuid::Uuid::new_v4());
    let tenant_id = "demo-tenant";

    info!(instance_id = %instance_id, "Creating SDK instance");

    let mut sdk = match RuntaraSdk::localhost(&instance_id, tenant_id) {
        Ok(sdk) => sdk,
        Err(e) => {
            warn!("Failed to create SDK: {}. Running in demo mode.", e);
            demonstrate_checkpoint_workflow(&items_to_process);
            return Ok(());
        }
    };

    // Connect to runtara-core
    match sdk.connect().await {
        Ok(_) => info!("Connected to runtara-core"),
        Err(e) => {
            warn!("Failed to connect: {}. Running in demo mode.", e);
            demonstrate_checkpoint_workflow(&items_to_process);
            return Ok(());
        }
    }

    // Register (no checkpoint_id - sdk.checkpoint() handles resume logic)
    sdk.register(None).await?;

    info!(total_items = items_to_process.len(), "Starting processing");

    // Start with empty state
    let mut state = ProcessingState::new();

    // Process items - sdk.checkpoint() handles resume automatically
    for (i, item) in items_to_process.iter().enumerate() {
        let checkpoint_id = format!("item-{}", i);

        // Serialize current state
        let state_bytes = serde_json::to_vec(&state)?;

        // checkpoint() returns CheckpointResult with existing_state() for resume
        // This handles resume automatically - if we're resuming, skip this iteration
        let result = sdk.checkpoint(&checkpoint_id, &state_bytes).await?;
        if let Some(existing_state) = result.existing_state() {
            state = serde_json::from_slice(existing_state)?;
            info!(
                checkpoint_id = %checkpoint_id,
                item = %item,
                "Checkpoint exists - skipping (already processed)"
            );
            continue;
        }

        // Fresh execution - process the item
        info!(index = i, item = %item, "Processing item");

        // Simulate processing work
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

        // Simulate computing a value (e.g., order value extraction)
        let item_value = (i as i64 + 1) * 100;

        // Update state AFTER processing
        state.processed_items.push(item.to_string());
        state.accumulated_value += item_value;

        info!(
            checkpoint_id = %checkpoint_id,
            processed_count = state.processed_items.len(),
            accumulated = state.accumulated_value,
            "Item processed"
        );
    }

    // Prepare final output
    let output = serde_json::json!({
        "status": "success",
        "total_processed": state.processed_items.len(),
        "accumulated_value": state.accumulated_value,
        "processed_items": state.processed_items,
    });
    let output_bytes = serde_json::to_vec(&output)?;

    sdk.completed(&output_bytes).await?;

    info!("=== Checkpoint Example Complete ===");
    info!(
        "Final state: {} items processed, accumulated value: {}",
        state.processed_items.len(),
        state.accumulated_value
    );

    Ok(())
}

/// Demonstrates the checkpoint workflow without an actual SDK connection.
fn demonstrate_checkpoint_workflow(items: &[&str]) {
    println!("\n--- Demo Mode: Checkpoint Workflow ---\n");

    println!("1. Define Checkpointable State:");
    println!("   #[derive(Serialize, Deserialize)]");
    println!("   struct ProcessingState {{");
    println!("       processed_items: Vec<String>,");
    println!("       accumulated_value: i64,");
    println!("   }}\n");

    println!("2. Process Items with checkpoint():");
    println!("   for (i, item) in items.iter().enumerate() {{");
    println!("       let checkpoint_id = format!(\"item-{{}}\", i);");
    println!("       let state_bytes = serde_json::to_vec(&state)?;");
    println!();
    println!("       // checkpoint() returns CheckpointResult with existing_state():");
    println!("       // - existing_state() returns Some(&[u8]) if checkpoint exists (resume)");
    println!("       // - existing_state() returns None if new checkpoint saved (fresh execution)");
    println!("       let result = sdk.checkpoint(&checkpoint_id, &state_bytes).await?;");
    println!("       if let Some(existing) = result.existing_state() {{");
    println!("           state = serde_json::from_slice(existing)?;");
    println!("           continue;  // Skip - already processed");
    println!("       }}");
    println!();
    println!("       // Fresh execution - process item");
    println!("       process_item(item);");
    println!("       state.processed_items.push(item.to_string());");
    println!("   }}\n");

    println!("Simulated processing:");
    for (i, item) in items.iter().enumerate() {
        println!("   checkpoint(\"item-{}\") -> process {}", i, item);
    }

    println!("\n3. Complete:");
    println!("   sdk.completed(&output_bytes).await?;\n");

    println!("Key Benefits:");
    println!("   - checkpoint() is a single call that handles save OR resume");
    println!("   - If process crashes at item-5, restart skips items 0-4");
    println!("   - No manual load_checkpoint() needed\n");

    println!("--- End Demo Mode ---\n");
}

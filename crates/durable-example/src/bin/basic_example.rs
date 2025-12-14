// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Basic Example - Demonstrates the fundamental runtara-sdk lifecycle.
//!
//! This example shows:
//! - SDK initialization
//! - Connection to runtara-core
//! - Instance registration
//! - Simple heartbeat reporting
//! - Completion with output
//!
//! Run with: cargo run -p durable-example --bin basic_example

use runtara_sdk::RuntaraSdk;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing for structured logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    info!("=== Basic Example: Runtara SDK Lifecycle ===");

    // Generate a unique instance ID for this run
    // Instance IDs can be any non-empty string (no UUID requirement)
    let instance_id = format!("basic-example-{}", uuid::Uuid::new_v4());
    let tenant_id = "demo-tenant";

    info!("Running basic_example - demonstrates SDK lifecycle");

    info!(instance_id = %instance_id, tenant_id = %tenant_id, "Creating SDK instance");

    // Create SDK configured for localhost
    // In production, use RuntaraSdk::from_env() to load configuration from environment
    let mut sdk = match RuntaraSdk::localhost(&instance_id, tenant_id) {
        Ok(sdk) => sdk,
        Err(e) => {
            warn!("Failed to create SDK: {}. Running in demo mode.", e);
            demonstrate_workflow_without_sdk();
            return Ok(());
        }
    };

    info!("SDK instance created successfully");

    // Connect to runtara-core
    info!("Connecting to runtara-core...");
    match sdk.connect().await {
        Ok(_) => {
            info!("Connected to runtara-core successfully");
        }
        Err(e) => {
            warn!(
                "Failed to connect to runtara-core: {}. Running in demo mode.",
                e
            );
            demonstrate_workflow_without_sdk();
            return Ok(());
        }
    }

    // Register this instance with runtara-core
    // The checkpoint_id is None because this is a fresh start (not resuming)
    info!("Registering instance with runtara-core...");
    sdk.register(None).await?;
    info!("Instance registered");

    // Simulate processing work in steps with heartbeat reporting
    let total_steps = 5;
    for step in 1..=total_steps {
        info!(step = step, total = total_steps, "Processing step");

        // Simulate some work
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        // Send heartbeat - simple "I'm alive" signal
        // For durable checkpointing, use sdk.checkpoint() instead
        sdk.heartbeat().await?;

        info!(step = step, "Heartbeat sent");
    }

    // Prepare output data
    let output = serde_json::json!({
        "status": "success",
        "steps_completed": total_steps,
        "message": "Basic workflow completed successfully"
    });
    let output_bytes = serde_json::to_vec(&output)?;

    // Send 'completed' event with output
    info!("Sending 'completed' event with output...");
    sdk.completed(&output_bytes).await?;

    info!("=== Basic Example Complete ===");
    info!("Output: {}", serde_json::to_string_pretty(&output)?);

    Ok(())
}

/// Demonstrates the workflow steps without an actual SDK connection.
/// This allows running the example even when runtara-core is not available.
fn demonstrate_workflow_without_sdk() {
    println!("\n--- Demo Mode: Showing workflow steps ---\n");

    println!("1. SDK Creation:");
    println!("   let sdk = RuntaraSdk::localhost(instance_id, tenant_id)?;");
    println!("   - Creates SDK configured for local development");
    println!("   - Alternative: RuntaraSdk::from_env() for production\n");

    println!("2. Connect:");
    println!("   sdk.connect().await?;");
    println!("   - Establishes QUIC connection to runtara-core\n");

    println!("3. Register:");
    println!("   sdk.register(None).await?;");
    println!("   - Registers instance with runtara-core");
    println!("   - Pass checkpoint_id if resuming from checkpoint\n");

    println!("4. Work Loop with Heartbeat:");
    println!("   for step in 1..=5 {{");
    println!("       // Do work...");
    println!("       sdk.heartbeat().await?;");
    println!("   }}");
    println!("   - Use heartbeat() for simple 'I'm alive' signals");
    println!("   - Use checkpoint() for durable state saving\n");

    println!("5. Completed:");
    println!("   sdk.completed(&output_bytes).await?;");
    println!("   - Signals successful completion with output data\n");

    println!("--- End Demo Mode ---\n");
}

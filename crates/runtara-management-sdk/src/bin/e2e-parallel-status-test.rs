// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! E2E test for parallel instance status race condition.
//!
//! This test verifies that when running many parallel instances, the status
//! reporting is correct and doesn't show transient "failed" states before
//! settling to the correct final status.
//!
//! Usage:
//!   cargo run -p runtara-management-sdk --bin e2e-parallel-status-test -- --image-id <IMAGE_ID> --tenant-id <TENANT_ID>
//!
//! The test:
//! 1. Starts N instances in parallel (default: 50)
//! 2. Polls status every 100ms for each instance
//! 3. Tracks all status transitions
//! 4. Reports any instances that showed "failed" before becoming "completed"
//!
//! If the race condition fix is working, NO instances should show the
//! "running -> failed -> completed" transition pattern.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use runtara_management_sdk::{InstanceStatus, ManagementSdk, StartInstanceOptions};
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
struct StatusHistory {
    instance_id: String,
    transitions: Vec<(Instant, InstanceStatus)>,
}

impl StatusHistory {
    fn new(instance_id: String) -> Self {
        Self {
            instance_id,
            transitions: Vec::new(),
        }
    }

    fn record(&mut self, status: InstanceStatus) {
        // Only record if status changed
        if self.transitions.last().map(|(_, s)| s) != Some(&status) {
            self.transitions.push((Instant::now(), status));
        }
    }

    fn had_incorrect_transition(&self) -> bool {
        // Check for "failed" appearing before "completed"
        let mut saw_failed = false;
        let mut saw_completed_after_failed = false;

        for (_, status) in &self.transitions {
            match status {
                InstanceStatus::Failed => saw_failed = true,
                InstanceStatus::Completed if saw_failed => saw_completed_after_failed = true,
                _ => {}
            }
        }

        saw_completed_after_failed
    }

    fn final_status(&self) -> Option<InstanceStatus> {
        self.transitions.last().map(|(_, s)| *s)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse arguments
    let args: Vec<String> = std::env::args().collect();

    let mut image_id = None;
    let mut tenant_id = None;
    let mut count = 50;
    let mut timeout_secs = 30;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--image-id" => {
                i += 1;
                image_id = Some(args.get(i).cloned().unwrap_or_default());
            }
            "--tenant-id" => {
                i += 1;
                tenant_id = Some(args.get(i).cloned().unwrap_or_default());
            }
            "--count" => {
                i += 1;
                count = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(50);
            }
            "--timeout" => {
                i += 1;
                timeout_secs = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(30);
            }
            "--help" | "-h" => {
                println!("E2E Parallel Status Test");
                println!();
                println!("Tests that parallel instance execution doesn't show race condition");
                println!("where instances briefly appear as 'failed' before 'completed'.");
                println!();
                println!("Usage:");
                println!("  e2e-parallel-status-test --image-id <ID> --tenant-id <ID> [OPTIONS]");
                println!();
                println!("Options:");
                println!("  --image-id <ID>    Image ID to use for instances (required)");
                println!("  --tenant-id <ID>   Tenant ID (required)");
                println!("  --count <N>        Number of parallel instances (default: 50)");
                println!("  --timeout <SECS>   Timeout in seconds (default: 30)");
                println!("  --help             Show this help");
                return Ok(());
            }
            _ => {}
        }
        i += 1;
    }

    let image_id = image_id.ok_or("--image-id is required")?;
    let tenant_id = tenant_id.ok_or("--tenant-id is required")?;

    println!("=== E2E Parallel Status Race Condition Test ===");
    println!();
    println!("Configuration:");
    println!("  Image ID:  {}", image_id);
    println!("  Tenant ID: {}", tenant_id);
    println!("  Count:     {}", count);
    println!("  Timeout:   {}s", timeout_secs);
    println!();

    // Connect to server
    println!("Connecting to runtara-environment...");
    let sdk = ManagementSdk::localhost()?;
    sdk.connect().await?;

    let health = sdk.health_check().await?;
    println!("Connected to server version: {}", health.version);
    println!();

    // Track status history for each instance
    let histories: Arc<Mutex<HashMap<String, StatusHistory>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // Start all instances in parallel
    println!("Starting {} instances in parallel...", count);
    let start_time = Instant::now();

    // Start instances sequentially but quickly (SDK doesn't support concurrent requests on same connection)
    let mut instance_ids = Vec::new();
    let mut start_errors = 0;

    for i in 0..count {
        let options = StartInstanceOptions::new(&image_id, &tenant_id)
            .with_input(serde_json::json!({"index": i, "input": "test"}));

        match sdk.start_instance(options).await {
            Ok(result) if result.success => {
                let mut h = histories.lock().await;
                h.insert(
                    result.instance_id.clone(),
                    StatusHistory::new(result.instance_id.clone()),
                );
                instance_ids.push(result.instance_id);
            }
            Ok(result) => {
                eprintln!("  Start failed: {:?}", result.error);
                start_errors += 1;
            }
            Err(e) => {
                eprintln!("  Start error: {}", e);
                start_errors += 1;
            }
        }
    }

    println!(
        "Started {} instances ({} errors) in {:?}",
        instance_ids.len(),
        start_errors,
        start_time.elapsed()
    );
    println!();

    if instance_ids.is_empty() {
        eprintln!("No instances started successfully!");
        return Err("No instances started".into());
    }

    // Poll status for all instances until they reach terminal state or timeout
    println!("Polling instance statuses...");
    let poll_start = Instant::now();
    let timeout = Duration::from_secs(timeout_secs);
    let poll_interval = Duration::from_millis(50);

    loop {
        if poll_start.elapsed() > timeout {
            println!("Timeout reached!");
            break;
        }

        // Get status for all instances
        let mut all_terminal = true;
        for instance_id in &instance_ids {
            match sdk.get_instance_status(instance_id).await {
                Ok(info) => {
                    let mut h = histories.lock().await;
                    if let Some(history) = h.get_mut(instance_id) {
                        history.record(info.status);
                    }

                    // Check if terminal
                    match info.status {
                        InstanceStatus::Completed
                        | InstanceStatus::Failed
                        | InstanceStatus::Cancelled => {}
                        _ => all_terminal = false,
                    }
                }
                Err(e) => {
                    eprintln!("  Status error for {}: {}", instance_id, e);
                }
            }
        }

        if all_terminal {
            println!(
                "All instances reached terminal state in {:?}",
                poll_start.elapsed()
            );
            break;
        }

        tokio::time::sleep(poll_interval).await;
    }

    println!();

    // Analyze results
    println!("=== Results ===");
    println!();

    let histories = histories.lock().await;

    let mut completed_count = 0;
    let mut failed_count = 0;
    let mut other_count = 0;
    let mut incorrect_transitions = Vec::new();

    for (_instance_id, history) in histories.iter() {
        match history.final_status() {
            Some(InstanceStatus::Completed) => completed_count += 1,
            Some(InstanceStatus::Failed) => failed_count += 1,
            _ => other_count += 1,
        }

        if history.had_incorrect_transition() {
            incorrect_transitions.push(history.clone());
        }
    }

    println!("Final status counts:");
    println!("  Completed: {}", completed_count);
    println!("  Failed:    {}", failed_count);
    println!("  Other:     {}", other_count);
    println!();

    if incorrect_transitions.is_empty() {
        println!("SUCCESS: No instances showed incorrect 'failed -> completed' transitions!");
        println!();
        println!("The race condition fix is working correctly.");
    } else {
        println!(
            "FAILURE: {} instances showed incorrect transitions!",
            incorrect_transitions.len()
        );
        println!();
        println!("These instances briefly showed 'failed' before becoming 'completed':");
        for history in &incorrect_transitions {
            println!("  Instance: {}", history.instance_id);
            println!("  Transitions:");
            for (time, status) in &history.transitions {
                println!("    {:?} - {:?}", time.elapsed(), status);
            }
            println!();
        }
        println!("This indicates the race condition is still present!");
        return Err("Race condition detected".into());
    }

    Ok(())
}

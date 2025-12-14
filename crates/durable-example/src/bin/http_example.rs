// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! HTTP Example - Demonstrates real work with HTTP requests.
//!
//! This example shows:
//! - Fetching data from runtara.com
//! - Processing HTTP responses
//! - Progress reporting during HTTP operations
//! - Checkpointing between requests
//!
//! Run with: cargo run -p durable-example --bin http_example

use reqwest::Client;
use runtara_sdk::RuntaraSdk;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{info, warn};

/// State tracking HTTP request progress.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct HttpWorkflowState {
    /// URLs that have been successfully fetched
    completed_urls: Vec<String>,
    /// Results from each fetch
    results: Vec<HttpResult>,
    /// Current URL index
    current_index: usize,
}

/// Result from an HTTP fetch operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct HttpResult {
    url: String,
    status_code: u16,
    content_length: Option<usize>,
    title: Option<String>,
}

impl HttpWorkflowState {
    fn new() -> Self {
        Self {
            completed_urls: Vec::new(),
            results: Vec::new(),
            current_index: 0,
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

    info!("=== HTTP Example: Real Work with HTTP Requests ===");

    // URLs to fetch (using runtara.com as requested)
    let urls = vec![
        "https://runtara.com",
        "https://runtara.com/docs",
        "https://runtara.com/pricing",
    ];

    // Instance IDs can be any non-empty string - descriptive names are encouraged
    let instance_id = format!("http-example-{}", uuid::Uuid::new_v4());
    let tenant_id = "demo-tenant";

    info!(instance_id = %instance_id, "Creating SDK instance");

    let mut sdk = match RuntaraSdk::localhost(&instance_id, tenant_id) {
        Ok(sdk) => sdk,
        Err(e) => {
            warn!("Failed to create SDK: {}. Running in demo mode.", e);
            demonstrate_http_workflow(&urls).await;
            return Ok(());
        }
    };

    // Connect to runtara-core
    match sdk.connect().await {
        Ok(_) => info!("Connected to runtara-core"),
        Err(e) => {
            warn!("Failed to connect: {}. Running in demo mode.", e);
            demonstrate_http_workflow(&urls).await;
            return Ok(());
        }
    }

    // Initialize state (checkpointing in loop handles resume)
    let mut state = HttpWorkflowState::new();

    sdk.register(None).await?;

    // Create HTTP client with reasonable defaults
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("runtara-example/1.0")
        .build()?;

    info!(
        completed = state.current_index,
        total = urls.len(),
        "Starting HTTP workflow"
    );

    // Process URLs from where we left off
    for (i, url) in urls.iter().enumerate().skip(state.current_index) {
        info!(index = i, url = %url, "Fetching URL");

        // Checkpoint before the HTTP request
        // This ensures we can resume if the request fails
        let pre_checkpoint_id = format!("pre-fetch-{}", i);
        let state_bytes = serde_json::to_vec(&state)?;
        let _ = sdk.checkpoint(&pre_checkpoint_id, &state_bytes).await?;

        // Perform the HTTP request
        let result = fetch_url(&client, url).await;

        match result {
            Ok(http_result) => {
                info!(
                    url = %url,
                    status = http_result.status_code,
                    length = ?http_result.content_length,
                    title = ?http_result.title,
                    "Fetch successful"
                );

                state.completed_urls.push(url.to_string());
                state.results.push(http_result);
                state.current_index = i + 1;

                // Checkpoint after successful fetch
                // checkpoint() saves state (returns None for fresh, Some for existing)
                let post_checkpoint_id = format!("post-fetch-{}", i);
                let state_bytes = serde_json::to_vec(&state)?;
                let _ = sdk.checkpoint(&post_checkpoint_id, &state_bytes).await?;
            }
            Err(e) => {
                warn!(url = %url, error = %e, "Fetch failed");

                // Record the failure but continue with other URLs
                state.results.push(HttpResult {
                    url: url.to_string(),
                    status_code: 0,
                    content_length: None,
                    title: Some(format!("Error: {}", e)),
                });
                state.current_index = i + 1;
            }
        }

        // Small delay between requests to be polite
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // Prepare final output
    let successful_count = state
        .results
        .iter()
        .filter(|r| r.status_code >= 200 && r.status_code < 300)
        .count();

    let output = serde_json::json!({
        "status": "completed",
        "total_urls": urls.len(),
        "successful_fetches": successful_count,
        "results": state.results,
    });
    let output_bytes = serde_json::to_vec(&output)?;

    sdk.completed(&output_bytes).await?;

    info!("=== HTTP Example Complete ===");
    info!(
        "Fetched {} URLs, {} successful",
        urls.len(),
        successful_count
    );

    // Print results summary
    for result in &state.results {
        println!(
            "  {} - Status: {}, Length: {:?}",
            result.url, result.status_code, result.content_length
        );
    }

    Ok(())
}

/// Fetch a URL and extract metadata.
async fn fetch_url(client: &Client, url: &str) -> Result<HttpResult, Box<dyn std::error::Error>> {
    let response = client.get(url).send().await?;

    let status_code = response.status().as_u16();

    // Get the body to extract title and measure actual size
    let body = response.text().await?;
    let content_length = body.len();

    // Try to extract title from HTML
    let title = extract_title(&body);

    Ok(HttpResult {
        url: url.to_string(),
        status_code,
        content_length: Some(content_length),
        title,
    })
}

/// Simple HTML title extraction.
fn extract_title(html: &str) -> Option<String> {
    // Simple regex-free title extraction
    let lower = html.to_lowercase();
    let start = lower.find("<title>")?;
    let end = lower.find("</title>")?;

    if start < end {
        let title_start = start + 7; // Length of "<title>"
        let title = &html[title_start..end];
        Some(title.trim().to_string())
    } else {
        None
    }
}

/// Demonstrates the HTTP workflow without an actual SDK connection.
async fn demonstrate_http_workflow(urls: &[&str]) {
    println!("\n--- Demo Mode: HTTP Workflow with Checkpoints ---\n");

    println!("Workflow Steps:");
    println!("  1. Load checkpoint (if resuming)");
    println!("  2. For each URL:");
    println!("     a. Save pre-fetch checkpoint");
    println!("     b. Perform HTTP request");
    println!("     c. Save post-fetch checkpoint");
    println!("     d. Report progress");
    println!("  3. Complete with results\n");

    // Actually perform the HTTP requests for demo
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("runtara-example/1.0")
        .build()
        .expect("Failed to create HTTP client");

    println!("Fetching URLs (real HTTP requests):\n");

    for url in urls {
        print!("  {} ... ", url);

        match fetch_url(&client, url).await {
            Ok(result) => {
                println!(
                    "OK (status: {}, size: {:?} bytes)",
                    result.status_code, result.content_length
                );
                if let Some(title) = result.title {
                    println!("    Title: {}", title);
                }
            }
            Err(e) => {
                println!("FAILED: {}", e);
            }
        }

        // Small delay
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    println!("\nCheckpoint Pattern:");
    println!("  // Before HTTP request");
    println!("  sdk.checkpoint(\"pre-fetch-0\", &state).await?;");
    println!();
    println!("  // Perform request");
    println!("  let response = client.get(url).send().await?;");
    println!();
    println!("  // After successful request");
    println!("  state.results.push(result);");
    println!("  sdk.checkpoint(\"post-fetch-0\", &state).await?;");
    println!();

    println!("Benefits:");
    println!("  - If crash during fetch, resume from pre-fetch checkpoint");
    println!("  - No duplicate requests on resume");
    println!("  - Progress visible in runtara-core dashboard\n");

    println!("--- End Demo Mode ---\n");
}

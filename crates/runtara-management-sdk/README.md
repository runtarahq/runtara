# runtara-management-sdk

[![Crates.io](https://img.shields.io/crates/v/runtara-management-sdk.svg)](https://crates.io/crates/runtara-management-sdk)
[![Documentation](https://docs.rs/runtara-management-sdk/badge.svg)](https://docs.rs/runtara-management-sdk)
[![License](https://img.shields.io/crates/l/runtara-management-sdk.svg)](LICENSE)

SDK for managing [Runtara](https://runtara.dev) workflow instances. Start, stop, monitor, and control workflow executions from your application.

## Overview

The Management SDK provides programmatic control over the Runtara platform:

- **Image Registration**: Upload compiled workflow binaries
- **Instance Lifecycle**: Start, stop, and query workflow instances
- **Signal Control**: Send cancel, pause, and resume signals
- **Status Monitoring**: Check instance status and retrieve outputs

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
runtara-management-sdk = "1.0"
```

## Usage

### Connecting to Runtara

```rust
use runtara_management_sdk::{ManagementSdk, SdkConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Connect to runtara-environment
    let config = SdkConfig::new("127.0.0.1:8002");
    let sdk = ManagementSdk::new(config)?;
    sdk.connect().await?;

    // Or use localhost defaults
    let sdk = ManagementSdk::new(SdkConfig::localhost())?;
    sdk.connect().await?;

    Ok(())
}
```

### Registering an Image

```rust
use runtara_management_sdk::{ManagementSdk, SdkConfig, RegisterImageOptions};
use std::fs;

async fn register_workflow(sdk: &ManagementSdk) -> Result<String, Box<dyn std::error::Error>> {
    // Read the compiled workflow binary
    let binary_bytes = fs::read("./compiled_workflow")?;

    // Register with runtara-environment
    let options = RegisterImageOptions::new("tenant-123", "order-processing", binary_bytes)
        .with_description("Order processing workflow v1.0");

    let result = sdk.register_image(options).await?;
    println!("Registered image: {}", result.image_id);

    Ok(result.image_id)
}
```

### Starting an Instance

```rust
use runtara_management_sdk::{ManagementSdk, StartInstanceOptions};

async fn start_workflow(
    sdk: &ManagementSdk,
    image_id: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let options = StartInstanceOptions::new(image_id, "tenant-123")
        .with_input(serde_json::json!({
            "order_id": "ORD-456",
            "customer_email": "user@example.com",
            "items": ["item-1", "item-2"]
        }));

    let result = sdk.start_instance(options).await?;
    println!("Started instance: {}", result.instance_id);

    Ok(result.instance_id)
}
```

### Querying Instance Status

```rust
use runtara_management_sdk::ManagementSdk;

async fn check_status(
    sdk: &ManagementSdk,
    instance_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let status = sdk.get_instance_status(instance_id).await?;

    println!("Instance: {}", status.instance_id);
    println!("Status: {}", status.status);
    println!("Started: {}", status.started_at);

    if let Some(output) = status.output {
        println!("Output: {}", String::from_utf8_lossy(&output));
    }

    Ok(())
}
```

### Listing Instances

```rust
use runtara_management_sdk::{ManagementSdk, ListInstancesOptions};

async fn list_workflows(sdk: &ManagementSdk) -> Result<(), Box<dyn std::error::Error>> {
    let options = ListInstancesOptions::new("tenant-123")
        .with_status("running")
        .with_limit(50);

    let instances = sdk.list_instances(options).await?;

    for instance in instances {
        println!("{}: {} ({})", instance.instance_id, instance.image_id, instance.status);
    }

    Ok(())
}
```

### Sending Signals

```rust
use runtara_management_sdk::ManagementSdk;

async fn control_instance(
    sdk: &ManagementSdk,
    instance_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // Pause a running instance
    sdk.pause_instance(instance_id).await?;
    println!("Instance paused");

    // Resume a paused instance
    sdk.resume_instance(instance_id).await?;
    println!("Instance resumed");

    // Cancel an instance
    sdk.cancel_instance(instance_id).await?;
    println!("Instance cancelled");

    Ok(())
}
```

### Complete Example

```rust
use runtara_management_sdk::{ManagementSdk, SdkConfig, RegisterImageOptions, StartInstanceOptions};
use std::fs;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Connect to runtara-environment
    let sdk = ManagementSdk::new(SdkConfig::localhost())?;
    sdk.connect().await?;

    // Register workflow image
    let binary = fs::read("./my_workflow")?;
    let image = sdk.register_image(
        RegisterImageOptions::new("my-tenant", "my-workflow", binary)
    ).await?;

    // Start an instance
    let instance = sdk.start_instance(
        StartInstanceOptions::new(&image.image_id, "my-tenant")
            .with_input(serde_json::json!({"key": "value"}))
    ).await?;

    // Monitor until complete
    loop {
        let status = sdk.get_instance_status(&instance.instance_id).await?;
        match status.status.as_str() {
            "completed" => {
                println!("Done! Output: {:?}", status.output);
                break;
            }
            "failed" => {
                println!("Failed: {:?}", status.error);
                break;
            }
            _ => {
                println!("Status: {}", status.status);
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    }

    Ok(())
}
```

## CLI Tool

This crate also provides `runtara-ctl`, a command-line tool for managing instances:

```bash
# List running instances
runtara-ctl list --tenant my-tenant

# Get instance status
runtara-ctl status <instance-id>

# Cancel an instance
runtara-ctl cancel <instance-id>

# Pause/resume
runtara-ctl pause <instance-id>
runtara-ctl resume <instance-id>
```

## Related Crates

- [`runtara-protocol`](https://crates.io/crates/runtara-protocol) - Wire protocol layer
- [`runtara-sdk`](https://crates.io/crates/runtara-sdk) - SDK for building workflows (used inside instances)
- [`runtara-workflows`](https://crates.io/crates/runtara-workflows) - Compile JSON DSL to workflow binaries

## License

This project is licensed under [AGPL-3.0-or-later](LICENSE).

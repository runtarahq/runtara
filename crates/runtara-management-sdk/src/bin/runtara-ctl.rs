// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runtara Control CLI
//!
//! CLI tool for interacting with runtara-environment.
//!
//! Usage:
//!   runtara-ctl <command> [options]
//!
//! Commands:
//!   health                        Check environment health
//!   register --binary <path> --tenant <id> --name <name>
//!   list-images [--tenant <id>]
//!   get-image <image_id> <tenant_id>
//!   delete-image <image_id> <tenant_id>
//!   start --image <id> --tenant <id> [--input <json>] [--instance-id <id>]
//!   status <instance_id>
//!   wait <instance_id>            Wait for instance to complete
//!   list-instances [--tenant <id>]
//!   stop <instance_id>
//!   cancel <instance_id>
//!   pause <instance_id>
//!   resume <instance_id>

use runtara_management_sdk::{
    ListImagesOptions, ListInstancesOptions, ManagementSdk, RegisterImageOptions, SdkConfig,
    StartInstanceOptions, StopInstanceOptions,
};
use std::fs;
use std::process::ExitCode;
use std::time::Duration;

fn print_usage() {
    eprintln!(
        r#"Usage: runtara-ctl <command> [options]

Interact with runtara-environment.

COMMANDS:
    health                          Check environment health
    register                        Register a binary as an image
    list-images                     List registered images
    get-image <image_id> <tenant_id>   Get image details
    delete-image <image_id> <tenant_id> Delete an image
    start                           Start an instance
    status <instance_id>            Get instance status
    wait <instance_id>              Wait for instance completion
    list-instances                  List instances
    stop <instance_id>              Stop an instance
    cancel <instance_id>            Cancel an instance
    pause <instance_id>             Pause an instance
    resume <instance_id>            Resume a paused instance

REGISTER OPTIONS:
    --binary <path>                 Path to binary file (required)
    --tenant <id>                   Tenant ID (required)
    --name <name>                   Image name (required)
    --description <text>            Image description

START OPTIONS:
    --image <id>                    Image ID (required)
    --tenant <id>                   Tenant ID (required)
    --input <json>                  Input JSON (default: {{}})
    --instance-id <id>              Custom instance ID
    --timeout <seconds>             Execution timeout

LIST OPTIONS:
    --tenant <id>                   Filter by tenant ID
    --limit <n>                     Max results (default: 100)

WAIT OPTIONS:
    --poll <ms>                     Poll interval in ms (default: 500)

ENVIRONMENT:
    RUNTARA_ENVIRONMENT_ADDR        Environment address (default: 127.0.0.1:8002)
    RUNTARA_SKIP_CERT_VERIFICATION  Skip TLS verification (default: false)

EXAMPLES:
    # Check health
    runtara-ctl health

    # Register a workflow binary
    runtara-ctl register --binary ./my-workflow --tenant acme --name order-sync

    # Start an instance with input
    runtara-ctl start --image img_123 --tenant acme --input '{{"order_id": 42}}'

    # Wait for completion and get output
    runtara-ctl wait inst_456
"#
    );
}

#[derive(Debug)]
enum Command {
    Health,
    Register {
        binary_path: String,
        tenant_id: String,
        name: String,
        description: Option<String>,
    },
    ListImages {
        tenant_id: Option<String>,
        limit: u32,
    },
    GetImage {
        image_id: String,
        tenant_id: String,
    },
    DeleteImage {
        image_id: String,
        tenant_id: String,
    },
    Start {
        image_id: String,
        tenant_id: String,
        input: Option<String>,
        instance_id: Option<String>,
        timeout: Option<u32>,
    },
    Status {
        instance_id: String,
    },
    Wait {
        instance_id: String,
        poll_ms: u64,
    },
    ListInstances {
        tenant_id: Option<String>,
        limit: u32,
    },
    Stop {
        instance_id: String,
    },
    Cancel {
        instance_id: String,
    },
    Pause {
        instance_id: String,
    },
    Resume {
        instance_id: String,
    },
}

fn parse_args() -> Result<Command, String> {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        return Err("No command specified".to_string());
    }

    match args[1].as_str() {
        "help" | "--help" | "-h" => {
            print_usage();
            std::process::exit(0);
        }
        "health" => Ok(Command::Health),
        "register" => {
            let mut binary_path: Option<String> = None;
            let mut tenant_id: Option<String> = None;
            let mut name: Option<String> = None;
            let mut description: Option<String> = None;

            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "--binary" => {
                        i += 1;
                        binary_path = Some(args.get(i).ok_or("--binary requires a path")?.clone());
                    }
                    "--tenant" => {
                        i += 1;
                        tenant_id = Some(args.get(i).ok_or("--tenant requires an ID")?.clone());
                    }
                    "--name" => {
                        i += 1;
                        name = Some(args.get(i).ok_or("--name requires a value")?.clone());
                    }
                    "--description" => {
                        i += 1;
                        description =
                            Some(args.get(i).ok_or("--description requires a value")?.clone());
                    }
                    arg => return Err(format!("Unknown argument: {}", arg)),
                }
                i += 1;
            }

            Ok(Command::Register {
                binary_path: binary_path.ok_or("--binary is required")?,
                tenant_id: tenant_id.ok_or("--tenant is required")?,
                name: name.ok_or("--name is required")?,
                description,
            })
        }
        "list-images" => {
            let mut tenant_id: Option<String> = None;
            let mut limit: u32 = 100;

            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "--tenant" => {
                        i += 1;
                        tenant_id = Some(args.get(i).ok_or("--tenant requires an ID")?.clone());
                    }
                    "--limit" => {
                        i += 1;
                        limit = args
                            .get(i)
                            .ok_or("--limit requires a number")?
                            .parse()
                            .map_err(|_| "Invalid limit")?;
                    }
                    arg => return Err(format!("Unknown argument: {}", arg)),
                }
                i += 1;
            }

            Ok(Command::ListImages { tenant_id, limit })
        }
        "get-image" => {
            let image_id = args.get(2).ok_or("Image ID required")?.clone();
            let tenant_id = args.get(3).ok_or("Tenant ID required")?.clone();
            Ok(Command::GetImage {
                image_id,
                tenant_id,
            })
        }
        "delete-image" => {
            let image_id = args.get(2).ok_or("Image ID required")?.clone();
            let tenant_id = args.get(3).ok_or("Tenant ID required")?.clone();
            Ok(Command::DeleteImage {
                image_id,
                tenant_id,
            })
        }
        "start" => {
            let mut image_id: Option<String> = None;
            let mut tenant_id: Option<String> = None;
            let mut input: Option<String> = None;
            let mut instance_id: Option<String> = None;
            let mut timeout: Option<u32> = None;

            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "--image" => {
                        i += 1;
                        image_id = Some(args.get(i).ok_or("--image requires an ID")?.clone());
                    }
                    "--tenant" => {
                        i += 1;
                        tenant_id = Some(args.get(i).ok_or("--tenant requires an ID")?.clone());
                    }
                    "--input" => {
                        i += 1;
                        input = Some(args.get(i).ok_or("--input requires JSON")?.clone());
                    }
                    "--instance-id" => {
                        i += 1;
                        instance_id =
                            Some(args.get(i).ok_or("--instance-id requires an ID")?.clone());
                    }
                    "--timeout" => {
                        i += 1;
                        timeout = Some(
                            args.get(i)
                                .ok_or("--timeout requires a number")?
                                .parse()
                                .map_err(|_| "Invalid timeout")?,
                        );
                    }
                    arg => return Err(format!("Unknown argument: {}", arg)),
                }
                i += 1;
            }

            Ok(Command::Start {
                image_id: image_id.ok_or("--image is required")?,
                tenant_id: tenant_id.ok_or("--tenant is required")?,
                input,
                instance_id,
                timeout,
            })
        }
        "status" => {
            let instance_id = args.get(2).ok_or("Instance ID required")?.clone();
            Ok(Command::Status { instance_id })
        }
        "wait" => {
            let instance_id = args.get(2).ok_or("Instance ID required")?.clone();
            let mut poll_ms: u64 = 500;

            let mut i = 3;
            while i < args.len() {
                match args[i].as_str() {
                    "--poll" => {
                        i += 1;
                        poll_ms = args
                            .get(i)
                            .ok_or("--poll requires a number")?
                            .parse()
                            .map_err(|_| "Invalid poll interval")?;
                    }
                    arg => return Err(format!("Unknown argument: {}", arg)),
                }
                i += 1;
            }

            Ok(Command::Wait {
                instance_id,
                poll_ms,
            })
        }
        "list-instances" => {
            let mut tenant_id: Option<String> = None;
            let mut limit: u32 = 100;

            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "--tenant" => {
                        i += 1;
                        tenant_id = Some(args.get(i).ok_or("--tenant requires an ID")?.clone());
                    }
                    "--limit" => {
                        i += 1;
                        limit = args
                            .get(i)
                            .ok_or("--limit requires a number")?
                            .parse()
                            .map_err(|_| "Invalid limit")?;
                    }
                    arg => return Err(format!("Unknown argument: {}", arg)),
                }
                i += 1;
            }

            Ok(Command::ListInstances { tenant_id, limit })
        }
        "stop" => {
            let instance_id = args.get(2).ok_or("Instance ID required")?.clone();
            Ok(Command::Stop { instance_id })
        }
        "cancel" => {
            let instance_id = args.get(2).ok_or("Instance ID required")?.clone();
            Ok(Command::Cancel { instance_id })
        }
        "pause" => {
            let instance_id = args.get(2).ok_or("Instance ID required")?.clone();
            Ok(Command::Pause { instance_id })
        }
        "resume" => {
            let instance_id = args.get(2).ok_or("Instance ID required")?.clone();
            Ok(Command::Resume { instance_id })
        }
        cmd => Err(format!("Unknown command: {}", cmd)),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let cmd = match parse_args() {
        Ok(cmd) => cmd,
        Err(e) => {
            eprintln!("Error: {}", e);
            eprintln!();
            print_usage();
            return ExitCode::FAILURE;
        }
    };

    // Create SDK from environment
    let config = match SdkConfig::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Configuration error: {}", e);
            return ExitCode::FAILURE;
        }
    };

    let sdk = match ManagementSdk::new(config) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to create SDK: {}", e);
            return ExitCode::FAILURE;
        }
    };

    if let Err(e) = sdk.connect().await {
        eprintln!("Failed to connect to environment: {}", e);
        return ExitCode::FAILURE;
    }

    match execute_command(&sdk, cmd).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {}", e);
            ExitCode::FAILURE
        }
    }
}

async fn execute_command(sdk: &ManagementSdk, cmd: Command) -> Result<(), String> {
    match cmd {
        Command::Health => {
            let health = sdk.health_check().await.map_err(|e| e.to_string())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&health).map_err(|e| e.to_string())?
            );
        }

        Command::Register {
            binary_path,
            tenant_id,
            name,
            description,
        } => {
            let binary = fs::read(&binary_path)
                .map_err(|e| format!("Failed to read binary {}: {}", binary_path, e))?;

            let mut options = RegisterImageOptions::new(&tenant_id, &name, binary);
            if let Some(desc) = description {
                options = options.with_description(desc);
            }

            let result = sdk
                .register_image(options)
                .await
                .map_err(|e| e.to_string())?;

            if result.success {
                println!("{}", result.image_id);
            } else {
                return Err(result.error.unwrap_or_else(|| "Unknown error".to_string()));
            }
        }

        Command::ListImages { tenant_id, limit } => {
            let mut options = ListImagesOptions::new().with_limit(limit);
            if let Some(tid) = tenant_id {
                options = options.with_tenant_id(tid);
            }

            let result = sdk.list_images(options).await.map_err(|e| e.to_string())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?
            );
        }

        Command::GetImage {
            image_id,
            tenant_id,
        } => {
            let image = sdk
                .get_image(&image_id, &tenant_id)
                .await
                .map_err(|e| e.to_string())?;
            match image {
                Some(img) => println!(
                    "{}",
                    serde_json::to_string_pretty(&img).map_err(|e| e.to_string())?
                ),
                None => return Err(format!("Image not found: {}", image_id)),
            }
        }

        Command::DeleteImage {
            image_id,
            tenant_id,
        } => {
            sdk.delete_image(&image_id, &tenant_id)
                .await
                .map_err(|e| e.to_string())?;
            println!("Deleted: {}", image_id);
        }

        Command::Start {
            image_id,
            tenant_id,
            input,
            instance_id,
            timeout,
        } => {
            let mut options = StartInstanceOptions::new(&image_id, &tenant_id);

            if let Some(input_json) = input {
                let input_value: serde_json::Value = serde_json::from_str(&input_json)
                    .map_err(|e| format!("Invalid input JSON: {}", e))?;
                options = options.with_input(input_value);
            }

            if let Some(id) = instance_id {
                options = options.with_instance_id(id);
            }

            if let Some(t) = timeout {
                options = options.with_timeout(t);
            }

            let result = sdk
                .start_instance(options)
                .await
                .map_err(|e| e.to_string())?;

            if result.success {
                println!("{}", result.instance_id);
            } else {
                return Err(result.error.unwrap_or_else(|| "Unknown error".to_string()));
            }
        }

        Command::Status { instance_id } => {
            let status = sdk
                .get_instance_status(&instance_id)
                .await
                .map_err(|e| e.to_string())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&status).map_err(|e| e.to_string())?
            );
        }

        Command::Wait {
            instance_id,
            poll_ms,
        } => {
            let poll_interval = Duration::from_millis(poll_ms);
            let result = sdk
                .wait_for_completion(&instance_id, poll_interval)
                .await
                .map_err(|e| e.to_string())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?
            );
        }

        Command::ListInstances { tenant_id, limit } => {
            let mut options = ListInstancesOptions::new().with_limit(limit);
            if let Some(tid) = tenant_id {
                options = options.with_tenant_id(tid);
            }

            let result = sdk
                .list_instances(options)
                .await
                .map_err(|e| e.to_string())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?
            );
        }

        Command::Stop { instance_id } => {
            let options = StopInstanceOptions::new(&instance_id);
            sdk.stop_instance(options)
                .await
                .map_err(|e| e.to_string())?;
            println!("Stopped: {}", instance_id);
        }

        Command::Cancel { instance_id } => {
            sdk.cancel_instance(&instance_id, Some("CLI cancel"))
                .await
                .map_err(|e| e.to_string())?;
            println!("Cancelled: {}", instance_id);
        }

        Command::Pause { instance_id } => {
            sdk.pause_instance(&instance_id)
                .await
                .map_err(|e| e.to_string())?;
            println!("Paused: {}", instance_id);
        }

        Command::Resume { instance_id } => {
            sdk.resume_instance(&instance_id)
                .await
                .map_err(|e| e.to_string())?;
            println!("Resumed: {}", instance_id);
        }
    }

    Ok(())
}

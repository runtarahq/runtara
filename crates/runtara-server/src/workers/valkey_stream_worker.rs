use std::time::Duration;
use uuid::Uuid;

use crate::valkey::ValkeyConfig;
use crate::valkey::client::ValkeyClient;
use crate::valkey::stream::StreamConsumer;

/// Background worker that consumes events from Valkey streams
/// Currently only logs events - does not trigger scenario invocations yet
pub async fn run(config: ValkeyConfig) {
    let worker_id = format!("valkey-worker-{}", Uuid::new_v4());
    println!("Starting Valkey stream worker: {}", worker_id);

    // Connect to Valkey
    let client = match ValkeyClient::new(config.clone()).await {
        Ok(client) => client,
        Err(e) => {
            eprintln!("Failed to connect to Valkey: {}", e);
            eprintln!("Valkey stream worker will not start");
            return;
        }
    };

    // Create stream consumer
    let connection = client.get_connection();
    let mut consumer = StreamConsumer::new(
        connection,
        config.stream_name.clone(),
        config.consumer_group.clone(),
        worker_id.clone(),
    );

    // Initialize consumer group
    if let Err(e) = consumer.initialize_consumer_group().await {
        eprintln!("Failed to initialize consumer group: {}", e);
        eprintln!("Valkey stream worker will not start");
        return;
    }

    println!(
        "Valkey worker listening on stream '{}' with consumer group '{}'",
        config.stream_name, config.consumer_group
    );

    // Main event processing loop
    loop {
        match consumer.read_events(5000, 100).await {
            Ok(events) => {
                for (entry_id, event) in events {
                    // Log the event
                    println!("=== Valkey Event Received ===");
                    println!("Entry ID: {}", entry_id);
                    println!(
                        "Event: {}",
                        serde_json::to_string_pretty(&event)
                            .unwrap_or_else(|_| format!("{:?}", event))
                    );
                    println!("============================");

                    // TODO: Queue scenario execution here
                    // For now, we just acknowledge the event

                    // Acknowledge the event
                    if let Err(e) = consumer.acknowledge_event(&entry_id).await {
                        eprintln!("Failed to acknowledge event {}: {}", entry_id, e);
                    } else {
                        println!("✓ Event {} acknowledged", entry_id);
                    }
                }
            }
            Err(e) => {
                eprintln!("Error reading from Valkey stream: {}", e);
                eprintln!("Retrying in 5 seconds...");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

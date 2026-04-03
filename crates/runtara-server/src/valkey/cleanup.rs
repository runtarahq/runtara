use std::time::Duration;

// Note: Using eprintln! and println! instead of tracing since this module
// may be used in contexts where tracing is not initialized

/// Retention period for unclaimed messages in Redis streams (48 hours)
const MESSAGE_RETENTION_HOURS: i64 = 48;

/// Cleanup interval - how often to run the cleanup task (6 hours)
const CLEANUP_INTERVAL_HOURS: u64 = 6;

/// Run periodic cleanup of old messages from Redis streams
///
/// This task runs every 6 hours and trims messages older than 48 hours
/// from all `smo:events:*` streams.
pub async fn start_cleanup_task(redis_client: redis::Client) {
    let cleanup_interval = Duration::from_secs(CLEANUP_INTERVAL_HOURS * 60 * 60);

    println!(
        "Starting cleanup task - will run every {} hours to remove messages older than {} hours",
        CLEANUP_INTERVAL_HOURS, MESSAGE_RETENTION_HOURS
    );

    loop {
        // Wait for the cleanup interval
        tokio::time::sleep(cleanup_interval).await;

        println!("Running cleanup task for Redis streams");

        match cleanup_old_messages(&redis_client).await {
            Ok(stats) => {
                println!(
                    "Cleanup completed - processed {} streams, trimmed approximately {} messages",
                    stats.streams_processed, stats.messages_trimmed
                );
            }
            Err(e) => {
                eprintln!("Cleanup task failed: {}", e);
            }
        }
    }
}

/// Statistics from cleanup operation
struct CleanupStats {
    streams_processed: usize,
    messages_trimmed: usize,
}

/// Clean up old messages from all event streams
async fn cleanup_old_messages(redis_client: &redis::Client) -> Result<CleanupStats, String> {
    // Get Redis connection
    let mut conn = redis_client
        .get_multiplexed_async_connection()
        .await
        .map_err(|e| format!("Failed to connect to Redis: {}", e))?;

    // Calculate cutoff timestamp (48 hours ago)
    let cutoff_timestamp_ms = calculate_cutoff_timestamp();
    let cutoff_stream_id = format!("{}-0", cutoff_timestamp_ms);

    println!(
        "Cutoff timestamp: {} ms (stream ID: {})",
        cutoff_timestamp_ms, cutoff_stream_id
    );

    // Scan for all event streams matching pattern smo:events:*
    let stream_keys = scan_event_streams(&mut conn).await?;

    println!("Found {} event streams to process", stream_keys.len());

    let mut total_trimmed = 0;
    let mut streams_processed = 0;

    // Trim old messages from each stream
    for stream_key in &stream_keys {
        match trim_stream(&mut conn, stream_key, &cutoff_stream_id).await {
            Ok(trimmed_count) => {
                if trimmed_count > 0 {
                    println!(
                        "Trimmed ~{} messages from stream: {}",
                        trimmed_count, stream_key
                    );
                }
                total_trimmed += trimmed_count;
                streams_processed += 1;
            }
            Err(e) => {
                eprintln!("Failed to trim stream {}: {}", stream_key, e);
                // Continue processing other streams even if one fails
            }
        }
    }

    Ok(CleanupStats {
        streams_processed,
        messages_trimmed: total_trimmed,
    })
}

/// Calculate the cutoff timestamp for message retention (48 hours ago in milliseconds)
fn calculate_cutoff_timestamp() -> i64 {
    let now = chrono::Utc::now();
    let cutoff = now - chrono::Duration::hours(MESSAGE_RETENTION_HOURS);
    cutoff.timestamp_millis()
}

/// Scan Redis for all event stream keys matching the pattern smo:events:*
async fn scan_event_streams(
    conn: &mut redis::aio::MultiplexedConnection,
) -> Result<Vec<String>, String> {
    let pattern = "smo:events:*";

    // Use SCAN to iterate through keys matching the pattern
    let mut cursor = 0;
    let mut all_keys = Vec::new();

    loop {
        let (new_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg(pattern)
            .arg("COUNT")
            .arg(100)
            .query_async(conn)
            .await
            .map_err(|e| format!("SCAN command failed: {}", e))?;

        all_keys.extend(keys);
        cursor = new_cursor;

        // SCAN returns 0 when iteration is complete
        if cursor == 0 {
            break;
        }
    }

    Ok(all_keys)
}

/// Trim old messages from a Redis stream using XTRIM MINID
///
/// Returns the approximate number of messages trimmed
async fn trim_stream(
    conn: &mut redis::aio::MultiplexedConnection,
    stream_key: &str,
    min_id: &str,
) -> Result<usize, String> {
    // Get stream length before trimming
    let len_before: usize = redis::cmd("XLEN")
        .arg(stream_key)
        .query_async(conn)
        .await
        .unwrap_or(0);

    // XTRIM with MINID removes all entries with IDs lower than min_id
    let _trimmed: usize = redis::cmd("XTRIM")
        .arg(stream_key)
        .arg("MINID")
        .arg(min_id)
        .query_async(conn)
        .await
        .map_err(|e| format!("XTRIM command failed: {}", e))?;

    // Get stream length after trimming to calculate actual trimmed count
    let len_after: usize = redis::cmd("XLEN")
        .arg(stream_key)
        .query_async(conn)
        .await
        .unwrap_or(0);

    let actual_trimmed = len_before.saturating_sub(len_after);

    Ok(actual_trimmed)
}

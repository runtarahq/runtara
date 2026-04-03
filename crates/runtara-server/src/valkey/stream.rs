use redis::{RedisResult, Value, aio::ConnectionManager};
use std::collections::HashMap;

use super::events::ValkeyEvent;

/// Stream consumer that reads from Valkey streams using consumer groups
pub struct StreamConsumer {
    connection: ConnectionManager,
    stream_name: String,
    consumer_group: String,
    consumer_name: String,
}

impl StreamConsumer {
    /// Create a new stream consumer
    pub fn new(
        connection: ConnectionManager,
        stream_name: String,
        consumer_group: String,
        consumer_name: String,
    ) -> Self {
        StreamConsumer {
            connection,
            stream_name,
            consumer_group,
            consumer_name,
        }
    }

    /// Initialize the consumer group (creates if doesn't exist)
    /// This should be called before consuming
    pub async fn initialize_consumer_group(&mut self) -> RedisResult<()> {
        // Try to create consumer group
        // XGROUP CREATE stream group id MKSTREAM
        // Using $ means "start from the end" - only new messages after group creation
        let result: RedisResult<String> = redis::cmd("XGROUP")
            .arg("CREATE")
            .arg(&self.stream_name)
            .arg(&self.consumer_group)
            .arg("$") // Start from end of stream
            .arg("MKSTREAM") // Create stream if it doesn't exist
            .query_async(&mut self.connection)
            .await;

        match result {
            Ok(_) => {
                println!(
                    "✓ Created consumer group '{}' for stream '{}'",
                    self.consumer_group, self.stream_name
                );
                Ok(())
            }
            Err(e) => {
                let err_msg = format!("{}", e);
                if err_msg.contains("BUSYGROUP") {
                    // Group already exists, this is fine
                    println!(
                        "✓ Consumer group '{}' already exists for stream '{}'",
                        self.consumer_group, self.stream_name
                    );
                    Ok(())
                } else {
                    eprintln!("Failed to create consumer group: {}", e);
                    Err(e)
                }
            }
        }
    }

    /// Read events from the stream using XREADGROUP
    /// Returns a vector of (entry_id, event) tuples
    pub async fn read_events(
        &mut self,
        block_ms: usize,
        count: usize,
    ) -> RedisResult<Vec<(String, ValkeyEvent)>> {
        // XREADGROUP GROUP group consumer [COUNT count] [BLOCK milliseconds] STREAMS stream >
        // Using ">" means "only new messages not yet delivered to other consumers"
        let result: Value = redis::cmd("XREADGROUP")
            .arg("GROUP")
            .arg(&self.consumer_group)
            .arg(&self.consumer_name)
            .arg("COUNT")
            .arg(count)
            .arg("BLOCK")
            .arg(block_ms)
            .arg("STREAMS")
            .arg(&self.stream_name)
            .arg(">") // Only new messages
            .query_async(&mut self.connection)
            .await?;

        // Parse the response
        let events = parse_xreadgroup_response(result);

        Ok(events)
    }

    /// Acknowledge an event (XACK)
    pub async fn acknowledge_event(&mut self, entry_id: &str) -> RedisResult<()> {
        let _: i32 = redis::cmd("XACK")
            .arg(&self.stream_name)
            .arg(&self.consumer_group)
            .arg(entry_id)
            .query_async(&mut self.connection)
            .await?;

        Ok(())
    }

    /// Claim pending events that have been idle for at least `min_idle_ms` milliseconds.
    ///
    /// Uses XAUTOCLAIM to atomically claim messages that haven't been acknowledged
    /// within the idle time threshold. This is used for retrying failed/unacked events.
    ///
    /// Returns:
    /// - `Ok((events, next_start_id))` where events are the claimed messages and
    ///   next_start_id should be used for the next call (pagination)
    /// - Use "0-0" as the initial start_id to begin from the earliest pending message
    pub async fn claim_pending_events(
        &mut self,
        min_idle_ms: u64,
        count: usize,
        start_id: &str,
    ) -> RedisResult<(Vec<(String, ValkeyEvent)>, String)> {
        // XAUTOCLAIM stream group consumer min-idle-time start [COUNT count]
        // Response: [next-start-id, [[entry_id, [field, value, ...]], ...], [deleted-ids]]
        let result: Value = redis::cmd("XAUTOCLAIM")
            .arg(&self.stream_name)
            .arg(&self.consumer_group)
            .arg(&self.consumer_name)
            .arg(min_idle_ms)
            .arg(start_id)
            .arg("COUNT")
            .arg(count)
            .query_async(&mut self.connection)
            .await?;

        parse_xautoclaim_response(result)
    }

    /// Get the delivery count for a specific entry using XPENDING
    ///
    /// Returns the number of times this entry has been delivered.
    /// Returns 0 if the entry is not in the pending list.
    pub async fn get_delivery_count(&mut self, entry_id: &str) -> RedisResult<u64> {
        // XPENDING stream group [start] [end] [count] [consumer]
        // For a single entry, we query with start and end as the same ID
        let result: Value = redis::cmd("XPENDING")
            .arg(&self.stream_name)
            .arg(&self.consumer_group)
            .arg(entry_id) // start
            .arg(entry_id) // end (same as start for single entry)
            .arg(1) // count
            .query_async(&mut self.connection)
            .await?;

        // Response format: [[entry_id, consumer, idle_ms, delivery_count], ...]
        // If empty, the entry is not pending
        match result {
            Value::Array(entries) if !entries.is_empty() => {
                if let Value::Array(entry) = &entries[0] {
                    // entry = [id, consumer, idle_ms, delivery_count]
                    if entry.len() >= 4
                        && let Value::Int(count) = entry[3]
                    {
                        return Ok(count as u64);
                    }
                }
                Ok(0)
            }
            _ => Ok(0),
        }
    }
}

/// Parse XREADGROUP response into events
/// XREADGROUP returns: [[stream_name, [[entry_id, [field1, value1, field2, value2, ...]], ...]]]
fn parse_xreadgroup_response(value: Value) -> Vec<(String, ValkeyEvent)> {
    let mut events = Vec::new();

    // Response format: Bulk([Bulk([Data(stream_name), Bulk([Bulk([Data(id), Bulk([...])])])])])
    match value {
        Value::Array(streams) => {
            for stream in streams {
                if let Value::Array(stream_data) = stream
                    && stream_data.len() >= 2
                {
                    // stream_data[0] is stream name, stream_data[1] is entries
                    if let Value::Array(entries) = &stream_data[1] {
                        for entry in entries {
                            if let Value::Array(entry_data) = entry
                                && entry_data.len() >= 2
                            {
                                // entry_data[0] is entry ID, entry_data[1] is fields
                                let entry_id = match &entry_data[0] {
                                    Value::BulkString(bytes) => {
                                        String::from_utf8_lossy(bytes).to_string()
                                    }
                                    _ => continue,
                                };

                                let fields = match &entry_data[1] {
                                    Value::Array(field_values) => parse_fields(field_values),
                                    _ => continue,
                                };

                                let event = ValkeyEvent::from_stream_fields(fields)
                                    .with_event_id(entry_id.clone());

                                events.push((entry_id, event));
                            }
                        }
                    }
                }
            }
        }
        Value::Nil => {
            // No events (timeout)
        }
        _ => {
            eprintln!("Unexpected XREADGROUP response format");
        }
    }

    events
}

/// Parse field-value pairs from Redis bulk response
/// Input: [field1, value1, field2, value2, ...]
fn parse_fields(field_values: &[Value]) -> HashMap<String, String> {
    let mut fields = HashMap::new();

    let mut i = 0;
    while i + 1 < field_values.len() {
        let field = match &field_values[i] {
            Value::BulkString(bytes) => String::from_utf8_lossy(bytes).to_string(),
            _ => {
                i += 2;
                continue;
            }
        };

        let value = match &field_values[i + 1] {
            Value::BulkString(bytes) => String::from_utf8_lossy(bytes).to_string(),
            _ => {
                i += 2;
                continue;
            }
        };

        fields.insert(field, value);
        i += 2;
    }

    fields
}

/// Parse XAUTOCLAIM response into events and next cursor
/// XAUTOCLAIM returns: [next-start-id, [[entry_id, [field, value, ...]], ...], [deleted-ids]]
fn parse_xautoclaim_response(value: Value) -> RedisResult<(Vec<(String, ValkeyEvent)>, String)> {
    let mut events = Vec::new();
    let mut next_start_id = "0-0".to_string();

    match value {
        Value::Array(parts) if parts.len() >= 2 => {
            // First element: next-start-id for pagination
            if let Value::BulkString(bytes) = &parts[0] {
                next_start_id = String::from_utf8_lossy(bytes).to_string();
            }

            // Second element: array of claimed entries
            if let Value::Array(entries) = &parts[1] {
                for entry in entries {
                    if let Value::Array(entry_data) = entry
                        && entry_data.len() >= 2
                    {
                        // entry_data[0] is entry ID, entry_data[1] is fields
                        let entry_id = match &entry_data[0] {
                            Value::BulkString(bytes) => String::from_utf8_lossy(bytes).to_string(),
                            _ => continue,
                        };

                        let fields = match &entry_data[1] {
                            Value::Array(field_values) => parse_fields(field_values),
                            _ => continue,
                        };

                        let event =
                            ValkeyEvent::from_stream_fields(fields).with_event_id(entry_id.clone());

                        events.push((entry_id, event));
                    }
                }
            }

            // Third element (if present): deleted IDs - we ignore these
            // They represent messages that were in PEL but deleted from the stream
        }
        Value::Nil => {
            // No pending events
        }
        _ => {
            eprintln!("Unexpected XAUTOCLAIM response format: {:?}", value);
        }
    }

    Ok((events, next_start_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_xautoclaim_response_with_events() {
        // Simulate XAUTOCLAIM response:
        // [next-start-id, [[entry_id, [field, value, ...]], ...], [deleted-ids]]
        let response = Value::Array(vec![
            // next-start-id
            Value::BulkString(b"1234567890123-1".to_vec()),
            // Array of entries
            Value::Array(vec![Value::Array(vec![
                // entry_id
                Value::BulkString(b"1234567890123-0".to_vec()),
                // fields: [field, value, field, value, ...]
                Value::Array(vec![
                    Value::BulkString(b"event_type".to_vec()),
                    Value::BulkString(b"trigger_scenario".to_vec()),
                    Value::BulkString(b"data".to_vec()),
                    Value::BulkString(b"{\"scenario_id\":\"test-123\"}".to_vec()),
                ]),
            ])]),
            // deleted-ids (empty)
            Value::Array(vec![]),
        ]);

        let (events, next_id) = parse_xautoclaim_response(response).unwrap();

        assert_eq!(next_id, "1234567890123-1");
        assert_eq!(events.len(), 1);

        let (entry_id, event) = &events[0];
        assert_eq!(entry_id, "1234567890123-0");
        assert_eq!(event.event_id, Some("1234567890123-0".to_string()));
        assert_eq!(event.event_type, Some("trigger_scenario".to_string()));
        assert_eq!(
            event.raw_data.get("data"),
            Some(&"{\"scenario_id\":\"test-123\"}".to_string())
        );
    }

    #[test]
    fn test_parse_xautoclaim_response_empty() {
        // XAUTOCLAIM with no pending events returns next-id with empty entries
        let response = Value::Array(vec![
            Value::BulkString(b"0-0".to_vec()),
            Value::Array(vec![]),
            Value::Array(vec![]),
        ]);

        let (events, next_id) = parse_xautoclaim_response(response).unwrap();

        assert_eq!(next_id, "0-0");
        assert!(events.is_empty());
    }

    #[test]
    fn test_parse_xautoclaim_response_nil() {
        // XAUTOCLAIM can return Nil when no pending events
        let response = Value::Nil;

        let (events, next_id) = parse_xautoclaim_response(response).unwrap();

        assert_eq!(next_id, "0-0");
        assert!(events.is_empty());
    }

    #[test]
    fn test_parse_xautoclaim_response_multiple_events() {
        // Multiple pending events
        let response = Value::Array(vec![
            Value::BulkString(b"1234567890123-5".to_vec()),
            Value::Array(vec![
                Value::Array(vec![
                    Value::BulkString(b"1234567890123-0".to_vec()),
                    Value::Array(vec![
                        Value::BulkString(b"event_type".to_vec()),
                        Value::BulkString(b"trigger".to_vec()),
                    ]),
                ]),
                Value::Array(vec![
                    Value::BulkString(b"1234567890123-1".to_vec()),
                    Value::Array(vec![
                        Value::BulkString(b"event_type".to_vec()),
                        Value::BulkString(b"webhook".to_vec()),
                    ]),
                ]),
                Value::Array(vec![
                    Value::BulkString(b"1234567890123-2".to_vec()),
                    Value::Array(vec![
                        Value::BulkString(b"event_type".to_vec()),
                        Value::BulkString(b"cron".to_vec()),
                    ]),
                ]),
            ]),
            Value::Array(vec![]),
        ]);

        let (events, next_id) = parse_xautoclaim_response(response).unwrap();

        assert_eq!(next_id, "1234567890123-5");
        assert_eq!(events.len(), 3);

        assert_eq!(events[0].0, "1234567890123-0");
        assert_eq!(events[0].1.event_type, Some("trigger".to_string()));

        assert_eq!(events[1].0, "1234567890123-1");
        assert_eq!(events[1].1.event_type, Some("webhook".to_string()));

        assert_eq!(events[2].0, "1234567890123-2");
        assert_eq!(events[2].1.event_type, Some("cron".to_string()));
    }

    #[test]
    fn test_parse_xreadgroup_response_with_events() {
        // XREADGROUP returns: [[stream_name, [[entry_id, [field, value, ...]], ...]]]
        let response = Value::Array(vec![Value::Array(vec![
            // stream name
            Value::BulkString(b"mystream".to_vec()),
            // entries
            Value::Array(vec![Value::Array(vec![
                Value::BulkString(b"1234567890123-0".to_vec()),
                Value::Array(vec![
                    Value::BulkString(b"event_type".to_vec()),
                    Value::BulkString(b"trigger_scenario".to_vec()),
                    Value::BulkString(b"scenario_id".to_vec()),
                    Value::BulkString(b"abc-123".to_vec()),
                ]),
            ])]),
        ])]);

        let events = parse_xreadgroup_response(response);

        assert_eq!(events.len(), 1);
        let (entry_id, event) = &events[0];
        assert_eq!(entry_id, "1234567890123-0");
        assert_eq!(event.event_type, Some("trigger_scenario".to_string()));
        assert_eq!(event.scenario_id, Some("abc-123".to_string()));
    }

    #[test]
    fn test_parse_xreadgroup_response_nil() {
        // XREADGROUP returns Nil on timeout with no events
        let response = Value::Nil;
        let events = parse_xreadgroup_response(response);
        assert!(events.is_empty());
    }

    #[test]
    fn test_parse_fields() {
        let field_values = vec![
            Value::BulkString(b"key1".to_vec()),
            Value::BulkString(b"value1".to_vec()),
            Value::BulkString(b"key2".to_vec()),
            Value::BulkString(b"value2".to_vec()),
        ];

        let fields = parse_fields(&field_values);

        assert_eq!(fields.get("key1"), Some(&"value1".to_string()));
        assert_eq!(fields.get("key2"), Some(&"value2".to_string()));
    }

    #[test]
    fn test_parse_fields_empty() {
        let field_values: Vec<Value> = vec![];
        let fields = parse_fields(&field_values);
        assert!(fields.is_empty());
    }

    #[test]
    fn test_parse_fields_odd_count() {
        // If there's an odd number of elements, the last one is ignored
        let field_values = vec![
            Value::BulkString(b"key1".to_vec()),
            Value::BulkString(b"value1".to_vec()),
            Value::BulkString(b"orphan".to_vec()),
        ];

        let fields = parse_fields(&field_values);

        assert_eq!(fields.len(), 1);
        assert_eq!(fields.get("key1"), Some(&"value1".to_string()));
    }

    // =========================================================================
    // get_delivery_count response parsing tests
    // =========================================================================
    // Note: get_delivery_count is an async method that requires a Redis connection.
    // These tests verify the parsing logic that happens inside the method.
    // The actual parsing is inline, but we can test the Value patterns it expects.

    /// Helper to simulate parsing the XPENDING response for delivery count
    /// This mirrors the logic in get_delivery_count
    fn parse_delivery_count_from_xpending(result: Value) -> u64 {
        match result {
            Value::Array(entries) if !entries.is_empty() => {
                if let Value::Array(entry) = &entries[0]
                    && entry.len() >= 4
                    && let Value::Int(count) = entry[3]
                {
                    return count as u64;
                }
                0
            }
            _ => 0,
        }
    }

    #[test]
    fn test_parse_delivery_count_with_entry() {
        // XPENDING response for single entry: [[entry_id, consumer, idle_ms, delivery_count], ...]
        let response = Value::Array(vec![Value::Array(vec![
            Value::BulkString(b"1234567890123-0".to_vec()), // entry_id
            Value::BulkString(b"consumer-1".to_vec()),      // consumer name
            Value::Int(5000),                               // idle time in ms
            Value::Int(3),                                  // delivery count
        ])]);

        let count = parse_delivery_count_from_xpending(response);
        assert_eq!(count, 3);
    }

    #[test]
    fn test_parse_delivery_count_high_value() {
        // Test with a high delivery count (edge case for retry exhaustion)
        let response = Value::Array(vec![Value::Array(vec![
            Value::BulkString(b"1234567890123-0".to_vec()),
            Value::BulkString(b"consumer-1".to_vec()),
            Value::Int(300000), // 5 minutes idle
            Value::Int(10),     // 10 deliveries (max_retries threshold)
        ])]);

        let count = parse_delivery_count_from_xpending(response);
        assert_eq!(count, 10);
    }

    #[test]
    fn test_parse_delivery_count_empty_response() {
        // Entry not in pending list - empty array
        let response = Value::Array(vec![]);

        let count = parse_delivery_count_from_xpending(response);
        assert_eq!(count, 0);
    }

    #[test]
    fn test_parse_delivery_count_nil_response() {
        // Nil response (shouldn't happen but handle gracefully)
        let response = Value::Nil;

        let count = parse_delivery_count_from_xpending(response);
        assert_eq!(count, 0);
    }

    #[test]
    fn test_parse_delivery_count_malformed_entry() {
        // Entry with fewer than 4 elements
        let response = Value::Array(vec![Value::Array(vec![
            Value::BulkString(b"1234567890123-0".to_vec()),
            Value::BulkString(b"consumer-1".to_vec()),
            // Missing idle_ms and delivery_count
        ])]);

        let count = parse_delivery_count_from_xpending(response);
        assert_eq!(count, 0);
    }

    #[test]
    fn test_parse_delivery_count_wrong_type_for_count() {
        // delivery_count field is a string instead of int
        let response = Value::Array(vec![Value::Array(vec![
            Value::BulkString(b"1234567890123-0".to_vec()),
            Value::BulkString(b"consumer-1".to_vec()),
            Value::Int(5000),
            Value::BulkString(b"3".to_vec()), // Wrong type - string instead of int
        ])]);

        let count = parse_delivery_count_from_xpending(response);
        assert_eq!(count, 0);
    }

    #[test]
    fn test_parse_delivery_count_first_delivery() {
        // First delivery - count should be 1
        let response = Value::Array(vec![Value::Array(vec![
            Value::BulkString(b"1234567890123-0".to_vec()),
            Value::BulkString(b"consumer-1".to_vec()),
            Value::Int(100),
            Value::Int(1), // First delivery
        ])]);

        let count = parse_delivery_count_from_xpending(response);
        assert_eq!(count, 1);
    }
}

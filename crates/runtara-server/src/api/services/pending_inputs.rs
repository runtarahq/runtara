//! Shared matching of `external_input_requested` events against the
//! `step_debug_end` events that resolve them.
//!
//! Every caller that needs to know whether a running instance is still waiting
//! on human input derives it from the same two event streams, so the pairing
//! lives here rather than being reimplemented per call site — the executions
//! list, the per-instance pending-input endpoint and the workflow action APIs
//! must not disagree about the same run.
//!
//! Pairing cannot be done on timestamps alone: `step_debug_end` is emitted for
//! every step type, not just the ones that requested input, and branches
//! genuinely overlap (Split carries a `parallelism` setting), so a step that
//! ends after a request says nothing about whether that request was answered.

use std::collections::HashSet;

use runtara_management_sdk::{EventSortOrder, EventSummary, ListEventsOptions};
use serde_json::Value;

use crate::runtime_client::RuntimeClient;

/// Fetch the `external_input_requested` and `step_debug_end` events for an
/// instance, oldest request first.
///
/// A failed request lookup is an error — the caller decides what an unknown
/// state means. A failed *end* lookup degrades to "nothing has completed",
/// which keeps open requests visible rather than hiding them.
pub async fn fetch_input_and_end_events(
    client: &RuntimeClient,
    instance_id: &str,
) -> Result<(Vec<EventSummary>, Vec<EventSummary>), String> {
    let input_options = ListEventsOptions::new()
        .with_limit(100)
        .with_event_type("custom")
        .with_subtype("external_input_requested")
        .with_sort_order(EventSortOrder::Asc);

    let input_events = client
        .list_events(instance_id, Some(input_options))
        .await
        .map_err(|error| error.to_string())?
        .events;

    let end_options = ListEventsOptions::new()
        .with_limit(1000)
        .with_event_type("custom")
        .with_subtype("step_debug_end");

    let end_events = client
        .list_events(instance_id, Some(end_options))
        .await
        .map(|result| result.events)
        .unwrap_or_default();

    Ok((input_events, end_events))
}

/// The subset of `input_events` that no `step_debug_end` event has resolved.
///
/// AI Agent tool calls complete under the synthetic step id
/// `{ai_step_id}.tool.{tool_name}.{call_number}`. Standalone waits are matched
/// by SIGNAL id instead — signal ids are per-invocation-site (one child step
/// can wait at several sites in one instance, embedded or composed), so a
/// completed wait at one site must not hide another site's open wait on the
/// same step id.
pub fn open_input_events<'a>(
    input_events: &'a [EventSummary],
    end_events: &[EventSummary],
) -> Vec<&'a EventSummary> {
    let completed_step_ids: HashSet<&str> = end_events
        .iter()
        .filter_map(|event| {
            event
                .payload
                .as_ref()
                .and_then(|payload| payload.get("step_id"))
                .and_then(Value::as_str)
        })
        .collect();

    // A resolved WaitForSignal's `step_debug_end` carries its signal id inside
    // the outputs envelope.
    let completed_signal_ids: HashSet<&str> = end_events
        .iter()
        .filter_map(|event| {
            event
                .payload
                .as_ref()
                .and_then(|payload| payload.get("outputs"))
                .and_then(|outputs| outputs.get("signal_id"))
                .and_then(Value::as_str)
        })
        .collect();

    input_events
        .iter()
        .filter(|event| {
            let Some(payload) = event.payload.as_ref() else {
                return false;
            };
            let Some(signal_id) = payload.get("signal_id").and_then(Value::as_str) else {
                return false;
            };

            match (
                payload.get("ai_agent_step_id").and_then(Value::as_str),
                payload.get("tool_name").and_then(Value::as_str),
                payload.get("call_number").and_then(Value::as_u64),
            ) {
                (Some(step), Some(tool), Some(number)) => !completed_step_ids
                    .contains(format!("{}.tool.{}.{}", step, tool, number).as_str()),
                _ => !completed_signal_ids.contains(signal_id),
            }
        })
        .collect()
}

/// Whether the instance still has at least one unresolved input request.
pub async fn has_open_inputs(client: &RuntimeClient, instance_id: &str) -> Result<bool, String> {
    let (input_events, end_events) = fetch_input_and_end_events(client, instance_id).await?;
    Ok(!open_input_events(&input_events, &end_events).is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, TimeZone, Utc};
    use serde_json::json;

    fn at(seconds: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + seconds, 0).unwrap()
    }

    fn event(id: i64, subtype: &str, created_at: DateTime<Utc>, payload: Value) -> EventSummary {
        EventSummary {
            id,
            instance_id: "instance-1".to_string(),
            event_type: "custom".to_string(),
            checkpoint_id: None,
            payload: Some(payload),
            created_at,
            subtype: Some(subtype.to_string()),
        }
    }

    /// A standalone wait, identified by its per-invocation-site signal id.
    fn wait_request(id: i64, seconds: i64, step_id: &str, signal_id: &str) -> EventSummary {
        event(
            id,
            "external_input_requested",
            at(seconds),
            json!({ "step_id": step_id, "signal_id": signal_id }),
        )
    }

    fn wait_resolved(id: i64, seconds: i64, step_id: &str, signal_id: &str) -> EventSummary {
        event(
            id,
            "step_debug_end",
            at(seconds),
            json!({ "step_id": step_id, "outputs": { "signal_id": signal_id } }),
        )
    }

    fn signal_ids(events: &[&EventSummary]) -> Vec<String> {
        events
            .iter()
            .map(|event| {
                event.payload.as_ref().unwrap()["signal_id"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn concurrent_split_iterations_stay_open_when_one_is_answered() {
        // Four iterations of one Split step, each waiting on an approval at its
        // own invocation site.
        let requests: Vec<EventSummary> = (1..=4)
            .map(|iteration| {
                wait_request(
                    iteration,
                    iteration,
                    "approve",
                    &format!("signal-{}", iteration),
                )
            })
            .collect();

        // Iteration 1 is approved, and its end event lands after every other
        // iteration's request — the trap a latest-timestamp comparison falls
        // into.
        let ends = vec![wait_resolved(10, 20, "approve", "signal-1")];

        let open = open_input_events(&requests, &ends);

        assert_eq!(signal_ids(&open), vec!["signal-2", "signal-3", "signal-4"]);
    }

    #[test]
    fn unrelated_step_completions_do_not_resolve_a_request() {
        let requests = vec![wait_request(1, 1, "approve", "signal-1")];
        // A plain step finishing later carries neither a matching signal id nor
        // a synthetic tool step id.
        let ends = vec![event(
            2,
            "step_debug_end",
            at(30),
            json!({ "step_id": "fetch_orders", "outputs": { "count": 12 } }),
        )];

        let open = open_input_events(&requests, &ends);

        assert_eq!(signal_ids(&open), vec!["signal-1"]);
    }

    #[test]
    fn agent_tool_calls_match_on_the_synthetic_step_id() {
        let tool_request = |id: i64, call_number: u64, signal_id: &str| {
            event(
                id,
                "external_input_requested",
                at(id),
                json!({
                    "signal_id": signal_id,
                    "ai_agent_step_id": "agent",
                    "tool_name": "ask_human",
                    "call_number": call_number,
                }),
            )
        };
        let requests = vec![
            tool_request(1, 1, "signal-1"),
            tool_request(2, 2, "signal-2"),
        ];

        // Only the first call completed; the same tool at a later call number
        // is a different request.
        let ends = vec![event(
            3,
            "step_debug_end",
            at(30),
            json!({ "step_id": "agent.tool.ask_human.1" }),
        )];

        let open = open_input_events(&requests, &ends);

        assert_eq!(signal_ids(&open), vec!["signal-2"]);
    }

    #[test]
    fn a_completed_wait_does_not_hide_another_site_on_the_same_step() {
        // One child step waiting at two invocation sites — the step ids are
        // identical, only the signal ids differ.
        let requests = vec![
            wait_request(1, 1, "approve", "signal-site-a"),
            wait_request(2, 2, "approve", "signal-site-b"),
        ];
        let ends = vec![wait_resolved(3, 3, "approve", "signal-site-a")];

        let open = open_input_events(&requests, &ends);

        assert_eq!(signal_ids(&open), vec!["signal-site-b"]);
    }

    #[test]
    fn every_request_is_open_without_end_events() {
        let requests = vec![
            wait_request(1, 1, "approve", "signal-1"),
            wait_request(2, 2, "approve", "signal-2"),
        ];

        let open = open_input_events(&requests, &[]);

        assert_eq!(signal_ids(&open), vec!["signal-1", "signal-2"]);
    }

    #[test]
    fn answering_the_last_open_request_leaves_nothing() {
        let requests = vec![
            wait_request(1, 1, "approve", "signal-1"),
            wait_request(2, 2, "approve", "signal-2"),
        ];
        let ends = vec![
            wait_resolved(3, 3, "approve", "signal-1"),
            wait_resolved(4, 4, "approve", "signal-2"),
        ];

        assert!(open_input_events(&requests, &ends).is_empty());
    }
}

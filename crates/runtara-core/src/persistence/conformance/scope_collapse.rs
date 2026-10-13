//! Scope collapse cases shared by every durable backend: recording a
//! composite step's result deletes the rows its closure names, in the same
//! transaction (see `docs/checkpoint-scope-collapse.md`).
use crate::domain::InstanceStatus;
use crate::persistence::invocations::{InvocationCheckpoint, InvocationFences, InvocationLease};
use crate::persistence::{CheckpointWrite, Persistence};
use serde_json::{Value, json};
use std::collections::BTreeSet;

const TENANT: &str = "scope-collapse";

async fn running(p: &dyn Persistence, name: &str) -> String {
    let id = format!("scope-collapse-{name}-{}", uuid::Uuid::new_v4());
    p.register_instance(&id, TENANT).await.unwrap();
    p.update_instance_status(&id, InstanceStatus::Running, None)
        .await
        .unwrap();
    id
}

/// A structured checkpoint key, shaped as the workflow stdlib writes it. A
/// fixture the grammar no longer reads would have no address and make every
/// case pass vacuously, so it fails here instead.
fn key(kind: &str, frames: Value, loops: Value, parts: Value) -> String {
    let key = format!("runtara:v2:{}", json!([kind, "wf", frames, loops, parts]));
    assert!(
        crate::persistence::checkpoint_scope::scope(&key).is_some(),
        "fixture key has no address: {key}"
    );
    key
}

/// The result row of Agent `step` in iteration `index` of Split `split`.
fn item(split: &str, index: u32, step: &str) -> String {
    key(
        "agent",
        json!([]),
        json!([["Split", split, index]]),
        json!(["utils", "random-double", step]),
    )
}

fn split(loops: Value, step: &str) -> String {
    key("split", json!([]), loops, json!([step]))
}

async fn save_all(p: &dyn Persistence, id: &str, keys: &[String]) {
    for key in keys {
        p.save_checkpoint(id, key, b"x").await.unwrap();
    }
}

async fn stored(p: &dyn Persistence, id: &str) -> BTreeSet<String> {
    p.list_checkpoints(id, None, 1000, 0, None, None)
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.checkpoint_id)
        .collect()
}

fn set<'a>(keys: impl IntoIterator<Item = &'a String>) -> BTreeSet<String> {
    keys.into_iter().cloned().collect()
}

/// Recording a Split's result deletes its iterations, including a child
/// namespace opened inside one, and the Split's own attempt and deadline
/// rows. Everything else stays: the result itself, the rows around the
/// Split, another Split's iterations, rows without an address, and another
/// instance's rows. Progress keeps rising.
pub async fn recording_a_split_result_collapses_its_iterations(p: &dyn Persistence) {
    let id = running(p, "split").await;
    let other = running(p, "split-other").await;
    let result = split(json!([]), "items");
    let site = json!([["Split", "items", 4]]);
    let inside: Vec<String> = (0..12)
        .map(|i| item("items", i, "each"))
        .chain([
            key("embed_workflow", json!([]), site.clone(), json!(["embed"])),
            key(
                "agent",
                json!([["child", "wf", site, ["embed"]]]),
                json!([]),
                json!(["utils", "random-double", "c1"]),
            ),
            format!("{result}::attempt::1"),
            format!("{result}::retry_sleep::2"),
            key(
                "loop-deadline",
                json!([]),
                json!([]),
                json!(["Split", "root", "items"]),
            ),
            key(
                "loop-complete",
                json!([]),
                json!([]),
                json!(["Split", "root", "items"]),
            ),
        ])
        .collect();
    let outside = vec![
        key(
            "agent",
            json!([]),
            json!([]),
            json!(["utils", "random-double", "before"]),
        ),
        item("items2", 0, "each"),
        key("wait", json!([]), json!([]), json!(["instance", "items"])),
        "wf::agent::utils::random-double::each::[0]".to_string(),
    ];
    save_all(p, &id, &inside).await;
    save_all(p, &id, &outside).await;
    save_all(p, &other, &inside).await;
    let progress = p.checkpoint_progress(&id).await.unwrap();

    assert_eq!(
        p.record_checkpoint(&id, &result, b"all", None)
            .await
            .unwrap(),
        CheckpointWrite::Recorded
    );

    let mut expected = set(&outside);
    expected.insert(result.clone());
    assert_eq!(stored(p, &id).await, expected);
    assert_eq!(
        stored(p, &other).await,
        set(&inside),
        "another instance's rows are untouched"
    );
    assert!(
        p.checkpoint_progress(&id).await.unwrap() > progress,
        "recording the result raises progress although rows were deleted"
    );
    p.delete_instances_batch(&[id, other]).await.unwrap();
}

/// A failed Split attempt is a suffixed row with no closure: recording it
/// keeps the items a retry will reuse.
pub async fn a_failed_split_attempt_collapses_nothing(p: &dyn Persistence) {
    let id = running(p, "attempt").await;
    let result = split(json!([]), "items");
    let items: Vec<String> = (0..3).map(|i| item("items", i, "each")).collect();
    save_all(p, &id, &items).await;

    p.record_checkpoint(&id, &format!("{result}::attempt::1"), b"error", None)
        .await
        .unwrap();

    let mut expected = set(&items);
    expected.insert(format!("{result}::attempt::1"));
    assert_eq!(stored(p, &id).await, expected);
    p.delete_instances_batch(&[id]).await.unwrap();
}

/// A Split inside another Split's iteration collapses into its own result
/// first; the outer result then takes the rest, inner result included.
pub async fn nested_splits_collapse_level_by_level(p: &dyn Persistence) {
    let id = running(p, "nested").await;
    let outer_items: Vec<String> = (0..3).map(|i| item("outer", i, "x")).collect();
    let inner_items: Vec<String> = (0..4)
        .map(|j| {
            key(
                "agent",
                json!([]),
                json!([["Split", "outer", 1], ["Split", "inner", j]]),
                json!(["utils", "random-double", "y"]),
            )
        })
        .collect();
    let inner_result = split(json!([["Split", "outer", 1]]), "inner");
    let outer_result = split(json!([]), "outer");
    save_all(p, &id, &outer_items).await;
    save_all(p, &id, &inner_items).await;

    p.record_checkpoint(&id, &inner_result, b"inner", None)
        .await
        .unwrap();
    let mut expected = set(&outer_items);
    expected.insert(inner_result);
    assert_eq!(
        stored(p, &id).await,
        expected,
        "only the inner Split's iterations are gone"
    );

    p.record_checkpoint(&id, &outer_result, b"outer", None)
        .await
        .unwrap();
    assert_eq!(stored(p, &id).await, set([&outer_result]));
    p.delete_instances_batch(&[id]).await.unwrap();
}

/// Only the call that stores the result collapses: one that finds it stored
/// writes nothing and deletes nothing.
pub async fn a_result_found_stored_collapses_nothing(p: &dyn Persistence) {
    let id = running(p, "existing").await;
    let result = split(json!([]), "items");
    p.record_checkpoint(&id, &result, b"first", None)
        .await
        .unwrap();
    let late = item("items", 0, "each");
    p.save_checkpoint(&id, &late, b"x").await.unwrap();

    assert_eq!(
        p.record_checkpoint(&id, &result, b"second", None)
            .await
            .unwrap(),
        CheckpointWrite::Existing(b"first".to_vec())
    );
    assert_eq!(stored(p, &id).await, set([&result, &late]));
    p.delete_instances_batch(&[id]).await.unwrap();
}

fn fences(p: &dyn Persistence) -> &dyn InvocationFences {
    p.invocation_fences()
        .expect("a durable backend provides invocation fences")
}

async fn claim(p: &dyn Persistence, id: &str) -> InvocationLease {
    fences(p)
        .claim_invocation_lease(TENANT, id, "runner", None)
        .await
        .unwrap()
}

/// A Split inside an isolated child invocation records its result through
/// the fenced path, which collapses the child's iterations and leaves the
/// parent's rows, including a parent Split with the same step id.
pub async fn a_fenced_split_result_collapses_its_iterations(p: &dyn Persistence) {
    let id = running(p, "fenced").await;
    let lease = claim(p, &id).await;
    let attempt = fences(p)
        .begin_invocation_attempt(&lease, "child", "start")
        .await
        .unwrap()
        .fence;
    let frame = json!([["child", "wf", [], ["call"]]]);
    let child_items: Vec<String> = (0..3)
        .map(|i| {
            key(
                "agent",
                frame.clone(),
                json!([["Split", "items", i]]),
                json!(["utils", "random-double", "each"]),
            )
        })
        .collect();
    let child_result = key("split", frame, json!([]), json!(["items"]));
    let parent_items: Vec<String> = (0..2).map(|i| item("items", i, "each")).collect();
    save_all(p, &id, &parent_items).await;
    for key in &child_items {
        fences(p)
            .invocation_checkpoint(
                &attempt,
                &InvocationCheckpoint {
                    checkpoint_id: key.clone(),
                    state: b"x".to_vec(),
                },
            )
            .await
            .unwrap();
    }

    fences(p)
        .invocation_checkpoint(
            &attempt,
            &InvocationCheckpoint {
                checkpoint_id: child_result.clone(),
                state: b"all".to_vec(),
            },
        )
        .await
        .unwrap();

    let mut expected = set(&parent_items);
    expected.insert(child_result);
    assert_eq!(stored(p, &id).await, expected);
    p.delete_instances_batch(&[id]).await.unwrap();
}

/// Every case in this module.
pub async fn run_all(p: &dyn Persistence) {
    recording_a_split_result_collapses_its_iterations(p).await;
    a_failed_split_attempt_collapses_nothing(p).await;
    nested_splits_collapse_level_by_level(p).await;
    a_result_found_stored_collapses_nothing(p).await;
    a_fenced_split_result_collapses_its_iterations(p).await;
}

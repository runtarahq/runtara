//! Scope collapse cases shared by every durable backend: recording a
//! composite step's result deletes the rows its closure names, in the same
//! transaction (see `docs/checkpoint-scope-collapse.md`).
use crate::domain::InstanceStatus;
use crate::persistence::checkpoint_scope::SharedChildSteps;
use crate::persistence::invocations::{InvocationCheckpoint, InvocationFences, InvocationLease};
use crate::persistence::{CheckpointWrite, Persistence};
use serde_json::{Value, json};
use std::collections::BTreeSet;

const TENANT: &str = "scope-collapse";

/// A running instance whose steps share no child namespace, as Environment
/// records for a workflow without `onWait` graphs.
async fn running(p: &dyn Persistence, name: &str) -> String {
    running_with(p, name, Some(SharedChildSteps::known::<&str>([]))).await
}

/// A running instance that records `shared`, or nothing (unknown) for `None`.
async fn running_with(p: &dyn Persistence, name: &str, shared: Option<SharedChildSteps>) -> String {
    let id = format!("scope-collapse-{name}-{}", uuid::Uuid::new_v4());
    p.register_instance(&id, TENANT).await.unwrap();
    p.update_instance_status(&id, InstanceStatus::Running, None)
        .await
        .unwrap();
    if let Some(shared) = shared {
        p.set_shared_child_steps(&id, &shared).await.unwrap();
    }
    id
}

/// A structured checkpoint key, shaped as the workflow stdlib writes it. A
/// fixture the grammar no longer reads would have no address and make every
/// case pass vacuously, so it fails here instead.
fn key(kind: &str, frames: Value, loops: Value, parts: Value) -> String {
    key_in("wf", kind, frames, loops, parts)
}

/// [`key`] in another workflow, as a workflow-agent's child writes it.
fn key_in(workflow: &str, kind: &str, frames: Value, loops: Value, parts: Value) -> String {
    let key = format!(
        "runtara:v2:{}",
        json!([kind, workflow, frames, loops, parts])
    );
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

/// Recording a While's result deletes its iterations, a Split nested in one
/// included, the local state its body checkpointed, and its own deadline
/// rows. The rows around the loop and another While's iterations stay.
pub async fn recording_a_while_result_collapses_its_iterations(p: &dyn Persistence) {
    let id = running(p, "while").await;
    let result = key("while", json!([]), json!([]), json!(["poll"]));
    let iteration = |index: u32| json!([["While", "poll", index]]);
    let inside: Vec<String> = (0..20)
        .map(|i| {
            key(
                "agent",
                json!([]),
                iteration(i),
                json!(["utils", "random-double", "check"]),
            )
        })
        .chain([
            key("split", json!([]), iteration(3), json!(["fan"])),
            key(
                "agent",
                json!([]),
                json!([["While", "poll", 3], ["Split", "fan", 1]]),
                json!(["utils", "random-double", "each"]),
            ),
            key("state-local", json!([]), iteration(7), json!(["set"])),
            key(
                "loop-deadline",
                json!([]),
                json!([]),
                json!(["While", "root", "poll"]),
            ),
            key(
                "loop-complete",
                json!([]),
                json!([]),
                json!(["While", "root", "poll"]),
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
        key(
            "agent",
            json!([]),
            json!([["While", "other", 0]]),
            json!(["utils", "random-double", "check"]),
        ),
        key(
            "loop-deadline",
            json!([]),
            json!([]),
            json!(["Split", "root", "poll"]),
        ),
    ];
    save_all(p, &id, &inside).await;
    save_all(p, &id, &outside).await;

    assert_eq!(
        p.record_checkpoint(&id, &result, b"{}", None)
            .await
            .unwrap(),
        CheckpointWrite::Recorded
    );

    let mut expected = set(&outside);
    expected.insert(result.clone());
    assert_eq!(stored(p, &id).await, expected);
    p.delete_instances_batch(&[id]).await.unwrap();
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

/// The frame a child invoked by step `step` at loop path `loops` (an
/// EmbedWorkflow's, or a workflow-agent's called from an Agent) writes under.
fn child(loops: Value, step: &str) -> Value {
    json!(["child", "wf", loops, [step]])
}

fn embed(loops: Value, step: &str) -> String {
    key("embed_workflow", json!([]), loops, json!([step]))
}

/// The result row of Agent `step` inside `frames`.
fn agent_in(frames: Value, loops: Value, step: &str) -> String {
    key(
        "agent",
        frames,
        loops,
        json!(["utils", "random-double", step]),
    )
}

/// Recording an EmbedWorkflow's result deletes its child's rows, a nested
/// child's included, and the Embed's own attempt, retry and deadline rows.
/// The result stays, and so do the rows around it, another Embed's child,
/// the same Embed in an iteration, and another instance's rows.
pub async fn recording_an_embed_result_collapses_its_child(p: &dyn Persistence) {
    let id = running(p, "embed").await;
    let other = running(p, "embed-other").await;
    let result = embed(json!([]), "embed");
    let frame = json!([child(json!([]), "embed")]);
    let inside = vec![
        agent_in(frame.clone(), json!([]), "c1"),
        agent_in(frame.clone(), json!([["Split", "items", 2]]), "each"),
        key("split", frame, json!([]), json!(["items"])),
        agent_in(
            json!([child(json!([]), "embed"), child(json!([]), "inner")]),
            json!([]),
            "deep",
        ),
        format!("{result}::attempt::1"),
        format!("{result}::retry::1"),
        format!("{result}::retry_sleep::2"),
        key(
            "embed-deadline",
            json!([]),
            json!([]),
            json!(["EmbedWorkflow", "root", "embed"]),
        ),
    ];
    let outside = vec![
        agent_in(json!([]), json!([]), "before"),
        agent_in(json!([child(json!([]), "embed2")]), json!([]), "c1"),
        agent_in(
            json!([child(json!([["Split", "s", 0]]), "embed")]),
            json!([]),
            "c1",
        ),
        embed(json!([["Split", "s", 0]]), "embed"),
        key(
            "embed-deadline",
            json!([]),
            json!([]),
            json!(["EmbedWorkflow", "root", "embed2"]),
        ),
    ];
    save_all(p, &id, &inside).await;
    save_all(p, &id, &outside).await;
    save_all(p, &other, &inside).await;

    assert_eq!(
        p.record_checkpoint(&id, &result, b"child output", None)
            .await
            .unwrap(),
        CheckpointWrite::Recorded
    );

    let mut expected = set(&outside);
    expected.insert(result);
    assert_eq!(stored(p, &id).await, expected);
    assert_eq!(
        stored(p, &other).await,
        set(&inside),
        "another instance's rows are untouched"
    );
    p.delete_instances_batch(&[id, other]).await.unwrap();
}

/// Recording an Agent's result deletes its own attempt, retry and retry sleep
/// rows and, for a workflow-agent, every row its child wrote under the call.
/// Its deadline row (untagged), another Agent reusing its step id at the same
/// position, and an AiAgent's tool and memory calls stay.
pub async fn recording_an_agent_result_collapses_its_child_and_retries(p: &dyn Persistence) {
    let id = running(p, "agent").await;
    let result = key(
        "agent",
        json!([]),
        json!([]),
        json!(["orders-sync", "run", "call"]),
    );
    let frame = json!([child(json!([]), "call")]);
    let inside = vec![
        key_in(
            "orders-sync",
            "agent",
            frame.clone(),
            json!([]),
            json!(["utils", "random-double", "c1"]),
        ),
        key_in("orders-sync", "split", frame, json!([]), json!(["items"])),
        format!("{result}::attempt::1"),
        format!("{result}::retry::1"),
        format!("{result}::retry_sleep::2"),
    ];
    let reused = key(
        "agent",
        json!([]),
        json!([]),
        json!(["slack", "post", "call"]),
    );
    let outside = vec![
        key(
            "agent-deadline",
            json!([]),
            json!([]),
            json!(["Agent", "root", "call"]),
        ),
        reused.clone(),
        format!("{reused}::attempt::1"),
        agent_in(
            json!([["tool-child", "wf", [], ["call", "lookup", 0]]]),
            json!([]),
            "t1",
        ),
        agent_in(
            json!([["agent-aux", "wf", [], ["call", "memory", "load-memory"]]]),
            json!([]),
            "call",
        ),
        agent_in(json!([]), json!([]), "next"),
    ];
    save_all(p, &id, &inside).await;
    save_all(p, &id, &outside).await;

    assert_eq!(
        p.record_checkpoint(&id, &result, b"child output", None)
            .await
            .unwrap(),
        CheckpointWrite::Recorded
    );

    let mut expected = set(&outside);
    expected.insert(result);
    assert_eq!(stored(p, &id).await, expected);
    p.delete_instances_batch(&[id]).await.unwrap();
}

/// Child scopes collapse level by level: a Split and an Embed inside an
/// Embed's child each take their own rows first, and the outer Embed's
/// result then takes the rest, their results included.
pub async fn nested_child_scopes_collapse_level_by_level(p: &dyn Persistence) {
    let id = running(p, "nested-child").await;
    let frame = json!([child(json!([]), "outer")]);
    let iterations: Vec<String> = (0..3)
        .map(|i| agent_in(frame.clone(), json!([["Split", "items", i]]), "each"))
        .collect();
    let inner_child: Vec<String> = ["c1", "c2"]
        .into_iter()
        .map(|step| {
            agent_in(
                json!([child(json!([]), "outer"), child(json!([]), "inner")]),
                json!([]),
                step,
            )
        })
        .collect();
    let split_result = key("split", frame.clone(), json!([]), json!(["items"]));
    let inner_result = key("embed_workflow", frame, json!([]), json!(["inner"]));
    let outer_result = embed(json!([]), "outer");
    let before = agent_in(json!([]), json!([]), "before");
    save_all(p, &id, &iterations).await;
    save_all(p, &id, &inner_child).await;
    save_all(p, &id, std::slice::from_ref(&before)).await;

    p.record_checkpoint(&id, &split_result, b"items", None)
        .await
        .unwrap();
    let mut expected = set(&inner_child);
    expected.extend([split_result.clone(), before.clone()]);
    assert_eq!(
        stored(p, &id).await,
        expected,
        "only the child Split's iterations are gone"
    );

    p.record_checkpoint(&id, &inner_result, b"inner", None)
        .await
        .unwrap();
    assert_eq!(
        stored(p, &id).await,
        set([&split_result, &inner_result, &before]),
        "the inner Embed took its own child"
    );

    p.record_checkpoint(&id, &outer_result, b"outer", None)
        .await
        .unwrap();
    assert_eq!(stored(p, &id).await, set([&outer_result, &before]));
    p.delete_instances_batch(&[id]).await.unwrap();
}

/// An Embed inside a Split's iteration collapses its child into its result,
/// and the Split's result then takes that result with the iteration.
pub async fn an_embed_in_a_split_iteration_collapses_into_the_split(p: &dyn Persistence) {
    let id = running(p, "embed-in-split").await;
    let site = json!([["Split", "items", 1]]);
    let child_rows: Vec<String> = ["c1", "c2"]
        .into_iter()
        .map(|step| agent_in(json!([child(site.clone(), "e")]), json!([]), step))
        .collect();
    let first_item = item("items", 0, "each");
    let embed_result = embed(site, "e");
    let split_result = split(json!([]), "items");
    save_all(p, &id, &child_rows).await;
    save_all(p, &id, std::slice::from_ref(&first_item)).await;

    p.record_checkpoint(&id, &embed_result, b"child output", None)
        .await
        .unwrap();
    assert_eq!(stored(p, &id).await, set([&first_item, &embed_result]));

    p.record_checkpoint(&id, &split_result, b"all", None)
        .await
        .unwrap();
    assert_eq!(stored(p, &id).await, set([&split_result]));
    p.delete_instances_batch(&[id]).await.unwrap();
}

/// A workflow-agent child running as an isolated invocation writes through
/// the fenced path; the caller's Agent result, recorded by the root
/// execution, deletes those rows and keeps the caller's own.
pub async fn a_fenced_workflow_agent_child_collapses_into_its_callers_result(p: &dyn Persistence) {
    let id = running(p, "fenced-agent").await;
    let lease = claim(p, &id).await;
    let attempt = fences(p)
        .begin_invocation_attempt(&lease, "call", "start")
        .await
        .unwrap()
        .fence;
    let frame = json!([child(json!([]), "call")]);
    let child_rows: Vec<String> = ["c1", "c2", "c3"]
        .into_iter()
        .map(|step| {
            key_in(
                "orders-sync",
                "agent",
                frame.clone(),
                json!([]),
                json!(["utils", "random-double", step]),
            )
        })
        .collect();
    for key in &child_rows {
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
    let before = agent_in(json!([]), json!([]), "before");
    p.record_checkpoint(&id, &before, b"x", Some(&lease))
        .await
        .unwrap();
    assert_eq!(
        stored(p, &id).await.len(),
        child_rows.len() + 1,
        "the child's rows reached the caller's store"
    );

    let result = key(
        "agent",
        json!([]),
        json!([]),
        json!(["orders-sync", "run", "call"]),
    );
    assert_eq!(
        p.record_checkpoint(&id, &result, b"child output", Some(&lease))
            .await
            .unwrap(),
        CheckpointWrite::Recorded
    );
    assert_eq!(stored(p, &id).await, set([&before, &result]));
    p.delete_instances_batch(&[id]).await.unwrap();
}

/// A run that does not know which steps share a child namespace keeps every
/// child namespace: an Embed's and an Agent's results delete only their own
/// retry rows.
pub async fn an_unknown_run_keeps_every_child_namespace(p: &dyn Persistence) {
    let id = running_with(p, "unknown", None).await;
    let embed_result = embed(json!([]), "e");
    let agent_result = key(
        "agent",
        json!([]),
        json!([]),
        json!(["orders-sync", "run", "call"]),
    );
    let children = vec![
        agent_in(json!([child(json!([]), "e")]), json!([]), "c1"),
        agent_in(json!([child(json!([]), "call")]), json!([]), "c1"),
    ];
    let retries = vec![
        format!("{embed_result}::attempt::1"),
        format!("{agent_result}::retry::1"),
    ];
    save_all(p, &id, &children).await;
    save_all(p, &id, &retries).await;

    for result in [&embed_result, &agent_result] {
        p.record_checkpoint(&id, result, b"done", None)
            .await
            .unwrap();
    }

    let mut expected = set(&children);
    expected.extend([embed_result, agent_result]);
    assert_eq!(stored(p, &id).await, expected);
    p.delete_instances_batch(&[id]).await.unwrap();
}

/// A step the run lists keeps its child namespace, whichever step records
/// the result; another step's namespace still collapses.
pub async fn a_listed_step_keeps_its_child_namespace(p: &dyn Persistence) {
    let id = running_with(p, "listed", Some(SharedChildSteps::known(["x"]))).await;
    // A main-graph Agent `x` and an onWait EmbedWorkflow `x` share one
    // namespace; `y` is called once.
    let shared_child = agent_in(json!([child(json!([]), "x")]), json!([]), "c1");
    let other_child = agent_in(json!([child(json!([]), "y")]), json!([]), "c1");
    let x = key(
        "agent",
        json!([]),
        json!([]),
        json!(["utils", "random-double", "x"]),
    );
    let x_retry = format!("{x}::attempt::1");
    let y = embed(json!([]), "y");
    save_all(p, &id, &[shared_child.clone(), other_child, x_retry]).await;

    p.record_checkpoint(&id, &x, b"x", None).await.unwrap();
    p.record_checkpoint(&id, &y, b"y", None).await.unwrap();

    assert_eq!(stored(p, &id).await, set([&shared_child, &x, &y]));
    p.delete_instances_batch(&[id]).await.unwrap();
}

/// A Split's result deletes its iterations whatever the run knows: once it
/// exists, replay never enters the loop, an onWait graph inside an iteration
/// included.
pub async fn a_split_collapse_ignores_shared_steps(p: &dyn Persistence) {
    let id = running_with(p, "split-unknown", None).await;
    let site = json!([["Split", "items", 0]]);
    let inside = vec![
        item("items", 0, "each"),
        agent_in(json!([child(site.clone(), "e")]), json!([]), "c1"),
    ];
    let result = split(json!([]), "items");
    save_all(p, &id, &inside).await;

    p.record_checkpoint(&id, &result, b"all", None)
        .await
        .unwrap();

    assert_eq!(stored(p, &id).await, set([&result]));
    p.delete_instances_batch(&[id]).await.unwrap();
}

/// The fenced path keeps a listed step's child namespace too.
pub async fn a_fenced_child_keeps_a_listed_namespace(p: &dyn Persistence) {
    let id = running_with(p, "fenced-listed", Some(SharedChildSteps::known(["call"]))).await;
    let lease = claim(p, &id).await;
    let attempt = fences(p)
        .begin_invocation_attempt(&lease, "call", "start")
        .await
        .unwrap()
        .fence;
    let call = key(
        "agent",
        json!([]),
        json!([]),
        json!(["orders-sync", "run", "call"]),
    );
    let frame = json!([child(json!([]), "call")]);
    let nested_result = key("embed_workflow", frame.clone(), json!([]), json!(["call"]));
    let nested_child = agent_in(
        json!([child(json!([]), "call"), child(json!([]), "call")]),
        json!([]),
        "c1",
    );
    // The child writes a nested Embed `call` through the fenced path; its own
    // child namespace is listed too, so the nested result keeps it.
    for (key, state) in [
        (&nested_child, b"x".to_vec()),
        (&nested_result, b"r".to_vec()),
    ] {
        fences(p)
            .invocation_checkpoint(
                &attempt,
                &InvocationCheckpoint {
                    checkpoint_id: key.clone(),
                    state,
                },
            )
            .await
            .unwrap();
    }
    p.record_checkpoint(&id, &call, b"done", Some(&lease))
        .await
        .unwrap();

    assert_eq!(
        stored(p, &id).await,
        set([&nested_child, &nested_result, &call])
    );
    p.delete_instances_batch(&[id]).await.unwrap();
}

/// Every case in this module.
pub async fn run_all(p: &dyn Persistence) {
    recording_a_split_result_collapses_its_iterations(p).await;
    recording_a_while_result_collapses_its_iterations(p).await;
    a_failed_split_attempt_collapses_nothing(p).await;
    nested_splits_collapse_level_by_level(p).await;
    a_result_found_stored_collapses_nothing(p).await;
    a_fenced_split_result_collapses_its_iterations(p).await;
    recording_an_embed_result_collapses_its_child(p).await;
    recording_an_agent_result_collapses_its_child_and_retries(p).await;
    nested_child_scopes_collapse_level_by_level(p).await;
    an_embed_in_a_split_iteration_collapses_into_the_split(p).await;
    a_fenced_workflow_agent_child_collapses_into_its_callers_result(p).await;
    an_unknown_run_keeps_every_child_namespace(p).await;
    a_listed_step_keeps_its_child_namespace(p).await;
    a_split_collapse_ignores_shared_steps(p).await;
    a_fenced_child_keeps_a_listed_namespace(p).await;
}

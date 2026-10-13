// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Where a structured checkpoint sits in its run, and which rows recording it
//! makes obsolete.
//!
//! A version 2 checkpoint key names an operation and its position:
//! `runtara:v2:[kind, workflow, namespace, loop_path, parts]`, optionally
//! followed by `::attempt::N`, `::retry_sleep::N` or `::retry::N`. The
//! position is spread over several tuple elements and the kind comes first,
//! so the rows inside one composite step share no string prefix.
//!
//! [`scope`] rewrites the position as one canonical *address*: a compact JSON
//! array listing, outermost first, each namespace frame's workflow, loop
//! iterations and frame marker, then the key's own workflow and loop
//! iterations, then a tag naming the step the row belongs to. Everything
//! inside a composite step (a Split's iterations, an embedded child's steps)
//! then shares a byte prefix of the address, which a store indexes and
//! deletes as one range.
//!
//! Once a composite step's result row is recorded, replay reads that row first
//! and never runs the step again, so the rows inside the step are dead.
//! [`closure`] names them for a recorded key, and the store deletes them in
//! the transaction that records it. A crash before that commit leaves the
//! inner rows for replay to resume from; after it, the result row answers.
//!
//! Address segments:
//! - `["wf", workflow]` opens each namespace level, so a child that runs a
//!   different workflow never shares an address with its caller;
//! - `[kind, step, index]` is a loop iteration, exactly as in the key;
//! - `["ns", kind, parts]` is a namespace frame's marker (child, tool-child,
//!   agent-aux);
//! - `["base", family, step]` tags a step's result row, and
//!   `["own", family, step]` the rows the step keeps for itself (retry
//!   attempts, retry records, retry sleeps, deadlines). A closure deletes
//!   `own` rows, never another step's `base` row.

use serde_json::{Value, json};

const V2_PREFIX: &str = "runtara:v2:";

/// Suffixes a step appends to its result key for per-attempt rows.
const ATTEMPT_SUFFIXES: [&str; 3] = ["::attempt::", "::retry_sleep::", "::retry::"];

/// Longest address a store keeps. A btree entry holds about 2.7 KB, so a
/// longer address is not stored and its row is never collapsed.
pub const MAX_SCOPE_BYTES: usize = 2048;

/// Step families as the key's deadline parts name them.
const FAMILIES: [&str; 4] = ["Agent", "EmbedWorkflow", "Split", "While"];

type Loop = (String, String, u32);
/// A loop path is `null` where the source carried none: the same as empty.
type Loops = Option<Vec<Loop>>;
type Frame = (String, String, Loops, Value);
type Tuple = (String, String, Vec<Frame>, Loops, Value);

/// The address of `checkpoint_id`, or `None` when it is not a canonical
/// version 2 key or its address is longer than [`MAX_SCOPE_BYTES`]. A row
/// with no address is never collapsed.
pub fn scope(checkpoint_id: &str) -> Option<String> {
    let key = Key::parse(checkpoint_id)?;
    let mut segments = key.position();
    segments.extend(key.tag());
    bounded(encode(&segments))
}

/// What recording a result row makes obsolete: the rows its step owns, and
/// everything inside the step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeClosure {
    /// The exact address of the rows the step keeps for itself.
    pub owned: String,
    /// A byte prefix of every address inside the step, if it has an inside.
    pub inner: Option<String>,
}

impl ScopeClosure {
    /// Whether a row at `scope` is obsolete. The recorded row itself is
    /// tagged `base`, never `own`, and no inner prefix reaches its position.
    pub fn covers(&self, scope: &str) -> bool {
        scope == self.owned
            || self
                .inner
                .as_deref()
                .is_some_and(|prefix| scope.starts_with(prefix))
    }

    /// `inner` as a half-open byte range `[low, high)`, for an index range
    /// scan under a bytewise ("C") collation.
    pub fn inner_bounds(&self) -> Option<(&str, String)> {
        let prefix = self.inner.as_deref()?;
        Some((prefix, byte_successor(prefix)))
    }
}

/// The closure for a step result recorded as `checkpoint_id`, if its kind
/// collapses its scope. Only an unsuffixed result key closes anything: a
/// failed attempt is a suffixed row, so it never deletes the work a retry
/// will reuse.
pub fn closure(checkpoint_id: &str) -> Option<ScopeClosure> {
    let key = Key::parse(checkpoint_id).filter(|key| !key.suffixed)?;
    let (_, rule) = RULES.iter().find(|(kind, _)| *kind == key.kind)?;
    rule(&key)
}

/// How a recorded result key of one kind closes its scope.
type Rule = fn(&Key) -> Option<ScopeClosure>;

/// Result kinds that close their scope when recorded, and how. A kind is
/// added here only once replay is known never to read the rows its closure
/// names after the result exists.
const RULES: &[(&str, Rule)] = &[];

/// The closure of a loop step's result: its own rows and every iteration of
/// `loop_kind` (`Split` or `While`) it ran.
pub fn loop_closure(checkpoint_id: &str, loop_kind: &str) -> Option<ScopeClosure> {
    let key = Key::parse(checkpoint_id)?;
    let step = key.result_owner()?.1;
    key.closure(Some(open_segment(&json!([loop_kind, step]))))
}

/// The closure of a step whose result stands for a child invocation (an
/// embedded workflow or a workflow-agent call): its own rows and the child's
/// namespace.
pub fn child_closure(checkpoint_id: &str) -> Option<ScopeClosure> {
    let key = Key::parse(checkpoint_id)?;
    let step = key.result_owner()?.1;
    key.closure(Some(encode_one(&json!(["ns", "child", [step]]))))
}

/// A parsed version 2 key.
struct Key {
    kind: String,
    workflow: String,
    frames: Vec<Frame>,
    loops: Vec<Loop>,
    parts: Value,
    suffixed: bool,
}

impl Key {
    /// Parse the canonical form only. A re-encoding that differs from the
    /// stored bytes is not the compiler's, and could otherwise give two keys
    /// one address.
    fn parse(checkpoint_id: &str) -> Option<Self> {
        let encoded = checkpoint_id.strip_prefix(V2_PREFIX)?;
        let mut decoder = serde_json::Deserializer::from_str(encoded).into_iter::<Tuple>();
        let tuple = decoder.next()?.ok()?;
        let end = decoder.byte_offset();
        if serde_json::to_string(&tuple).ok()? != encoded[..end] {
            return None;
        }
        let suffixed = attempt_suffix(&encoded[end..])?;
        let (kind, workflow, frames, loops, parts) = tuple;
        let loops = loops.unwrap_or_default();
        let loop_kinds = frames
            .iter()
            .flat_map(|frame| frame.2.iter().flatten())
            .chain(&loops)
            .all(|(kind, _, _)| matches!(kind.as_str(), "Split" | "While"));
        loop_kinds.then_some(Self {
            kind,
            workflow,
            frames,
            loops,
            parts,
            suffixed,
        })
    }

    /// The address segments of the key's position: its namespace levels and
    /// loop iterations, without the tag.
    fn position(&self) -> Vec<Value> {
        let mut segments = Vec::new();
        for (kind, workflow, loops, parts) in &self.frames {
            segments.push(json!(["wf", workflow]));
            segments.extend(loops.iter().flatten().map(loop_segment));
            segments.push(json!(["ns", kind, parts]));
        }
        segments.push(json!(["wf", self.workflow]));
        segments.extend(self.loops.iter().map(loop_segment));
        segments
    }

    /// The step a result row belongs to: `(family, step)`.
    fn result_owner(&self) -> Option<(&'static str, &str)> {
        let family = match self.kind.as_str() {
            "agent" => "Agent",
            "split" => "Split",
            "embed_workflow" => "EmbedWorkflow",
            _ => return None,
        };
        let parts = self.parts.as_array()?;
        let step = match family {
            // agent_id, capability_id, step_id
            "Agent" if parts.len() == 3 => parts[2].as_str(),
            "Split" | "EmbedWorkflow" if parts.len() == 1 => parts[0].as_str(),
            _ => None,
        }?;
        Some((family, step))
    }

    /// A deadline or loop-completion row's step: `(family, step)` from its
    /// `[step_type, graph_scope, step_id]` parts.
    fn deadline_owner(&self) -> Option<(&str, &str)> {
        if !matches!(
            self.kind.as_str(),
            "agent-deadline" | "embed-deadline" | "loop-deadline" | "loop-complete"
        ) {
            return None;
        }
        match self.parts.as_array()?.as_slice() {
            [Value::String(family), _, Value::String(step)]
                if FAMILIES.contains(&family.as_str()) =>
            {
                Some((family, step))
            }
            _ => None,
        }
    }

    fn tag(&self) -> Option<Value> {
        if let Some((family, step)) = self.result_owner() {
            let tag = if self.suffixed { "own" } else { "base" };
            return Some(json!([tag, family, step]));
        }
        if self.suffixed {
            return None;
        }
        let (family, step) = self.deadline_owner()?;
        Some(json!(["own", family, step]))
    }

    /// The closure of this result key: its step's `own` rows, plus every
    /// address starting with the position followed by `inner`.
    fn closure(&self, inner: Option<String>) -> Option<ScopeClosure> {
        if self.suffixed {
            return None;
        }
        let (family, step) = self.result_owner()?;
        let mut owned = self.position();
        owned.push(json!(["own", family, step]));
        let owned = bounded(encode(&owned))?;
        let inner = inner.map(|segment| open(&self.position()) + &segment);
        Some(ScopeClosure { owned, inner })
    }
}

/// `Some(true)` for a well-formed attempt suffix, `Some(false)` for none.
fn attempt_suffix(rest: &str) -> Option<bool> {
    if rest.is_empty() {
        return Some(false);
    }
    let counter = ATTEMPT_SUFFIXES
        .iter()
        .find_map(|suffix| rest.strip_prefix(suffix))?;
    let parsed: u32 = counter.parse().ok()?;
    (parsed.to_string() == counter).then_some(true)
}

fn loop_segment((kind, step, index): &Loop) -> Value {
    json!([kind, step, index])
}

fn encode(segments: &[Value]) -> String {
    serde_json::to_string(segments).expect("JSON values always encode")
}

fn encode_one(segment: &Value) -> String {
    serde_json::to_string(segment).expect("JSON values always encode")
}

/// An address array with its closing bracket replaced by a separator, ready
/// for one more segment. Positions always hold at least a workflow segment.
fn open(position: &[Value]) -> String {
    let mut encoded = encode(position);
    encoded.pop();
    encoded.push(',');
    encoded
}

/// A segment array with its closing bracket replaced by a separator, so the
/// prefix matches the segment with any further elements (a loop's index).
fn open_segment(segment: &Value) -> String {
    let mut encoded = encode_one(segment);
    encoded.pop();
    encoded.push(',');
    encoded
}

fn bounded(address: String) -> Option<String> {
    (address.len() <= MAX_SCOPE_BYTES).then_some(address)
}

/// The least string greater than every string starting with `prefix`, in
/// byte order. Prefixes here end in an ASCII `,` or `]`.
fn byte_successor(prefix: &str) -> String {
    let mut bytes = prefix.as_bytes().to_vec();
    let last = bytes.pop().expect("closure prefixes are never empty");
    assert!(last < 0x7f, "closure prefixes end in an ASCII separator");
    bytes.push(last + 1);
    String::from_utf8(bytes).expect("bumping an ASCII byte keeps UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(kind: &str, workflow: &str, frames: Value, loops: Value, parts: Value) -> String {
        format!(
            "{V2_PREFIX}{}",
            json!([kind, workflow, frames, loops, parts])
        )
    }

    fn agent(step: &str, frames: Value, loops: Value) -> String {
        key(
            "agent",
            "wf",
            frames,
            loops,
            json!(["utils", "return-input", step]),
        )
    }

    fn child(step: &str, loops: Value) -> Value {
        json!(["child", "wf", loops, [step]])
    }

    #[test]
    fn a_root_agent_result_is_tagged_with_its_step() {
        assert_eq!(
            scope(&agent("a", json!([]), json!([]))).unwrap(),
            r#"[["wf","wf"],["base","Agent","a"]]"#
        );
    }

    #[test]
    fn loop_iterations_follow_the_workflow_segment() {
        assert_eq!(
            scope(&agent("each", json!([]), json!([["Split", "items", 3]]))).unwrap(),
            r#"[["wf","wf"],["Split","items",3],["base","Agent","each"]]"#
        );
    }

    #[test]
    fn composite_results_are_tagged_by_their_family() {
        assert_eq!(
            scope(&key("split", "wf", json!([]), json!([]), json!(["items"]))).unwrap(),
            r#"[["wf","wf"],["base","Split","items"]]"#
        );
        assert_eq!(
            scope(&key(
                "embed_workflow",
                "wf",
                json!([]),
                json!([]),
                json!(["e"])
            ))
            .unwrap(),
            r#"[["wf","wf"],["base","EmbedWorkflow","e"]]"#
        );
    }

    #[test]
    fn a_child_namespace_sits_under_its_call_site() {
        let frames = json!([child("e", json!([["Split", "items", 3]]))]);
        assert_eq!(
            scope(&agent("c1", frames, json!([]))).unwrap(),
            r#"[["wf","wf"],["Split","items",3],["ns","child",["e"]],["wf","wf"],["base","Agent","c1"]]"#
        );
    }

    #[test]
    fn keys_the_stdlib_emits_have_addresses() {
        // Literals asserted by the stdlib's own key tests.
        let cases = [
            (
                r#"runtara:v2:["wait","wf",[],[["While","a/::[]\"λ",0]],["instance","wait"]]"#,
                r#"[["wf","wf"],["While","a/::[]\"λ",0]]"#,
            ),
            (
                r#"runtara:v2:["wait-instances","wf",[],null,["wait"]]"#,
                r#"[["wf","wf"]]"#,
            ),
            (
                r#"runtara:v2:["state","wf",[],[],["set"]]"#,
                r#"[["wf","wf"]]"#,
            ),
        ];
        for (id, address) in cases {
            assert_eq!(scope(id).as_deref(), Some(address), "{id}");
        }
    }

    #[test]
    fn attempt_rows_are_owned_by_their_step() {
        let result = agent("a", json!([]), json!([]));
        for suffix in ["::attempt::2", "::retry::3", "::retry_sleep::3"] {
            assert_eq!(
                scope(&format!("{result}{suffix}")).unwrap(),
                r#"[["wf","wf"],["own","Agent","a"]]"#,
                "{suffix}"
            );
        }
    }

    #[test]
    fn deadline_rows_are_owned_by_their_step() {
        let cases = [
            ("agent-deadline", "Agent", "a"),
            ("embed-deadline", "EmbedWorkflow", "e"),
            ("loop-deadline", "Split", "items"),
            ("loop-complete", "While", "poll"),
        ];
        for (kind, family, step) in cases {
            let id = key(
                kind,
                "wf",
                json!([]),
                json!([]),
                json!([family, "root", step]),
            );
            assert_eq!(
                scope(&id).unwrap(),
                format!(r#"[["wf","wf"],["own","{family}","{step}"]]"#),
                "{kind}"
            );
        }
    }

    #[test]
    fn other_kinds_carry_their_position_only() {
        for kind in ["wait", "delay", "breakpoint", "state-local", "ai_turn"] {
            let id = key(
                kind,
                "wf",
                json!([]),
                json!([["While", "w", 0]]),
                json!(["s"]),
            );
            assert_eq!(
                scope(&id).unwrap(),
                r#"[["wf","wf"],["While","w",0]]"#,
                "{kind}"
            );
        }
    }

    #[test]
    fn keys_outside_the_canonical_grammar_have_no_address() {
        let result = agent("a", json!([]), json!([]));
        let rejected = [
            "wf::agent::utils::return-input::a::[0]".to_string(),
            "runtara:loop:v1:[\"loop-deadline\"]".to_string(),
            format!(
                "runtara:scope:v2:[{}]::agent::utils::x::a",
                child("e", json!([]))
            ),
            result.replace(r#"["agent","#, r#"[ "agent","#),
            format!("{result}::attempt::01"),
            format!("{result}::attempt::"),
            format!("{result}::other::1"),
            key(
                "agent",
                "wf",
                json!(["opaque-prefix"]),
                json!([]),
                json!(["u", "c", "a"]),
            ),
            key(
                "agent",
                "wf",
                json!([]),
                json!([["Loop", "x", 0]]),
                json!(["u", "c", "a"]),
            ),
            agent(&"x".repeat(MAX_SCOPE_BYTES), json!([]), json!([])),
        ];
        for id in rejected {
            assert_eq!(scope(&id), None, "{id}");
        }
    }

    #[test]
    fn no_kind_has_a_closure_rule() {
        for id in [
            agent("a", json!([]), json!([])),
            key("split", "wf", json!([]), json!([]), json!(["items"])),
            key("embed_workflow", "wf", json!([]), json!([]), json!(["e"])),
        ] {
            assert_eq!(closure(&id), None, "{id}");
        }
    }

    #[test]
    fn a_loop_closure_covers_its_iterations_and_owned_rows_only() {
        let outer = json!([["Split", "s", 1]]);
        let split = key("split", "wf", json!([]), outer.clone(), json!(["t"]));
        let closure = loop_closure(&split, "Split").unwrap();

        let inside = [
            agent(
                "x",
                json!([]),
                json!([["Split", "s", 1], ["Split", "t", 0]]),
            ),
            agent(
                "x",
                json!([]),
                json!([["Split", "s", 1], ["Split", "t", 12]]),
            ),
            agent(
                "y",
                json!([child("e", json!([["Split", "s", 1], ["Split", "t", 4]]))]),
                json!([]),
            ),
            format!("{split}::attempt::1"),
            key(
                "loop-deadline",
                "wf",
                json!([]),
                outer.clone(),
                json!(["Split", "g", "t"]),
            ),
            key(
                "loop-complete",
                "wf",
                json!([]),
                outer.clone(),
                json!(["Split", "g", "t"]),
            ),
        ];
        let outside = [
            split.clone(),
            agent(
                "x",
                json!([]),
                json!([["Split", "s", 10], ["Split", "t", 0]]),
            ),
            agent(
                "x",
                json!([]),
                json!([["Split", "s", 1], ["Split", "t2", 0]]),
            ),
            agent(
                "x",
                json!([]),
                json!([["Split", "s", 1], ["While", "t", 0]]),
            ),
            agent("t", json!([]), outer.clone()),
            agent("sibling", json!([]), outer.clone()),
            key("split", "wf", json!([]), outer.clone(), json!(["t2"])),
            key(
                "loop-deadline",
                "wf",
                json!([]),
                outer.clone(),
                json!(["Split", "g", "t2"]),
            ),
        ];
        for id in inside {
            assert!(closure.covers(&scope(&id).unwrap()), "should cover {id}");
        }
        for id in outside {
            assert!(!closure.covers(&scope(&id).unwrap()), "should keep {id}");
        }
    }

    #[test]
    fn a_child_closure_covers_its_namespace_and_owned_rows_only() {
        let call = agent("e", json!([]), json!([["Split", "s", 2]]));
        let closure = child_closure(&call).unwrap();
        let site = json!([["Split", "s", 2]]);
        let inside = [
            agent("c1", json!([child("e", site.clone())]), json!([])),
            agent(
                "c2",
                json!([child("e", site.clone())]),
                json!([["Split", "inner", 7]]),
            ),
            agent(
                "deep",
                json!([child("e", site.clone()), child("e2", json!([]))]),
                json!([]),
            ),
            format!("{call}::retry_sleep::2"),
            key(
                "agent-deadline",
                "wf",
                json!([]),
                site.clone(),
                json!(["Agent", "g", "e"]),
            ),
        ];
        let outside = [
            call.clone(),
            agent(
                "c1",
                json!([child("e", json!([["Split", "s", 3]]))]),
                json!([]),
            ),
            agent("c1", json!([child("e2", site.clone())]), json!([])),
            agent("e", json!([]), json!([["Split", "s", 3]])),
        ];
        for id in inside {
            assert!(closure.covers(&scope(&id).unwrap()), "should cover {id}");
        }
        for id in outside {
            assert!(!closure.covers(&scope(&id).unwrap()), "should keep {id}");
        }
    }

    #[test]
    fn a_suffixed_key_closes_nothing() {
        let split = key("split", "wf", json!([]), json!([]), json!(["s"]));
        assert_eq!(loop_closure(&format!("{split}::attempt::1"), "Split"), None);
        assert_eq!(child_closure(&format!("{split}::attempt::1")), None);
    }

    #[test]
    fn a_step_reusing_another_steps_id_keeps_its_result() {
        // An onWait branch may reuse a main-graph step id at the same position;
        // only the recorded step's own rows are named, never another result.
        let main = agent("x", json!([]), json!([]));
        let on_wait = key(
            "agent",
            "wf",
            json!([]),
            json!([]),
            json!(["slack", "post", "x"]),
        );
        let closure = child_closure(&main).unwrap();
        assert!(!closure.covers(&scope(&on_wait).unwrap()));
    }

    #[test]
    fn a_tool_child_running_another_workflow_never_matches_its_caller() {
        // A workflow-agent tool child shares the caller's tool-child namespace
        // while running its own workflow id.
        let frame = json!([["tool-child", "caller", [], ["ai", "lookup", 0]]]);
        let caller_row = key(
            "agent-deadline",
            "caller",
            frame.clone(),
            json!([["Split", "items", 3]]),
            json!(["Agent", "g", "x"]),
        );
        let child_split = key("split", "child", frame.clone(), json!([]), json!(["items"]));
        let child_step = key("agent", "child", frame, json!([]), json!(["u", "c", "x"]));
        let caller = scope(&caller_row).unwrap();
        assert!(!loop_closure(&child_split, "Split").unwrap().covers(&caller));
        assert!(!child_closure(&child_step).unwrap().covers(&caller));
    }

    #[test]
    fn escaped_step_ids_keep_their_own_scope() {
        for step in [r#"a"b"#, r"a\b", "ünï", "a,b]"] {
            let split = key("split", "wf", json!([]), json!([]), json!([step]));
            let closure = loop_closure(&split, "Split").unwrap();
            let item = agent("x", json!([]), json!([["Split", step, 0]]));
            let other = agent("x", json!([]), json!([["Split", format!("{step}x"), 0]]));
            assert!(closure.covers(&scope(&item).unwrap()), "{step}");
            assert!(!closure.covers(&scope(&other).unwrap()), "{step}");
        }
    }

    #[test]
    fn inner_bounds_select_exactly_the_prefixed_addresses() {
        let split = key("split", "wf", json!([]), json!([]), json!(["s"]));
        let closure = loop_closure(&split, "Split").unwrap();
        let (low, high) = closure.inner_bounds().unwrap();
        let candidates = [
            agent("x", json!([]), json!([["Split", "s", 0]])),
            agent("x", json!([]), json!([["Split", "s", 99]])),
            agent("x", json!([]), json!([["Split", "s2", 0]])),
            agent("x", json!([]), json!([["Split", "r", 0]])),
            agent("x", json!([]), json!([])),
            split.clone(),
        ];
        for id in candidates {
            let address = scope(&id).unwrap();
            let in_range = low <= address.as_str() && address.as_str() < high.as_str();
            assert_eq!(
                in_range,
                closure.covers(&address) && address != closure.owned,
                "{id}"
            );
        }
    }
}

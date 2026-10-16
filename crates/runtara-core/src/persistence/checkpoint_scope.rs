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
//! - `["base", kind, parts]` tags a result row (Agent, Split, While,
//!   EmbedWorkflow), and `["own", kind, parts]` the rows that result keeps for
//!   itself: its retry attempts, retry records and retry sleeps, and a
//!   Split's, While's or EmbedWorkflow's deadline rows. The tag repeats the
//!   result key's kind and parts, so it tells steps apart exactly as well as
//!   their result keys do. A closure deletes `own` rows, never another step's
//!   `base` row.
//!
//! A Split's, While's or EmbedWorkflow's result key names only its step, so
//! its deadline rows map onto it without losing anything. An Agent's deadline row
//! names the graph scope and its result key names the agent, so the two cannot
//! be matched when steps share an id at one position (an `onWait` branch may
//! reuse a main-graph id). It is left untagged, and goes when an enclosing
//! step collapses.
//!
//! Stores keep the address in each row, and later closures match rows written
//! earlier, so the format only grows: a change that renders an existing kind
//! differently leaves its older rows unmatched, never collapsed.

use serde_json::{Value, json};

const V2_PREFIX: &str = "runtara:v2:";

/// Suffixes a step appends to its result key for per-attempt rows.
const ATTEMPT_SUFFIXES: [&str; 3] = ["::attempt::", "::retry_sleep::", "::retry::"];

/// Longest address a store keeps. A btree entry holds about 2.7 KB, so a
/// longer address is not stored and its row is never collapsed.
pub const MAX_SCOPE_BYTES: usize = 2048;

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
    /// A byte prefix of every address inside the step.
    pub inner: String,
    /// The calling step, when `inner` is a child namespace (an EmbedWorkflow
    /// or an Agent result); `None` for a loop's iterations. A child namespace
    /// names its step by id alone, so another step with that id at the same
    /// position may share it (see [`SharedChildSteps`]).
    pub child_step: Option<String>,
}

impl ScopeClosure {
    /// Whether a row at `scope` is obsolete. The recorded row itself is
    /// tagged `base`, never `own`, and no inner prefix reaches its position.
    pub fn covers(&self, scope: &str) -> bool {
        scope == self.owned || scope.starts_with(&self.inner)
    }

    /// [`Self::covers`] in a run that knows `shared`: the step's own rows
    /// always go, its child namespace only when no other step can share it.
    pub fn covers_in(&self, scope: &str, shared: &SharedChildSteps) -> bool {
        scope == self.owned || (self.deletes_inner(shared) && scope.starts_with(&self.inner))
    }

    /// Whether the rows under `inner` go in a run that knows `shared`.
    pub fn deletes_inner(&self, shared: &SharedChildSteps) -> bool {
        self.child_step
            .as_deref()
            .is_none_or(|step| !shared.keeps(step))
    }

    /// `inner` as a half-open byte range `[low, high)`, for an index range
    /// scan under a bytewise ("C") collation.
    pub fn inner_bounds(&self) -> (&str, String) {
        (&self.inner, byte_successor(&self.inner))
    }
}

/// The steps of a run whose child namespace another step may share: every
/// step id that occurs inside an `onWait` graph anywhere in the run's
/// workflow, its embedded children and the workflow-agents it calls.
///
/// An `onWait` graph keeps its wait step's position, so a step there that
/// reuses an id of the graph around it shares that step's child namespace,
/// and either one's result would delete the other's child rows. When the
/// other step has no result to replay from (`durable: false`, a failure its
/// `onError` handled, or still in progress), its child steps would run again.
/// So a child namespace whose step is listed here is never collapsed.
///
/// Unknown (the default) lists every step: a run whose workflow could not be
/// read, or that started before runs recorded this, keeps every child
/// namespace.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SharedChildSteps(Option<std::collections::BTreeSet<String>>);

impl SharedChildSteps {
    /// Nothing is known: every child namespace is kept.
    pub fn unknown() -> Self {
        Self(None)
    }

    /// Exactly `steps` may share their child namespace.
    pub fn known<S: Into<String>>(steps: impl IntoIterator<Item = S>) -> Self {
        Self(Some(steps.into_iter().map(Into::into).collect()))
    }

    /// The listed steps, or `None` when unknown.
    pub fn steps(&self) -> Option<&std::collections::BTreeSet<String>> {
        self.0.as_ref()
    }

    /// Whether `step`'s child namespace must be kept.
    pub fn keeps(&self, step: &str) -> bool {
        self.0.as_ref().is_none_or(|steps| steps.contains(step))
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
///
/// - `split`: a Split's result is saved only once its loop has finished
///   (every item succeeded, or with `dontStopOnFailed` every item ran and
///   the result records the failures), and replay looks it up before
///   entering the loop, its deadline or its retries, so its iterations and
///   its own rows are never read again.
/// - `while`: a While's result is saved only once its loop has exited, its
///   condition false or its maximum reached; a failure, a timeout or a
///   suspension inside it saves none. Replay looks it up before the first
///   condition and its deadline, so its iterations and its own rows are never
///   read again.
/// - `embed_workflow`: an EmbedWorkflow's result is saved only once its child
///   has finished (it succeeded, or, as an AiAgent tool, its failure became
///   the tool result), never when the child parks or an enclosing deadline
///   unwinds. Replay looks it up before mapping the child's input, its
///   deadline or its retries, so the child's rows and its own are never read
///   again.
/// - `agent`: an Agent's result is saved only when its call succeeded, and
///   replay looks it up before its deadline, its retries, its continuation
///   and the call itself. A workflow-agent's child rows are never read again,
///   and neither are the attempt, retry and retry-sleep rows of any Agent
///   that retried: the key does not say whether the agent is a workflow.
const RULES: &[(&str, Rule)] = &[
    ("split", Key::loop_closure),
    ("while", Key::loop_closure),
    ("embed_workflow", Key::child_closure),
    ("agent", Key::child_closure),
];

/// The closure of a Split's or While's result: its own rows and every
/// iteration it ran.
pub fn loop_closure(checkpoint_id: &str) -> Option<ScopeClosure> {
    Key::parse(checkpoint_id)?.loop_closure()
}

/// The closure of a result that stands for a child invocation (an
/// EmbedWorkflow, or an Agent calling a workflow-agent): its own rows and the
/// child's namespace, which is empty for an Agent that calls no workflow.
///
/// The child namespace names the calling step by its id alone, so the closure
/// carries that id: a store keeps the namespace when the run lists the step
/// in its [`SharedChildSteps`].
pub fn child_closure(checkpoint_id: &str) -> Option<ScopeClosure> {
    Key::parse(checkpoint_id)?.child_closure()
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

    /// The step id of a result key (Agent, Split, EmbedWorkflow).
    fn result_step(&self) -> Option<&str> {
        match (self.kind.as_str(), self.parts.as_array()?.as_slice()) {
            // agent_id, capability_id, step_id
            ("agent", [_, _, step]) | ("split" | "while" | "embed_workflow", [step]) => {
                step.as_str()
            }
            _ => None,
        }
    }

    /// The result a Split's, While's or EmbedWorkflow's deadline row belongs
    /// to, as that result key's kind and parts. Deadline parts are
    /// `[step_type, graph_scope, step_id]`.
    fn deadline_result(&self) -> Option<(&'static str, Value)> {
        let [Value::String(owner), _, Value::String(step)] = self.parts.as_array()?.as_slice()
        else {
            return None;
        };
        let result_kind = match (self.kind.as_str(), owner.as_str()) {
            ("loop-deadline" | "loop-complete", "Split") => "split",
            ("loop-deadline" | "loop-complete", "While") => "while",
            ("embed-deadline", "EmbedWorkflow") => "embed_workflow",
            _ => return None,
        };
        Some((result_kind, json!([step])))
    }

    fn tag(&self) -> Option<Value> {
        if self.result_step().is_some() {
            let tag = if self.suffixed { "own" } else { "base" };
            return Some(json!([tag, self.kind, self.parts]));
        }
        if self.suffixed {
            return None;
        }
        let (kind, parts) = self.deadline_result()?;
        Some(json!(["own", kind, parts]))
    }

    /// A Split's or While's result closure: its own rows and every iteration
    /// it ran.
    fn loop_closure(&self) -> Option<ScopeClosure> {
        let loop_kind = match self.kind.as_str() {
            "split" => "Split",
            "while" => "While",
            _ => return None,
        };
        let step = self.result_step()?;
        self.closure(opened(encode(&json!([loop_kind, step]))), None)
    }

    /// A child-invoking result's closure: its own rows and the child's
    /// namespace.
    fn child_closure(&self) -> Option<ScopeClosure> {
        if !matches!(self.kind.as_str(), "agent" | "embed_workflow") {
            return None;
        }
        let step = self.result_step()?;
        self.closure(encode(&json!(["ns", "child", [step]])), Some(step))
    }

    /// The closure of this result key: its `own` rows, plus every address
    /// starting with the position followed by `inner`, which is the child
    /// namespace of `child_step` when there is one.
    fn closure(&self, inner: String, child_step: Option<&str>) -> Option<ScopeClosure> {
        if self.suffixed || self.result_step().is_none() {
            return None;
        }
        let mut owned = self.position();
        owned.push(json!(["own", self.kind, self.parts]));
        let owned = bounded(encode(&owned))?;
        let inner = opened(encode(&self.position())) + &inner;
        Some(ScopeClosure {
            owned,
            inner,
            child_step: child_step.map(str::to_string),
        })
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

fn encode<T: serde::Serialize + ?Sized>(value: &T) -> String {
    serde_json::to_string(value).expect("JSON values always encode")
}

/// An encoded non-empty array with its closing bracket replaced by a
/// separator: a prefix of the same array with more elements. Opening a
/// position makes room for one more segment; opening a loop segment matches
/// any of its iterations.
fn opened(mut encoded: String) -> String {
    debug_assert!(encoded.ends_with(']') && encoded != "[]");
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
            r#"[["wf","wf"],["base","agent",["utils","return-input","a"]]]"#
        );
    }

    #[test]
    fn loop_iterations_follow_the_workflow_segment() {
        assert_eq!(
            scope(&agent("each", json!([]), json!([["Split", "items", 3]]))).unwrap(),
            r#"[["wf","wf"],["Split","items",3],["base","agent",["utils","return-input","each"]]]"#
        );
    }

    #[test]
    fn composite_results_are_tagged_with_their_key() {
        assert_eq!(
            scope(&key("split", "wf", json!([]), json!([]), json!(["items"]))).unwrap(),
            r#"[["wf","wf"],["base","split",["items"]]]"#
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
            r#"[["wf","wf"],["base","embed_workflow",["e"]]]"#
        );
    }

    #[test]
    fn a_child_namespace_sits_under_its_call_site() {
        let frames = json!([child("e", json!([["Split", "items", 3]]))]);
        assert_eq!(
            scope(&agent("c1", frames, json!([]))).unwrap(),
            r#"[["wf","wf"],["Split","items",3],["ns","child",["e"]],["wf","wf"],["base","agent",["utils","return-input","c1"]]]"#
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
                r#"[["wf","wf"],["own","agent",["utils","return-input","a"]]]"#,
                "{suffix}"
            );
        }
    }

    #[test]
    fn loop_and_embed_deadline_rows_are_owned_by_their_result() {
        let cases = [
            ("loop-deadline", "Split", r#"["own","split",["items"]]"#),
            ("loop-complete", "Split", r#"["own","split",["items"]]"#),
            ("loop-deadline", "While", r#"["own","while",["items"]]"#),
            ("loop-complete", "While", r#"["own","while",["items"]]"#),
            (
                "embed-deadline",
                "EmbedWorkflow",
                r#"["own","embed_workflow",["items"]]"#,
            ),
        ];
        for (kind, step_type, tag) in cases {
            let id = key(
                kind,
                "wf",
                json!([]),
                json!([]),
                json!([step_type, "root", "items"]),
            );
            assert_eq!(
                scope(&id).unwrap(),
                format!(r#"[["wf","wf"],{tag}]"#),
                "{kind}"
            );
        }
    }

    #[test]
    fn agent_deadline_rows_carry_their_position_only() {
        let cases = [
            ("agent-deadline", "Agent"),
            ("loop-deadline", "Agent"),
            ("embed-deadline", "Split"),
        ];
        for (kind, step_type) in cases {
            let id = key(
                kind,
                "wf",
                json!([]),
                json!([]),
                json!([step_type, "root", "s"]),
            );
            assert_eq!(scope(&id).unwrap(), r#"[["wf","wf"]]"#, "{kind}");
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
    fn only_composite_results_have_a_closure_rule() {
        let split = key(
            "split",
            "wf",
            json!([]),
            json!([["Split", "outer", 2]]),
            json!(["items"]),
        );
        let embed = key("embed_workflow", "wf", json!([]), json!([]), json!(["e"]));
        let call = agent("a", json!([]), json!([]));
        let repeat = key("while", "wf", json!([]), json!([]), json!(["w"]));
        assert_eq!(closure(&split), loop_closure(&split));
        assert_eq!(closure(&embed), child_closure(&embed));
        assert_eq!(closure(&call), child_closure(&call));
        assert_eq!(closure(&repeat), loop_closure(&repeat));
        for id in [&split, &embed, &call, &repeat] {
            assert!(closure(id).is_some(), "{id}");
        }
        for id in [
            format!("{split}::attempt::1"),
            key(
                "loop-complete",
                "wf",
                json!([]),
                json!([]),
                json!(["While", "root", "w"]),
            ),
            format!("{embed}::retry_sleep::2"),
            format!("{call}::retry::3"),
            key(
                "loop-complete",
                "wf",
                json!([]),
                json!([]),
                json!(["Split", "root", "items"]),
            ),
            key(
                "embed-deadline",
                "wf",
                json!([]),
                json!([]),
                json!(["EmbedWorkflow", "root", "e"]),
            ),
            key(
                "agent-deadline",
                "wf",
                json!([]),
                json!([]),
                json!(["Agent", "root", "a"]),
            ),
            key("wait", "wf", json!([]), json!([]), json!(["instance", "w"])),
            key("ai_turn", "wf", json!([]), json!([]), json!(["ai", 0])),
            key("state", "wf", json!([]), json!([]), json!(["set"])),
        ] {
            assert_eq!(closure(&id), None, "{id}");
        }
    }

    #[test]
    fn a_child_closure_names_its_step_and_a_loop_closure_none() {
        let split = key("split", "wf", json!([]), json!([]), json!(["items"]));
        let embed = key("embed_workflow", "wf", json!([]), json!([]), json!(["e"]));
        assert_eq!(loop_closure(&split).unwrap().child_step, None);
        assert_eq!(
            child_closure(&embed).unwrap().child_step.as_deref(),
            Some("e")
        );
        assert_eq!(
            child_closure(&agent("a", json!([]), json!([])))
                .unwrap()
                .child_step
                .as_deref(),
            Some("a")
        );
    }

    #[test]
    fn a_shared_child_namespace_is_kept_and_owned_rows_still_go() {
        let call = agent("x", json!([]), json!([]));
        let closure = child_closure(&call).unwrap();
        let child_row = scope(&agent("c1", json!([child("x", json!([]))]), json!([]))).unwrap();
        let retry = scope(&format!("{call}::retry::1")).unwrap();
        for (shared, deletes) in [
            (SharedChildSteps::unknown(), false),
            (SharedChildSteps::known(["x"]), false),
            (SharedChildSteps::known(["y"]), true),
            (SharedChildSteps::known::<&str>([]), true),
        ] {
            assert_eq!(closure.deletes_inner(&shared), deletes, "{shared:?}");
            assert_eq!(
                closure.covers_in(&child_row, &shared),
                deletes,
                "{shared:?}"
            );
            assert!(closure.covers_in(&retry, &shared), "{shared:?}");
        }
        let split = key("split", "wf", json!([]), json!([]), json!(["items"]));
        assert!(
            loop_closure(&split)
                .unwrap()
                .deletes_inner(&SharedChildSteps::unknown()),
            "a loop's iterations go whatever the run knows"
        );
    }

    #[test]
    fn an_agent_closure_keeps_its_tool_and_memory_calls() {
        // An AiAgent's tool calls and memory calls run under their own frames,
        // not the child namespace its result closes.
        let ai = agent("ai", json!([]), json!([]));
        let closure = child_closure(&ai).unwrap();
        for frame in [
            json!(["tool-child", "wf", [], ["ai", "lookup", 0]]),
            json!(["agent-aux", "wf", [], ["ai", "memory", "load-memory"]]),
        ] {
            let call = agent("ai", json!([frame]), json!([]));
            assert!(!closure.covers(&scope(&call).unwrap()), "{call}");
        }
    }

    #[test]
    fn a_loop_closure_covers_its_iterations_and_owned_rows_only() {
        let outer = json!([["Split", "s", 1]]);
        let split = key("split", "wf", json!([]), outer.clone(), json!(["t"]));
        let closure = loop_closure(&split).unwrap();

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
            key(
                "loop-deadline",
                "wf",
                json!([]),
                outer.clone(),
                json!(["While", "g", "t"]),
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
    fn a_while_closure_covers_its_iterations_and_owned_rows_only() {
        let outer = json!([["Split", "s", 1]]);
        let repeat = key("while", "wf", json!([]), outer.clone(), json!(["t"]));
        let closure = loop_closure(&repeat).unwrap();
        let iteration = |i: u64| json!([["Split", "s", 1], ["While", "t", i]]);
        let deadline = |kind: &str, owner: &str, step: &str| {
            key(
                kind,
                "wf",
                json!([]),
                outer.clone(),
                json!([owner, "g", step]),
            )
        };

        let inside = [
            agent("x", json!([]), iteration(0)),
            agent("x", json!([]), iteration(999)),
            agent(
                "y",
                json!([]),
                json!([["Split", "s", 1], ["While", "t", 3], ["Split", "inner", 2]]),
            ),
            agent("z", json!([child("e", iteration(4))]), json!([])),
            key("state-local", "wf", json!([]), iteration(5), json!(["set"])),
            deadline("loop-deadline", "While", "t"),
            deadline("loop-complete", "While", "t"),
        ];
        let outside = [
            repeat.clone(),
            agent(
                "x",
                json!([]),
                json!([["Split", "s", 2], ["While", "t", 0]]),
            ),
            agent(
                "x",
                json!([]),
                json!([["Split", "s", 1], ["While", "t2", 0]]),
            ),
            agent(
                "x",
                json!([]),
                json!([["Split", "s", 1], ["Split", "t", 0]]),
            ),
            agent("t", json!([]), outer.clone()),
            key("split", "wf", json!([]), outer.clone(), json!(["t"])),
            deadline("loop-deadline", "While", "t2"),
            deadline("loop-deadline", "Split", "t"),
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
        ];
        let outside = [
            call.clone(),
            key(
                "agent-deadline",
                "wf",
                json!([]),
                site.clone(),
                json!(["Agent", "g", "e"]),
            ),
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
    fn an_embed_closure_covers_its_child_attempts_and_deadline() {
        let site = json!([["Split", "s", 2]]);
        let call = key(
            "embed_workflow",
            "wf",
            json!([]),
            site.clone(),
            json!(["e"]),
        );
        let closure = child_closure(&call).unwrap();
        for id in [
            agent("c1", json!([child("e", site.clone())]), json!([])),
            format!("{call}::attempt::1"),
            key(
                "embed-deadline",
                "wf",
                json!([]),
                site.clone(),
                json!(["EmbedWorkflow", "g", "e"]),
            ),
        ] {
            assert!(closure.covers(&scope(&id).unwrap()), "should cover {id}");
        }
        assert!(!closure.covers(&scope(&call).unwrap()));
    }

    #[test]
    fn a_childs_local_state_rows_go_with_its_caller() {
        // A child keeps its state locally and checkpoints each SetState and
        // GetState under its own namespace, so the caller's result closes them
        // with the rest of the child's rows.
        let state = |kind: &str, frames: Value, loops: Value, step: &str| {
            key(kind, "wf", frames, loops, json!([step]))
        };
        let embed = key("embed_workflow", "wf", json!([]), json!([]), json!(["e"]));
        let workflow_agent = key(
            "agent",
            "wf",
            json!([]),
            json!([]),
            json!(["orders-sync", "run", "e"]),
        );
        for call in [embed, workflow_agent] {
            let closure = child_closure(&call).unwrap();
            for id in [
                state(
                    "state-local",
                    json!([child("e", json!([]))]),
                    json!([]),
                    "s1",
                ),
                state(
                    "state-read-local",
                    json!([child("e", json!([]))]),
                    json!([]),
                    "g",
                ),
                state(
                    "state-local",
                    json!([child("e", json!([]))]),
                    json!([["While", "w", 4]]),
                    "s2",
                ),
                state(
                    "state-local",
                    json!([child("e", json!([])), child("inner", json!([]))]),
                    json!([]),
                    "s3",
                ),
            ] {
                assert!(
                    closure.covers(&scope(&id).unwrap()),
                    "{call} should cover {id}"
                );
            }
            for id in [
                state("state-read", json!([]), json!([]), "g"),
                state(
                    "state-local",
                    json!([child("other", json!([]))]),
                    json!([]),
                    "s1",
                ),
            ] {
                assert!(
                    !closure.covers(&scope(&id).unwrap()),
                    "{call} should keep {id}"
                );
            }
        }
    }

    #[test]
    fn closures_only_come_from_their_own_result_kinds() {
        let split = key("split", "wf", json!([]), json!([]), json!(["s"]));
        let embed = key("embed_workflow", "wf", json!([]), json!([]), json!(["s"]));
        assert!(loop_closure(&agent("s", json!([]), json!([]))).is_none());
        assert!(loop_closure(&embed).is_none());
        assert!(child_closure(&split).is_none());
        assert!(
            child_closure(&key("wait", "wf", json!([]), json!([]), json!(["i", "s"]))).is_none()
        );
    }

    #[test]
    fn a_suffixed_key_closes_nothing() {
        let split = key("split", "wf", json!([]), json!([]), json!(["s"]));
        assert_eq!(loop_closure(&format!("{split}::attempt::1")), None);
        assert_eq!(child_closure(&format!("{split}::attempt::1")), None);
    }

    #[test]
    fn a_step_reusing_another_steps_id_keeps_its_rows() {
        // An onWait branch may reuse a main-graph step id at the same position.
        // Recording the main step names its own rows only: not the onWait
        // step's result, attempts or deadline. Their child namespaces are one,
        // though: it is keyed by the step id alone.
        let main = agent("x", json!([]), json!([]));
        let on_wait = key(
            "agent",
            "wf",
            json!([]),
            json!([]),
            json!(["slack", "post", "x"]),
        );
        let closure = child_closure(&main).unwrap();
        for id in [
            on_wait.clone(),
            format!("{on_wait}::attempt::1"),
            format!("{on_wait}::retry::2"),
            key(
                "agent-deadline",
                "wf",
                json!([]),
                json!([]),
                json!(["Agent", "root/onWait/w", "x"]),
            ),
        ] {
            assert!(!closure.covers(&scope(&id).unwrap()), "should keep {id}");
        }
        assert!(closure.covers(&scope(&format!("{main}::retry::2")).unwrap()));
        let on_wait_child = agent("c1", json!([child("x", json!([]))]), json!([]));
        assert!(
            closure.covers(&scope(&on_wait_child).unwrap()),
            "a child of either step is in the shared namespace"
        );
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
        assert!(!loop_closure(&child_split).unwrap().covers(&caller));
        assert!(!child_closure(&child_step).unwrap().covers(&caller));
    }

    #[test]
    fn escaped_step_ids_keep_their_own_scope() {
        for step in [r#"a"b"#, r"a\b", "ünï", "a,b]"] {
            let split = key("split", "wf", json!([]), json!([]), json!([step]));
            let closure = loop_closure(&split).unwrap();
            let item = agent("x", json!([]), json!([["Split", step, 0]]));
            let other = agent("x", json!([]), json!([["Split", format!("{step}x"), 0]]));
            assert!(closure.covers(&scope(&item).unwrap()), "{step}");
            assert!(!closure.covers(&scope(&other).unwrap()), "{step}");
        }
    }

    #[test]
    fn inner_bounds_select_exactly_the_prefixed_addresses() {
        let split = key("split", "wf", json!([]), json!([]), json!(["s"]));
        let closure = loop_closure(&split).unwrap();
        let (low, high) = closure.inner_bounds();
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

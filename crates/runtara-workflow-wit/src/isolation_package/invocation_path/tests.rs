use super::*;
use crate::isolation_package::AgentCallSite;
use serde_json::json;

fn manifest() -> InvocationManifest {
    InvocationManifest {
        call_sites: Vec::new(),
        version: 1,
        workflow_id: "root::雪".into(),
        agent_calls: vec![AgentCallSite {
            binding: "agent:utils".into(),
            agent_id: "utils".into(),
            capability: "copy".into(),
            step_id: "s::雪".into(),
            domains: vec![0, 1, 2, 3, 4, 5],
        }],
    }
}
fn key() -> Value {
    json!(["agent", "root::雪", [], [], ["utils", "copy", "s::雪"]])
}
fn path(key: &Value, domain: &str, activation: &str) -> String {
    format!("runtara:v2:{key}:{domain}:{activation}")
}
fn standard(key: &Value) -> String {
    path(key, "aaaaaaaa", "aaaaaaaa")
}

#[test]
fn canonical_unicode_namespaces_loops_and_max_counters_decode_without_delimiter_aliases() {
    let mut key = key();
    key[2] = json!([
        [
            "child",
            "root::雪",
            [["Split", "outer:[]", 4294967295u32]],
            ["embed::雪"]
        ],
        [
            "tool-child",
            "root::雪",
            [["While", "loop", 3]],
            ["ai", "tool::雪", 4294967295u32]
        ]
    ]);
    key[3] = json!([["While", "inner", 7], ["Split", "nested", 0]]);
    let decoded = manifest()
        .resolve_agent_invocation(
            "agent:utils",
            "copy",
            &path(&key, "aaaaaaad", "pppppppp"),
            1,
        )
        .unwrap();
    assert_eq!(decoded.selector, InvocationSelector::Domain(3));
    assert_eq!(decoded.activation, u32::MAX);
    assert_eq!(
        decoded.loops,
        vec![
            LoopFrame(LoopKind::While, "inner".into(), 7),
            LoopFrame(LoopKind::Split, "nested".into(), 0)
        ]
    );
    assert!(
        matches!(&decoded.namespace[0], NamespaceFrame::Child { step_id, loops, .. } if step_id == "embed::雪" && loops[0].2 == u32::MAX)
    );
    assert!(
        matches!(&decoded.namespace[1], NamespaceFrame::ToolChild { call_counter, .. } if *call_counter == u32::MAX)
    );
}

#[test]
fn malformed_ancestry_counters_and_noncanonical_addresses_are_rejected() {
    for (field, value) in [
        (0, json!("checkpoint")),
        (2, json!(["legacy opaque prefix"])),
        (2, json!([["child", "root", [], ["one", "two"]]])),
        (2, json!([["tool-child", "root", [], ["ai", "label", -1]]])),
        (2, json!([["unknown", "root", [], ["site"]]])),
        (3, json!([["Split", "s", -1]])),
        (3, json!([["Split", "s", 1.0]])),
        (3, json!([["Split", "s", 4294967296u64]])),
        (3, json!([["While", "s", "1"]])),
        (3, json!([["Split", "s", 1, 2]])),
        (3, json!([["not-a-loop", "s", 1]])),
        (3, Value::Null),
        (4, json!(["utils", "copy"])),
    ] {
        let mut key = key();
        key[field] = value;
        assert_eq!(
            AgentInvocationPath::decode(&standard(&key)),
            Err(InvocationPathError::Malformed)
        );
    }
    let good = standard(&key());
    for bad in [
        good.replace("runtara:v2:", "runtara:v1:"),
        good.replace("[\"agent\",", "[\"agent\", "),
        good.replace('雪', "\\u96ea"),
        format!("{good}:extra"),
        path(&key(), "aaaaaaaa", "aaaaaaa"),
        path(&key(), "aaaaaaaq", "aaaaaaaa"),
        path(&key(), "aaaaaaag", "aaaaaaaa"),
        path(&key(), "aaaaaaaa", "aaaaaaab"),
        path(&key(), "aaaaaaae", "aaaaaaab"),
        path(&key(), "aaaaaaaa", "AAAAaaaa"),
        path(&key(), "aaaaaaaa", "雪雪雪雪"),
    ] {
        assert_eq!(
            AgentInvocationPath::decode(&bad),
            Err(InvocationPathError::Malformed),
            "accepted {bad}"
        );
    }
    for end in 0..good.len() {
        if good.is_char_boundary(end) {
            assert!(AgentInvocationPath::decode(&good[..end]).is_err());
        }
    }
}

#[test]
fn compiler_inventory_checks_entry_identity_domain_and_attempt_before_authority() {
    let inventory = manifest();
    for domain in 0..6u8 {
        let domain = format!("aaaaaaa{}", char::from(b'a' + domain));
        assert!(
            inventory
                .resolve_agent_invocation(
                    "agent:utils",
                    "copy",
                    &path(&key(), &domain, "aaaaaaaa"),
                    1
                )
                .is_ok()
        );
    }
    for (binding, capability, attempt) in [
        ("other", "copy", 1),
        ("agent:utils", "wrong", 1),
        ("agent:utils", "copy", 0),
    ] {
        assert!(
            inventory
                .resolve_agent_invocation(binding, capability, &standard(&key()), attempt)
                .is_err()
        );
    }
    assert_eq!(
        inventory.resolve_agent_invocation(
            "agent:utils",
            "copy",
            &path(&key(), "aaaaaaac", "aaaaaaaa"),
            2
        ),
        Err(InvocationPathError::InvalidAttempt)
    );
    for (field, value) in [
        (1, json!("other-root")),
        (4, json!(["utils", "copy", "sibling"])),
        (4, json!(["forged", "copy", "s::雪"])),
    ] {
        let mut key = key();
        key[field] = value;
        assert_eq!(
            inventory.resolve_agent_invocation("agent:utils", "copy", &standard(&key), 1),
            Err(InvocationPathError::UnknownCall)
        );
    }
    let mut restricted = inventory;
    restricted.agent_calls[0].domains = vec![0];
    assert_eq!(
        restricted.resolve_agent_invocation(
            "agent:utils",
            "copy",
            &path(&key(), "aaaaaaac", "aaaaaaaa"),
            1
        ),
        Err(InvocationPathError::UnknownCall)
    );
}

#[test]
fn qualified_tokens_resolve_semantic_rules_without_reinterpreting_legacy_domains() {
    use crate::isolation_package::InvocationCallSite;
    let mut inventory = manifest();
    inventory.version = 2;
    inventory.agent_calls[0].domains = vec![0, 3];
    inventory.call_sites = vec![
        InvocationCallSite {
            token: 15,
            identity: 0,
            agent_reference: 0,
            caller_reference: 0,
            domain: 0,
        },
        InvocationCallSite {
            token: u32::MAX,
            identity: 0,
            agent_reference: 0,
            caller_reference: 1,
            domain: 3,
        },
    ];
    let qualified = |token: &str, activation: &str| {
        path(&key(), token, activation).replacen("runtara:v2:", "runtara:v3:", 1)
    };
    let step = qualified("aaaaaaap", "aaaaaaaa");
    let tool = qualified("pppppppp", "pppppppp");
    assert_eq!(
        inventory
            .resolve_agent_invocation("agent:utils", "copy", &step, u64::MAX)
            .unwrap()
            .selector,
        InvocationSelector::CallSite(15)
    );
    assert_eq!(
        inventory
            .resolve_agent_invocation("agent:utils", "copy", &tool, 1)
            .unwrap()
            .activation,
        u32::MAX
    );
    for (path, attempt, expected) in [
        (step.clone(), 0, InvocationPathError::InvalidAttempt),
        (tool.clone(), 2, InvocationPathError::InvalidAttempt),
        (
            qualified("aaaaaaap", "aaaaaaab"),
            1,
            InvocationPathError::UnknownCall,
        ),
        (
            qualified("aaaaaaaa", "aaaaaaaa"),
            1,
            InvocationPathError::UnknownCall,
        ),
        (standard(&key()), 1, InvocationPathError::UnknownCall),
    ] {
        assert_eq!(
            inventory.resolve_agent_invocation("agent:utils", "copy", &path, attempt),
            Err(expected)
        );
    }
    assert_eq!(
        manifest().resolve_agent_invocation("agent:utils", "copy", &step, 1),
        Err(InvocationPathError::UnknownCall)
    );
    // An otherwise valid token may not name another flat identity.
    let mut other = inventory.agent_calls[0].clone();
    other.step_id = "z".into();
    inventory.agent_calls.push(other);
    inventory.call_sites[0].identity = 1;
    assert_eq!(
        inventory.resolve_agent_invocation("agent:utils", "copy", &step, 1),
        Err(InvocationPathError::UnknownCall)
    );
}

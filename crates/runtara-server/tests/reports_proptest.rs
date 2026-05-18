//! Reports proptest — Phase 0 of the reports refactor.
//!
//! Generates randomized `ReportDefinition` shapes and asserts two invariants
//! that must survive every later phase:
//!
//! 1. **No panics anywhere on the read path.** The JSON Schema validator and
//!    serde deserializer must reject malformed inputs gracefully — never panic.
//! 2. **Serde round-trip is a fixed point.** For any definition that does
//!    deserialize successfully, `serialize → deserialize → serialize` matches
//!    `serialize → deserialize → serialize → deserialize → serialize`. In
//!    other words: one round-trip stabilizes the JSON shape.
//!
//! These are deliberately weak invariants — they protect against the worst
//! drift class (the refactor introduces a panic on a previously-handled input)
//! without locking us into specific normalization choices.

use proptest::prelude::*;
use runtara_server::api::dto::reports::ReportDefinition;
use runtara_server::api::services::reports::ReportService;
use serde_json::{Map, Value};

/// Generate a leaf JSON value (no recursion).
fn arb_leaf() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| Value::Number(n.into())),
        // Restrict strings so the validator's error messages stay readable on shrink.
        "[a-zA-Z0-9_./:@-]{0,16}".prop_map(Value::String),
    ]
}

/// Recursive JSON values: objects of plausible report-shape keys plus arrays
/// plus leaves. The key set deliberately overlaps with real definition fields
/// so the validator gets meaningful work to do.
fn arb_value() -> impl Strategy<Value = Value> {
    let leaf = arb_leaf();
    leaf.prop_recursive(4, 32, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::vec(
                (
                    prop_oneof![
                        Just("definitionVersion".to_string()),
                        Just("layout".to_string()),
                        Just("filters".to_string()),
                        Just("blocks".to_string()),
                        Just("datasets".to_string()),
                        Just("views".to_string()),
                        Just("id".to_string()),
                        Just("type".to_string()),
                        Just("blockId".to_string()),
                        Just("title".to_string()),
                        Just("label".to_string()),
                        Just("source".to_string()),
                        Just("mode".to_string()),
                        Just("schema".to_string()),
                        Just("groupBy".to_string()),
                        Just("aggregates".to_string()),
                        Just("orderBy".to_string()),
                        Just("limit".to_string()),
                        Just("kind".to_string()),
                        Just("entity".to_string()),
                        Just("op".to_string()),
                        Just("arguments".to_string()),
                        Just("field".to_string()),
                        Just("appliesTo".to_string()),
                        Just("options".to_string()),
                        "[a-z][a-z0-9_]{0,12}".prop_map(String::from),
                    ],
                    inner,
                ),
                0..6,
            )
            .prop_map(|pairs| {
                let mut m = Map::new();
                for (k, v) in pairs {
                    m.insert(k, v);
                }
                Value::Object(m)
            }),
        ]
    })
}

/// Generate a report-shaped envelope (object with the top-level fields the DTO expects).
fn arb_report_envelope() -> impl Strategy<Value = Value> {
    (
        // definitionVersion
        prop_oneof![
            Just(Value::Null),
            any::<i32>().prop_map(|n| Value::Number(n.into()))
        ],
        prop::collection::vec(arb_value(), 0..3),
        prop::collection::vec(arb_value(), 0..3),
        prop::collection::vec(arb_value(), 0..3),
        prop::collection::vec(arb_value(), 0..2),
        prop::collection::vec(arb_value(), 0..2),
    )
        .prop_map(|(def_v, layout, filters, blocks, datasets, views)| {
            let mut m = Map::new();
            m.insert("definitionVersion".to_string(), def_v);
            m.insert("layout".to_string(), Value::Array(layout));
            m.insert("filters".to_string(), Value::Array(filters));
            m.insert("blocks".to_string(), Value::Array(blocks));
            m.insert("datasets".to_string(), Value::Array(datasets));
            m.insert("views".to_string(), Value::Array(views));
            Value::Object(m)
        })
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_shrink_iters: 4096,
        ..ProptestConfig::default()
    })]

    /// The JSON Schema validator must never panic on arbitrary input.
    /// It is allowed to return errors; it is not allowed to crash.
    #[test]
    fn json_schema_validator_does_not_panic(value in arb_report_envelope()) {
        let _ = ReportService::validate_report_definition_json_syntax_issues(&value);
    }

    /// The serde deserializer must never panic on arbitrary report-shaped input.
    /// Most random values deserialize to an error; a small fraction succeed.
    #[test]
    fn dto_deserialize_does_not_panic(value in arb_report_envelope()) {
        let _ = serde_json::from_value::<ReportDefinition>(value);
    }

    /// For values that *do* deserialize successfully, one serialize→deserialize
    /// pass must produce a stable shape — any subsequent pass is a fixed point.
    #[test]
    fn dto_round_trip_is_a_fixed_point(value in arb_report_envelope()) {
        let Ok(dto) = serde_json::from_value::<ReportDefinition>(value) else {
            return Ok(());
        };
        let once = serde_json::to_value(&dto).expect("serialize once");
        let dto2: ReportDefinition =
            serde_json::from_value(once.clone()).expect("redeserialize once");
        let twice = serde_json::to_value(&dto2).expect("serialize twice");
        prop_assert_eq!(once, twice);
    }
}

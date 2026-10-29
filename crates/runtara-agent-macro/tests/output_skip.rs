//! `#[field(skip)]` keeps an output field out of the metadata while serde still
//! (de)serializes it.
use std::collections::HashMap;

use runtara_agent_macro::CapabilityOutput;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(CapabilityOutput, Serialize, Deserialize)]
struct Deal {
    #[field(display_name = "Title")]
    title: Option<String>,
    #[field(skip)]
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

#[test]
fn skipped_output_field_is_absent_from_metadata() {
    let names: Vec<&str> = __OUTPUT_META_Deal.fields.iter().map(|f| f.name).collect();
    assert_eq!(names, ["title"]);
}

#[test]
fn skipped_output_field_still_round_trips() {
    let deal: Deal = serde_json::from_value(json!({ "title": "A", "stage": 3 })).unwrap();
    assert_eq!(deal.extra.get("stage"), Some(&json!(3)));
    assert_eq!(
        serde_json::to_value(&deal).unwrap(),
        json!({ "title": "A", "stage": 3 })
    );
}

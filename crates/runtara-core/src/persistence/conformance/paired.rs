//! Paired-record rule shared by every backend: one record per
//! (correlation, scope), opened by the first start and closed by the first
//! end after it.
//!
//! A resumed run replays its completed steps and re-enters a parked one, so
//! the producer emits their start (and end) again. Those later events are
//! replays; a record must not be repeated for them.
use chrono::{Duration, Utc};
use serde_json::json;

use crate::domain::EventType;
use crate::persistence::{
    EventRecord, EventSortOrder, EventVocabulary, EventVocabularySpec, ListPairedRecordsFilter,
    PairedRecordStatus, Persistence,
};

fn vocabulary() -> EventVocabulary {
    EventVocabulary::new(EventVocabularySpec {
        start_subtype: "pairing_start",
        end_subtype: "pairing_end",
        correlation_key: "unit_id",
        kind_key: "unit_kind",
        label_key: "unit_label",
        inputs_key: "given",
        outputs_key: "produced",
        error_key: "failure",
        error_flag_key: "_failed",
        launched_at_key: "began_ms",
        settled_at_key: "ended_ms",
    })
    .expect("valid vocabulary")
}

/// One event of a sequence: `S` a start, `E` an end, `F` a failed end; each
/// for a unit in an optional scope.
#[derive(Clone, Copy)]
enum Mark {
    S,
    E,
    F,
}

async fn emit(
    backend: &dyn Persistence,
    instance_id: &str,
    events: &[(Mark, &str, Option<&str>)],
) -> chrono::DateTime<Utc> {
    // Distinct, increasing timestamps a second apart, backdated so none is in
    // the future.
    let base = Utc::now() - Duration::minutes(10);
    for (n, (mark, unit, scope)) in events.iter().enumerate() {
        let mut payload = json!({ "unit_id": unit, "unit_kind": "Kind", "seq": n });
        if let Some(scope) = scope {
            payload["scope_id"] = json!(scope);
        }
        let subtype = match mark {
            Mark::S => {
                payload["given"] = json!({ "seq": n });
                "pairing_start"
            }
            Mark::E => {
                payload["produced"] = json!({ "seq": n });
                "pairing_end"
            }
            Mark::F => {
                payload["failure"] = json!({ "seq": n });
                "pairing_end"
            }
        };
        backend
            .insert_event(&EventRecord {
                id: None,
                instance_id: instance_id.to_string(),
                event_type: EventType::Custom,
                checkpoint_id: None,
                payload: Some(serde_json::to_vec(&payload).unwrap()),
                created_at: base + Duration::seconds(n as i64),
                subtype: Some(subtype.to_string()),
            })
            .await
            .expect("insert_event failed");
    }
    base
}

async fn fresh_instance(backend: &dyn Persistence, name: &str) -> String {
    let id = format!("pairing-{name}-{}", uuid::Uuid::new_v4());
    backend
        .register_instance(&id, "pairing")
        .await
        .expect("register_instance failed");
    id
}

fn filter() -> ListPairedRecordsFilter {
    ListPairedRecordsFilter {
        sort_order: EventSortOrder::Asc,
        ..Default::default()
    }
}

async fn records(
    backend: &dyn Persistence,
    instance_id: &str,
    filter: &ListPairedRecordsFilter,
) -> Vec<crate::persistence::PairedRecordSummary> {
    let vocabulary = vocabulary();
    let listed = backend
        .list_paired_records(instance_id, &vocabulary, filter, 100, 0)
        .await
        .expect("list_paired_records failed");
    let counted = backend
        .count_paired_records(instance_id, &vocabulary, filter)
        .await
        .expect("count_paired_records failed");
    assert_eq!(
        counted,
        listed.len() as i64,
        "the count must agree with the listing"
    );
    listed
}

/// S,S,E: a step that parked and was re-entered on resume is one record,
/// from its first start to its end.
pub async fn reentered_start(backend: &dyn Persistence) {
    use Mark::*;
    let id = fresh_instance(backend, "sse").await;
    let base = emit(
        backend,
        &id,
        &[(S, "a", None), (S, "a", None), (E, "a", None)],
    )
    .await;
    let got = records(backend, &id, &filter()).await;
    assert_eq!(got.len(), 1, "S,S,E is one record");
    let record = &got[0];
    assert_eq!(record.status, PairedRecordStatus::Completed);
    assert_eq!(record.started_at, base, "opened by the first start");
    assert_eq!(record.completed_at, Some(base + Duration::seconds(2)));
    assert_eq!(record.duration_ms, Some(2_000));
    assert_eq!(record.inputs, Some(json!({ "seq": 0 })));
    assert_eq!(record.outputs, Some(json!({ "seq": 2 })));
}

/// S,E,S,E: a completed step replayed on resume keeps its original record,
/// closed by the first end.
pub async fn replayed_pair(backend: &dyn Persistence) {
    use Mark::*;
    let id = fresh_instance(backend, "sese").await;
    let base = emit(
        backend,
        &id,
        &[
            (S, "a", None),
            (E, "a", None),
            (S, "a", None),
            (E, "a", None),
        ],
    )
    .await;
    let got = records(backend, &id, &filter()).await;
    assert_eq!(got.len(), 1, "S,E,S,E is one record");
    assert_eq!(got[0].status, PairedRecordStatus::Completed);
    assert_eq!(got[0].started_at, base);
    assert_eq!(got[0].completed_at, Some(base + Duration::seconds(1)));
    assert_eq!(got[0].outputs, Some(json!({ "seq": 1 })));

    // A replay still in flight (S,E,S) reads as the completed original.
    let id = fresh_instance(backend, "ses").await;
    emit(
        backend,
        &id,
        &[(S, "a", None), (E, "a", None), (S, "a", None)],
    )
    .await;
    let got = records(backend, &id, &filter()).await;
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].status, PairedRecordStatus::Completed);
}

/// E,S,E: an end before any start pairs with nothing; the start pairs with
/// the end after it.
pub async fn stray_end(backend: &dyn Persistence) {
    use Mark::*;
    let id = fresh_instance(backend, "ese").await;
    let base = emit(
        backend,
        &id,
        &[(F, "a", None), (S, "a", None), (E, "a", None)],
    )
    .await;
    let got = records(backend, &id, &filter()).await;
    assert_eq!(got.len(), 1, "E,S,E is one record");
    assert_eq!(
        got[0].status,
        PairedRecordStatus::Completed,
        "the earlier failed end belongs to no start"
    );
    assert_eq!(got[0].started_at, base + Duration::seconds(1));
    assert_eq!(got[0].completed_at, Some(base + Duration::seconds(2)));

    // A start without an end after it is running, even with an earlier end.
    let id = fresh_instance(backend, "es").await;
    emit(backend, &id, &[(E, "a", None), (S, "a", None)]).await;
    let got = records(backend, &id, &filter()).await;
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].status, PairedRecordStatus::Running);
    assert_eq!(got[0].completed_at, None);
}

/// The rule is per (correlation, scope): the same unit in two scopes, and
/// two units interleaved, stay separate records; status filters and counts
/// see the one record per key.
pub async fn keys_stay_separate(backend: &dyn Persistence) {
    use Mark::*;
    let id = fresh_instance(backend, "keys").await;
    emit(
        backend,
        &id,
        &[
            (S, "a", Some("loop-0")),
            (S, "b", None),
            (E, "a", Some("loop-0")),
            (S, "a", Some("loop-1")),
            (F, "b", None),
            (S, "a", Some("loop-0")),
            (S, "b", None),
            (E, "b", None),
            (E, "a", Some("loop-0")),
        ],
    )
    .await;
    let got = records(backend, &id, &filter()).await;
    let summary: Vec<_> = got
        .iter()
        .map(|r| (r.correlation_id.as_str(), r.scope_id.as_deref(), r.status))
        .collect();
    assert_eq!(
        summary,
        [
            ("a", Some("loop-0"), PairedRecordStatus::Completed),
            ("b", None, PairedRecordStatus::Failed),
            ("a", Some("loop-1"), PairedRecordStatus::Running),
        ]
    );
    for (status, expected) in [
        (PairedRecordStatus::Completed, 1),
        (PairedRecordStatus::Failed, 1),
        (PairedRecordStatus::Running, 1),
    ] {
        let filter = ListPairedRecordsFilter {
            status: Some(status),
            ..filter()
        };
        assert_eq!(records(backend, &id, &filter).await.len(), expected);
    }
}

/// Every paired-record rule case.
pub async fn run_all(backend: &dyn Persistence) {
    reentered_start(backend).await;
    replayed_pair(backend).await;
    stray_end(backend).await;
    keys_stay_separate(backend).await;
}

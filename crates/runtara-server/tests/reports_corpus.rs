//! Reports corpus tests — Phase 0 of the reports refactor.
//!
//! See `docs/reports-refactoring-plan.md`. This file owns the fixture corpus
//! that every later phase has to keep green:
//!
//! - DTO round-trip: deserialize → serialize → deserialize → serialize must
//!   converge after one pass. Catches serde drift and missing `default` /
//!   `skip_serializing_if` attributes.
//! - JSON Schema syntax validation: every fixture is structurally valid
//!   against the schema the service publishes. Snapshots are committed via
//!   `insta`; review with `cargo insta review`.
//!
//! Fixtures live in `tests/fixtures/reports/*.json`. Filenames are sorted so
//! snapshot output is deterministic. Add new fixtures by dropping a JSON file
//! in that directory — both tests pick it up.

use std::fs;
use std::path::PathBuf;

use runtara_server::api::dto::reports::ReportDefinition;
use runtara_server::api::services::reports::ReportService;
use serde_json::Value;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/reports")
}

fn collect_fixtures() -> Vec<(String, Value)> {
    let dir = fixtures_dir();
    let mut entries: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    entries.sort();
    entries
        .into_iter()
        .map(|path| {
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_owned)
                .unwrap_or_else(|| path.display().to_string());
            let raw = fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            let value: Value =
                serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{name}: parse JSON: {e}"));
            (name, value)
        })
        .collect()
}

#[test]
fn fixtures_round_trip_through_dto() {
    for (name, value) in collect_fixtures() {
        let dto: ReportDefinition = serde_json::from_value(value.clone())
            .unwrap_or_else(|e| panic!("{name}: deserialize: {e}"));
        let once = serde_json::to_value(&dto).unwrap_or_else(|e| panic!("{name}: serialize: {e}"));
        let dto2: ReportDefinition = serde_json::from_value(once.clone())
            .unwrap_or_else(|e| panic!("{name}: re-deserialize: {e}"));
        let twice =
            serde_json::to_value(&dto2).unwrap_or_else(|e| panic!("{name}: re-serialize: {e}"));
        assert_eq!(once, twice, "{name}: round-trip diverged after one pass");
    }
}

#[test]
fn fixtures_pass_syntax_validation() {
    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path(fixtures_dir().join("__snapshots__"));
    settings.set_prepend_module_to_snapshot(false);
    settings.bind(|| {
        for (name, value) in collect_fixtures() {
            let issues = ReportService::validate_report_definition_json_syntax_issues(&value)
                .unwrap_or_else(|e| panic!("{name}: syntax check failed: {e:?}"));
            insta::assert_json_snapshot!(format!("syntax_{name}"), issues);
        }
    });
}

#[test]
fn corpus_is_not_empty() {
    let fixtures = collect_fixtures();
    assert!(
        !fixtures.is_empty(),
        "no fixtures found in {}",
        fixtures_dir().display()
    );
}

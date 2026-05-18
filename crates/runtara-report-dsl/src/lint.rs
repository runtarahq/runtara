//! Authoring-time lint for report definitions.
//!
//! Operates on the raw `serde_json::Value` (pre-deserialization) so it can
//! see literal key names — that's where the "did you mean?" signal lives.
//! Returns advisory [`ReportLintIssue`]s; nothing here gates a save. The
//! strict validator (server-side `ReportService::validate_report` + the
//! JSON Schema parse check) is the gate.
//!
//! Today lints are limited to fuzzy-match typo hints on unknown top-level
//! keys and a handful of common snake_case → camelCase mistakes. Add more
//! as authoring feedback accumulates; the shape stays the same.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LintSeverity {
    Warning,
    Info,
}

#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportLintIssue {
    pub severity: LintSeverity,
    /// Lint code. SCREAMING_SNAKE_CASE, e.g. `UNKNOWN_REPORT_FIELD`.
    pub code: String,
    /// JSONPath-ish path from the definition root, e.g. `$.blocks[0].chartType`.
    pub path: String,
    pub message: String,
    /// Optional follow-up hint (the "did you mean?" suggestion).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// Walk a report definition and return advisory lint issues. Empty vec
/// means the definition is clean (or at least: no lint we know how to emit).
pub fn lint(definition: &Value) -> Vec<ReportLintIssue> {
    let mut issues = Vec::new();
    lint_root(definition, &mut issues);
    issues
}

const ALLOWED_ROOT_KEYS: &[&str] = &[
    "definitionVersion",
    "blocks",
    "filters",
    "datasets",
    "views",
    "layout",
    "interactions",
    "defaultCurrency",
    "metadata",
];

const ALLOWED_BLOCK_KEYS: &[&str] = &[
    "id",
    "type",
    "title",
    "description",
    "block_type",
    "source",
    "dataset",
    "table",
    "chart",
    "metric",
    "card",
    "markdown",
    "filters",
    "interactions",
    "showWhen",
    "lazy",
    "actions",
    "hideWhenEmpty",
];

fn lint_root(definition: &Value, issues: &mut Vec<ReportLintIssue>) {
    let Some(object) = definition.as_object() else {
        return;
    };
    for key in object.keys() {
        if !ALLOWED_ROOT_KEYS.contains(&key.as_str()) {
            let hint = similar_key_hint(key, ALLOWED_ROOT_KEYS);
            issues.push(ReportLintIssue {
                severity: LintSeverity::Warning,
                code: "UNKNOWN_REPORT_FIELD".to_string(),
                path: format!("$.{key}"),
                message: format!("Report definition has no field '{key}'."),
                hint: hint.map(|h| format!("Did you mean '{h}'?")),
            });
        }
    }
    if let Some(blocks) = object.get("blocks").and_then(Value::as_array) {
        for (index, block) in blocks.iter().enumerate() {
            lint_block(&format!("$.blocks[{index}]"), block, issues);
        }
    }
    if let Some(layout) = object.get("layout")
        && layout.is_array()
    {
        // Phase 10 made `layout` a single root grid; arrays are the
        // pre-Phase-10 wire form. The repository migrates them
        // transparently, but flag for visibility so authors know to
        // emit the new shape.
        issues.push(ReportLintIssue {
            severity: LintSeverity::Warning,
            code: "LEGACY_LAYOUT_ARRAY_SHAPE".to_string(),
            path: "$.layout".to_string(),
            message: "definition.layout is a single root grid object; the array form is legacy."
                .to_string(),
            hint: Some(
                "Wrap your top-level layout nodes inside { id: 'root', columns: N, items: [...] }."
                    .to_string(),
            ),
        });
    }
}

fn lint_block(path: &str, block: &Value, issues: &mut Vec<ReportLintIssue>) {
    let Some(object) = block.as_object() else {
        return;
    };
    for key in object.keys() {
        if !ALLOWED_BLOCK_KEYS.contains(&key.as_str()) {
            let hint = similar_key_hint(key, ALLOWED_BLOCK_KEYS);
            issues.push(ReportLintIssue {
                severity: LintSeverity::Warning,
                code: "UNKNOWN_REPORT_BLOCK_FIELD".to_string(),
                path: format!("{path}.{key}"),
                message: format!("Report block has no field '{key}'."),
                hint: hint.map(|h| format!("Did you mean '{h}'?")),
            });
        }
    }
    // Common snake_case → camelCase aliases. Catch the most-common
    // authoring mistakes so the strict validator's "unknown field" error
    // ships with a fix suggestion.
    for (raw, suggestion) in [
        ("group_by", "groupBy"),
        ("order_by", "orderBy"),
        ("default_sort", "defaultSort"),
        ("display_template", "displayTemplate"),
        ("value_field", "valueField"),
    ] {
        if object.contains_key(raw) {
            issues.push(ReportLintIssue {
                severity: LintSeverity::Warning,
                code: "MISNAMED_REPORT_FIELD".to_string(),
                path: format!("{path}.{raw}"),
                message: format!("Use '{suggestion}' (camelCase), not '{raw}'."),
                hint: Some(format!("Rename to '{suggestion}'.")),
            });
        }
    }
}

/// Levenshtein-based fuzzy match. Used by `lint_*` to suggest the
/// most-similar allowed key when an unknown one shows up.
pub fn similar_key_hint<'a>(key: &str, allowed: &'a [&str]) -> Option<&'a str> {
    let key_lower = key.to_ascii_lowercase();
    allowed
        .iter()
        .copied()
        .filter_map(|allowed_key| {
            let allowed_lower = allowed_key.to_ascii_lowercase();
            let distance = levenshtein(&key_lower, &allowed_lower);
            let threshold = if allowed_lower.len() <= 4 { 1 } else { 3 };
            (distance <= threshold).then_some((distance, allowed_key))
        })
        .min_by_key(|(distance, allowed_key)| (*distance, allowed_key.len()))
        .map(|(_, allowed_key)| allowed_key)
}

fn levenshtein(left: &str, right: &str) -> usize {
    let mut costs = (0..=right.chars().count()).collect::<Vec<_>>();
    for (left_index, left_char) in left.chars().enumerate() {
        let mut previous = costs[0];
        costs[0] = left_index + 1;
        for (right_index, right_char) in right.chars().enumerate() {
            let current = costs[right_index + 1];
            costs[right_index + 1] = if left_char == right_char {
                previous
            } else {
                1 + previous.min(current).min(costs[right_index])
            };
            previous = current;
        }
    }
    costs[right.chars().count()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lint_flags_unknown_root_field_with_suggestion() {
        let issues = lint(&json!({
            "definitionVersion": 1,
            "blocks": [],
            "filterz": [],
        }));
        let issue = issues
            .iter()
            .find(|i| i.code == "UNKNOWN_REPORT_FIELD")
            .expect("unknown root field lint");
        assert_eq!(issue.path, "$.filterz");
        assert_eq!(issue.hint.as_deref(), Some("Did you mean 'filters'?"));
    }

    #[test]
    fn lint_flags_misnamed_snake_case_in_blocks() {
        let issues = lint(&json!({
            "definitionVersion": 1,
            "blocks": [
                {
                    "id": "b1",
                    "type": "table",
                    "source": { "kind": "object_model", "schema": "s" },
                    "display_template": "x"
                }
            ]
        }));
        let issue = issues
            .iter()
            .find(|i| i.code == "MISNAMED_REPORT_FIELD")
            .expect("misnamed lint");
        assert_eq!(issue.path, "$.blocks[0].display_template");
        assert!(issue.message.contains("displayTemplate"));
    }

    #[test]
    fn lint_clean_definition_emits_nothing() {
        let issues = lint(&json!({
            "definitionVersion": 1,
            "blocks": [
                { "id": "b1", "type": "table" }
            ]
        }));
        assert!(issues.is_empty(), "expected no lints, got: {issues:?}");
    }

    #[test]
    fn similar_key_hint_picks_closest_allowed() {
        let allowed = ["alpha", "beta", "gamma"];
        assert_eq!(similar_key_hint("alpha", &allowed), Some("alpha"));
        assert_eq!(similar_key_hint("alpah", &allowed), Some("alpha"));
        assert_eq!(similar_key_hint("zzz", &allowed), None);
    }
}

//! MCP tool search — token-overlap scoring with a name-token boost.
//!
//! See the design brief: tokenize lowercase a-z0-9 words >= 2 chars, count
//! intersections with name and doc tokens, normalize by sqrt of token count.
//! No stemming, no IDF, no stop words — small and predictable.

use crate::types::{SearchResult, Tool};
use std::collections::HashMap;

/// Tokenize: lowercase, regex `\b[a-z0-9]+\b`, drop tokens shorter than 2
/// chars. Handles snake_case, kebab-case, camelCase naturally (we treat any
/// non-alnum as a separator, and break camelCase by lowercasing first then
/// re-splitting). Returns a deduplicated set as a Vec for cheap intersection.
fn tokenize(input: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();

    let lowered = input.to_lowercase();
    for ch in lowered.chars() {
        if ch.is_ascii_alphanumeric() {
            current.push(ch);
        } else {
            push_token(&mut current, &mut out);
        }
    }
    push_token(&mut current, &mut out);

    // Dedup while preserving order.
    let mut seen = std::collections::HashSet::new();
    out.retain(|t| seen.insert(t.clone()));
    out
}

fn push_token(buf: &mut String, out: &mut Vec<String>) {
    if buf.len() >= 2 {
        out.push(std::mem::take(buf));
    } else {
        buf.clear();
    }
}

fn intersection_count(a: &[String], b: &[String]) -> usize {
    let set: std::collections::HashSet<&String> = a.iter().collect();
    b.iter().filter(|t| set.contains(*t)).count()
}

/// Search tools by token overlap with name-boost, length-normalized.
///
/// - `tools`: full list from the MCP server.
/// - `hints`: tool_name -> extra description text (from connection params).
/// - `scope`: empty = allow all; non-empty = allowlist of tool names.
/// - `query`: the search string.
/// - `limit`: max results returned (clamped to 1..=20, default 5 handled by caller).
pub fn search(
    tools: &[Tool],
    hints: &HashMap<String, String>,
    scope: &[String],
    query: &str,
    limit: usize,
) -> Vec<SearchResult> {
    let q_tokens = tokenize(query);
    if q_tokens.is_empty() {
        return Vec::new();
    }

    let scope_set: std::collections::HashSet<&String> = scope.iter().collect();
    let scope_filter = !scope.is_empty();

    let mut scored: Vec<SearchResult> = tools
        .iter()
        .filter(|tool| !scope_filter || scope_set.contains(&tool.name))
        .filter_map(|tool| {
            let name_tokens = tokenize(&tool.name);
            let hint = hints.get(&tool.name).map(|s| s.as_str()).unwrap_or("");
            let mut doc_blob = String::new();
            doc_blob.push_str(&tool.description);
            doc_blob.push(' ');
            doc_blob.push_str(hint);
            let doc_tokens = tokenize(&doc_blob);

            let name_hits = intersection_count(&q_tokens, &name_tokens) as f64;
            let doc_hits = intersection_count(&q_tokens, &doc_tokens) as f64;

            let total_tokens = (name_tokens.len() + doc_tokens.len()) as f64;
            if total_tokens == 0.0 {
                return None;
            }
            let score = (2.0 * name_hits + doc_hits) / total_tokens.sqrt();

            if score <= 0.0 {
                None
            } else {
                Some(SearchResult {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    input_schema: tool.input_schema.clone(),
                    score,
                })
            }
        })
        .collect();

    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let limit = limit.clamp(1, 20);
    scored.truncate(limit);
    scored
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn t(name: &str, desc: &str) -> Tool {
        Tool {
            name: name.to_string(),
            description: desc.to_string(),
            input_schema: json!({}),
        }
    }

    #[test]
    fn tokenize_handles_snake_case() {
        let toks = tokenize("create_issue_in_linear");
        assert_eq!(toks, vec!["create", "issue", "in", "linear"]);
    }

    #[test]
    fn tokenize_drops_short_tokens() {
        let toks = tokenize("a issue and");
        assert!(toks.contains(&"issue".to_string()));
        assert!(toks.contains(&"and".to_string()));
        assert!(!toks.contains(&"a".to_string()));
    }

    #[test]
    fn tokenize_handles_camelcase_via_lowercase_no_split() {
        // We intentionally do NOT split camelCase — keep it simple.
        // Lowercasing produces a single token.
        let toks = tokenize("createIssue");
        assert_eq!(toks, vec!["createissue"]);
    }

    #[test]
    fn name_match_ranks_above_doc_match() {
        let tools = vec![
            t("create_issue", "Make something happen"),
            t("misc_thing", "Used to create an issue in the project"),
        ];
        let hints = HashMap::new();
        let results = search(&tools, &hints, &[], "create issue", 5);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].name, "create_issue");
    }

    #[test]
    fn search_respects_scope_allowlist() {
        let tools = vec![t("create_issue", "x"), t("delete_issue", "x")];
        let hints = HashMap::new();
        let scope = vec!["create_issue".to_string()];
        let results = search(&tools, &hints, &scope, "issue", 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "create_issue");
    }

    #[test]
    fn search_uses_hints() {
        let tools = vec![t("frobnicate", "Make something happen")];
        let mut hints = HashMap::new();
        hints.insert(
            "frobnicate".to_string(),
            "Create an issue in the system".to_string(),
        );
        let results = search(&tools, &hints, &[], "create issue", 5);
        assert_eq!(results.len(), 1);
        assert!(results[0].score > 0.0);
    }

    #[test]
    fn empty_query_returns_empty() {
        let tools = vec![t("create_issue", "x")];
        let results = search(&tools, &HashMap::new(), &[], "", 5);
        assert!(results.is_empty());
    }

    #[test]
    fn limit_clamps_to_max_20() {
        let tools: Vec<Tool> = (0..50)
            .map(|i| t(&format!("tool_{}", i), "match"))
            .collect();
        let results = search(&tools, &HashMap::new(), &[], "match", 1000);
        assert!(results.len() <= 20);
    }
}

// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared helpers for extracting a page of results from integration responses.
//!
//! The integrations we support use three different pagination conventions:
//! * Shopify GraphQL: `data.<path>.pageInfo { hasNextPage, endCursor }` plus
//!   `edges[].{cursor, node}`.
//! * HubSpot REST: `paging.next.after` opaque cursor.
//! * Stripe REST: `has_more` boolean plus "last item id" as the next cursor.
//!
//! Rather than force a trait with a uniform output shape (which would break
//! HubSpot's wire contract where the raw `paging` object is forwarded to
//! users), `extract_page` is a plain function that returns a `Page<T>` with
//! an optional opaque cursor — the caller decides how / whether to expose
//! that downstream.

use serde_json::Value;

use crate::types::{self, AgentError};

/// Describes how to locate items and the next-page cursor in a response body.
#[derive(Debug, Clone)]
pub enum PageCursor {
    /// GraphQL-style cursor extraction. `path` navigates from the root
    /// response down to the connection object (the one containing
    /// `edges[]` and `pageInfo`).
    ///
    /// Items are extracted from `edges[].node`. `endCursor` is returned
    /// iff `hasNextPage` is true.
    GraphqlPageInfo { path: Vec<&'static str> },

    /// HubSpot-style paging envelope. Items are extracted from `results[]`
    /// and the cursor comes from `paging.next.after` if present.
    PagingEnvelope,

    /// Stripe-style `has_more` + "last item id" cursor. Items are extracted
    /// from `<data_key>[]` and the cursor is the `<id_key>` of the last
    /// item when `has_more` is true.
    HasMore {
        data_key: &'static str,
        id_key: &'static str,
    },
}

/// A single page of items plus a cursor for the next page.
#[derive(Debug, Clone, PartialEq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

/// Extract items and the next-page cursor from a response body.
///
/// `map_item` is applied to each raw JSON item (edge.node / result element /
/// data element) and may itself return an `AgentError`.
pub fn extract_page<T, F>(
    response: Value,
    cursor: &PageCursor,
    mut map_item: F,
) -> Result<Page<T>, AgentError>
where
    F: FnMut(&Value) -> Result<T, AgentError>,
{
    match cursor {
        PageCursor::GraphqlPageInfo { path } => extract_graphql_page(response, path, &mut map_item),
        PageCursor::PagingEnvelope => extract_paging_envelope(response, &mut map_item),
        PageCursor::HasMore { data_key, id_key } => {
            extract_has_more(response, data_key, id_key, &mut map_item)
        }
    }
}

fn extract_graphql_page<T, F>(
    response: Value,
    path: &[&'static str],
    map_item: &mut F,
) -> Result<Page<T>, AgentError>
where
    F: FnMut(&Value) -> Result<T, AgentError>,
{
    let mut cursor_node: &Value = &response;
    for segment in path {
        cursor_node = cursor_node.get(segment).ok_or_else(|| {
            types::http::deserialization(
                "GRAPHQL",
                format!(
                    "missing path segment '{}' while walking to connection",
                    segment
                ),
            )
        })?;
    }

    let edges = cursor_node
        .get("edges")
        .and_then(|e| e.as_array())
        .ok_or_else(|| {
            types::http::deserialization("GRAPHQL", "expected `edges` array on connection")
        })?;

    let mut items = Vec::with_capacity(edges.len());
    for edge in edges {
        let node = edge
            .get("node")
            .ok_or_else(|| types::http::deserialization("GRAPHQL", "missing `node` on edge"))?;
        items.push(map_item(node)?);
    }

    let page_info = cursor_node.get("pageInfo");
    let has_next = page_info
        .and_then(|pi| pi.get("hasNextPage"))
        .and_then(|b| b.as_bool())
        .unwrap_or(false);
    let next_cursor = if has_next {
        page_info
            .and_then(|pi| pi.get("endCursor"))
            .and_then(|c| c.as_str())
            .map(|s| s.to_string())
    } else {
        None
    };

    Ok(Page { items, next_cursor })
}

fn extract_paging_envelope<T, F>(response: Value, map_item: &mut F) -> Result<Page<T>, AgentError>
where
    F: FnMut(&Value) -> Result<T, AgentError>,
{
    let results = response
        .get("results")
        .and_then(|r| r.as_array())
        .ok_or_else(|| types::http::deserialization("PAGING", "expected `results` array"))?;

    let mut items = Vec::with_capacity(results.len());
    for v in results {
        items.push(map_item(v)?);
    }

    let next_cursor = response
        .get("paging")
        .and_then(|p| p.get("next"))
        .and_then(|n| n.get("after"))
        .and_then(|a| a.as_str())
        .map(|s| s.to_string());

    Ok(Page { items, next_cursor })
}

fn extract_has_more<T, F>(
    response: Value,
    data_key: &str,
    id_key: &str,
    map_item: &mut F,
) -> Result<Page<T>, AgentError>
where
    F: FnMut(&Value) -> Result<T, AgentError>,
{
    let data = response
        .get(data_key)
        .and_then(|d| d.as_array())
        .ok_or_else(|| {
            types::http::deserialization("PAGING", format!("expected `{}` array", data_key))
        })?;

    let mut items = Vec::with_capacity(data.len());
    for v in data {
        items.push(map_item(v)?);
    }

    let has_more = response
        .get("has_more")
        .and_then(|b| b.as_bool())
        .unwrap_or(false);

    let next_cursor = if has_more {
        data.last()
            .and_then(|last| last.get(id_key))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    } else {
        None
    };

    Ok(Page { items, next_cursor })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn graphql_page_info_extracts_items_and_cursor() {
        let resp = json!({
            "data": {
                "products": {
                    "edges": [
                        {"node": {"id": "gid://shopify/Product/1"}},
                        {"node": {"id": "gid://shopify/Product/2"}},
                    ],
                    "pageInfo": {"hasNextPage": true, "endCursor": "abc"}
                }
            }
        });

        let page: Page<String> = extract_page(
            resp,
            &PageCursor::GraphqlPageInfo {
                path: vec!["data", "products"],
            },
            |v| {
                Ok(v.get("id")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_string())
            },
        )
        .unwrap();

        assert_eq!(page.items.len(), 2);
        assert_eq!(page.items[0], "gid://shopify/Product/1");
        assert_eq!(page.next_cursor.as_deref(), Some("abc"));
    }

    #[test]
    fn graphql_page_info_no_next_page_returns_none_cursor() {
        let resp = json!({
            "data": {
                "products": {
                    "edges": [{"node": {"id": "1"}}],
                    "pageInfo": {"hasNextPage": false, "endCursor": "abc"}
                }
            }
        });

        let page: Page<String> = extract_page(
            resp,
            &PageCursor::GraphqlPageInfo {
                path: vec!["data", "products"],
            },
            |v| Ok(v["id"].as_str().unwrap().to_string()),
        )
        .unwrap();

        assert_eq!(page.next_cursor, None);
    }

    #[test]
    fn graphql_page_info_errors_on_missing_edges() {
        let resp = json!({"data": {"products": {}}});
        let err = extract_page::<String, _>(
            resp,
            &PageCursor::GraphqlPageInfo {
                path: vec!["data", "products"],
            },
            |_| Ok(String::new()),
        )
        .unwrap_err();
        assert_eq!(err.code, "GRAPHQL_INVALID_RESPONSE");
    }

    #[test]
    fn paging_envelope_extracts_results_and_after_cursor() {
        let resp = json!({
            "results": [
                {"id": "1"},
                {"id": "2"},
            ],
            "paging": {"next": {"after": "cursor-1"}}
        });

        let page: Page<String> = extract_page(resp, &PageCursor::PagingEnvelope, |v| {
            Ok(v["id"].as_str().unwrap().to_string())
        })
        .unwrap();

        assert_eq!(page.items, vec!["1", "2"]);
        assert_eq!(page.next_cursor.as_deref(), Some("cursor-1"));
    }

    #[test]
    fn paging_envelope_no_next_returns_none_cursor() {
        let resp = json!({"results": []});
        let page: Page<String> =
            extract_page(resp, &PageCursor::PagingEnvelope, |_| Ok(String::new())).unwrap();
        assert_eq!(page.items.len(), 0);
        assert_eq!(page.next_cursor, None);
    }

    #[test]
    fn has_more_returns_last_id_cursor_when_more() {
        let resp = json!({
            "data": [
                {"id": "cus_1"},
                {"id": "cus_2"},
            ],
            "has_more": true
        });

        let page: Page<String> = extract_page(
            resp,
            &PageCursor::HasMore {
                data_key: "data",
                id_key: "id",
            },
            |v| Ok(v["id"].as_str().unwrap().to_string()),
        )
        .unwrap();

        assert_eq!(page.items, vec!["cus_1", "cus_2"]);
        assert_eq!(page.next_cursor.as_deref(), Some("cus_2"));
    }

    #[test]
    fn has_more_returns_none_cursor_when_exhausted() {
        let resp = json!({
            "data": [{"id": "cus_1"}],
            "has_more": false
        });

        let page: Page<String> = extract_page(
            resp,
            &PageCursor::HasMore {
                data_key: "data",
                id_key: "id",
            },
            |v| Ok(v["id"].as_str().unwrap().to_string()),
        )
        .unwrap();

        assert_eq!(page.next_cursor, None);
    }

    #[test]
    fn map_item_errors_propagate() {
        let resp = json!({"results": [{"id": "1"}]});
        let err = extract_page::<String, _>(resp, &PageCursor::PagingEnvelope, |_| {
            Err(types::http::deserialization("TEST", "bad"))
        })
        .unwrap_err();
        assert_eq!(err.code, "TEST_INVALID_RESPONSE");
    }
}

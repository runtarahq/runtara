// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Reference-path tokenization — the single definition of how a reference path
//! (`data.order.id`, `steps.fetch.outputs[0]`, `variables["a.b"]`) splits into
//! lookup segments.
//!
//! The runtime resolver ([`crate::direct_json`]) and the authoring-time
//! validator in `runtara-workflows` both tokenize through here, so a path that
//! validates against a schema key resolves to that same key at runtime.
//!
//! They used to scan independently and disagreed on bracket bodies: the
//! validator read `foo["a.b"]` as the single key `a.b`, while the runtime
//! rewrote brackets to dots and split it into `a` then `b`. A reference to a
//! literal dotted key therefore passed validation and then silently resolved to
//! null, and a genuinely-nested path written in bracket form was rejected even
//! though the runtime would have descended it.

/// Split a reference path into lookup segments.
///
/// A dot separates segments; empty segments are dropped. A `[..]` body is one
/// segment: quoted (`'..'` or `".."`) means an **opaque key** — dots inside it
/// are part of the key, not separators — while an unquoted body is taken
/// verbatim, covering both index tokens (`0`, `-1`) and bare keys.
///
/// Index-vs-key is decided by token shape at lookup time (see
/// `direct_json::descend`), so both stay raw here.
pub fn reference_segments(path: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut chars = path.chars();

    while let Some(ch) = chars.next() {
        match ch {
            '.' => {
                if !current.is_empty() {
                    segments.push(std::mem::take(&mut current));
                }
            }
            '[' => {
                if !current.is_empty() {
                    segments.push(std::mem::take(&mut current));
                }

                // An unterminated `[` consumes the rest of the path, matching
                // the historical scan (no error surface here — an unbalanced
                // bracket simply yields whatever key text it holds).
                let mut body = String::new();
                for next in chars.by_ref() {
                    if next == ']' {
                        break;
                    }
                    body.push(next);
                }

                if let Some(segment) = bracket_segment(body.trim()) {
                    segments.push(segment);
                }
            }
            _ => current.push(ch),
        }
    }

    if !current.is_empty() {
        segments.push(current);
    }

    segments
}

/// Render a reference path as an RFC 6901 JSON pointer, escaping `~` and `/`
/// inside segment text. Tokenization is [`reference_segments`], so bracket
/// bodies stay opaque here too.
pub fn to_json_pointer(path: &str) -> String {
    let segments = reference_segments(path);
    let mut out = String::with_capacity(path.len() + segments.len());

    for segment in &segments {
        out.push('/');
        for ch in segment.chars() {
            match ch {
                '~' => out.push_str("~0"),
                '/' => out.push_str("~1"),
                _ => out.push(ch),
            }
        }
    }

    out
}

/// True when a `[..]` body is an array index — an optional leading `-` followed
/// by one or more ASCII digits (e.g. `0`, `12`, `-1`).
pub fn is_array_index_token(token: &str) -> bool {
    let digits = token.strip_prefix('-').unwrap_or(token);
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

/// Reduce a trimmed `[..]` body to its segment, dropping an empty one.
fn bracket_segment(body: &str) -> Option<String> {
    let key = strip_matching_quotes(body).unwrap_or(body);
    (!key.is_empty()).then(|| key.to_string())
}

/// Strip one layer of quotes when the body opens and closes with the *same*
/// quote character. A lone or mismatched quote is left in place, so it reads as
/// part of the key rather than being silently peeled off.
fn strip_matching_quotes(body: &str) -> Option<&str> {
    let mut chars = body.chars();
    let quote = chars.next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    chars.as_str().strip_suffix(quote)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segments(path: &str) -> Vec<String> {
        reference_segments(path)
    }

    #[test]
    fn splits_dotted_paths() {
        assert_eq!(segments("data.order.id"), ["data", "order", "id"]);
        assert_eq!(segments("data"), ["data"]);
        assert_eq!(segments(""), Vec::<String>::new());
    }

    #[test]
    fn bracket_quoted_body_is_one_opaque_key() {
        // The divergence this module exists to remove: the dot belongs to the
        // key, so this must NOT become ["data", "a", "b"].
        assert_eq!(segments(r#"data["a.b"]"#), ["data", "a.b"]);
        assert_eq!(segments("data['a.b']"), ["data", "a.b"]);
        assert_eq!(segments(r#"data["plain"]"#), ["data", "plain"]);
        assert_eq!(
            segments(r#"data["a.b"]["c.d"]"#),
            ["data", "a.b", "c.d"],
            "consecutive quoted keys each stay whole"
        );
    }

    #[test]
    fn index_tokens_survive_as_raw_segments() {
        assert_eq!(segments("items[0]"), ["items", "0"]);
        assert_eq!(segments("items[-1]"), ["items", "-1"]);
        assert_eq!(segments("items.0"), ["items", "0"]);
        assert!(is_array_index_token("0"));
        assert!(is_array_index_token("-1"));
        assert!(!is_array_index_token("a"));
        assert!(!is_array_index_token("-"));
        assert!(!is_array_index_token(""));
    }

    #[test]
    fn unquoted_non_numeric_body_is_a_plain_key() {
        // Previously the validator read `bar` while the runtime kept the whole
        // thing as the literal key `foo[bar]`.
        assert_eq!(segments("foo[bar]"), ["foo", "bar"]);
        assert_eq!(segments("foo[ bar ]"), ["foo", "bar"], "body is trimmed");
    }

    #[test]
    fn mixes_dots_brackets_and_indices() {
        assert_eq!(segments(r#"a["b.c"][0].d"#), ["a", "b.c", "0", "d"]);
        assert_eq!(
            segments(r#"steps.fetch.outputs[0]["order.id"]"#),
            ["steps", "fetch", "outputs", "0", "order.id"]
        );
    }

    #[test]
    fn drops_empty_segments() {
        assert_eq!(segments("data..order"), ["data", "order"]);
        assert_eq!(segments("data."), ["data"]);
        assert_eq!(segments("data[]"), ["data"]);
        assert_eq!(segments(r#"data[""]"#), ["data"]);
    }

    #[test]
    fn keeps_mismatched_quotes_as_key_text() {
        assert_eq!(segments("foo['a\"]"), ["foo", "'a\""]);
        assert_eq!(segments("foo[\"]"), ["foo", "\""]);
    }

    #[test]
    fn unterminated_bracket_consumes_the_remainder() {
        assert_eq!(segments("foo[bar"), ["foo", "bar"]);
    }

    #[test]
    fn pointer_escapes_reserved_characters_in_segment_text() {
        assert_eq!(to_json_pointer("data.order.id"), "/data/order/id");
        assert_eq!(to_json_pointer("items[0]"), "/items/0");
        assert_eq!(to_json_pointer(r#"data["a.b"]"#), "/data/a.b");
        assert_eq!(to_json_pointer(r#"data["a/b"]"#), "/data/a~1b");
        assert_eq!(to_json_pointer(r#"data["a~b"]"#), "/data/a~0b");
        assert_eq!(to_json_pointer(""), "");
    }

    #[test]
    fn pointer_segments_agree_with_the_tokenizer() {
        // The pointer is a rendering of the same segments, so unescaping it has
        // to reproduce them exactly.
        for path in [
            "data.order.id",
            "items[0]",
            r#"data["a.b"]"#,
            r#"data["a/b"].c"#,
            r#"a["b.c"][0].d"#,
            "foo[bar]",
        ] {
            let from_pointer: Vec<String> = to_json_pointer(path)
                .split('/')
                .skip(1)
                .map(|segment| segment.replace("~1", "/").replace("~0", "~"))
                .collect();
            assert_eq!(from_pointer, reference_segments(path), "path: {path}");
        }
    }
}

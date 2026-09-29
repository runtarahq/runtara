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

/// Something [`tokenize_reference`] dropped or read past while splitting a
/// path. The segments are unaffected — the runtime resolves the path exactly as
/// tokenized — so these only matter to callers that want to reject a malformed
/// path rather than resolve it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PathDefect {
    /// Two dots in a row outside a closed `[..]` body (`steps..outputs`), so
    /// an empty dot segment was dropped. Dots inside a closed body belong to
    /// the key (`data["a..b"]`); inside an *unterminated* body they still
    /// count, since `outputs[0..name` is a malformed path, not a key.
    ConsecutiveDots,
    /// A closed `[..]` body that is empty after trimming and unquoting (`[]`,
    /// `[""]`, `['']`, `[ ]`), which yields no segment at all. An unterminated
    /// `[` with nothing after it is [`PathDefect::UnterminatedBracket`] only —
    /// there is no body to be empty.
    EmptyBracketKey,
    /// A `[` with no closing `]`; the rest of the path became its body.
    UnterminatedBracket,
    /// The path ends in a dot outside any bracket body (`data.`,
    /// `variables.x.`), so an empty final segment was dropped. A leading dot is
    /// not reported: it leaves an empty root, which callers reject as such.
    TrailingDot,
}

/// A reference path split into lookup segments, plus each distinct
/// [`PathDefect`] met along the way (each kind is recorded at most once).
///
/// Not every dropped segment is a defect: a leading dot (`.data`) is dropped
/// silently, since it leaves an empty root that callers already reject.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenizedPath {
    pub segments: Vec<String>,
    pub defects: Vec<PathDefect>,
}

impl TokenizedPath {
    pub fn has_defect(&self, defect: PathDefect) -> bool {
        self.defects.contains(&defect)
    }

    fn record(&mut self, defect: PathDefect) {
        if !self.has_defect(defect) {
            self.defects.push(defect);
        }
    }
}

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
    tokenize_reference(path).segments
}

/// [`reference_segments`], also reporting each [`PathDefect`] met on the way.
///
/// This is the one scan both the resolver and the validator go through, so a
/// validator check for a malformed path can never disagree with how the
/// runtime actually split it.
pub fn tokenize_reference(path: &str) -> TokenizedPath {
    let mut tokenized = TokenizedPath::default();
    let mut current = String::new();
    let mut previous_dot = false;
    let mut chars = path.chars();

    while let Some(ch) = chars.next() {
        match ch {
            '.' => {
                if previous_dot {
                    tokenized.record(PathDefect::ConsecutiveDots);
                }
                previous_dot = true;
                if !current.is_empty() {
                    tokenized.segments.push(std::mem::take(&mut current));
                }
            }
            '[' => {
                previous_dot = false;
                if !current.is_empty() {
                    tokenized.segments.push(std::mem::take(&mut current));
                }

                // An unterminated `[` consumes the rest of the path, matching
                // the historical scan: an unbalanced bracket still yields
                // whatever key text it holds, and is reported as a defect.
                let mut body = String::new();
                let mut closed = false;
                for next in chars.by_ref() {
                    if next == ']' {
                        closed = true;
                        break;
                    }
                    body.push(next);
                }

                if !closed {
                    tokenized.record(PathDefect::UnterminatedBracket);
                    if body.contains("..") {
                        tokenized.record(PathDefect::ConsecutiveDots);
                    }
                }

                match bracket_segment(body.trim()) {
                    Some(segment) => tokenized.segments.push(segment),
                    None if closed => tokenized.record(PathDefect::EmptyBracketKey),
                    None => {}
                }
            }
            _ => {
                previous_dot = false;
                current.push(ch);
            }
        }
    }

    if previous_dot {
        tokenized.record(PathDefect::TrailingDot);
    }

    if !current.is_empty() {
        tokenized.segments.push(current);
    }

    tokenized
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
///
/// Shared with the validator so a segment that indexes an array at run time is
/// also treated as an index at authoring time. A plain `parse::<usize>()` is
/// *not* equivalent: it rejects the negative forms the resolver accepts.
pub fn is_array_index_token(token: &str) -> bool {
    let digits = token.strip_prefix('-').unwrap_or(token);
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

/// Resolve an index segment against an array of length `len`, supporting
/// Python-style negative suffix indexing: `-1` is the last element, `-2` the
/// second-to-last. Non-numeric segments and out-of-range negatives return
/// `None`, so an unmatched index falls through to the resolver's null/default
/// path exactly like an out-of-range positive index does.
pub fn array_index(segment: &str, len: usize) -> Option<usize> {
    let raw: i64 = segment.parse().ok()?;
    if raw >= 0 {
        usize::try_from(raw).ok()
    } else {
        // `unsigned_abs` avoids overflow at `i64::MIN`.
        len.checked_sub(usize::try_from(raw.unsigned_abs()).ok()?)
    }
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

    fn defects(path: &str) -> Vec<PathDefect> {
        tokenize_reference(path).defects
    }

    fn has_consecutive_dots(path: &str) -> bool {
        tokenize_reference(path).has_defect(PathDefect::ConsecutiveDots)
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
    fn array_index_resolves_negatives_from_the_end() {
        assert_eq!(array_index("0", 3), Some(0));
        assert_eq!(array_index("2", 3), Some(2));
        assert_eq!(array_index("-1", 3), Some(2));
        assert_eq!(array_index("-3", 3), Some(0));
        // Out of range in either direction is a miss, not a wrap-around.
        assert_eq!(array_index("-4", 3), None);
        assert_eq!(array_index("a", 3), None);
    }

    #[test]
    fn negative_indices_are_index_tokens_not_keys() {
        // The trap this pair exists to avoid: `parse::<usize>()` rejects the
        // negative forms `array_index` resolves, so a validator using it would
        // reject a reference the runtime happily resolves.
        for token in ["-1", "-2", "-10"] {
            assert!(is_array_index_token(token), "{token}");
            assert!(token.parse::<usize>().is_err(), "{token}");
        }
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
    fn consecutive_dots_count_only_outside_brackets() {
        assert!(has_consecutive_dots("steps..outputs"));
        assert!(has_consecutive_dots("data.a..b"));
        assert!(has_consecutive_dots(r#"data["a"]..b"#));
        assert!(has_consecutive_dots(r#"data["a..b"].c..d"#));

        // Inside a bracket body the dots are part of one opaque key, which
        // the tokenizer keeps whole.
        for path in [
            r#"data["a..b"]"#,
            "data['a..b']",
            "data[a..b]",
            r#"steps["fetch"].outputs["x...y"].z"#,
        ] {
            assert!(!has_consecutive_dots(path), "{path}");
        }
        assert_eq!(segments(r#"data["a..b"]"#), ["data", "a..b"]);

        // A single dot around a bracket is not an empty segment.
        assert!(!has_consecutive_dots("data.order.id"));
        assert!(!has_consecutive_dots(r#"data.["a"].b"#));
        assert!(!has_consecutive_dots(""));
        // An unterminated bracket is a malformed path, not an opaque key.
        assert!(has_consecutive_dots("data[a..b"));
        assert!(has_consecutive_dots("steps.fetch.outputs[0..name"));
        assert!(!has_consecutive_dots("data[a.b"));
    }

    #[test]
    fn well_formed_paths_have_no_defects() {
        for path in [
            "",
            "data",
            "data.order.id",
            r#"data["a..b"]"#,
            "items[0].sku",
            "items[-1]",
            r#"data.["a"].b"#,
            r#"data[" "]"#,
        ] {
            assert_eq!(defects(path), [], "{path}");
        }
    }

    #[test]
    fn empty_bracket_keys_are_reported() {
        for path in [
            "data[]",
            r#"data[""]"#,
            "data['']",
            "data[ ]",
            r#"data[ "" ]"#,
        ] {
            assert!(
                tokenize_reference(path).has_defect(PathDefect::EmptyBracketKey),
                "{path}"
            );
            assert_eq!(segments(path), ["data"], "{path}");
        }
        assert!(tokenize_reference("steps.a.outputs[].id").has_defect(PathDefect::EmptyBracketKey));
    }

    #[test]
    fn unterminated_brackets_are_reported() {
        assert_eq!(defects("data[a.b"), [PathDefect::UnterminatedBracket]);
        // No body at all: a missing `]`, not an empty key.
        assert_eq!(defects("data["), [PathDefect::UnterminatedBracket]);
        assert_eq!(defects("data[ "), [PathDefect::UnterminatedBracket]);
        assert_eq!(
            defects("data[a..b"),
            [PathDefect::UnterminatedBracket, PathDefect::ConsecutiveDots]
        );
    }

    #[test]
    fn defects_do_not_change_segments() {
        // The runtime resolves exactly these segments, defects or not.
        assert_eq!(segments("steps..outputs"), ["steps", "outputs"]);
        assert_eq!(segments("data[a..b"), ["data", "a..b"]);
        assert_eq!(segments("data[]"), ["data"]);
        let tokenized = tokenize_reference(r#"data["a"]..b[]"#);
        assert_eq!(tokenized.segments, ["data", "a", "b"]);
        assert_eq!(
            tokenized.defects,
            [PathDefect::ConsecutiveDots, PathDefect::EmptyBracketKey]
        );
    }

    #[test]
    fn each_defect_kind_is_recorded_once() {
        assert_eq!(defects("a...b..c"), [PathDefect::ConsecutiveDots]);
        assert_eq!(defects("a[][]"), [PathDefect::EmptyBracketKey]);
    }

    #[test]
    fn trailing_dot_is_reported_leading_dot_is_not() {
        assert_eq!(defects("steps.a.outputs."), [PathDefect::TrailingDot]);
        assert_eq!(defects("data."), [PathDefect::TrailingDot]);
        assert_eq!(defects(r#"data["a"]."#), [PathDefect::TrailingDot]);
        assert_eq!(segments("steps.a.outputs."), ["steps", "a", "outputs"]);

        // Inside a closed bracket the dot is part of the key.
        assert_eq!(defects(r#"data["a."]"#), []);
        // A leading dot leaves an empty root, which callers reject as such.
        assert_eq!(defects(".data"), []);
        // Consecutive dots at the end are both.
        assert_eq!(
            defects("data.."),
            [PathDefect::ConsecutiveDots, PathDefect::TrailingDot]
        );
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

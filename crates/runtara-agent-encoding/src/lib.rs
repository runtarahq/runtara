// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared character-encoding vocabulary for runtara agents.
//!
//! Every encoding-sensitive agent (text, csv, xml) speaks the **same** set of
//! encoding names by routing through this crate. The vocabulary is anchored on
//! [`encoding_rs`]: `Encoding::for_label` is the WHATWG label/alias table and
//! `Encoding::name` is the canonical name. [`chardetng`] (the same upstream)
//! returns an `encoding_rs::Encoding` directly, so a *detected* encoding and a
//! *requested* encoding are guaranteed to share one naming scheme.
//!
//! - [`Encoding`] — what agents accept as a capability input. It deserializes
//!   from **any** WHATWG label (so `utf8`, `latin-1`, `iso-8859-1`, `cp1252`,
//!   `sjis`, `gb2312`, … all parse, as do every canonical name the detector can
//!   emit), plus the special `"Auto"`. It also advertises a *curated* set of
//!   common names ([`Encoding::variant_names`]) that the Step Picker renders as
//!   a dropdown — suggestions, not a hard limit, which is why a detected name
//!   always round-trips even when it isn't one of the suggestions.
//! - [`detect`] — BOM sniffing, then statistical detection via chardetng.
//! - [`decode`] — bytes → string for a chosen [`Encoding`] (lossy, never fails);
//!   [`Encoding::Auto`] detects first. The canonical name actually used is
//!   reported back so callers can echo an aligned name.

use std::fmt;

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::Encoding as ErsEncoding;
use runtara_dsl::agent_meta::EnumVariants;
use serde::{Deserialize, Deserializer, de};

/// Cap on how many leading bytes are fed to the statistical detector. Detection
/// only needs a representative prefix; decoding still uses the whole input.
const DETECT_SAMPLE_LIMIT: usize = 64 * 1024;

/// An encoding choice accepted by encoding-sensitive agents.
///
/// Deserializes leniently from any WHATWG encoding label via
/// `encoding_rs::Encoding::for_label` (covering every alias and every canonical
/// name the detector can produce), or from `"Auto"` to request detection. An
/// unknown label is a deserialization error rather than a silent fallback.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// Detect the encoding from the bytes (BOM, then statistical analysis).
    Auto,
    /// A concrete encoding.
    Charset(&'static ErsEncoding),
}

impl Encoding {
    /// The concrete `encoding_rs` encoding, or `None` for [`Encoding::Auto`]
    /// (which is resolved at decode time from the bytes themselves).
    pub fn resolve(self) -> Option<&'static ErsEncoding> {
        match self {
            Encoding::Auto => None,
            Encoding::Charset(enc) => Some(enc),
        }
    }

    /// Parse a label the way deserialization does: `"auto"` (any case) →
    /// [`Encoding::Auto`]; otherwise the WHATWG label table. Empty/whitespace is
    /// treated as `Auto`.
    pub fn from_label(label: &str) -> Option<Encoding> {
        let trimmed = label.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") {
            return Some(Encoding::Auto);
        }
        // `latin-1` (with the hyphen) is how the pre-shared-crate agents spelled
        // it, but it is not a WHATWG label — `latin1` and `iso-8859-1` are. Map
        // that one legacy spelling so old workflows keep parsing.
        let label_bytes: &[u8] = if trimmed.eq_ignore_ascii_case("latin-1") {
            b"latin1"
        } else {
            trimmed.as_bytes()
        };
        ErsEncoding::for_label(label_bytes).map(Encoding::Charset)
    }
}

impl Default for Encoding {
    /// UTF-8 — preserves historical agent behavior when no encoding is given.
    fn default() -> Self {
        Encoding::Charset(encoding_rs::UTF_8)
    }
}

impl fmt::Debug for Encoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Encoding::Auto => f.write_str("Auto"),
            Encoding::Charset(enc) => write!(f, "Charset({})", enc.name()),
        }
    }
}

impl<'de> Deserialize<'de> for Encoding {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let label = String::deserialize(deserializer)?;
        Encoding::from_label(&label).ok_or_else(|| {
            de::Error::custom(format!(
                "unknown character encoding label: `{label}` (expected a WHATWG \
                 encoding name such as UTF-8, windows-1252, Shift_JIS, or `Auto`)"
            ))
        })
    }
}

impl EnumVariants for Encoding {
    /// Curated dropdown suggestions, in display order. These are exactly the
    /// canonical `encoding_rs` names (plus `"Auto"`); free-text labels and other
    /// canonical names still parse (see the type docs), so this list never
    /// constrains alignment — it only shapes the picker UI. Kept honest by
    /// `test_variant_names_are_canonical`.
    fn variant_names() -> &'static [&'static str] {
        &[
            "Auto",
            "UTF-8",
            "UTF-16LE",
            "UTF-16BE",
            "windows-1252",
            "windows-1250",
            "windows-1251",
            "Shift_JIS",
            "EUC-JP",
            "GBK",
            "Big5",
            "KOI8-R",
        ]
    }
}

/// Outcome of [`detect`].
#[derive(Debug, Clone, Copy)]
pub struct Detection {
    /// Canonical encoding name (e.g. `"UTF-8"`, `"windows-1252"`). Always a
    /// value [`Encoding`] can parse and the decoder understands.
    pub encoding_name: &'static str,
    /// Whether the guess rests on real signal: `true` if a BOM was found or the
    /// input contains non-ASCII bytes. Pure-ASCII input makes the *label*
    /// arbitrary (every ASCII-superset decodes identically), so it is reported
    /// as not confident even though decoding is unaffected.
    pub confident: bool,
    /// Whether a byte-order mark determined the result.
    pub bom: bool,
}

/// Outcome of [`decode`].
#[derive(Debug, Clone)]
pub struct DecodeOutcome {
    /// The decoded text. Malformed sequences become U+FFFD (decoding never
    /// fails); inspect [`DecodeOutcome::had_errors`] to detect that.
    pub text: String,
    /// Canonical name of the encoding actually used (a leading BOM can override
    /// the requested encoding — that override is reflected here).
    pub encoding_name: &'static str,
    /// Whether any malformed sequence was replaced during decoding.
    pub had_errors: bool,
}

/// Detect the encoding of `bytes`.
///
/// A byte-order mark wins outright; otherwise a leading sample is fed to
/// chardetng. `tld` is an optional country-code TLD hint (e.g. `b"jp"`, `b"ru"`)
/// that biases the guess — invalid hints are ignored rather than panicking.
/// `allow_utf8` lets the detector return UTF-8.
pub fn detect(bytes: &[u8], tld: Option<&[u8]>, allow_utf8: bool) -> Detection {
    let (enc, confident, bom) = detect_inner(bytes, tld, allow_utf8);
    Detection {
        encoding_name: enc.name(),
        confident,
        bom,
    }
}

/// Decode `bytes` to a `String` using `encoding`. [`Encoding::Auto`] detects
/// first. Lossy and infallible — see [`DecodeOutcome`].
pub fn decode(bytes: &[u8], encoding: Encoding) -> DecodeOutcome {
    let enc = match encoding.resolve() {
        Some(enc) => enc,
        None => detect_inner(bytes, None, true).0,
    };
    // `encoding_rs::Encoding::decode` also strips/handles a leading BOM and may
    // return a *different* encoding than `enc` when one is present; report the
    // one actually used.
    let (text, actual, had_errors) = enc.decode(bytes);
    DecodeOutcome {
        text: text.into_owned(),
        encoding_name: actual.name(),
        had_errors,
    }
}

/// Shared detection core returning the concrete encoding so [`decode`] can use
/// it without a name round-trip.
fn detect_inner(
    bytes: &[u8],
    tld: Option<&[u8]>,
    allow_utf8: bool,
) -> (&'static ErsEncoding, bool, bool) {
    if let Some((enc, _bom_len)) = ErsEncoding::for_bom(bytes) {
        return (enc, true, true);
    }
    let mut detector = EncodingDetector::new(Iso2022JpDetection::Deny);
    let sample = &bytes[..bytes.len().min(DETECT_SAMPLE_LIMIT)];
    detector.feed(sample, true);
    let utf8 = if allow_utf8 {
        Utf8Detection::Allow
    } else {
        Utf8Detection::Deny
    };
    let enc = detector.guess(sanitize_tld(tld), utf8);
    let confident = bytes.iter().any(|&b| b >= 0x80);
    (enc, confident, false)
}

/// `EncodingDetector::guess` panics if the TLD contains a period, upper-case, or
/// non-ASCII byte. Pass the hint through only when it can't trip that.
fn sanitize_tld(tld: Option<&[u8]>) -> Option<&[u8]> {
    tld.filter(|t| {
        !t.is_empty()
            && t.iter()
                .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(label: &str) -> Encoding {
        serde_json::from_value(serde_json::Value::String(label.to_string()))
            .unwrap_or_else(|e| panic!("`{label}` should parse as an Encoding: {e}"))
    }

    /// Every curated dropdown name is either `Auto` or a real canonical
    /// `encoding_rs` name (so it parses and round-trips byte-identically).
    /// Guards the hand-written `variant_names()` against drift.
    #[test]
    fn test_variant_names_are_canonical() {
        for &name in Encoding::variant_names() {
            let enc = parse(name);
            if name == "Auto" {
                assert_eq!(enc, Encoding::Auto);
                assert!(enc.resolve().is_none());
            } else {
                let resolved = enc.resolve().expect("non-Auto variants resolve");
                assert_eq!(
                    resolved.name(),
                    name,
                    "dropdown name `{name}` must equal encoding_rs canonical name"
                );
            }
        }
    }

    #[test]
    fn test_aliases_parse() {
        assert_eq!(parse("utf8").resolve().unwrap().name(), "UTF-8");
        assert_eq!(parse("UTF-8").resolve().unwrap().name(), "UTF-8");
        assert_eq!(parse("latin-1").resolve().unwrap().name(), "windows-1252");
        assert_eq!(
            parse("ISO-8859-1").resolve().unwrap().name(),
            "windows-1252"
        );
        assert_eq!(parse("cp1252").resolve().unwrap().name(), "windows-1252");
        assert_eq!(parse("sjis").resolve().unwrap().name(), "Shift_JIS");
        assert_eq!(parse("gb2312").resolve().unwrap().name(), "GBK");
        assert_eq!(parse("auto"), Encoding::Auto);
        assert_eq!(parse("AUTO"), Encoding::Auto);
        assert_eq!(parse(""), Encoding::Auto);
    }

    #[test]
    fn test_unknown_label_errors() {
        let r: Result<Encoding, _> =
            serde_json::from_value(serde_json::Value::String("not-an-encoding".into()));
        assert!(r.is_err());
    }

    #[test]
    fn test_default_is_utf8() {
        assert_eq!(Encoding::default().resolve().unwrap().name(), "UTF-8");
    }

    #[test]
    fn test_decode_utf8() {
        let out = decode("héllo".as_bytes(), Encoding::default());
        assert_eq!(out.text, "héllo");
        assert_eq!(out.encoding_name, "UTF-8");
        assert!(!out.had_errors);
    }

    #[test]
    fn test_decode_windows_1252() {
        // 0xE9 is 'é' in windows-1252 / ISO-8859-1.
        let out = decode(&[b'h', 0xE9, b'l', b'l', b'o'], parse("windows-1252"));
        assert_eq!(out.text, "héllo");
        assert_eq!(out.encoding_name, "windows-1252");
    }

    #[test]
    fn test_detect_bom_utf16le() {
        // UTF-16LE BOM (0xFF 0xFE) + "hi".
        let bytes = [0xFF, 0xFE, b'h', 0x00, b'i', 0x00];
        let det = detect(&bytes, None, true);
        assert_eq!(det.encoding_name, "UTF-16LE");
        assert!(det.bom);
        assert!(det.confident);
    }

    #[test]
    fn test_detect_plain_ascii_not_confident() {
        let det = detect(b"hello, world", None, true);
        // ASCII is valid UTF-8; not confident because the label is arbitrary.
        assert_eq!(det.encoding_name, "UTF-8");
        assert!(!det.bom);
        assert!(!det.confident);
    }

    #[test]
    fn test_invalid_tld_does_not_panic() {
        // Periods / upper-case / non-ascii would panic chardetng; must be ignored.
        let _ = detect("héllo".as_bytes(), Some(b"co.uk"), true);
        let _ = detect("héllo".as_bytes(), Some(b"JP"), true);
        let _ = detect("héllo".as_bytes(), Some("рф".as_bytes()), true);
    }

    /// End-to-end alignment: the name `detect` reports parses straight back into
    /// an `Encoding`, and `Auto` decode uses that same detected encoding.
    #[test]
    fn test_auto_decode_matches_detect() {
        let bytes = [b'h', 0xE9, b'l', b'l', b'o']; // windows-1252 'é'
        let det = detect(&bytes, None, true);
        let via_auto = decode(&bytes, Encoding::Auto);
        let reparsed = parse(via_auto.encoding_name);
        assert!(reparsed.resolve().is_some());
        assert_eq!(via_auto.encoding_name, det.encoding_name);
    }
}

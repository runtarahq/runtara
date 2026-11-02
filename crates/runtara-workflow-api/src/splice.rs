// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! A composed component handed back as a splice: the bytes only it has, and
//! where each component it was given lies in it as it is. A workflow composed
//! from the shared components holds them byte for byte, so it crosses as the
//! little that is its own, and the host rebuilds the rest from the components
//! it served the compile, copying bytes: it reads nothing it rebuilds as code.
//!
//! The body is a sequence of records:
//! - `L`, a little-endian `u32` length, then that many bytes, taken as they are;
//! - `R`, a component's SHA-256 in 64 lowercase hex digits, then a
//!   little-endian `u64` offset and `u64` length: those bytes of it.

/// Where one of the compile's components lies in the composed component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    /// The component's SHA-256, lowercase hex.
    pub sha256: String,
    /// Where in the composed component it starts.
    pub at: usize,
    /// The bytes of it that lie there, from its start.
    pub length: usize,
}

/// `artifact` as a splice of `placements`, which must not overlap and must
/// each lie within it.
pub fn encode(artifact: &[u8], placements: &[Placement]) -> Vec<u8> {
    let mut placements: Vec<&Placement> = placements.iter().collect();
    placements.sort_by_key(|placement| placement.at);
    let mut body = Vec::new();
    let literal = |body: &mut Vec<u8>, bytes: &[u8]| {
        for chunk in bytes.chunks(u32::MAX as usize) {
            body.push(b'L');
            body.extend((chunk.len() as u32).to_le_bytes());
            body.extend(chunk);
        }
    };
    let mut at = 0;
    for placement in placements {
        literal(&mut body, &artifact[at..placement.at]);
        body.push(b'R');
        body.extend(placement.sha256.as_bytes());
        body.extend(0u64.to_le_bytes());
        body.extend((placement.length as u64).to_le_bytes());
        at = placement.at + placement.length;
    }
    literal(&mut body, &artifact[at..]);
    body
}

/// Rebuilds the composed component from `body`, reading each component it
/// names with `component` (by SHA-256), up to `limit` bytes in all.
pub fn decode<E>(
    body: &[u8],
    mut component: impl FnMut(&str) -> Result<std::sync::Arc<Vec<u8>>, E>,
    limit: usize,
) -> Result<Vec<u8>, DecodeError<E>> {
    let mut whole = Vec::new();
    let mut rest = body;
    // Each component is read once, however often it is named.
    let mut read: std::collections::HashMap<String, std::sync::Arc<Vec<u8>>> =
        std::collections::HashMap::new();
    fn take<'a, E>(rest: &mut &'a [u8], n: usize) -> Result<&'a [u8], DecodeError<E>> {
        if rest.len() < n {
            return Err(DecodeError::Malformed("a record is cut short"));
        }
        let (head, tail) = rest.split_at(n);
        *rest = tail;
        Ok(head)
    }
    let fits = |whole: &Vec<u8>, n: usize| -> Result<(), DecodeError<E>> {
        if whole.len().saturating_add(n) > limit {
            return Err(DecodeError::Malformed("over the limit"));
        }
        Ok(())
    };
    let mut records = 0;
    while let Some((&tag, tail)) = rest.split_first() {
        rest = tail;
        records += 1;
        if records > MAX_RECORDS {
            return Err(DecodeError::Malformed("too many records"));
        }
        match tag {
            b'L' => {
                let len = u32::from_le_bytes(take(&mut rest, 4)?.try_into().unwrap()) as usize;
                fits(&whole, len)?;
                whole.extend_from_slice(take(&mut rest, len)?);
            }
            b'R' => {
                let sha256 = std::str::from_utf8(take(&mut rest, 64)?)
                    .ok()
                    .filter(|sha| {
                        sha.bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    })
                    .ok_or(DecodeError::Malformed("not a component's SHA-256"))?
                    .to_owned();
                let offset = u64::from_le_bytes(take(&mut rest, 8)?.try_into().unwrap());
                let length = u64::from_le_bytes(take(&mut rest, 8)?.try_into().unwrap());
                if length == 0 {
                    return Err(DecodeError::Malformed("an empty reference"));
                }
                let source = match read.get(&sha256) {
                    Some(source) => source.clone(),
                    None => {
                        let source = component(&sha256).map_err(DecodeError::Component)?;
                        read.insert(sha256, source.clone());
                        source
                    }
                };
                let range = usize::try_from(offset)
                    .ok()
                    .zip(usize::try_from(length).ok())
                    .and_then(|(offset, length)| Some(offset..offset.checked_add(length)?))
                    .filter(|range| range.end <= source.len())
                    .ok_or(DecodeError::Malformed("not within the component"))?;
                fits(&whole, range.len())?;
                whole.extend_from_slice(&source[range]);
            }
            _ => return Err(DecodeError::Malformed("not a record")),
        }
    }
    Ok(whole)
}

/// The most records a splice has: a composed workflow's has a few dozen.
const MAX_RECORDS: usize = 4096;

/// Why a splice did not rebuild.
#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError<E> {
    /// The body is not a splice, or names bytes a component does not have.
    Malformed(&'static str),
    /// A component it names could not be read.
    Component(E),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[test]
    fn a_splice_rebuilds_the_component_from_its_own_bytes_and_those_it_was_given() {
        let shared = Arc::new(b"the shared component ".repeat(100));
        let sha = "a".repeat(64);
        let artifact = [
            b"head ".as_slice(),
            &shared,
            b" own ",
            &shared[..10],
            b" tail",
        ]
        .concat();
        let placements = vec![Placement {
            sha256: sha.clone(),
            at: 5,
            length: shared.len(),
        }];
        let body = encode(&artifact, &placements);
        // The shared bytes cross as a reference, not again.
        assert!(body.len() < artifact.len() - shared.len() + 100);
        let given = HashMap::from([(sha.clone(), shared.clone())]);
        let read = |sha: &str| given.get(sha).cloned().ok_or(());
        assert_eq!(decode(&body, read, 1 << 20).unwrap(), artifact);
        // Nothing but the literal: a plain copy.
        assert_eq!(
            decode(&encode(&artifact, &[]), read, 1 << 20).unwrap(),
            artifact
        );
    }

    #[test]
    fn a_splice_names_only_bytes_its_components_have_within_the_limit() {
        let shared = Arc::new(vec![7u8; 100]);
        let sha = "b".repeat(64);
        let read = |_: &str| Ok::<_, ()>(shared.clone());
        let mut body = vec![b'R'];
        body.extend(sha.as_bytes());
        body.extend(50u64.to_le_bytes());
        body.extend(51u64.to_le_bytes());
        assert_eq!(
            decode(&body, read, 1 << 20),
            Err(DecodeError::Malformed("not within the component"))
        );
        let body = encode(&[1, 2, 3], &[]);
        assert_eq!(
            decode(&body, read, 2),
            Err(DecodeError::Malformed("over the limit"))
        );
        assert_eq!(
            decode(&body[..body.len() - 1], read, 1 << 20),
            Err(DecodeError::Malformed("a record is cut short"))
        );
        assert!(decode::<()>(b"X", |_| Err(()), 1 << 20).is_err());
        assert_eq!(
            decode(b"Rnot-hex", |_| Err::<Arc<Vec<u8>>, ()>(()), 1 << 20),
            Err(DecodeError::Malformed("a record is cut short"))
        );
    }

    #[test]
    fn a_component_named_again_and_again_is_read_once_and_records_are_bounded() {
        let shared = Arc::new(vec![9u8; 1000]);
        let sha = "c".repeat(64);
        let reference = |length: u64| {
            let mut record = vec![b'R'];
            record.extend(sha.as_bytes());
            record.extend(0u64.to_le_bytes());
            record.extend(length.to_le_bytes());
            record
        };
        let reads = std::cell::Cell::new(0);
        let read = |_: &str| {
            reads.set(reads.get() + 1);
            Ok::<_, ()>(shared.clone())
        };
        let body: Vec<u8> = (0..100).flat_map(|_| reference(10)).collect();
        assert_eq!(decode(&body, read, 1 << 20).unwrap().len(), 1000);
        assert_eq!(reads.get(), 1);
        assert_eq!(
            decode(&reference(0), read, 1 << 20),
            Err(DecodeError::Malformed("an empty reference"))
        );
        let body: Vec<u8> = (0..=MAX_RECORDS).flat_map(|_| reference(1)).collect();
        assert_eq!(
            decode(&body, read, 1 << 20),
            Err(DecodeError::Malformed("too many records"))
        );
    }
}

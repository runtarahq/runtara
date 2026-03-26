// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! `OneOrMany<T>` — a non-empty collection that holds one or more items.
//!
//! Serializes as a JSON array (always). Deserializes from either a single
//! item or an array of at least one item.

use serde::de::{self, Deserializer, SeqAccess, Visitor};
use serde::ser::{SerializeSeq, Serializer};
use serde::{Deserialize, Serialize};
use std::fmt;

/// A non-empty collection: exactly one item, or many.
///
/// Serializes as an array; deserializes from a single value **or** an array.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OneOrMany<T> {
    first: T,
    rest: Vec<T>,
}

/// Error returned when trying to construct a `OneOrMany` from an empty iterator.
#[derive(Debug, thiserror::Error)]
#[error("Cannot create OneOrMany with an empty vector.")]
pub struct EmptyListError;

impl<T: Clone> OneOrMany<T> {
    /// Create a `OneOrMany` with exactly one item.
    pub fn one(item: T) -> Self {
        OneOrMany {
            first: item,
            rest: vec![],
        }
    }

    /// Create a `OneOrMany` from an iterator of at least one item.
    pub fn many<I>(items: I) -> Result<Self, EmptyListError>
    where
        I: IntoIterator<Item = T>,
    {
        let mut iter = items.into_iter();
        Ok(OneOrMany {
            first: iter.next().ok_or(EmptyListError)?,
            rest: iter.collect(),
        })
    }

    /// Get a clone of the first item.
    pub fn first(&self) -> T {
        self.first.clone()
    }

    /// Get the rest of the items (excluding the first).
    pub fn rest(&self) -> Vec<T> {
        self.rest.clone()
    }

    /// Append an item.
    pub fn push(&mut self, item: T) {
        self.rest.push(item);
    }

    /// Insert at a given index (0 = before current first).
    pub fn insert(&mut self, index: usize, item: T) {
        if index == 0 {
            let old_first = std::mem::replace(&mut self.first, item);
            self.rest.insert(0, old_first);
        } else {
            self.rest.insert(index - 1, item);
        }
    }

    /// Total number of items.
    pub fn len(&self) -> usize {
        1 + self.rest.len()
    }

    /// Always false (there is always at least one item).
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Iterate by reference.
    pub fn iter(&self) -> Iter<'_, T> {
        Iter {
            first: Some(&self.first),
            rest: self.rest.iter(),
        }
    }

    /// Iterate by mutable reference.
    pub fn iter_mut(&mut self) -> IterMut<'_, T> {
        IterMut {
            first: Some(&mut self.first),
            rest: self.rest.iter_mut(),
        }
    }

    /// Merge multiple `OneOrMany` into one.
    pub fn merge<I>(items: I) -> Result<Self, EmptyListError>
    where
        I: IntoIterator<Item = OneOrMany<T>>,
    {
        let all: Vec<T> = items.into_iter().flat_map(|om| om.into_iter()).collect();
        OneOrMany::many(all)
    }
}

// ---------------------------------------------------------------------------
// Iterators
// ---------------------------------------------------------------------------

pub struct Iter<'a, T> {
    first: Option<&'a T>,
    rest: std::slice::Iter<'a, T>,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<Self::Item> {
        self.first.take().or_else(|| self.rest.next())
    }
}

pub struct IntoIter<T> {
    first: Option<T>,
    rest: std::vec::IntoIter<T>,
}

impl<T: Clone> IntoIterator for OneOrMany<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            first: Some(self.first),
            rest: self.rest.into_iter(),
        }
    }
}

impl<T: Clone> Iterator for IntoIter<T> {
    type Item = T;
    fn next(&mut self) -> Option<Self::Item> {
        self.first.take().or_else(|| self.rest.next())
    }
}

pub struct IterMut<'a, T> {
    first: Option<&'a mut T>,
    rest: std::slice::IterMut<'a, T>,
}

impl<'a, T> Iterator for IterMut<'a, T> {
    type Item = &'a mut T;
    fn next(&mut self) -> Option<Self::Item> {
        self.first.take().or_else(|| self.rest.next())
    }
}

// ---------------------------------------------------------------------------
// Serde: always serialize as array
// ---------------------------------------------------------------------------

impl<T: Serialize + Clone> Serialize for OneOrMany<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut seq = serializer.serialize_seq(Some(self.len()))?;
        for e in self.iter() {
            seq.serialize_element(e)?;
        }
        seq.end()
    }
}

impl<'de, T: Deserialize<'de> + Clone> Deserialize<'de> for OneOrMany<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct OneOrManyVisitor<T>(std::marker::PhantomData<T>);

        impl<'de, T: Deserialize<'de> + Clone> Visitor<'de> for OneOrManyVisitor<T> {
            type Value = OneOrMany<T>;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a single item or a non-empty array")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let first: T = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(0, &self))?;
                let mut rest = Vec::new();
                while let Some(v) = seq.next_element()? {
                    rest.push(v);
                }
                Ok(OneOrMany { first, rest })
            }

            // Also accept a single map/value as OneOrMany::one(...)
            fn visit_map<M>(self, map: M) -> Result<Self::Value, M::Error>
            where
                M: de::MapAccess<'de>,
            {
                let item = T::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(OneOrMany::one(item))
            }
        }

        deserializer.deserialize_any(OneOrManyVisitor(std::marker::PhantomData))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serialize_one() {
        let om = OneOrMany::one(42);
        assert_eq!(serde_json::to_value(&om).unwrap(), json!([42]));
    }

    #[test]
    fn serialize_many() {
        let om = OneOrMany::many(vec![1, 2, 3]).unwrap();
        assert_eq!(serde_json::to_value(&om).unwrap(), json!([1, 2, 3]));
    }

    #[test]
    fn deserialize_array() {
        let om: OneOrMany<i32> = serde_json::from_value(json!([10, 20])).unwrap();
        assert_eq!(om.len(), 2);
        assert_eq!(om.first(), 10);
    }

    #[test]
    fn roundtrip() {
        let om = OneOrMany::many(vec!["a".to_string(), "b".to_string()]).unwrap();
        let json = serde_json::to_value(&om).unwrap();
        let om2: OneOrMany<String> = serde_json::from_value(json).unwrap();
        assert_eq!(om, om2);
    }
}

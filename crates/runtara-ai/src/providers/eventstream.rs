// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! AWS event stream (`application/vnd.amazon.eventstream`), read from a body
//! that arrives in pieces: each message, once all of it has arrived.
//!
//! A message is its total length and its headers' length (4 bytes each, big
//! endian), a checksum of those, the headers, the payload, and a checksum of
//! the message. The checksums are not checked: the bytes came over TLS.

use std::collections::HashMap;

/// The most one message may be: AWS's own limit.
const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

/// One message: its string headers, and its payload.
#[derive(Debug, PartialEq)]
pub(crate) struct Message {
    pub(crate) headers: HashMap<String, String>,
    pub(crate) payload: Vec<u8>,
}

impl Message {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

/// Messages read so far, and the start of the next.
#[derive(Default)]
pub(crate) struct Messages {
    pending: Vec<u8>,
}

impl Messages {
    /// Each message `bytes` finishes, in order.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<Vec<Message>, String> {
        self.pending.extend_from_slice(bytes);
        let mut messages = Vec::new();
        while self.pending.len() >= 12 {
            let total = be32(&self.pending[0..4]) as usize;
            let headers = be32(&self.pending[4..8]) as usize;
            if !(16..=MAX_MESSAGE_BYTES).contains(&total) || headers > total - 16 {
                return Err(format!(
                    "an event stream message of {total} bytes, {headers} of them headers"
                ));
            }
            if self.pending.len() < total {
                break;
            }
            let message: Vec<u8> = self.pending.drain(..total).collect();
            messages.push(Message {
                headers: read_headers(&message[12..12 + headers])?,
                payload: message[12 + headers..total - 4].to_vec(),
            });
        }
        Ok(messages)
    }

    /// Whether a message was cut short by the body's end.
    pub(crate) fn is_cut(&self) -> bool {
        !self.pending.is_empty()
    }
}

fn be32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// A message's headers; those that are not strings are passed over.
fn read_headers(mut bytes: &[u8]) -> Result<HashMap<String, String>, String> {
    let short = || "an event stream header was cut short".to_owned();
    let take = |bytes: &mut &[u8], n: usize| -> Result<Vec<u8>, String> {
        if bytes.len() < n {
            return Err(short());
        }
        let (taken, rest) = bytes.split_at(n);
        *bytes = rest;
        Ok(taken.to_vec())
    };
    let mut headers = HashMap::new();
    while !bytes.is_empty() {
        let name_length = take(&mut bytes, 1)?[0] as usize;
        let name = String::from_utf8_lossy(&take(&mut bytes, name_length)?).into_owned();
        let kind = take(&mut bytes, 1)?[0];
        let fixed = match kind {
            0 | 1 => Some(0),
            2 => Some(1),
            3 => Some(2),
            4 => Some(4),
            5 | 8 => Some(8),
            9 => Some(16),
            6 | 7 => None,
            other => return Err(format!("an event stream header of type {other}")),
        };
        match fixed {
            Some(length) => {
                take(&mut bytes, length)?;
            }
            None => {
                let length = take(&mut bytes, 2)?;
                let value = take(
                    &mut bytes,
                    u16::from_be_bytes([length[0], length[1]]) as usize,
                )?;
                if kind == 7 {
                    headers.insert(name, String::from_utf8_lossy(&value).into_owned());
                }
            }
        }
    }
    Ok(headers)
}

/// For tests: a message, with string headers, as AWS frames it (with its
/// checksums left as zeros).
#[cfg(test)]
pub(crate) fn frame(headers: &[(&str, &str)], payload: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::new();
    for (name, value) in headers {
        encoded.push(name.len() as u8);
        encoded.extend_from_slice(name.as_bytes());
        encoded.push(7);
        encoded.extend_from_slice(&(value.len() as u16).to_be_bytes());
        encoded.extend_from_slice(value.as_bytes());
    }
    let total = 16 + encoded.len() + payload.len();
    let mut message = Vec::new();
    message.extend_from_slice(&(total as u32).to_be_bytes());
    message.extend_from_slice(&(encoded.len() as u32).to_be_bytes());
    message.extend_from_slice(&[0; 4]);
    message.extend_from_slice(&encoded);
    message.extend_from_slice(payload);
    message.extend_from_slice(&[0; 4]);
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_read_however_the_body_is_cut() {
        let mut body = frame(&[(":event-type", "a"), (":message-type", "event")], b"{}");
        // A header that is not a string is passed over.
        let mut odd = frame(&[(":event-type", "b")], b"[1]");
        let headers = be32(&odd[4..8]) as usize;
        let at = 12 + headers;
        odd.splice(at..at, [4, b'n', b'u', b'm', b'!', 4, 0, 0, 0, 7]);
        let total = (be32(&odd[0..4]) + 10).to_be_bytes();
        odd[0..4].copy_from_slice(&total);
        odd[4..8].copy_from_slice(&((headers + 10) as u32).to_be_bytes());
        body.extend(odd);
        for cut in 0..=body.len() {
            let mut messages = Messages::default();
            let mut read = messages.push(&body[..cut]).unwrap();
            read.extend(messages.push(&body[cut..]).unwrap());
            assert!(!messages.is_cut());
            assert_eq!(read.len(), 2, "cut at {cut}");
            assert_eq!(read[0].header(":event-type"), Some("a"));
            assert_eq!(read[0].payload, b"{}");
            assert_eq!(read[1].header(":event-type"), Some("b"));
            assert_eq!(read[1].header("num!"), None);
            assert_eq!(read[1].payload, b"[1]");
        }
    }

    #[test]
    fn a_message_that_cannot_be_is_refused() {
        let mut messages = Messages::default();
        assert!(
            messages
                .push(&[0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0])
                .is_err()
        );
    }
}

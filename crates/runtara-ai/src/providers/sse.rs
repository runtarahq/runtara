// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Server-sent events, read from a body that arrives in pieces: each event's
//! `data`, once the blank line that ends it has arrived.

/// Whether an answer is server-sent events.
pub(crate) fn is_event_stream(headers: &std::collections::HashMap<String, String>) -> bool {
    headers
        .get("content-type")
        .is_some_and(|kind| kind.to_ascii_lowercase().starts_with("text/event-stream"))
}

/// A streamed answer read whole, as an unstreamed one is.
pub(crate) async fn whole(
    mut response: runtara_http::StreamingResponse,
) -> Result<runtara_http::HttpResponse, crate::CompletionError> {
    let body = response.rest().await.map_err(cut)?;
    Ok(runtara_http::HttpResponse {
        status: response.status,
        body,
        headers: std::mem::take(&mut response.headers),
    })
}

/// A streamed answer that stopped part-way.
pub(crate) fn cut(error: runtara_http::HttpError) -> crate::CompletionError {
    crate::CompletionError::HttpError(format!("the answer was cut: {error}"))
}

/// Events read so far, and the start of the next.
#[derive(Default)]
pub(crate) struct Events {
    pending: Vec<u8>,
}

impl Events {
    /// The `data` of each event `bytes` finishes, in order.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.pending.extend_from_slice(bytes);
        let mut events = Vec::new();
        while let Some((end, gap)) = event_end(&self.pending) {
            let event: Vec<u8> = self.pending.drain(..end + gap).take(end).collect();
            if let Some(data) = data_of(&String::from_utf8_lossy(&event)) {
                events.push(data);
            }
        }
        events
    }

    /// The `data` of an event the body ended without a blank line after.
    pub(crate) fn finish(&mut self) -> Option<String> {
        let rest = std::mem::take(&mut self.pending);
        data_of(&String::from_utf8_lossy(&rest))
    }
}

/// Where the first event in `bytes` ends, and how long the blank line is.
fn event_end(bytes: &[u8]) -> Option<(usize, usize)> {
    (0..bytes.len()).find_map(|at| {
        let rest = &bytes[at..];
        [&b"\r\n\r\n"[..], b"\n\n", b"\r\r"]
            .into_iter()
            .find(|gap| rest.starts_with(gap))
            .map(|gap| (at, gap.len()))
    })
}

/// An event's `data` lines, joined; none if it has none.
fn data_of(event: &str) -> Option<String> {
    let lines: Vec<&str> = event
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(|data| data.strip_prefix(' ').unwrap_or(data))
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// For tests: a provider on this machine that answers one request with
/// `body` as `content_type`, and the request it was sent; and a way to await
/// a future that never waits on anything but the call.
#[cfg(test)]
pub(crate) mod stand_in {
    use std::io::{Read, Write};

    pub(crate) fn serve(
        content_type: &'static str,
        body: &'static str,
    ) -> (String, std::thread::JoinHandle<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let served = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut asked = Vec::new();
            let mut buffer = [0u8; 8192];
            loop {
                let read = socket.read(&mut buffer).unwrap();
                asked.extend_from_slice(&buffer[..read]);
                let text = String::from_utf8_lossy(&asked).to_string();
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text[..end]
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|value| value.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if asked.len() >= end + 4 + length {
                        break;
                    }
                }
                if read == 0 {
                    break;
                }
            }
            write!(
                socket,
                "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            String::from_utf8_lossy(&asked).into_owned()
        });
        (base, served)
    }

    pub(crate) fn wait<T>(future: impl std::future::Future<Output = T>) -> T {
        let mut future = std::pin::pin!(future);
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        loop {
            if let std::task::Poll::Ready(value) = future.as_mut().poll(&mut context) {
                return value;
            }
            std::thread::yield_now();
        }
    }

    /// Pieces, kept.
    #[derive(Default)]
    pub(crate) struct Kept(pub(crate) Vec<String>);

    impl crate::completion::Pieces for Kept {
        fn piece<'a>(
            &'a mut self,
            text: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'a>> {
            self.0.push(text.to_owned());
            Box::pin(async {})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_read_however_the_body_is_cut() {
        let body =
            b": a comment\n\ndata: {\"a\":1}\r\n\r\nevent: x\ndata: one\ndata: two\n\ndata: [DONE]";
        for cut in 1..body.len() {
            let mut events = Events::default();
            let mut read = events.push(&body[..cut]);
            read.extend(events.push(&body[cut..]));
            read.extend(events.finish());
            assert_eq!(read, ["{\"a\":1}", "one\ntwo", "[DONE]"], "cut at {cut}");
        }
    }
}

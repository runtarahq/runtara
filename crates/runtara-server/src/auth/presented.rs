// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! What a caller presented to be authenticated by, kept as it arrived.
//!
//! The API authenticates a caller for its own routes. What it asks of an
//! engine in another process it asks with what the caller presented, and the
//! engine verifies that itself ([`crate::engine::listener`]): who the caller
//! is and which tenant it acts for are never the API's word.
//!
//! It is held for the length of the request it came with, as the tenant's
//! scope is, and is lost the same way by work that outlives the request: a
//! spawned task or a streamed answer keeps it with [`inherit`] or [`kept`].

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::http::{HeaderMap, HeaderName, HeaderValue, header};
use serde::{Deserialize, Serialize};

/// Which of the two ways in a caller came by. Each has its own audience for
/// a token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Door {
    Api,
    Mcp,
}

/// What a caller presented. It holds a credential: it is never printed.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Presented {
    pub door: Door,
    /// The headers a caller is authenticated by, as they arrived.
    headers: Vec<(String, String)>,
    /// The tenant the request's path named.
    pub requested_tenant: Option<String>,
    /// Whether the caller is held to be a member of the tenant. Not so only
    /// where a route answers about the caller across tenants.
    #[serde(default = "yes")]
    pub as_a_member: bool,
}

fn yes() -> bool {
    true
}

impl std::fmt::Debug for Presented {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Presented")
            .field("door", &self.door)
            .field("requested_tenant", &self.requested_tenant)
            .finish_non_exhaustive()
    }
}

impl Presented {
    /// From the headers of a request: the `Authorization` header, and
    /// whatever else (`also`) the provider in use reads.
    pub fn of(
        door: Door,
        headers: &HeaderMap,
        also: &[HeaderName],
        requested_tenant: Option<String>,
        as_a_member: bool,
    ) -> Self {
        let mut kept = Vec::new();
        for name in std::iter::once(&header::AUTHORIZATION).chain(also) {
            for value in headers.get_all(name) {
                if let Ok(value) = value.to_str() {
                    kept.push((name.as_str().to_owned(), value.to_owned()));
                }
            }
        }
        Self {
            door,
            headers: kept,
            requested_tenant,
            as_a_member,
        }
    }

    /// The headers it was taken from.
    pub fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in &self.headers {
            if let (Ok(name), Ok(mut value)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(value),
            ) {
                value.set_sensitive(true);
                headers.append(name, value);
            }
        }
        headers
    }
}

tokio::task_local! {
    static PRESENTED: Presented;
}

/// Runs `work` as asked by whoever presented `presented`.
pub async fn presenting<F: Future>(presented: Presented, work: F) -> F::Output {
    PRESENTED.scope(presented, work).await
}

/// What the caller of the current work presented.
pub fn presented() -> Option<Presented> {
    PRESENTED.try_with(Clone::clone).ok()
}

/// `work` as asked by the current caller, wherever it is later run: for work
/// spawned onto another task.
pub fn inherit<F: Future>(work: F) -> impl Future<Output = F::Output> {
    let presented = presented();
    async move {
        match presented {
            Some(presented) => presenting(presented, work).await,
            None => work.await,
        }
    }
}

/// `stream` as asked by the current caller: for an answer that is streamed
/// after the request that asked for it has returned.
pub fn kept<S: futures::Stream>(stream: S) -> Kept<S> {
    Kept {
        presented: presented(),
        stream: Box::pin(stream),
    }
}

/// See [`kept`].
pub struct Kept<S> {
    presented: Option<Presented>,
    stream: Pin<Box<S>>,
}

impl<S: futures::Stream> futures::Stream for Kept<S> {
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let stream = this.stream.as_mut();
        match &this.presented {
            Some(presented) => PRESENTED.sync_scope(presented.clone(), || stream.poll_next(cx)),
            None => stream.poll_next(cx),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.stream.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    fn bearer(token: &str) -> Presented {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        headers.insert("x-other", "left behind".parse().unwrap());
        Presented::of(Door::Api, &headers, &[], Some("acme".to_owned()), true)
    }

    #[test]
    fn it_keeps_what_authenticates_and_nothing_else() {
        let presented = bearer("abc");
        let headers = presented.headers();
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[header::AUTHORIZATION], "Bearer abc");

        let mut arrived = HeaderMap::new();
        arrived.insert("x-forwarded-user", "alice".parse().unwrap());
        let also = [HeaderName::from_static("x-forwarded-user")];
        let presented = Presented::of(Door::Mcp, &arrived, &also, None, true);
        assert_eq!(presented.headers()["x-forwarded-user"], "alice");
    }

    #[test]
    fn it_is_not_printed() {
        let printed = format!("{:?}", bearer("a-secret-token"));
        assert!(!printed.contains("a-secret-token"), "{printed}");
        assert!(printed.contains("acme"));
    }

    #[test]
    fn it_crosses_as_it_was() {
        let presented = bearer("abc");
        let written = serde_json::to_vec(&presented).unwrap();
        let read: Presented = serde_json::from_slice(&written).unwrap();
        assert_eq!(read, presented);
    }

    #[tokio::test]
    async fn it_is_held_for_the_work_it_came_with_and_no_other() {
        assert_eq!(presented(), None);
        let held = presenting(bearer("abc"), async { presented() }).await;
        assert_eq!(held, Some(bearer("abc")));
        assert_eq!(presented(), None);
    }

    #[tokio::test]
    async fn spawned_work_keeps_it_only_when_it_inherits_it() {
        let (kept, left) = presenting(bearer("abc"), async {
            let kept = tokio::spawn(inherit(async { presented() }));
            let left = tokio::spawn(async { presented() });
            (kept.await.unwrap(), left.await.unwrap())
        })
        .await;
        assert_eq!(kept, Some(bearer("abc")));
        assert_eq!(left, None);
    }

    #[tokio::test]
    async fn a_stream_keeps_it_after_the_request_is_over() {
        let stream = presenting(bearer("abc"), async {
            kept(futures::stream::unfold(0, |n| async move {
                (n < 2).then(|| (presented(), n + 1))
            }))
        })
        .await;
        let seen: Vec<_> = stream.collect().await;
        assert_eq!(seen, vec![Some(bearer("abc")), Some(bearer("abc"))]);
    }
}
